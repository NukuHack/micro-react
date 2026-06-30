// ─── router.rs ───────────────────────────────────────────────────────────────
//
// SPA router — same API as the JS version.
//
// Usage:
//   let router = Router::new(routes! {
//       "/" => home_component,
//       "/about" => about_component,
//       "/user/:id" => user_component,
//       "*" => not_found_component,
//   });
//   router.render()
//
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use wasm_bindgen::{prelude::*, JsCast};
use js_sys::{Function, Object, Reflect};

use crate::vnode::{VNode, VNodeInner, ComponentFn, Props, PropVal};
use crate::hooks::{use_state, use_effect_nodrop};
use crate::context::Context;
use crate::bindings::{js_to_vnode, vnode_to_js};

// ─────────────────────────────────────────────────────────────────────────────
// Pattern matching
// ─────────────────────────────────────────────────────────────────────────────

/// Compiled route pattern.
pub struct Pattern {
    pub raw: String,
    param_names: Vec<String>,
    regex: String,
}

impl Pattern {
    pub fn compile(pattern: &str) -> Self {
        let mut names = Vec::new();
        let mut regex = String::from("^");

        for segment in pattern.split('/') {
            if segment.is_empty() { continue; }
            regex.push('/');
            if segment.starts_with(':') {
                names.push(segment[1..].to_string());
                regex.push_str("([^/]+)");
            } else if segment == "*" {
                regex.push_str(".*");
            } else {
                regex.push_str(&regex_escape(segment));
            }
        }
        regex.push_str("(?:/)?$");

        Pattern { raw: pattern.to_string(), param_names: names, regex }
    }

    /// Returns `Some(params)` if `path` matches, `None` otherwise.
    pub fn matches(&self, path: &str) -> Option<HashMap<String, String>> {
        // We use JS RegExp via js_sys since we can't use the `regex` crate (too heavy for WASM without features).
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
    s.chars().flat_map(|c| match c {
        '.' | '+' | '?' | '^' | '$' | '{' | '}' | '[' | ']' | '(' | ')' | '|' | '\\' => {
            vec!['\\', c]
        }
        _ => vec![c],
    }).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Route
// ─────────────────────────────────────────────────────────────────────────────

pub struct Route {
    pub pattern: Pattern,
    pub component: ComponentFn,
}

// ─────────────────────────────────────────────────────────────────────────────
// Router component
// ─────────────────────────────────────────────────────────────────────────────

/// A declarative router component.
pub fn make_router(routes: Vec<Route>) -> ComponentFn {
    ComponentFn::new(move |_props| {
        let window = web_sys::window().expect("no window");
        let initial_path = window.location().pathname().unwrap_or_else(|_| "/".to_string());
        let initial_search = window.location().search().unwrap_or_default();

        let (path, set_path) = use_state(initial_path);
        let (search, set_search) = use_state(initial_search);

        // Listen for popstate
        let set_path2 = set_path.clone();
        let set_search2 = set_search.clone();
        use_effect_nodrop(move || {
            let closure = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                let window = web_sys::window().unwrap();
                let p = window.location().pathname().unwrap_or_default();
                let s = window.location().search().unwrap_or_default();
                set_path2(p);
                set_search2(s);
            }) as Box<dyn Fn(web_sys::Event)>);

            let window = web_sys::window().unwrap();
            let _ = window.add_event_listener_with_callback(
                "popstate",
                closure.as_ref().unchecked_ref(),
            );
            closure.forget();
        }, Some(vec![]));

        // Match route
        let mut matched_comp: Option<&ComponentFn> = None;
        let mut matched_params: HashMap<String, String> = HashMap::new();

        for route in &routes {
            if let Some(params) = route.pattern.matches(&path) {
                matched_comp = Some(&route.component);
                matched_params = params;
                break;
            }
        }

        // Build location context props
        let mut ctx_props: Props = vec![
            ("path".to_string(), PropVal::Str(path.clone())),
            ("search".to_string(), PropVal::Str(search.clone())),
        ];
        for (k, v) in &matched_params {
            ctx_props.push((format!("param_{}", k), PropVal::Str(v.clone())));
        }

        match matched_comp {
            Some(comp) => comp.call(ctx_props),
            None => VNode::text("404 Not Found"),
        }
    })
}

/// Convenience macro for declaring routes.
///
/// ```rust
/// use micro_react_wasm::router::{make_router, Route, Pattern};
/// use micro_react_wasm::vnode::ComponentFn;
///
/// let router = make_router(vec![
///     Route { pattern: Pattern::compile("/"),      component: ComponentFn::new(|_| home()) },
///     Route { pattern: Pattern::compile("/about"), component: ComponentFn::new(|_| about()) },
/// ]);
/// ```
#[macro_export]
macro_rules! routes {
    ($($pattern:literal => $comp:expr),* $(,)?) => {
        vec![
            $(
                $crate::router::Route {
                    pattern: $crate::router::Pattern::compile($pattern),
                    component: $crate::vnode::ComponentFn::new($comp),
                }
            ),*
        ]
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Link component
// ─────────────────────────────────────────────────────────────────────────────

pub fn link(to: &str, children: Vec<VNode>) -> VNode {
    let to = to.to_string();
    let onclick = {
        let to = to.clone();
        Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            if e.default_prevented() || e.button() != 0 || e.meta_key() || e.ctrl_key() {
                return;
            }
            e.prevent_default();
            let window = web_sys::window().unwrap();
            let history = window.history().unwrap();
            let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&to));
            window.dispatch_event(&web_sys::Event::new("popstate").unwrap()).ok();
        }) as Box<dyn Fn(web_sys::MouseEvent)>)
    };
    let onclick_fn: js_sys::Function = onclick.as_ref().unchecked_ref::<js_sys::Function>().clone();
    onclick.forget();

    VNode::tag("a")
        .attr("href", &to as &str)
        .on("onClick", onclick_fn)
        .children(children)
        .build()
}

// ─────────────────────────────────────────────────────────────────────────────
// useNavigate
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a `navigate(to: &str)` closure.
pub fn use_navigate() -> impl Fn(&str) {
    move |to: &str| {
        let window = web_sys::window().unwrap();
        let history = window.history().unwrap();
        let _ = history.push_state_with_url(&JsValue::NULL, "", Some(to));
        window.dispatch_event(&web_sys::Event::new("popstate").unwrap()).ok();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JS-visible bindings
//
// These mirror the JS-only Router/Link/useLocation/useNavigate that used to
// live as plain functions in index.html. They're implemented here on top of
// the same Rust hooks (use_state / use_effect_nodrop) and Context machinery
// used by createContext, so index.html only needs `window.Router = ...`
// style assignments — no real component logic in JS.
// ─────────────────────────────────────────────────────────────────────────────

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

/// `Router({ routes })` — `routes` is a plain JS object mapping a path
/// pattern (e.g. `"/user/:id"`) to a zero-arg render function returning a
/// vnode. Matches the current URL, listens for `popstate`, and provides
/// `{ path, search, params }` via the location context used by `useLocation`.
#[wasm_bindgen(js_name = Router)]
pub fn js_router(props: JsValue) -> JsValue {
    let routes_obj = Reflect::get(&props, &"routes".into()).unwrap_or(JsValue::NULL);

    let (initial_path, initial_search) = current_location();
    let (path, set_path) = use_state::<String>(initial_path);
    let (search, set_search) = use_state::<String>(initial_search);

    {
        let set_path = set_path.clone();
        let set_search = set_search.clone();
        use_effect_nodrop(move || {
            let set_path = set_path.clone();
            let set_search = set_search.clone();
            let closure = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                let (p, s) = current_location();
                set_path(p);
                set_search(s);
            }) as Box<dyn Fn(web_sys::Event)>);
            let window = web_sys::window().expect("no window");
            let _ = window.add_event_listener_with_callback(
                "popstate",
                closure.as_ref().unchecked_ref(),
            );
            closure.forget();
        }, Some(vec![]));
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

/// `Link({ to, className, children })` — an anchor that performs client-side
/// navigation via `history.pushState` + a synthetic `popstate` event.
#[wasm_bindgen(js_name = Link)]
pub fn js_link(props: JsValue) -> JsValue {
    let to = Reflect::get(&props, &"to".into())
        .ok().and_then(|v| v.as_string()).unwrap_or_default();
    let class_name = Reflect::get(&props, &"className".into())
        .ok().and_then(|v| v.as_string());
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
        window.dispatch_event(&web_sys::Event::new("popstate").unwrap()).ok();
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
    ROUTER_CTX.with(|ctx| crate::context::use_context(ctx))
}

/// `useNavigate()` — returns a `navigate(to)` function.
#[wasm_bindgen(js_name = useNavigate)]
pub fn js_use_navigate() -> JsValue {
    let navigate = Closure::wrap(Box::new(move |to: String| {
        let window = web_sys::window().expect("no window");
        let history = window.history().expect("no history");
        let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&to));
        window.dispatch_event(&web_sys::Event::new("popstate").unwrap()).ok();
    }) as Box<dyn Fn(String)>);
    navigate.into_js_value()
}
