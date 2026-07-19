//! Tests for `router::Pattern` (compile + matches).
//!
//! `Pattern::matches` builds and executes a `js_sys::RegExp`, which calls
//! into a real JS engine — under a plain `cargo test --lib` target that
//! panics ("not supported outside wasm"), so like `tests/browser/reconciler.rs`
//! these run via `wasm-bindgen-test` in an actual (headless) browser:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox
//!
//! `Pattern`'s fields are private, so these go through the crate's public
//! surface (`compile` + `matches`) exactly as `router.rs`'s own
//! `js_router` does, rather than reaching into internals.

use js_sys::{Array, Object, Reflect};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::create_element;
use micro_react::hooks::use_state;
use micro_react::render::Root;
use micro_react::router::{Pattern, js_link, js_route, js_router, js_routes, js_use_navigate};
use micro_react::scheduler::flush_rerenders;
use micro_react::vnode::{ComponentFn, Props, VNode};

#[wasm_bindgen_test]
fn static_path_matches_exactly() {
	let p = Pattern::compile("/about");
	let params = p.matches("/about").expect("should match");
	assert!(params.is_empty());
}

#[wasm_bindgen_test]
fn static_path_does_not_match_other_paths() {
	let p = Pattern::compile("/about");
	assert!(p.matches("/contact").is_none());
	assert!(p.matches("/about/team").is_none());
	assert!(p.matches("/abou").is_none());
}

#[wasm_bindgen_test]
fn root_path_matches_root() {
	let p = Pattern::compile("/");
	assert!(p.matches("/").is_some());
}

#[wasm_bindgen_test]
fn single_param_segment_is_captured() {
	let p = Pattern::compile("/users/:id");
	let params = p.matches("/users/42").expect("should match");
	assert_eq!(params.get("id"), Some(&"42".to_string()));
}

#[wasm_bindgen_test]
fn multiple_param_segments_are_all_captured() {
	let p = Pattern::compile("/users/:userId/posts/:postId");
	let params = p.matches("/users/7/posts/99").expect("should match");
	assert_eq!(params.get("userId"), Some(&"7".to_string()));
	assert_eq!(params.get("postId"), Some(&"99".to_string()));
}

#[wasm_bindgen_test]
fn param_segment_does_not_match_across_slashes() {
	let p = Pattern::compile("/users/:id");
	// "42/extra" should not be captured as a single :id segment.
	assert!(p.matches("/users/42/extra").is_none());
}

#[wasm_bindgen_test]
fn wildcard_matches_anything_including_nested_segments() {
	let p = Pattern::compile("/files/*");
	assert!(p.matches("/files/a").is_some());
	assert!(p.matches("/files/a/b/c.txt").is_some());
	assert!(p.matches("/files/").is_some());
}

#[wasm_bindgen_test]
fn trailing_slash_is_tolerated() {
	let p = Pattern::compile("/about");
	assert!(p.matches("/about/").is_some());
}

#[wasm_bindgen_test]
fn mixed_static_and_param_segments() {
	let p = Pattern::compile("/blog/:year/:slug");
	let params = p.matches("/blog/2024/hello-world").expect("should match");
	assert_eq!(params.get("year"), Some(&"2024".to_string()));
	assert_eq!(params.get("slug"), Some(&"hello-world".to_string()));
}

#[wasm_bindgen_test]
fn regex_special_characters_in_static_segments_are_escaped() {
	// A literal "." in a path segment must not act as a regex wildcard.
	let p = Pattern::compile("/v1.0/status");
	assert!(p.matches("/v1.0/status").is_some());
	// If "." weren't escaped, "v1X0" would also match — it must not.
	assert!(p.matches("/v1X0/status").is_none());
}

#[wasm_bindgen_test]
fn empty_pattern_matches_only_root() {
	let p = Pattern::compile("");
	assert!(p.matches("/").is_some());
	assert!(p.matches("/x").is_none());
}

#[wasm_bindgen_test]
fn param_segment_captures_unicode_characters() {
	let p = Pattern::compile("/users/:name");
	let params = p.matches("/users/José").expect("should match");
	assert_eq!(params.get("name"), Some(&"José".to_string()));
}

#[wasm_bindgen_test]
fn matching_is_case_sensitive_for_static_segments() {
	let p = Pattern::compile("/About");
	assert!(p.matches("/About").is_some());
	assert!(p.matches("/about").is_none());
}

#[wasm_bindgen_test]
fn param_segment_can_contain_dots_and_hyphens() {
	let p = Pattern::compile("/files/:name");
	let params = p.matches("/files/report-v1.2.pdf").expect("should match");
	assert_eq!(params.get("name"), Some(&"report-v1.2.pdf".to_string()));
}

#[wasm_bindgen_test]
fn trailing_wildcard_after_static_prefix_matches_deep_paths() {
	let p = Pattern::compile("/docs/*");
	assert!(p.matches("/docs").is_none(), "wildcard requires the /docs/ prefix to be present");
	assert!(p.matches("/docs/a/b/c").is_some());
}

#[wasm_bindgen_test]
fn bare_wildcard_pattern_matches_everything() {
	let p = Pattern::compile("/*");
	assert!(p.matches("/").is_some());
	assert!(p.matches("/anything/at/all").is_some());
}

#[wasm_bindgen_test]
fn param_name_repeated_in_pattern_keeps_the_last_captured_value() {
	// Not necessarily desirable behavior, but pins down what actually
	// happens today: both capture groups map to the same key, so the
	// later one in iteration order wins in the resulting HashMap.
	let p = Pattern::compile("/:id/nested/:id");
	let params = p.matches("/1/nested/2").expect("should match");
	assert_eq!(params.get("id"), Some(&"2".to_string()));
}

#[wasm_bindgen_test]
fn pattern_with_only_slashes_behaves_like_root() {
	let p = Pattern::compile("//");
	assert!(p.matches("/").is_some());
}

#[wasm_bindgen_test]
fn param_segment_does_not_match_an_empty_segment() {
	let p = Pattern::compile("/users/:id");
	assert!(p.matches("/users/").is_none());
}

#[wasm_bindgen_test]
fn static_segment_with_regex_metacharacters_is_escaped_throughout() {
	let p = Pattern::compile("/a+b(c)/x?");
	assert!(p.matches("/a+b(c)/x?").is_some());
	// If unescaped, "+" "(" ")" "?" would all behave as regex operators.
	assert!(p.matches("/aab/x").is_none());
}

// ─── useNavigate memoization ───

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

// `useNavigate` wraps its closure in `use_memo` with empty deps so the same
// `Closure`/`JsValue` is handed back on every render instead of a fresh one
// leaking each time. This drives two renders of the same component instance
// and checks the returned JsValue is the same JS function reference both times.
#[wasm_bindgen_test]
fn use_navigate_returns_same_closure_across_rerenders() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let captured: Rc<RefCell<Vec<JsValue>>> = Rc::new(RefCell::new(Vec::new()));
	let captured_for_comp = captured.clone();
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (_tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		captured_for_comp.borrow_mut().push(js_use_navigate());
		VNode::text("x")
	});
	root.render(VNode::component("NavComp", comp, vec![])).unwrap();
	assert_eq!(captured.borrow().len(), 1, "expected exactly one render so far");

	// Force a second render of the same instance via setState, not a fresh mount.
	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();
	assert_eq!(captured.borrow().len(), 2, "expected the re-render to have happened");

	let first = captured.borrow()[0].clone();
	let second = captured.borrow()[1].clone();
	assert!(first.loose_eq(&second), "useNavigate should return the same memoized closure across re-renders, not a fresh one each time");
}

// ─── Link onclick handler stability across re-renders ───

// `Link` is wired up here exactly the way `createElement` (JSX) would use
// it: as a plain JS function passed as `type_`, so this exercises the real
// `js_link` entry point rather than any Rust-only shortcut.
//
// `create_element` reads the wrapped function's JS `name` property to tag
// the resulting `Component` vnode (`router.rs::collect_routes` later
// matches on that name being exactly `"Route"`), so it has to be set
// explicitly here — a `Closure`-derived function has no meaningful default
// name of its own.
fn wrap_as_js_component(f: fn(JsValue) -> JsValue, name: &str) -> JsValue {
	let closure: JsValue = Closure::wrap(Box::new(f) as Box<dyn Fn(JsValue) -> JsValue>).into_js_value();
	let descriptor = Object::new();
	let _ = Reflect::set(&descriptor, &"value".into(), &JsValue::from_str(name));
	let _ = Reflect::set(&descriptor, &"configurable".into(), &JsValue::TRUE);
	let _: Object = js_sys::Object::define_property(closure.unchecked_ref(), &"name".into(), &descriptor);
	closure
}

fn build_link_props(to: &str) -> JsValue {
	let props = Object::new();
	let _ = Reflect::set(&props, &"to".into(), &JsValue::from_str(to));
	let _ = Reflect::set(&props, &"children".into(), &JsValue::from_str("Home"));
	props.into()
}

// `Link`'s onclick handler is built once (via `use_memo` keyed on `to`) and
// reused, rather than a fresh `Closure` leaking on every render. This mounts
// a `<Link>`, re-renders the same tree with equivalent props, and checks
// the DOM's registered click listener (`__mrListeners.click`, set by
// `events.rs::set_event_handler`) is the same JS function reference both
// times rather than a newly built one.
#[wasm_bindgen_test]
fn link_onclick_handler_is_stable_across_rerenders() {
	let container = make_container();
	let link_fn = wrap_as_js_component(js_link, "Link");

	let vnode_1 = create_element(&link_fn, &build_link_props("/x"), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("initial render should succeed");

	let anchor = container.query_selector("a").expect("query should not error").expect("expected an <a> element");
	let listeners_1 = Reflect::get(anchor.as_ref(), &"__mrListeners".into()).expect("listeners map should exist after first render");
	let handler_1 = Reflect::get(&listeners_1, &"click".into()).expect("click listener should be registered after first render");
	assert!(handler_1.is_function(), "expected a click handler to be registered on the anchor");

	let vnode_2 = create_element(&link_fn, &build_link_props("/x"), JsValue::NULL).expect("createElement should succeed");
	root.render(vnode_2).expect("second render should succeed");

	let listeners_2 = Reflect::get(anchor.as_ref(), &"__mrListeners".into()).expect("listeners map should exist after second render");
	let handler_2 = Reflect::get(&listeners_2, &"click".into()).expect("click listener should still be registered after second render");

	assert!(
		js_sys::Object::is(&handler_1, &handler_2),
		"Link's onclick handler should be memoized (the same Function reference) across re-renders with equivalent props, not rebuilt every render"
	);
}

// The memoization fix above must be keyed on `to` — if it were memoized
// with permanently-empty deps (the same mistake `Routes` had), a `<Link>`
// whose `to` prop actually changes across renders would keep firing the
// *old* target's handler, silently navigating to the wrong place. This
// pins down that the handler *does* change when `to` does.
#[wasm_bindgen_test]
fn link_onclick_handler_changes_when_to_changes() {
	let container = make_container();
	let link_fn = wrap_as_js_component(js_link, "Link");

	let vnode_1 = create_element(&link_fn, &build_link_props("/x"), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("initial render should succeed");

	let anchor = container.query_selector("a").expect("query should not error").expect("expected an <a> element");
	let listeners_1 = Reflect::get(anchor.as_ref(), &"__mrListeners".into()).expect("listeners map should exist after first render");
	let handler_1 = Reflect::get(&listeners_1, &"click".into()).expect("click listener should be registered after first render");

	let vnode_2 = create_element(&link_fn, &build_link_props("/y"), JsValue::NULL).expect("createElement should succeed");
	root.render(vnode_2).expect("second render should succeed");

	assert_eq!(anchor.get_attribute("href").as_deref(), Some("/y"), "expected the anchor's href to reflect the new `to` prop");

	let listeners_2 = Reflect::get(anchor.as_ref(), &"__mrListeners".into()).expect("listeners map should exist after second render");
	let handler_2 = Reflect::get(&listeners_2, &"click".into()).expect("click listener should still be registered after second render");

	assert!(
		!js_sys::Object::is(&handler_1, &handler_2),
		"Link's onclick handler should be rebuilt when `to` changes, not kept from the previous `to` value"
	);
}

// ─── Routes recomputes its route table when children change ───

fn build_route_props(path: &str, element: JsValue) -> JsValue {
	let props = Object::new();
	let _ = Reflect::set(&props, &"path".into(), &JsValue::from_str(path));
	let _ = Reflect::set(&props, &"element".into(), &element);
	props.into()
}

fn build_div_text(text: &str) -> JsValue {
	let children = js_sys::Array::new();
	children.push(&JsValue::from_str(text));
	create_element(&JsValue::from_str("div"), &JsValue::NULL, children.into()).expect("createElement should succeed")
}

fn build_routes_vnode(route_fn: &JsValue, routes_fn: &JsValue, entries: &[(&str, &str)]) -> JsValue {
	let children = js_sys::Array::new();
	for (path, text) in entries {
		let element = build_div_text(text);
		let route_vnode = create_element(route_fn, &build_route_props(path, element), JsValue::NULL).expect("createElement should succeed");
		children.push(&route_vnode);
	}
	let routes_props = Object::new();
	let _ = Reflect::set(&routes_props, &"children".into(), &children);
	create_element(routes_fn, &routes_props.into(), JsValue::NULL).expect("createElement should succeed")
}

// `Routes` flattens its `<Route>` children into a lookup table on every
// render (not memoized with permanently-empty deps, which used to make the
// table stick forever). This pins down that re-rendering `<Routes>` with a
// *different* set of `<Route>` children correctly updates which route matches.
#[wasm_bindgen_test]
fn routes_recomputes_route_table_when_children_change() {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/a"));

	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");

	// First render: only "/a" is a known route, and it matches the current
	// location, so its element ("First") should be what's rendered.
	let vnode_1 = build_routes_vnode(&route_fn, &routes_fn, &[("/a", "First")]);
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("initial render should succeed");
	assert_eq!(container.text_content().as_deref(), Some("First"), "expected the initially matched route's element to render");

	// Second render: the parent now supplies a *different* route set that no
	// longer includes "/a" at all. If the route table were recomputed per
	// render (as `Routes`'s JSX suggests it should be), this would now fall
	// through to the "404 Not Found" default. Because the table is memoized
	// forever, the stale "/a" entry from the first render keeps matching instead.
	let vnode_2 = build_routes_vnode(&route_fn, &routes_fn, &[("/b", "Second")]);
	root.render(vnode_2).expect("second render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("404 Not Found"),
		"Routes should recompute its route table from the new children and fall through to the 404 default \
		 once \"/a\" is no longer among them, instead of continuing to serve the route table built on first mount"
	);
}

// Complements the test above with the positive case: the current path stays
// matched across both renders, but the *matched route's own content*
// changes. This rules out a fix that only handles "route disappears ->
// falls through to 404" but not "route survives with different content."
#[wasm_bindgen_test]
fn routes_reflects_updated_element_for_a_route_that_still_matches() {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/a"));

	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");

	let vnode_1 = build_routes_vnode(&route_fn, &routes_fn, &[("/a", "First")]);
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("initial render should succeed");
	assert_eq!(container.text_content().as_deref(), Some("First"));

	// Same pattern ("/a"), same matching path, but a different element.
	let vnode_2 = build_routes_vnode(&route_fn, &routes_fn, &[("/a", "First Updated")]);
	root.render(vnode_2).expect("second render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("First Updated"),
		"Routes should reflect the new element for a route that still matches, not the element captured on first mount"
	);
}

// `js_routes` (`Routes`) takes a `JsValue` props object directly rather
// than matching `js_link`/`js_route`'s bare-value signature, so it needs
// its own thin `fn(JsValue) -> JsValue` adapter to be usable with
// `wrap_as_js_component`/`create_element`.
fn js_routes_as_fn(props: JsValue) -> JsValue {
	js_routes(props).unwrap_or(JsValue::NULL)
}

// ─── `Router({ routes })` match order (Array vs. Object) ───
//
// See the TODO/review note this addresses: `Object::keys` enumeration order
// hoists keys that parse as a canonical array index (e.g. "1") ahead of any
// non-index string key (e.g. "*"), *regardless of the order they were
// written in the object literal*. `Router` picks the first matching
// pattern, so a plain-Object `routes` table with such a key is exposed to
// out-of-declaration-order matching. Passing an `Array` of `[pattern, fn]`
// pairs instead sidesteps this entirely, since array order is always
// insertion order.

fn route_handler(text: &str) -> JsValue {
	let owned = text.to_string();
	let closure = Closure::wrap(Box::new(move || -> JsValue { build_div_text(&owned) }) as Box<dyn Fn() -> JsValue>);
	closure.into_js_value()
}

fn set_path(path: &str) {
	let window = web_sys::window().expect("window should be available");
	let history = window.history().expect("history should be available");
	let _ = history.push_state_with_url(&JsValue::NULL, "", Some(path));
}

fn router_props_from(routes: &JsValue) -> JsValue {
	let props = Object::new();
	let _ = Reflect::set(&props, &"routes".into(), routes);
	props.into()
}

// The positive case: an `Array` of `[pattern, fn]` pairs is matched in
// exactly the order given, even though one pattern ("1") looks like a JS
// array index and would normally be hoisted ahead of a non-index pattern
// ("*") in a plain object's enumeration order.
#[wasm_bindgen_test]
fn router_matches_array_routes_in_declaration_order() {
	set_path("/1");
	let container = make_container();
	let router_fn = wrap_as_js_component(js_router, "Router");

	let routes = Array::new();
	let wildcard_pair = Array::new();
	wildcard_pair.push(&JsValue::from_str("*"));
	wildcard_pair.push(&route_handler("wildcard"));
	routes.push(&wildcard_pair);
	let numeric_pair = Array::new();
	numeric_pair.push(&JsValue::from_str("1"));
	numeric_pair.push(&route_handler("numeric"));
	routes.push(&numeric_pair);

	let vnode = create_element(&router_fn, &router_props_from(&routes.into()), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("wildcard"),
		"an Array of [pattern, fn] pairs should be matched in the order given, so the wildcard \
		 pattern declared first should win even though \"1\" looks like an array index"
	);

	root.unmount();
	container.remove();
}

// The documented quirk this replaces: a plain `Object` routes table with the
// *same* declaration order ("*" written before "1") does NOT preserve that
// order, because "1" is a canonical array-index key and gets enumerated
// ahead of "*" by the JS engine regardless of insertion order. This pins
// down the caveat `route_entries`'s doc comment calls out, so a future
// change that accidentally "fixes" Object ordering (which isn't actually
// possible without changing the input's own JS semantics) is at least
// forced to update this test rather than silently regressing the
// documentation.
#[wasm_bindgen_test]
fn router_object_routes_hoist_index_like_keys_ahead_of_declaration_order() {
	set_path("/1");
	let container = make_container();
	let router_fn = wrap_as_js_component(js_router, "Router");

	let routes = Object::new();
	let _ = Reflect::set(&routes, &"*".into(), &route_handler("wildcard"));
	let _ = Reflect::set(&routes, &"1".into(), &route_handler("numeric"));

	let vnode = create_element(&router_fn, &router_props_from(&routes.into()), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("numeric"),
		"a plain Object's \"1\" key is enumerated ahead of \"*\" by the JS engine even though \"*\" \
		 was written first — this is the caveat callers should avoid by passing an Array instead"
	);

	root.unmount();
	container.remove();
}

// `<Routes>` builds its flattened route table as an Array of [pattern, fn]
// pairs (not a plain Object) precisely so it never depends on the
// Object-key-ordering quirk above, even if a `<Route path="...">` happens to
// look like an array index. This mounts a `<Route path="1">` ahead of a
// `<Route path="*">` and checks the first-declared one still wins.
#[wasm_bindgen_test]
fn routes_preserves_declaration_order_for_index_like_route_paths() {
	set_path("/1");
	let container = make_container();
	let route_fn = wrap_as_js_component(js_route, "Route");
	let routes_fn = wrap_as_js_component(js_routes_as_fn, "Routes");

	let vnode = build_routes_vnode(&route_fn, &routes_fn, &[("1", "first-declared"), ("*", "wildcard")]);
	let root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	assert_eq!(
		container.text_content().as_deref(),
		Some("first-declared"),
		"Routes should match the first-declared <Route> regardless of whether its path looks \
		 like an array index"
	);

	root.unmount();
	container.remove();
}
