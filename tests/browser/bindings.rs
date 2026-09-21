//! Follow-up coverage for `src/bindings.rs`, the JS-facing surface.
//!
//! `bindings.rs` already has an internal `#[cfg(test)] mod vnode_store_tests`
//! covering the VNode slot-map leak-guard/idempotent-take behavior (pure
//! Rust logic, no JS engine needed). Everything else in that file needs a
//! real JS engine to marshal `JsValue`s through, so — like the rest of
//! `tests/browser/` — these run via `wasm-bindgen-test` in a headless browser:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox
//!
//! This file covers a slice of what was still untested per the TODO:
//! `create_element`'s numeric/boolean key coercion, `ref` extraction for
//! both the callback-ref and object-ref (`{ current }`) shapes, `useState`'s
//! functional-updater path resolving against the live cell, `useState`'s
//! setter identity caching across re-renders, and `memo`'s skip-vs-rerender
//! behavior (default shallow-equal and a custom `compare`).

use js_sys::{Array, Object, Reflect};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::{create_element, js_memo, js_use_state};
use micro_react::hooks::use_state;
use micro_react::render::Root;
use micro_react::vnode::{ComponentFn, Props, VNode};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

fn wrap_as_js_component(f: impl Fn(JsValue) -> JsValue + 'static, name: &str) -> JsValue {
	let closure: JsValue = Closure::wrap(Box::new(f) as Box<dyn Fn(JsValue) -> JsValue>).into_js_value();
	let descriptor = Object::new();
	let _ = Reflect::set(&descriptor, &"value".into(), &JsValue::from_str(name));
	let _ = Reflect::set(&descriptor, &"configurable".into(), &JsValue::TRUE);
	let _: Object = js_sys::Object::define_property(closure.unchecked_ref(), &"name".into(), &descriptor);
	closure
}

// ─── create_element: numeric/boolean key coercion ───

fn li_with_js_key(key: JsValue, text: &str) -> JsValue {
	let props = Object::new();
	let _ = Reflect::set(&props, &"key".into(), &key);
	let children = Array::new();
	children.push(&JsValue::from_str(text));
	create_element(&JsValue::from_str("li"), &props.into(), children.into()).expect("createElement should succeed")
}

fn collect_li_texts(container: &web_sys::Element) -> Vec<String> {
	let children = container.children();
	(0..children.length()).map(|i| children.item(i).unwrap().text_content().unwrap_or_default()).collect()
}

#[wasm_bindgen_test]
fn numeric_key_is_coerced_and_preserves_identity_across_reorder() {
	// A numeric `key` prop (e.g. `key={0}`, `key={1}`) must be stringified
	// like React/JS does, not silently dropped (which would collapse
	// distinct numeric keys to "no key" and break keyed reconciliation).
	let container = make_container();

	let list_1 = Array::new();
	list_1.push(&li_with_js_key(JsValue::from_f64(0.0), "Zero"));
	list_1.push(&li_with_js_key(JsValue::from_f64(1.0), "One"));
	let vnode_1 = create_element(&get_fragment(), &JsValue::NULL, list_1.into()).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("initial render should succeed");
	assert_eq!(collect_li_texts(&container), vec!["Zero", "One"]);

	let zero_node = container.children().item(0).unwrap();
	let one_node = container.children().item(1).unwrap();

	// Reorder: key 1 first, then key 0.
	let list_2 = Array::new();
	list_2.push(&li_with_js_key(JsValue::from_f64(1.0), "One"));
	list_2.push(&li_with_js_key(JsValue::from_f64(0.0), "Zero"));
	let vnode_2 = create_element(&get_fragment(), &JsValue::NULL, list_2.into()).expect("createElement should succeed");
	root.render(vnode_2).expect("second render should succeed");

	assert_eq!(collect_li_texts(&container), vec!["One", "Zero"]);
	assert!(
		container.children().item(0).unwrap().is_same_node(Some(&one_node)),
		"numeric key 1's DOM node should have moved, not been recreated (i.e. the key was actually used for reconciliation)"
	);
	assert!(container.children().item(1).unwrap().is_same_node(Some(&zero_node)), "numeric key 0's DOM node should have moved, not been recreated");
}

#[wasm_bindgen_test]
fn boolean_key_is_coerced_to_a_distinct_string_key() {
	// `key={true}` and `key={false}` should become distinct keys ("true"/
	// "false"), not both collapse to "no key" (which would make the two
	// items indistinguishable to the reconciler).
	let container = make_container();

	let list_1 = Array::new();
	list_1.push(&li_with_js_key(JsValue::from_bool(true), "True"));
	list_1.push(&li_with_js_key(JsValue::from_bool(false), "False"));
	let vnode_1 = create_element(&get_fragment(), &JsValue::NULL, list_1.into()).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("initial render should succeed");
	assert_eq!(collect_li_texts(&container), vec!["True", "False"]);

	let true_node = container.children().item(0).unwrap();

	let list_2 = Array::new();
	list_2.push(&li_with_js_key(JsValue::from_bool(false), "False"));
	list_2.push(&li_with_js_key(JsValue::from_bool(true), "True"));
	let vnode_2 = create_element(&get_fragment(), &JsValue::NULL, list_2.into()).expect("createElement should succeed");
	root.render(vnode_2).expect("second render should succeed");

	assert_eq!(collect_li_texts(&container), vec!["False", "True"]);
	assert!(
		container.children().item(1).unwrap().is_same_node(Some(&true_node)),
		"key=true's DOM node should have moved to the new position, not been recreated"
	);
}

fn get_fragment() -> JsValue {
	micro_react::bindings::get_fragment()
}

// ─── ref extraction: object-ref vs callback-ref ───

#[wasm_bindgen_test]
fn object_ref_current_is_synced_to_the_dom_node() {
	let container = make_container();
	let ref_obj = Object::new();
	let props = Object::new();
	let _ = Reflect::set(&props, &"ref".into(), &ref_obj);
	let vnode = create_element(&JsValue::from_str("input"), &props.into(), JsValue::NULL).expect("createElement should succeed");
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	let current = Reflect::get(&ref_obj, &"current".into()).expect("ref object should have a current property");
	assert!(current.is_object(), "ref.current should have been synced to the mounted DOM node");
	let el: web_sys::Element = current.dyn_into().expect("ref.current should be the <input> element");
	assert_eq!(el.tag_name().to_lowercase(), "input");
}

#[wasm_bindgen_test]
fn object_ref_current_is_cleared_on_unmount() {
	let container = make_container();
	let ref_obj = Object::new();
	let props = Object::new();
	let _ = Reflect::set(&props, &"ref".into(), &ref_obj);
	let vnode = create_element(&JsValue::from_str("input"), &props.into(), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	root.unmount();
	let current = Reflect::get(&ref_obj, &"current".into()).unwrap_or(JsValue::UNDEFINED);
	assert!(current.is_null() || current.is_undefined(), "ref.current should be cleared once the node unmounts");
}

#[wasm_bindgen_test]
fn callback_ref_is_invoked_with_the_dom_node_then_null_on_unmount() {
	let container = make_container();
	let seen: Rc<RefCell<Vec<Option<web_sys::Node>>>> = Rc::new(RefCell::new(Vec::new()));
	let seen_for_cb = seen.clone();
	let callback_ref: JsValue = Closure::wrap(Box::new(move |node: JsValue| {
		let n: Option<web_sys::Node> = node.dyn_into().ok();
		seen_for_cb.borrow_mut().push(n);
	}) as Box<dyn Fn(JsValue)>)
	.into_js_value();

	let props = Object::new();
	let _ = Reflect::set(&props, &"ref".into(), &callback_ref);
	let vnode = create_element(&JsValue::from_str("input"), &props.into(), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	assert_eq!(seen.borrow().len(), 1, "callback ref should have been invoked once on mount");
	assert!(seen.borrow()[0].is_some(), "callback ref should have received the DOM node on mount");

	root.unmount();
	assert_eq!(seen.borrow().len(), 2, "callback ref should be invoked again on unmount");
	assert!(seen.borrow()[1].is_none(), "callback ref should receive None/null on unmount");
}

// ─── useState functional updater ───

#[wasm_bindgen_test]
fn use_state_functional_updater_resolves_against_live_cell_not_stale_snapshot() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let setter_slot: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let arr = js_use_state(JsValue::from_f64(0.0));
		let value = arr.get(0);
		let setter = arr.get(1);
		*setter_slot_for_comp.borrow_mut() = Some(setter);
		VNode::text(value.as_f64().unwrap_or(-1.0).to_string())
	});
	root.render(VNode::component("Counter", comp, vec![])).unwrap();
	assert_eq!(container.text_content().as_deref(), Some("0"));

	let setter = setter_slot.borrow().clone().expect("setter should have been captured");
	let setter_fn: js_sys::Function = setter.unchecked_into();

	// Two functional updates queued back-to-back in the same tick, each
	// depending on the *result* of the previous one. If the updater
	// resolved against a stale snapshot instead of the live cell, both
	// would read the same starting value and the final result would be
	// wrong (e.g. 1 instead of 2).
	let updater: JsValue = Closure::once_into_js(move |prev: JsValue| -> JsValue { JsValue::from_f64(prev.as_f64().unwrap_or(0.0) + 1.0) });
	let _ = setter_fn.call1(&JsValue::NULL, &updater);
	let updater2: JsValue = Closure::once_into_js(move |prev: JsValue| -> JsValue { JsValue::from_f64(prev.as_f64().unwrap_or(0.0) + 1.0) });
	let _ = setter_fn.call1(&JsValue::NULL, &updater2);

	micro_react::scheduler::flush_rerenders();
	assert_eq!(
		container.text_content().as_deref(),
		Some("2"),
		"both functional updates should have applied against the live value, not a stale snapshot"
	);
}

#[wasm_bindgen_test]
fn use_state_setter_identity_is_cached_across_rerenders() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let captured: Rc<RefCell<Vec<JsValue>>> = Rc::new(RefCell::new(Vec::new()));
	let captured_for_comp = captured.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		let arr = js_use_state(JsValue::from_f64(0.0));
		captured_for_comp.borrow_mut().push(arr.get(1));
		if tick == 0 {
			set_tick(1); // force a second render on the same instance
		}
		VNode::text("x")
	});
	root.render(VNode::component("Comp", comp, vec![])).unwrap();
	micro_react::scheduler::flush_rerenders();

	assert_eq!(captured.borrow().len(), 2, "expected two renders of the same instance");
	let first = captured.borrow()[0].clone();
	let second = captured.borrow()[1].clone();
	assert!(
		js_sys::Object::is(&first, &second),
		"useState's JS setter should be the same cached Function reference across re-renders of the same hook slot"
	);
}

// ─── memo(): skip vs. re-render ───

fn make_memo_child(render_count: Rc<RefCell<u32>>) -> JsValue {
	wrap_as_js_component(
		move |props: JsValue| -> JsValue {
			*render_count.borrow_mut() += 1;
			let text = Reflect::get(&props, &"label".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
			let children = Array::new();
			children.push(&JsValue::from_str(&text));
			create_element(&JsValue::from_str("div"), &JsValue::NULL, children.into()).expect("createElement should succeed")
		},
		"MemoChild",
	)
}

#[wasm_bindgen_test]
fn memo_skips_rerender_on_shallow_equal_props_and_rerenders_on_change() {
	let container = make_container();
	let render_count = Rc::new(RefCell::new(0u32));
	let child_fn = make_memo_child(render_count.clone());
	let child_fn: js_sys::Function = child_fn.unchecked_into();
	let memoized = js_memo(&child_fn, JsValue::UNDEFINED).expect("memo() should succeed");
	let memoized: JsValue = memoized;

	let props_1 = Object::new();
	let _ = Reflect::set(&props_1, &"label".into(), &JsValue::from_str("hello"));
	let vnode_1 = create_element(&memoized, &props_1.into(), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("render should succeed");
	assert_eq!(*render_count.borrow(), 1);
	assert_eq!(container.text_content().as_deref(), Some("hello"));

	// Same shape, shallow-equal props (a fresh object, but same key/value):
	// should be skipped.
	let props_2 = Object::new();
	let _ = Reflect::set(&props_2, &"label".into(), &JsValue::from_str("hello"));
	let vnode_2 = create_element(&memoized, &props_2.into(), JsValue::NULL).expect("createElement should succeed");
	root.render(vnode_2).expect("render should succeed");
	assert_eq!(*render_count.borrow(), 1, "memo should have skipped the re-render for shallow-equal props");
	assert_eq!(container.text_content().as_deref(), Some("hello"), "the skipped render should still show the previously rendered content");

	// Different props: should re-render.
	let props_3 = Object::new();
	let _ = Reflect::set(&props_3, &"label".into(), &JsValue::from_str("world"));
	let vnode_3 = create_element(&memoized, &props_3.into(), JsValue::NULL).expect("createElement should succeed");
	root.render(vnode_3).expect("render should succeed");
	assert_eq!(*render_count.borrow(), 2, "memo should re-render when props actually differ");
	assert_eq!(container.text_content().as_deref(), Some("world"));
}

#[wasm_bindgen_test]
fn memo_custom_compare_function_overrides_default_shallow_equal() {
	let container = make_container();
	let render_count = Rc::new(RefCell::new(0u32));
	let child_fn = make_memo_child(render_count.clone());
	let child_fn: js_sys::Function = child_fn.unchecked_into();

	// Custom compare that always reports "equal", regardless of props, so
	// the child should never re-render after the first mount, even though
	// the label prop keeps changing.
	let always_equal: JsValue =
		Closure::wrap(Box::new(|_prev: JsValue, _next: JsValue| -> bool { true }) as Box<dyn Fn(JsValue, JsValue) -> bool>).into_js_value();
	let memoized = js_memo(&child_fn, always_equal).expect("memo() should succeed");

	let props_1 = Object::new();
	let _ = Reflect::set(&props_1, &"label".into(), &JsValue::from_str("first"));
	let vnode_1 = create_element(&memoized, &props_1.into(), JsValue::NULL).expect("createElement should succeed");
	let root = micro_react::bindings::render(vnode_1, container.clone()).expect("render should succeed");
	assert_eq!(*render_count.borrow(), 1);

	let props_2 = Object::new();
	let _ = Reflect::set(&props_2, &"label".into(), &JsValue::from_str("second"));
	let vnode_2 = create_element(&memoized, &props_2.into(), JsValue::NULL).expect("createElement should succeed");
	root.render(vnode_2).expect("render should succeed");

	assert_eq!(*render_count.borrow(), 1, "a custom compare() returning true should skip the re-render even though props changed");
	assert_eq!(container.text_content().as_deref(), Some("first"), "expected the cached first render's content, not a fresh render of the new props");
}
