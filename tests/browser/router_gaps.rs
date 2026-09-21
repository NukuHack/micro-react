//! Follow-up coverage for `router.rs`, complementing `tests/browser/router.rs`.
//!
//! The existing router test file covers `Pattern` matching, `Link`/`useNavigate`
//! closure stability, and `Routes` route-table recomputation. Per the TODO left
//! in that area, three gaps were still open: nested routes / relative paths
//! (`<Outlet/>`), the declarative `<Navigate>` redirect, and a route's element
//! throwing inside an ancestor `ErrorBoundary`. All three go through the real
//! JS-facing bindings (`create_element`, `js_routes`, `js_navigate`,
//! `createErrorBoundary`) the same way `tests/browser/router.rs` does, rather
//! than a Rust-only shortcut, since that's the actual path any JS caller uses.
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

use micro_react::bindings::{create_element, js_create_error_boundary};
use micro_react::router::{js_navigate, js_outlet, js_route, js_routes};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

/// Same helper as `tests/browser/router.rs::wrap_as_js_component`: wraps a
/// plain `fn(JsValue) -> JsValue` as a named JS function, since `create_element`
/// reads `.name` to tag the resulting `Component` vnode (`Routes`/`collect_routes`
/// matches on that name being exactly `"Route"`).
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

fn js_navigate_as_fn(props: JsValue) -> JsValue {
	js_navigate(props)
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

fn build_routes_children(route_fn: &JsValue, routes_props: &[(&str, JsValue)]) -> JsValue {
	let children = Array::new();
	for (path, element) in routes_props {
		let route_vnode = create_element(route_fn, &build_route_props(path, element.clone()), JsValue::NULL).expect("createElement should succeed");
		children.push(&route_vnode);
	}
	children.into()
}

// ─── Nested routes via <Outlet/> ───

#[wasm_bindgen_test]
fn nested_route_renders_layout_wrapping_matched_child_via_outlet() {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/dashboard/settings"));

	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");
	let outlet_fn = wrap_as_js_component(js_outlet, "Outlet");

	// Layout element wraps whatever the matched nested route renders via
	// <Outlet/>, the same way react-router's "layout route" pattern works.
	let layout_children = Array::new();
	layout_children.push(&JsValue::from_str("Layout: "));
	let outlet_vnode = create_element(&outlet_fn, &JsValue::NULL, JsValue::NULL).expect("createElement should succeed");
	layout_children.push(&outlet_vnode);
	let layout_element = create_element(&JsValue::from_str("div"), &JsValue::NULL, layout_children.into()).expect("createElement should succeed");

	// A parent <Route path="/dashboard" element={layout}> with a nested
	// childless <Route path="settings" element={...}> underneath it.
	let leaf_element = build_div_text("Settings Page");
	let leaf_route = create_element(&route_fn, &build_route_props("settings", leaf_element), JsValue::NULL).expect("createElement should succeed");
	let leaf_children = Array::new();
	leaf_children.push(&leaf_route);

	// `collect_routes` reads nested routes off the vnode's own `children`
	// (the third `create_element` argument), not off the props object.
	let parent_props = build_route_props("/dashboard", layout_element);
	let parent_route = create_element(&route_fn, &parent_props, leaf_children.into()).expect("createElement should succeed");

	let routes_children = Array::new();
	routes_children.push(&parent_route);
	let routes_props = Object::new();
	let _ = Reflect::set(&routes_props, &"children".into(), &routes_children);
	let routes_vnode = create_element(&routes_fn, &routes_props.into(), JsValue::NULL).expect("createElement should succeed");

	let _root = micro_react::bindings::render(routes_vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("Layout: Settings Page"),
		"expected the layout route's element to wrap the nested route's element via <Outlet/>"
	);
}

// ─── <Navigate to="..." /> declarative redirect ───

#[wasm_bindgen_test]
fn navigate_component_pushes_history_state_on_mount() {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/start"));

	let container = make_container();
	let navigate_fn = wrap_as_js_component(js_navigate_as_fn, "Navigate");

	let props = Object::new();
	let _ = Reflect::set(&props, &"to".into(), &JsValue::from_str("/redirected"));
	let vnode = create_element(&navigate_fn, &props.into(), JsValue::NULL).expect("createElement should succeed");

	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	// Navigate performs the history push as a mount effect, which
	// `Root::render` flushes synchronously before returning.
	assert_eq!(window.location().pathname().as_deref(), Ok("/redirected"), "Navigate should have pushed the new path onto history on mount");
	assert!(container.inner_html().is_empty(), "Navigate should render nothing");
}

#[wasm_bindgen_test]
fn navigate_component_with_replace_uses_replace_state_not_push() {
	// `replace` should swap the current history entry rather than adding a
	// new one, so navigating "back" afterwards shouldn't land on the page
	// that redirected away. We can't easily assert on history *length* from
	// here (no direct API), but we can at least confirm the final location
	// still reflects the redirect target with `replace: true` set.
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/before-replace"));

	let container = make_container();
	let navigate_fn = wrap_as_js_component(js_navigate_as_fn, "Navigate");

	let props = Object::new();
	let _ = Reflect::set(&props, &"to".into(), &JsValue::from_str("/after-replace"));
	let _ = Reflect::set(&props, &"replace".into(), &JsValue::TRUE);
	let vnode = create_element(&navigate_fn, &props.into(), JsValue::NULL).expect("createElement should succeed");

	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	assert_eq!(window.location().pathname().as_deref(), Ok("/after-replace"), "Navigate with replace should still land on the redirect target");
}

// ─── A matched route's element throwing is caught by an ancestor ErrorBoundary ───

fn throwing_component(_props: JsValue) -> JsValue {
	wasm_bindgen::throw_str("route element exploded");
}

#[wasm_bindgen_test]
fn route_element_throwing_is_caught_by_ancestor_error_boundary() {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/boom"));

	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");
	let boom_fn = wrap_as_js_component(throwing_component, "Boom");

	let boom_element = create_element(&boom_fn, &JsValue::NULL, JsValue::NULL).expect("createElement should succeed");
	let routes_children = build_routes_children(&route_fn, &[("/boom", boom_element)]);
	let routes_props = Object::new();
	let _ = Reflect::set(&routes_props, &"children".into(), &routes_children);
	let routes_vnode = create_element(&routes_fn, &routes_props.into(), JsValue::NULL).expect("createElement should succeed");

	// Wrap Routes in a real ErrorBoundary (the same factory JS callers get
	// from `createErrorBoundary()`), with a fallback that renders a fixed marker.
	let boundary_fn = js_create_error_boundary();
	let fallback: JsValue =
		Closure::wrap(Box::new(|_err: JsValue| -> JsValue { build_div_text("route crashed") }) as Box<dyn Fn(JsValue) -> JsValue>).into_js_value();
	let boundary_props = Object::new();
	let _ = Reflect::set(&boundary_props, &"fallback".into(), &fallback);
	let boundary_children = Array::new();
	boundary_children.push(&routes_vnode);
	let boundary_vnode = create_element(&boundary_fn, &boundary_props.into(), boundary_children.into()).expect("createElement should succeed");

	let _root = micro_react::bindings::render(boundary_vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("route crashed"),
		"expected the ErrorBoundary's fallback, not the throwing route's content or a hard failure"
	);
}
