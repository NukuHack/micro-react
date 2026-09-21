//! Second follow-up pass on `router.rs` test coverage, complementing
//! `tests/browser/router.rs` and `tests/browser/router_gaps.rs`.
//!
//! Auditing those two files against `router.rs`'s public surface turned up
//! several pieces with no coverage at all:
//!
//! - `NavLink` (`js_nav_link`) — not referenced anywhere in `tests/`. Its
//!   `is_active` computation (exact vs. descendant match, the `end` prop,
//!   the `to="/"` special case) and its `class`/`className` handling
//!   (plain string vs. `({ isActive }) => string`) were all unexercised.
//! - `useLocation` (`js_use_location`) — never called from a test; only
//!   exercised incidentally via `NavLink`'s internal use of `ROUTER_CTX`.
//! - `useOutletContext` (`js_use_outlet_context`) — `Outlet`'s `context`
//!   prop is documented but nothing ever reads it back.
//! - `Route` rendered standalone, outside `<Routes>` — its doc comment says
//!   it "just falls back to rendering its own `element`", but that fallback
//!   path had no test.
//! - Nested `<Route>` param capture — `nested_route_renders_layout_...` in
//!   `router_gaps.rs` covers the *layout wrapping* half of nesting, but not
//!   that `:param` segments from parent and child path segments are both
//!   captured into the same `params` map for the matched leaf.
//! - `Router`'s `popstate` listener actually driving a re-render — every
//!   existing test sets the initial location via `history.push_state`
//!   *before* the first render, so the effect-installed popstate listener
//!   itself (`set_path`/`set_search` firing off a real `popstate` event
//!   after mount) was never triggered.
//!
//! Runs via `wasm-bindgen-test` in a headless browser, same as the rest of
//! `tests/browser/`:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::create_element;
use micro_react::router::{js_nav_link, js_outlet, js_route, js_router, js_routes, js_use_location, js_use_outlet_context};
use micro_react::scheduler::flush_rerenders;

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

/// Same helper as the other two router test files: wraps a plain
/// `fn(JsValue) -> JsValue` as a named JS function, since `create_element`
/// reads `.name` to tag the resulting `Component` vnode.
fn wrap_as_js_component(f: fn(JsValue) -> JsValue, name: &str) -> JsValue {
	let closure: JsValue = Closure::wrap(Box::new(f) as Box<dyn Fn(JsValue) -> JsValue>).into_js_value();
	let descriptor = Object::new();
	let _ = Reflect::set(&descriptor, &"value".into(), &JsValue::from_str(name));
	let _ = Reflect::set(&descriptor, &"configurable".into(), &JsValue::TRUE);
	let _: Object = js_sys::Object::define_property(closure.unchecked_ref(), &"name".into(), &descriptor);
	closure
}

fn js_routes_as_fn(props: JsValue) -> JsValue {
	js_routes(props).unwrap_or(JsValue::NULL)
}

fn set_path(path: &str) {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some(path));
}

fn build_div_text(text: &str) -> JsValue {
	let children = Array::new();
	children.push(&JsValue::from_str(text));
	create_element(&JsValue::from_str("div"), &JsValue::NULL, children.into()).expect("createElement should succeed")
}

fn build_route_props(path: &str, element: JsValue) -> JsValue {
	let props = Object::new();
	let _ = Reflect::set(&props, &"path".into(), &JsValue::from_str(path));
	let _ = Reflect::set(&props, &"element".into(), &element);
	props.into()
}

// ─── NavLink: active-state computation ───

fn nav_link_props(to: &str, end: bool) -> JsValue {
	let props = Object::new();
	let _ = Reflect::set(&props, &"to".into(), &JsValue::from_str(to));
	if end {
		let _ = Reflect::set(&props, &"end".into(), &JsValue::TRUE);
	}
	let _ = Reflect::set(&props, &"children".into(), &JsValue::from_str("Link Text"));
	props.into()
}

/// Puts a matching `Router` through one render so `ROUTER_CTX` (the global
/// location context `NavLink` reads via `useLocation`) reflects `path`.
/// `NavLink` itself is unrelated to `Router`'s own route matching — it just
/// needs *some* `Router` to have published a location — so a single
/// catch-all route is enough.
fn seed_router_location(path: &str) {
	set_path(path);
	let container = make_container();
	let router_fn = wrap_as_js_component(js_router, "Router");
	let routes = Array::new();
	let pair = Array::new();
	pair.push(&JsValue::from_str("*"));
	let handler: JsValue = Closure::wrap(Box::new(|| -> JsValue { build_div_text("seed") }) as Box<dyn Fn() -> JsValue>).into_js_value();
	pair.push(&handler);
	routes.push(&pair);
	let props = Object::new();
	let _ = Reflect::set(&props, &"routes".into(), &routes);
	let vnode = create_element(&router_fn, &props.into(), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	root.unmount();
	container.remove();
}

fn mount_nav_link(to: &str, end: bool) -> web_sys::Element {
	let container = make_container();
	let nav_link_fn = wrap_as_js_component(js_nav_link, "NavLink");
	let vnode = create_element(&nav_link_fn, &nav_link_props(to, end), JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	container.query_selector("a").expect("query should not error").expect("expected an <a> element")
}

#[wasm_bindgen_test]
fn nav_link_is_active_on_exact_match() {
	seed_router_location("/dashboard");
	let anchor = mount_nav_link("/dashboard", false);
	assert_eq!(anchor.class_name(), "active", "NavLink should be active when the current path exactly matches `to`");
	assert_eq!(anchor.get_attribute("href").as_deref(), Some("/dashboard"));
}

#[wasm_bindgen_test]
fn nav_link_is_active_for_a_descendant_path_when_end_is_not_set() {
	seed_router_location("/dashboard/settings");
	let anchor = mount_nav_link("/dashboard", false);
	assert_eq!(anchor.class_name(), "active", "without `end`, NavLink should be active for a descendant of `to`, not just an exact match");
}

#[wasm_bindgen_test]
fn nav_link_end_prop_requires_an_exact_match() {
	seed_router_location("/dashboard/settings");
	let anchor = mount_nav_link("/dashboard", true);
	assert_eq!(anchor.class_name(), "", "with `end` set, a descendant path should not count as active");
}

#[wasm_bindgen_test]
fn nav_link_is_inactive_for_an_unrelated_path() {
	seed_router_location("/other");
	let anchor = mount_nav_link("/dashboard", false);
	assert_eq!(anchor.class_name(), "", "NavLink should not be active for a completely unrelated path");
}

#[wasm_bindgen_test]
fn nav_link_root_path_requires_exact_match_even_without_end() {
	// Every path starts with "/", so a naive "starts with" check against a
	// root `to="/"` would make NavLink permanently active. `is_active`
	// special-cases an empty trimmed `to` to require an exact match instead.
	seed_router_location("/anything");
	let anchor = mount_nav_link("/", false);
	assert_eq!(anchor.class_name(), "", "a root `to=\"/\"` NavLink should require an exact match, not treat every path as a descendant of root");
}

#[wasm_bindgen_test]
fn nav_link_root_path_is_active_when_path_is_exactly_root() {
	seed_router_location("/");
	let anchor = mount_nav_link("/", false);
	assert_eq!(anchor.class_name(), "active");
}

#[wasm_bindgen_test]
fn nav_link_string_class_gets_active_suffix_appended_when_active() {
	seed_router_location("/dashboard");
	let container = make_container();
	let nav_link_fn = wrap_as_js_component(js_nav_link, "NavLink");
	let props = nav_link_props("/dashboard", false);
	let _ = Reflect::set(&props, &"className".into(), &JsValue::from_str("btn"));
	let vnode = create_element(&nav_link_fn, &props, JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	let anchor = container.query_selector("a").expect("query should not error").expect("expected an <a> element");
	assert_eq!(anchor.class_name(), "btn active", "a non-empty string class should keep its base value with \" active\" appended");
}

#[wasm_bindgen_test]
fn nav_link_string_class_is_unchanged_when_inactive() {
	seed_router_location("/elsewhere");
	let container = make_container();
	let nav_link_fn = wrap_as_js_component(js_nav_link, "NavLink");
	let props = nav_link_props("/dashboard", false);
	let _ = Reflect::set(&props, &"className".into(), &JsValue::from_str("btn"));
	let vnode = create_element(&nav_link_fn, &props, JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	let anchor = container.query_selector("a").expect("query should not error").expect("expected an <a> element");
	assert_eq!(anchor.class_name(), "btn", "an inactive NavLink with a base class should not append \" active\"");
}

#[wasm_bindgen_test]
fn nav_link_function_class_is_called_with_the_computed_is_active_flag() {
	seed_router_location("/dashboard");
	let container = make_container();
	let nav_link_fn = wrap_as_js_component(js_nav_link, "NavLink");
	let props = nav_link_props("/dashboard", false);
	let class_fn: JsValue = Closure::wrap(Box::new(|arg: JsValue| -> JsValue {
		let is_active = Reflect::get(&arg, &"isActive".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false);
		JsValue::from_str(if is_active { "state-active" } else { "state-inactive" })
	}) as Box<dyn Fn(JsValue) -> JsValue>)
	.into_js_value();
	let _ = Reflect::set(&props, &"className".into(), &class_fn);
	let vnode = create_element(&nav_link_fn, &props, JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	let anchor = container.query_selector("a").expect("query should not error").expect("expected an <a> element");
	assert_eq!(
		anchor.class_name(),
		"state-active",
		"a function className should be called with `{{ isActive }}` and its return value used as-is (no \" active\" appended)"
	);
}

#[wasm_bindgen_test]
fn nav_link_delegates_children_to_link() {
	seed_router_location("/dashboard");
	let anchor = mount_nav_link("/dashboard", false);
	assert_eq!(anchor.text_content().as_deref(), Some("Link Text"), "NavLink should render its children the same way Link does");
}

// ─── useLocation ───

fn location_reader(_props: JsValue) -> JsValue {
	let loc = js_use_location();
	let pathname = Reflect::get(&loc, &"pathname".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	let search = Reflect::get(&loc, &"search".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
	let params = Reflect::get(&loc, &"params".into()).unwrap_or(JsValue::NULL);
	let id = Reflect::get(&params, &"id".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "none".to_string());
	build_div_text(&format!("{pathname}|{search}|id={id}"))
}

#[wasm_bindgen_test]
fn use_location_reflects_pathname_search_and_params_of_the_matched_route() {
	set_path("/users/42?tab=profile");
	let container = make_container();
	let router_fn = wrap_as_js_component(js_router, "Router");
	let reader_fn = wrap_as_js_component(location_reader, "Reader");

	let routes = Array::new();
	let pair = Array::new();
	pair.push(&JsValue::from_str("/users/:id"));
	let reader_fn_clone = reader_fn.clone();
	let handler: JsValue = Closure::wrap(Box::new(move || -> JsValue {
		create_element(&reader_fn_clone, &JsValue::NULL, JsValue::NULL).expect("createElement should succeed")
	}) as Box<dyn Fn() -> JsValue>)
	.into_js_value();
	pair.push(&handler);
	routes.push(&pair);

	let props = Object::new();
	let _ = Reflect::set(&props, &"routes".into(), &routes);
	let vnode = create_element(&router_fn, &props.into(), JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("/users/42|?tab=profile|id=42"),
		"useLocation should expose the matched route's pathname, search string, and captured params"
	);
}

// ─── useOutletContext ───

fn outlet_context_reader(_props: JsValue) -> JsValue {
	let ctx = js_use_outlet_context();
	let text = if ctx.is_undefined() {
		"no-context".to_string()
	} else {
		Reflect::get(&ctx, &"msg".into()).ok().and_then(|v| v.as_string()).unwrap_or_default()
	};
	build_div_text(&text)
}

fn build_layout_with_outlet(outlet_fn: &JsValue, outlet_props: JsValue) -> JsValue {
	let outlet_vnode = create_element(outlet_fn, &outlet_props, JsValue::NULL).expect("createElement should succeed");
	let children = Array::new();
	children.push(&outlet_vnode);
	create_element(&JsValue::from_str("div"), &JsValue::NULL, children.into()).expect("createElement should succeed")
}

#[wasm_bindgen_test]
fn use_outlet_context_reads_the_value_passed_to_the_ancestor_outlet() {
	set_path("/dash/page");
	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");
	let outlet_fn = wrap_as_js_component(js_outlet, "Outlet");
	let reader_fn = wrap_as_js_component(outlet_context_reader, "Reader");

	let ctx_obj = Object::new();
	let _ = Reflect::set(&ctx_obj, &"msg".into(), &JsValue::from_str("hello-from-outlet"));
	let outlet_props = Object::new();
	let _ = Reflect::set(&outlet_props, &"context".into(), &ctx_obj);
	let layout_element = build_layout_with_outlet(&outlet_fn, outlet_props.into());

	let leaf_element = create_element(&reader_fn, &JsValue::NULL, JsValue::NULL).expect("createElement should succeed");
	let leaf_route = create_element(&route_fn, &build_route_props("page", leaf_element), JsValue::NULL).expect("createElement should succeed");
	let leaf_children = Array::new();
	leaf_children.push(&leaf_route);
	let parent_route =
		create_element(&route_fn, &build_route_props("/dash", layout_element), leaf_children.into()).expect("createElement should succeed");

	let routes_children = Array::new();
	routes_children.push(&parent_route);
	let routes_props = Object::new();
	let _ = Reflect::set(&routes_props, &"children".into(), &routes_children);
	let routes_vnode = create_element(&routes_fn, &routes_props.into(), JsValue::NULL).expect("createElement should succeed");

	let _root = micro_react::bindings::render(routes_vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("hello-from-outlet"),
		"useOutletContext should read the value passed via <Outlet context={{...}}/>"
	);
}

#[wasm_bindgen_test]
fn use_outlet_context_is_undefined_when_outlet_receives_no_context_prop() {
	set_path("/dash2/page");
	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");
	let outlet_fn = wrap_as_js_component(js_outlet, "Outlet");
	let reader_fn = wrap_as_js_component(outlet_context_reader, "Reader");

	// No `context` prop passed to <Outlet/> this time.
	let layout_element = build_layout_with_outlet(&outlet_fn, JsValue::NULL);

	let leaf_element = create_element(&reader_fn, &JsValue::NULL, JsValue::NULL).expect("createElement should succeed");
	let leaf_route = create_element(&route_fn, &build_route_props("page", leaf_element), JsValue::NULL).expect("createElement should succeed");
	let leaf_children = Array::new();
	leaf_children.push(&leaf_route);
	let parent_route =
		create_element(&route_fn, &build_route_props("/dash2", layout_element), leaf_children.into()).expect("createElement should succeed");

	let routes_children = Array::new();
	routes_children.push(&parent_route);
	let routes_props = Object::new();
	let _ = Reflect::set(&routes_props, &"children".into(), &routes_children);
	let routes_vnode = create_element(&routes_fn, &routes_props.into(), JsValue::NULL).expect("createElement should succeed");

	let _root = micro_react::bindings::render(routes_vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("no-context"),
		"useOutletContext should read as undefined when the matched <Outlet/> was not given a `context` prop"
	);
}

// ─── Route rendered standalone (outside <Routes>) ───

#[wasm_bindgen_test]
fn route_rendered_standalone_falls_back_to_rendering_its_own_element() {
	let container = make_container();
	let element = build_div_text("Solo Element");
	let props = build_route_props("/whatever", element);
	let out = js_route(props);
	let _root = micro_react::bindings::render(out, container.clone()).expect("render should succeed");
	assert_eq!(
		container.text_content().as_deref(),
		Some("Solo Element"),
		"Route rendered standalone (not looked up via collect_routes/<Routes>) should just render its own `element` prop"
	);
}

// ─── Nested routes: params from parent and child segments both captured ───

fn nested_params_reader(_props: JsValue) -> JsValue {
	let loc = js_use_location();
	let params = Reflect::get(&loc, &"params".into()).unwrap_or(JsValue::NULL);
	let user_id = Reflect::get(&params, &"userId".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "none".to_string());
	let post_id = Reflect::get(&params, &"postId".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "none".to_string());
	build_div_text(&format!("userId={user_id},postId={post_id}"))
}

#[wasm_bindgen_test]
fn nested_route_captures_param_segments_from_both_parent_and_child_path() {
	set_path("/users/7/posts/99");
	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");
	let reader_fn = wrap_as_js_component(nested_params_reader, "Reader");

	// Parent route has no `element` of its own (pure path-joining layer);
	// only the leaf renders anything.
	let leaf_element = create_element(&reader_fn, &JsValue::NULL, JsValue::NULL).expect("createElement should succeed");
	let leaf_route =
		create_element(&route_fn, &build_route_props("posts/:postId", leaf_element), JsValue::NULL).expect("createElement should succeed");
	let leaf_children = Array::new();
	leaf_children.push(&leaf_route);

	let parent_props = Object::new();
	let _ = Reflect::set(&parent_props, &"path".into(), &JsValue::from_str("/users/:userId"));
	let parent_route = create_element(&route_fn, &parent_props.into(), leaf_children.into()).expect("createElement should succeed");

	let routes_children = Array::new();
	routes_children.push(&parent_route);
	let routes_props = Object::new();
	let _ = Reflect::set(&routes_props, &"children".into(), &routes_children);
	let routes_vnode = create_element(&routes_fn, &routes_props.into(), JsValue::NULL).expect("createElement should succeed");

	let _root = micro_react::bindings::render(routes_vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("userId=7,postId=99"),
		"the matched leaf's params should include :param captures from both the parent route's path and its own"
	);
}

// ─── Router's popstate listener drives a re-render ───

#[wasm_bindgen_test]
fn router_rerenders_matched_route_after_a_real_popstate_event() {
	set_path("/first");
	let window = web_sys::window().expect("window should be available");
	let container = make_container();
	let router_fn = wrap_as_js_component(js_router, "Router");

	let routes = Array::new();
	for (pattern, text) in [("/first", "First"), ("/second", "Second")] {
		let pair = Array::new();
		pair.push(&JsValue::from_str(pattern));
		let owned = text.to_string();
		let handler: JsValue = Closure::wrap(Box::new(move || -> JsValue { build_div_text(&owned) }) as Box<dyn Fn() -> JsValue>).into_js_value();
		pair.push(&handler);
		routes.push(&pair);
	}
	let props = Object::new();
	let _ = Reflect::set(&props, &"routes".into(), &routes);
	let vnode = create_element(&router_fn, &props.into(), JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	assert_eq!(container.text_content().as_deref(), Some("First"));

	// Change the URL directly (as the browser does for a real back/forward
	// navigation) and fire the real `popstate` event Router's mount effect
	// is listening for, rather than going through Link/Navigate/useNavigate
	// (which push state *and* dispatch popstate themselves, so they'd never
	// exercise the listener in isolation the way an actual browser
	// back/forward gesture does).
	set_path("/second");
	window.dispatch_event(&web_sys::Event::new("popstate").expect("valid event name")).expect("dispatch should succeed");
	flush_rerenders();

	assert_eq!(
		container.text_content().as_deref(),
		Some("Second"),
		"Router's popstate listener should update the matched route when the location changes out from under it"
	);
}
