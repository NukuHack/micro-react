//! SPA router exposed to JS as Router/Link/useLocation/useNavigate.
//! Routes are matched by path pattern (":param" segments, "*" catch-all)
//! against the browser's current location.

use js_sys::{Function, Object, Reflect};
use std::collections::HashMap;
use wasm_bindgen::{prelude::*, JsCast};

use crate::bindings::{js_to_vnode, vnode_to_js};
use crate::context::Context;
use crate::hooks::{use_effect_nodrop, use_state};
use crate::vnode::{VNode, VNodeInner};

// ─── Pattern matching ───

/// Compiled route pattern.
pub struct Pattern {
	param_names: Vec<String>,
	regex: String,
}

impl Pattern {
	pub fn compile(pattern: &str) -> Self {
		let mut names = Vec::new();
		let mut regex = String::from("^");

		for segment in pattern.split('/') {
			if segment.is_empty() {
				continue;
			}
			regex.push('/');
			if let Some(stripped) = segment.strip_prefix(':') {
				names.push(stripped.to_string());
				regex.push_str("([^/]+)");
			} else if segment == "*" {
				regex.push_str(".*");
			} else {
				regex.push_str(&regex_escape(segment));
			}
		}
		regex.push_str("(?:/)?$");

		Pattern { param_names: names, regex }
	}

	/// Returns `Some(params)` if `path` matches, `None` otherwise.
	pub fn matches(&self, path: &str) -> Option<HashMap<String, String>> {
		// Uses JS RegExp via js_sys since the `regex` crate is too heavy for WASM.
		let re = js_sys::RegExp::new(&self.regex, "");
		let result = re.exec(path);
		match result {
			None => None,
			Some(arr) if arr.is_null() => None,
			Some(arr) => {
				let mut params = HashMap::new();
				for (i, name) in self.param_names.iter().enumerate() {
					if let Some(val) = arr.get((i + 1) as u32).as_string() {
						params.insert(name.clone(), val);
					}
				}
				Some(params)
			}
		}
	}
}

fn regex_escape(s: &str) -> String {
	s.chars()
		.flat_map(|c| match c {
			'.' | '+' | '?' | '^' | '$' | '{' | '}' | '[' | ']' | '(' | ')' | '|' | '\\' => {
				vec!['\\', c]
			}
			_ => vec![c],
		})
		.collect()
}

// ─── JS-visible bindings ───

thread_local! {
	/// Shared location context: { path, search, params }
	static ROUTER_CTX: Context<JsValue> = Context::new(JsValue::NULL);
}

fn current_location() -> (String, String) {
	let window = web_sys::window().expect("no window");
	let path = window.location().pathname().unwrap_or_else(|_| "/".to_string());
	let search = window.location().search().unwrap_or_default();
	(path, search)
}

/// `Router({ routes })` matches the current URL against the given path
/// patterns and provides `{ path, search, params }` via the location context.
#[wasm_bindgen(js_name = Router)]
pub fn js_router(props: JsValue) -> JsValue {
	let routes_obj = Reflect::get(&props, &"routes".into()).unwrap_or(JsValue::NULL);

	let (initial_path, initial_search) = current_location();
	let (path, set_path) = use_state::<String>(initial_path);
	let (search, set_search) = use_state::<String>(initial_search);

	{
		let set_path = set_path.clone();
		let set_search = set_search.clone();
		use_effect_nodrop(
			move || {
				let set_path = set_path.clone();
				let set_search = set_search.clone();
				let closure = Closure::wrap(Box::new(move |_e: web_sys::Event| {
					let (p, s) = current_location();
					set_path(p);
					set_search(s);
				}) as Box<dyn Fn(web_sys::Event)>);
				let window = web_sys::window().expect("no window");
				let _ = window.add_event_listener_with_callback("popstate", closure.as_ref().unchecked_ref());
				closure.forget();
			},
			Some(vec![]),
		);
	}

	// Match the current path against the route patterns (object keys).
	let mut matched_fn: Option<Function> = None;
	let mut params: HashMap<String, String> = HashMap::new();

	if routes_obj.is_object() {
		if let Some(obj) = routes_obj.dyn_ref::<Object>() {
			for key in Object::keys(obj).iter() {
				let pattern_str = key.as_string().unwrap_or_default();
				let pattern = Pattern::compile(&pattern_str);
				if let Some(p) = pattern.matches(&path) {
					if let Ok(val) = Reflect::get(&routes_obj, &key) {
						if val.is_function() {
							matched_fn = val.dyn_into().ok();
							params = p;
							break;
						}
					}
				}
			}
		}
	}

	// Publish { path, search, params } to the location context.
	let loc_obj = Object::new();
	let _ = Reflect::set(&loc_obj, &"path".into(), &JsValue::from_str(&path));
	let _ = Reflect::set(&loc_obj, &"search".into(), &JsValue::from_str(&search));
	let params_obj = Object::new();
	for (k, v) in &params {
		let _ = Reflect::set(&params_obj, &JsValue::from_str(k), &JsValue::from_str(v));
	}
	let _ = Reflect::set(&loc_obj, &"params".into(), &params_obj);
	ROUTER_CTX.with(|ctx| ctx.set_value(loc_obj.into()));

	match matched_fn {
		Some(f) => f.call0(&JsValue::NULL).unwrap_or(JsValue::NULL),
		None => {
			let vn = VNode::tag("p").text("404 Not Found").build();
			vnode_to_js(vn).unwrap_or(JsValue::NULL)
		}
	}
}

/// `Link({ to, class/className, children })` — an anchor that performs
/// client-side navigation via `history.pushState` + a synthetic `popstate`
/// event.
#[wasm_bindgen(js_name = Link)]
pub fn js_link(props: JsValue) -> JsValue {
	let to = Reflect::get(&props, &"to".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	// `html` authoring uses real HTML attribute names (`class`, not
	// `className`; see script.js), so Link honors that. `className` is
	// kept as a fallback for `h()`-style JSX callers.
	let class_name = Reflect::get(&props, &"class".into())
		.ok()
		.and_then(|v| v.as_string())
		.or_else(|| Reflect::get(&props, &"className".into()).ok().and_then(|v| v.as_string()));
	let children = Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL);

	let to_for_click = to.clone();
	let onclick = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
		if e.default_prevented() || e.button() != 0 || e.meta_key() || e.ctrl_key() {
			return;
		}
		e.prevent_default();
		let window = web_sys::window().expect("no window");
		let history = window.history().expect("no history");
		let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&to_for_click));
		window.dispatch_event(&web_sys::Event::new("popstate").expect("valid event name")).ok();
	}) as Box<dyn Fn(web_sys::MouseEvent)>);
	let onclick_fn: Function = onclick.as_ref().unchecked_ref::<Function>().clone();
	onclick.forget();

	let mut builder = VNode::tag("a").attr("href", to.as_str()).on("onClick", onclick_fn);
	if let Some(cn) = class_name {
		builder = builder.attr("className", cn.as_str());
	}

	if let Ok(child_vn) = js_to_vnode(&children) {
		if !matches!(child_vn.inner, VNodeInner::Null) {
			builder = builder.child(child_vn);
		}
	}

	vnode_to_js(builder.build()).unwrap_or(JsValue::NULL)
}

/// `useLocation()` — returns the current `{ path, search, params }`.
#[wasm_bindgen(js_name = useLocation)]
pub fn js_use_location() -> JsValue {
	ROUTER_CTX.with(crate::context::use_context)
}

/// `useNavigate()` — returns a `navigate(to)` function.
#[wasm_bindgen(js_name = useNavigate)]
pub fn js_use_navigate() -> JsValue {
	let navigate = Closure::wrap(Box::new(move |to: String| {
		let window = web_sys::window().expect("no window");
		let history = window.history().expect("no history");
		let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&to));
		window.dispatch_event(&web_sys::Event::new("popstate").expect("valid event name")).ok();
	}) as Box<dyn Fn(String)>);
	navigate.into_js_value()
}
