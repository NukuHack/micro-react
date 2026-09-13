//! SPA router exposed to JS as Router/Link/useLocation/useNavigate.
//! Routes are matched by path pattern (":param" segments, "*" catch-all)
//! against the browser's current location.
//!
//! The `js_*` component functions below (`js_router`, `js_link`, etc.) all
//! take `props: JsValue` by value rather than `&JsValue`. That's
//! deliberate, not an oversight `clippy::needless_pass_by_value` should
//! flag: every JS-callable "component" in this crate shares the single
//! shape `fn(JsValue) -> JsValue`, and callers (in particular
//! `create_element`'s function-component path, and this crate's own tests)
//! rely on being able to pass these functions around as that bare,
//! non-capturing `fn` type. Narrowing an individual component's
//! parameter to `&JsValue` would break that uniform calling convention
//! for a copy that's cheap to begin with.
#![allow(clippy::needless_pass_by_value)]

use js_sys::{Array, Function, Object, Reflect};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use wasm_bindgen::{JsCast, prelude::*};

use crate::bindings::{js_to_vnode, js_to_vnode_peek, vnode_to_js};
use crate::context::Context;
use crate::hooks::{DepVal, use_effect, use_effect_nodrop, use_memo, use_state};
use crate::vnode::{PropVal, VNode, VNodeInner};

// ─── Pattern matching ───

/// Compiled route pattern.
#[derive(Debug)]
pub struct Pattern {
	param_names: Vec<String>,
	regex: String,
}

impl Pattern {
	#[must_use]
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

		Self { param_names: names, regex }
	}

	/// Returns `Some(params)` if `path` matches, `None` otherwise.
	#[must_use]
	pub fn matches(&self, path: &str) -> Option<HashMap<String, String>> {
		// Uses JS RegExp via js_sys since the `regex` crate is too heavy for WASM. Cached per
		// pattern string (like html.rs's per-call-site template cache) since Router/Link
		// re-match every render and recompiling the same RegExp each time is wasted work.
		let re = REGEX_CACHE.with(|c| {
			if let Some(re) = c.borrow().get(&self.regex) {
				return re.clone();
			}
			let re = js_sys::RegExp::new(&self.regex, "");
			c.borrow_mut().insert(self.regex.clone(), re.clone());
			re
		});
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

thread_local! {
	/// Compiled `RegExp` per pattern string, avoiding a fresh compile on every render.
	static REGEX_CACHE: RefCell<HashMap<String, js_sys::RegExp>> = RefCell::new(HashMap::new());
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
	/// Shared location context: { pathname, search, params, state }
	static ROUTER_CTX: Context<JsValue> = Context::new(JsValue::NULL);
}

fn current_location() -> (String, String, JsValue) {
	let Some(window) = web_sys::window() else {
		crate::console_warn!("[micro-react] Router: no window available, defaulting location to \"/\"");
		return ("/".to_string(), String::new(), JsValue::NULL);
	};
	let path = window.location().pathname().unwrap_or_else(|_| "/".to_string());
	let search = window.location().search().unwrap_or_default();
	let state = window.history().and_then(|h| h.state()).unwrap_or(JsValue::NULL);
	(path, search, state)
}

/// Yields `(pattern, handler)` pairs from a `routes` value in the order
/// `Router` should try them.
///
/// If `routes` is a JS `Array` (e.g. `[["/a", fn], ["/b", fn]]`), pairs are
/// yielded in that array's order, which is always insertion order —
/// unaffected by the integer-key quirk below. This is the recommended shape
/// whenever route order matters, and `Routes`/`Route` build it this way.
///
/// If `routes` is a plain `Object` (the legacy/simple `{ "/a": fn }` shape),
/// pairs are yielded via `Object::keys`, which is enumeration order per the
/// JS spec: keys that parse as a canonical array index (e.g. `"0"`, `"23"`,
/// but not `"01"` or `"-1"`) are visited first in ascending numeric order,
/// *before* any string keys, regardless of where they were written in the
/// object literal. Callers whose route patterns could look like array
/// indices and who care about match order should pass an `Array` instead.
fn route_entries(routes: &JsValue) -> Vec<(String, JsValue)> {
	if let Some(arr) = routes.dyn_ref::<Array>() {
		return arr
			.iter()
			.filter_map(|entry| {
				let pair = entry.dyn_ref::<Array>()?;
				let pattern = pair.get(0).as_string()?;
				let handler = pair.get(1);
				Some((pattern, handler))
			})
			.collect();
	}
	if routes.is_object()
		&& let Some(obj) = routes.dyn_ref::<Object>()
	{
		return Object::keys(obj)
			.iter()
			.filter_map(|key| {
				let pattern = key.as_string()?;
				let handler = Reflect::get(routes, &key).ok()?;
				Some((pattern, handler))
			})
			.collect();
	}
	Vec::new()
}

/// `Router({ routes })` matches the current URL against the given path
/// patterns and provides `{ pathname, search, params, state }` via the
/// location context, where `state` mirrors `history.state` (see `Link`'s
/// `state` prop and `Navigate`'s `state` prop for how it gets set).
///
/// `routes` may be a plain `Object` (`{ "/a": fn }`) or an `Array` of
/// `[pattern, fn]` pairs; see `route_entries` for the tradeoff between the
/// two when it comes to match order.
#[wasm_bindgen(js_name = Router)]
#[must_use]
pub fn js_router(props: JsValue) -> JsValue {
	let routes_obj = Reflect::get(&props, &"routes".into()).unwrap_or(JsValue::NULL);

	let (initial_path, initial_search, initial_state) = current_location();
	let (pathname, set_path) = use_state::<String>(initial_path);
	let (search, set_search) = use_state::<String>(initial_search);
	let (nav_state, set_nav_state) = use_state::<JsValue>(initial_state);

	{
		let set_path = set_path.clone();
		let set_search = set_search.clone();
		let set_nav_state = set_nav_state.clone();
		use_effect(
			move || {
				let set_path = set_path.clone();
				let set_search = set_search.clone();
				let set_nav_state = set_nav_state.clone();
				let closure = Closure::wrap(Box::new(move |_e: web_sys::Event| {
					let (p, s, st) = current_location();
					set_path(p);
					set_search(s);
					set_nav_state(st);
				}) as Box<dyn Fn(web_sys::Event)>);
				let Some(window) = web_sys::window() else {
					crate::console_warn!("[micro-react] Router: no window available, skipping popstate listener");
					return Box::new(|| {}) as Box<dyn FnOnce()>;
				};
				let _ = window.add_event_listener_with_callback("popstate", closure.as_ref().unchecked_ref());
				// Unlike the old `.forget()`, this keeps `closure` alive only for
				// as long as the listener is actually registered: on unmount (or
				// a dep change, though deps are `[]` here so that's mount/unmount
				// only) the cleanup removes the listener and then drops the
				// closure, instead of leaking one closure per mount forever.
				Box::new(move || {
					let _ = window.remove_event_listener_with_callback("popstate", closure.as_ref().unchecked_ref());
					drop(closure);
				}) as Box<dyn FnOnce()>
			},
			Some(vec![]),
		);
	}

	// Match the current path against the route patterns, in the order
	// `route_entries` yields them (see its doc comment for the Array-vs-Object
	// ordering guarantee).
	let mut matched_fn: Option<Function> = None;
	let mut params: HashMap<String, String> = HashMap::new();

	for (pattern_str, val) in route_entries(&routes_obj) {
		let pattern = Pattern::compile(&pattern_str);
		if let Some(p) = pattern.matches(&pathname)
			&& val.is_function()
		{
			matched_fn = val.dyn_into().ok();
			params = p;
			break;
		}
	}

	// Publish { pathname, search, params, state } to the location context.
	let loc_obj = Object::new();
	let _ = Reflect::set(&loc_obj, &"pathname".into(), &JsValue::from_str(&pathname));
	let _ = Reflect::set(&loc_obj, &"search".into(), &JsValue::from_str(&search));
	let params_obj = Object::new();
	for (k, v) in &params {
		let _ = Reflect::set(&params_obj, &JsValue::from_str(k), &JsValue::from_str(v));
	}
	let _ = Reflect::set(&loc_obj, &"params".into(), &params_obj);
	let _ = Reflect::set(&loc_obj, &"state".into(), &nav_state);
	ROUTER_CTX.with(|ctx| ctx.set_value(loc_obj.into()));

	matched_fn.map_or_else(
		|| {
			let vn = VNode::tag("p").text("404 Not Found").build();
			vnode_to_js(vn).unwrap_or(JsValue::NULL)
		},
		|f| f.call0(&JsValue::NULL).unwrap_or(JsValue::NULL),
	)
}

/// `Link({ to, class/className, target, rel, state, children })` — an anchor
/// that performs client-side navigation via `history.pushState` + a
/// synthetic `popstate` event. A `target` other than `"_self"` (e.g.
/// `"_blank"`) opts out of client-side navigation, matching real
/// `<a>`/React Router behavior. `state` is passed through to
/// `history.pushState` and surfaces as `useLocation().state`.
#[wasm_bindgen(js_name = Link)]
#[must_use]
pub fn js_link(props: JsValue) -> JsValue {
	let to = Reflect::get(&props, &"to".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	// `html` authoring uses real HTML attribute names (`class`, not
	// `className`; see script.js), so Link honors that. `className` is
	// kept as a fallback for `h()`-style JSX callers.
	let class_name = Reflect::get(&props, &"class".into())
		.ok()
		.and_then(|v| v.as_string())
		.or_else(|| Reflect::get(&props, &"className".into()).ok().and_then(|v| v.as_string()));
	let target = Reflect::get(&props, &"target".into()).ok().and_then(|v| v.as_string());
	let rel = Reflect::get(&props, &"rel".into()).ok().and_then(|v| v.as_string());
	let state = Reflect::get(&props, &"state".into()).unwrap_or(JsValue::NULL);
	let children = Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL);

	// Memoized by `to`/`target`/`state` so the same `Closure`/`Function` is
	// handed back across re-renders instead of a fresh one leaking every
	// render (the same pattern `useNavigate` uses below). `state` is reduced
	// to a JSON string for the dependency comparison since `DepVal` compares
	// by `String`; non-JSON-serializable state (e.g. functions) will compare
	// as unchanged, same caveat as any `useMemo`/`useEffect` dep in JS.
	let to_for_click = to.clone();
	let target_for_click = target.clone();
	let state_for_click = state.clone();
	let state_dep = js_sys::JSON::stringify(&state).ok().and_then(|s| s.as_string()).unwrap_or_default();
	let onclick_fn: Function = use_memo(
		move || {
			let closure = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
				if e.default_prevented() || e.button() != 0 || e.meta_key() || e.ctrl_key() {
					return;
				}
				// A non-"_self" target (e.g. `target="_blank"`) opens the link in
				// another browsing context, so client-side navigation here would
				// be wrong; let the browser handle it like a normal anchor.
				if target_for_click.as_deref().is_some_and(|t| t != "_self") {
					return;
				}
				e.prevent_default();
				let Some(window) = web_sys::window() else {
					crate::console_warn!("[micro-react] Link: no window available, navigation ignored");
					return;
				};
				let Ok(history) = window.history() else {
					crate::console_warn!("[micro-react] Link: no history available, navigation ignored");
					return;
				};
				let _ = history.push_state_with_url(&state_for_click, "", Some(&to_for_click));
				window.dispatch_event(&web_sys::Event::new("popstate").expect("valid event name")).ok();
			}) as Box<dyn Fn(web_sys::MouseEvent)>);
			closure.into_js_value().unchecked_into::<Function>()
		},
		Some(vec![DepVal(to.clone()), DepVal(target.clone().unwrap_or_default()), DepVal(state_dep)]),
	);

	let mut builder = VNode::tag("a").attr("href", to.as_str()).on("onClick", onclick_fn);
	if let Some(cn) = class_name {
		builder = builder.attr("className", cn.as_str());
	}
	if let Some(t) = target {
		builder = builder.attr("target", t.as_str());
	}
	if let Some(r) = rel {
		builder = builder.attr("rel", r.as_str());
	}

	if let Ok(child_vn) = js_to_vnode(&children)
		&& !matches!(child_vn.inner, VNodeInner::Null)
	{
		builder = builder.child(child_vn);
	}

	vnode_to_js(builder.build()).unwrap_or(JsValue::NULL)
}

/// `NavLink({ to, end, class/className, target, rel, state, children })` — a
/// `Link` that knows whether it's "active" (current location is `to`, or a
/// descendant path of it unless `end` is set) and reflects that in its class
/// list. `class`/`className` may be a plain string (appended with a trailing
/// `" active"` when active) or a function `({ isActive }) => string`,
/// mirroring React Router's `NavLink`. `target`/`rel`/`state` are forwarded
/// to the underlying `Link` unchanged.
#[wasm_bindgen(js_name = NavLink)]
pub fn js_nav_link(props: JsValue) -> JsValue {
	let to = Reflect::get(&props, &"to".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	let end = Reflect::get(&props, &"end".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false);

	let location = ROUTER_CTX.with(crate::context::use_context);
	let pathname = Reflect::get(&location, &"pathname".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	let trimmed_to = to.trim_end_matches('/');
	let is_active = if end || trimmed_to.is_empty() {
		// `end` forces exact match; a root `to="/"` (trimmed_to == "") must
		// also be exact, since every path starts with "/" and would
		// otherwise always match as a "descendant" of it.
		pathname == to
	} else {
		pathname == to || pathname.starts_with(&format!("{trimmed_to}/"))
	};

	let class_prop =
		Reflect::get(&props, &"class".into()).ok().filter(|v| !v.is_undefined()).or_else(|| Reflect::get(&props, &"className".into()).ok());

	let class_name = match class_prop {
		Some(f) if f.is_function() => {
			let arg = Object::new();
			let _ = Reflect::set(&arg, &"isActive".into(), &JsValue::from_bool(is_active));
			f.unchecked_ref::<Function>().call1(&JsValue::NULL, &arg).ok().and_then(|v| v.as_string()).unwrap_or_default()
		}
		Some(v) => {
			let base = v.as_string().unwrap_or_default();
			match (base.is_empty(), is_active) {
				(true, true) => "active".to_string(),
				(true, false) => String::new(),
				(false, true) => format!("{base} active"),
				(false, false) => base,
			}
		}
		None => {
			if is_active {
				"active".to_string()
			} else {
				String::new()
			}
		}
	};

	// Delegate the actual anchor/click-navigation building to Link, then
	// swap in the computed class so the two stay in sync with each other.
	// `target`/`rel`/`state` aren't NavLink-specific, so just forward
	// whatever the caller passed through unchanged.
	let link_props = Object::new();
	let _ = Reflect::set(&link_props, &"to".into(), &JsValue::from_str(&to));
	let _ = Reflect::set(&link_props, &"className".into(), &JsValue::from_str(&class_name));
	for key in ["target", "rel", "state", "children"] {
		if let Ok(v) = Reflect::get(&props, &key.into())
			&& !v.is_undefined()
		{
			let _ = Reflect::set(&link_props, &key.into(), &v);
		}
	}
	js_link(link_props.into())
}

/// `useLocation()` — returns the current `{ pathname, search, params, state }`.
#[wasm_bindgen(js_name = useLocation)]
pub fn js_use_location() -> JsValue {
	ROUTER_CTX.with(crate::context::use_context)
}

/// `useNavigate()` — returns a `navigate(to)` function.
/// Memoized with empty deps so the underlying `Closure` is created once per
/// component instance instead of leaking a new one on every render.
#[wasm_bindgen(js_name = useNavigate)]
#[must_use]
pub fn js_use_navigate() -> JsValue {
	use_memo(
		|| {
			let navigate = Closure::wrap(Box::new(move |to: String| {
				let Some(window) = web_sys::window() else {
					crate::console_warn!("[micro-react] useNavigate: no window available, navigation ignored");
					return;
				};
				let Ok(history) = window.history() else {
					crate::console_warn!("[micro-react] useNavigate: no history available, navigation ignored");
					return;
				};
				let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&to));
				window.dispatch_event(&web_sys::Event::new("popstate").expect("valid event name")).ok();
			}) as Box<dyn Fn(String)>);
			navigate.into_js_value()
		},
		Some(Vec::new()),
	)
}

// ─── Routes / Route / Outlet ───
// A react-router-style layer on top of `Router` above: `<Routes>` flattens
// its `<Route>` tree (including nested "layout route" `<Route>`s rendered
// via `<Outlet/>`) into the same `{ pattern: () => vnode }` table `Router`
// already matches against, and simply delegates to it.

thread_local! {
	/// Content queued for the next `<Outlet/>` encountered while a matched
	/// route tree renders. Populated once per match by `RouteEntry::render`
	/// and consumed front-to-back as the tree is walked depth-first; since
	/// rendering here is single-threaded and synchronous, a plain FIFO is
	/// enough to hand each nested layout the right content.
	static OUTLET_QUEUE: RefCell<VecDeque<JsValue>> = const { RefCell::new(VecDeque::new()) };

	/// Value passed via `<Outlet context={...} />`, exposed to descendants
	/// of the outlet's rendered content through `useOutletContext()`.
	static OUTLET_CTX: Context<JsValue> = Context::new(JsValue::NULL);
}

/// One flattened leaf route: a URL pattern plus the chain of layout
/// elements (outermost first) that wrap the matched leaf element.
struct RouteEntry {
	pattern: String,
	ancestors: Vec<VNode>,
	leaf: VNode,
}

impl RouteEntry {
	fn render(&self) -> JsValue {
		if self.ancestors.is_empty() {
			return vnode_to_js(fresh_instance(&self.leaf)).unwrap_or(JsValue::NULL);
		}
		let mut queue: VecDeque<JsValue> = self.ancestors[1..].iter().map(|v| vnode_to_js(fresh_instance(v)).unwrap_or(JsValue::NULL)).collect();
		queue.push_back(vnode_to_js(fresh_instance(&self.leaf)).unwrap_or(JsValue::NULL));
		OUTLET_QUEUE.with(|q| *q.borrow_mut() = queue);
		vnode_to_js(fresh_instance(&self.ancestors[0])).unwrap_or(JsValue::NULL)
	}
}

/// Deep-clones a stored route template into a vnode with its own,
/// independent component identity.
///
/// `RouteEntry`'s `ancestors`/`leaf` are parsed once and reused for every
/// activation of that route. A plain `VNode::clone()` only copies the `Rc`
/// inside `Component`'s `ComponentInstSlot` (see `vnode.rs`), so every clone
/// taken from the same stored template — across every past and future
/// activation, and across every `RouteEntry` that shares an ancestor (e.g.
/// several leaves nested under the same layout `<Route>`) — would alias the
/// exact same mutable slot. `diff_component`/`unmount_vnode` write and clear
/// that slot as instances mount/unmount, so without this, one route's mount
/// or unmount can silently clobber another's live component instance,
/// eventually handing the reconciler a stale `dom_node` reference and producing
/// an `insertBefore` failure. Giving each clone its own fresh slot keeps
/// every activation's component identity independent, as intended.
fn fresh_instance(vnode: &VNode) -> VNode {
	let mut v = vnode.clone();
	match &mut v.inner {
		VNodeInner::Component { inst, children, .. } => {
			*inst = crate::vnode::ComponentInstSlot::new();
			*children = children.iter().map(fresh_instance).collect();
		}
		VNodeInner::Element { children, .. } | VNodeInner::Fragment { children, .. } | VNodeInner::Portal { children, .. } => {
			children.0 = children.0.iter().map(fresh_instance).collect();
		}
		VNodeInner::Text(_) | VNodeInner::Null => {}
	}
	v
}

/// `<Navigate to="/" replace state={...} />` — declarative redirect. Performs
/// the navigation as an effect (once per mount, or again if `to`/`replace`/
/// `state` change) and renders nothing. `replace` swaps in the new entry via
/// `history.replaceState` instead of `pushState`, so the redirect doesn't
/// leave the page it redirected away from in the back-button history —
/// important for guarded/redirect routes. `state` is passed through to
/// `history.pushState`/`replaceState` and surfaces as `useLocation().state`.
#[wasm_bindgen(js_name = Navigate)]
#[must_use]
pub fn js_navigate(props: JsValue) -> JsValue {
	let to = Reflect::get(&props, &"to".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	let replace = Reflect::get(&props, &"replace".into()).ok().is_some_and(|v| v.is_truthy());
	let state = Reflect::get(&props, &"state".into()).unwrap_or(JsValue::NULL);
	// Reduced to a JSON string purely for the effect's dependency
	// comparison, same caveat as `Link`'s memo dep above.
	let state_dep = js_sys::JSON::stringify(&state).ok().and_then(|s| s.as_string()).unwrap_or_default();

	use_effect_nodrop(
		{
			let to = to.clone();
			move || {
				let Some(window) = web_sys::window() else {
					crate::console_warn!("[micro-react] Navigate: no window available, navigation ignored");
					return;
				};
				let Ok(history) = window.history() else {
					crate::console_warn!("[micro-react] Navigate: no history available, navigation ignored");
					return;
				};
				let result =
					if replace { history.replace_state_with_url(&state, "", Some(&to)) } else { history.push_state_with_url(&state, "", Some(&to)) };
				let _ = result;
				window.dispatch_event(&web_sys::Event::new("popstate").expect("valid event name")).ok();
			}
		},
		Some(vec![DepVal(to), DepVal(replace.to_string()), DepVal(state_dep)]),
	);

	vnode_to_js(VNode::null()).unwrap_or(JsValue::NULL)
}

/// `<Route path="..." element={...}>...</Route>` — a config-only marker
/// read directly by `<Routes>` (via its raw, uninvoked vnode) before
/// anything is rendered. Rendered standalone, outside `<Routes>`, it just
/// falls back to rendering its own `element`.
#[wasm_bindgen(js_name = Route)]
#[must_use]
pub fn js_route(props: JsValue) -> JsValue {
	Reflect::get(&props, &"element".into()).unwrap_or(JsValue::NULL)
}

/// `<Outlet context={{ a, b, c }} />` — renders whichever nested route
/// matched, in place, inside a layout route's `element`. Only meaningful
/// inside a route rendered by `<Routes>`. The optional `context` prop is
/// published for the rendered subtree to read via `useOutletContext()`.
#[wasm_bindgen(js_name = Outlet)]
#[must_use]
pub fn js_outlet(props: JsValue) -> JsValue {
	let context = Reflect::get(&props, &"context".into()).unwrap_or(JsValue::UNDEFINED);
	OUTLET_CTX.with(|ctx| ctx.set_value(context));
	OUTLET_QUEUE.with(|q| q.borrow_mut().pop_front()).unwrap_or(JsValue::NULL)
}

/// `useOutletContext()` — reads the `context` value passed to the nearest
/// ancestor `<Outlet context={...} />`, or `undefined` if none was passed.
#[wasm_bindgen(js_name = useOutletContext)]
pub fn js_use_outlet_context() -> JsValue {
	OUTLET_CTX.with(crate::context::use_context)
}

/// `<Routes><Route path="..." element={...} />...</Routes>` — flattens the
/// `<Route>` tree into the flat `routes` table `Router` expects and
/// delegates to it, so nested/react-router-style JSX compiles down to
/// exactly the same matching engine as the original flat `routes` object.
#[wasm_bindgen(js_name = Routes)]
pub fn js_routes(props: JsValue) -> Result<JsValue, JsValue> {
	let children = Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL);

	// Rebuilt on every render (unlike `useNavigate`/`Link`'s closures above,
	// which are safe to memoize forever by their own stable identity): the
	// route table's *content* depends on `children`, which can legitimately
	// differ from one render to the next (e.g. a parent conditionally
	// including/excluding `<Route>`s), so memoizing this with permanently-
	// empty deps would silently keep serving the first-mount route table
	// forever. The per-pattern thunks are still cheap, small closures, so
	// rebuilding them each render is preferable to serving stale routes.
	let table = build_route_table(&children);
	let routes_arr = Array::new();
	for entry in table {
		let pattern = entry.pattern.clone();
		let thunk = Closure::wrap(Box::new(move || -> JsValue { entry.render() }) as Box<dyn Fn() -> JsValue>);
		let pair = Array::new();
		pair.push(&JsValue::from_str(&pattern));
		pair.push(thunk.as_ref());
		routes_arr.push(&pair);
		thunk.forget();
	}

	let router_props = Object::new();
	Reflect::set(&router_props, &"routes".into(), &routes_arr)?;
	Ok(js_router(router_props.into()))
}

fn build_route_table(children: &JsValue) -> Vec<RouteEntry> {
	let mut out = Vec::new();
	collect_routes(&js_children_to_vnodes(children), "", &[], &mut out);
	out
}

/// Normalizes a JSX `children` prop (single vnode, array, or null/undefined)
/// into owned `VNode`s — the same rule `createElement` itself applies.
fn js_children_to_vnodes(children: &JsValue) -> Vec<VNode> {
	let arr: Array = match children.clone().dyn_into::<Array>() {
		Ok(a) => a,
		Err(orig) if orig.is_null() || orig.is_undefined() => Array::new(),
		Err(orig) => {
			let a = Array::new();
			a.push(&orig);
			a
		}
	};
	arr.iter().filter_map(|c| js_to_vnode_peek(&c).ok()).filter(|v| !matches!(v.inner, VNodeInner::Null)).collect()
}

/// Recursively walks `<Route>` vnodes, joining `path`s and threading
/// `element`s as ancestor "layout" wrappers for any nested `<Route>`s,
/// pushing one `RouteEntry` per leaf (childless) `<Route>`.
fn collect_routes(nodes: &[VNode], parent_path: &str, ancestors: &[VNode], out: &mut Vec<RouteEntry>) {
	for node in nodes {
		let VNodeInner::Component { name, props, children, .. } = &node.inner else { continue };
		if name.as_str() != "Route" {
			continue;
		}

		let mut path: Option<String> = None;
		let mut element: Option<VNode> = None;
		for (k, v) in props {
			match (k.as_str(), v) {
				("path", PropVal::Str(s)) => path = Some(s.clone()),
				("element", PropVal::Js(js)) => element = js_to_vnode_peek(js).ok(),
				_ => {}
			}
		}

		let full_path = path.as_ref().map_or_else(|| parent_path.to_string(), |p| join_path(parent_path, p));

		if children.is_empty() {
			let Some(leaf) = element else { continue };
			out.push(RouteEntry { pattern: if full_path.is_empty() { "/".to_string() } else { full_path }, ancestors: ancestors.to_vec(), leaf });
		} else {
			let mut next_ancestors = ancestors.to_vec();
			if let Some(el) = element {
				next_ancestors.push(el);
			}
			collect_routes(children, &full_path, &next_ancestors, out);
		}
	}
}

fn join_path(parent: &str, child: &str) -> String {
	format!("{}/{}", parent.trim_end_matches('/'), child.trim_start_matches('/'))
}
