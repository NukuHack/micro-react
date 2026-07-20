//! Follow-up coverage for the `bindings.rs` gaps the TODO flagged as still
//! genuinely untested after `tests/browser/bindings.rs`'s first pass:
//! `create_element`'s Fragment-symbol detection and the
//! variadic/non-array-child workaround, every `js_use_*` hook wrapper
//! besides `js_use_state`, the `createContext`/`useContext` object shape
//! (including `useContext`'s error path), and `memo`'s skipped-render path
//! minting a fresh vnode box rather than re-handing out the cached one.

use js_sys::{Array, Object, Reflect};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::{
	create_element, js_create_context, js_memo, js_use_callback, js_use_context, js_use_id, js_use_memo, js_use_reducer, js_use_ref,
};
use micro_react::hooks::use_state;
use micro_react::render::Root;
use micro_react::scheduler::flush_rerenders;
use micro_react::vnode::{ComponentFn, Props, VNode};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

fn get_fragment() -> JsValue {
	micro_react::bindings::get_fragment()
}

// ─── create_element: Fragment-symbol detection ───

#[wasm_bindgen_test]
fn create_element_with_fragment_symbol_wraps_children_without_a_host_element() {
	let children = Array::new();
	children.push(&create_element(&JsValue::from_str("span"), &JsValue::NULL, JsValue::from_str("a")).unwrap());
	children.push(&create_element(&JsValue::from_str("span"), &JsValue::NULL, JsValue::from_str("b")).unwrap());

	let vnode = create_element(&get_fragment(), &JsValue::NULL, children.into()).expect("createElement with the Fragment symbol should succeed");

	let container = make_container();
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");

	// No wrapper element: both <span>s should be direct children of the container.
	assert_eq!(container.children().length(), 2, "a Fragment should not introduce a wrapping host element");
	assert_eq!(container.children().item(0).unwrap().tag_name().to_lowercase(), "span");
	assert_eq!(container.children().item(1).unwrap().tag_name().to_lowercase(), "span");
	assert_eq!(container.text_content().as_deref(), Some("ab"));
}

// ─── create_element: variadic-children / non-array-child workaround ───

#[wasm_bindgen_test]
fn create_element_treats_a_lone_non_array_child_as_a_one_element_children_array() {
	// wasm-bindgen's fixed-arity export shim can hand `create_element` a
	// single non-array value for `children` (the "any non-array child
	// becomes a one-element array" branch), rather than the `Array` the
	// `h()` JS wrapper normally builds. Exercise that branch directly.
	let vnode = create_element(&JsValue::from_str("div"), &JsValue::NULL, JsValue::from_str("lone child"))
		.expect("a lone non-array child should be treated as a single-element children array, not rejected");

	let container = make_container();
	let _root = micro_react::bindings::render(vnode, container.clone()).expect("render should succeed");
	assert_eq!(container.text_content().as_deref(), Some("lone child"));
}

#[wasm_bindgen_test]
fn create_element_treats_null_or_undefined_children_as_no_children() {
	let vnode_null = create_element(&JsValue::from_str("div"), &JsValue::NULL, JsValue::NULL).expect("null children should be accepted");
	let container = make_container();
	let _root = micro_react::bindings::render(vnode_null, container.clone()).expect("render should succeed");
	assert_eq!(container.text_content().as_deref(), Some(""));
}

// ─── js_use_reducer ───

#[wasm_bindgen_test]
fn js_use_reducer_dispatch_updates_state_through_the_js_reducer_function() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let reducer: js_sys::Function = Closure::wrap(Box::new(|state: JsValue, action: JsValue| -> JsValue {
		JsValue::from_f64(state.as_f64().unwrap_or(0.0) + action.as_f64().unwrap_or(0.0))
	}) as Box<dyn Fn(JsValue, JsValue) -> JsValue>)
	.into_js_value()
	.unchecked_into();

	let dispatch_slot: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
	let dispatch_slot_for_comp = dispatch_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let arr = js_use_reducer(&reducer, JsValue::from_f64(10.0));
		let state = arr.get(0);
		*dispatch_slot_for_comp.borrow_mut() = Some(arr.get(1));
		VNode::text(state.as_f64().unwrap_or(-1.0).to_string())
	});
	root.render(VNode::component("Reducer", comp, vec![])).unwrap();
	assert_eq!(container.text_content().as_deref(), Some("10"));

	let dispatch = dispatch_slot.borrow().clone().expect("dispatch should have been captured");
	let dispatch_fn: js_sys::Function = dispatch.unchecked_into();
	let _ = dispatch_fn.call1(&JsValue::NULL, &JsValue::from_f64(5.0));

	flush_rerenders();
	assert_eq!(container.text_content().as_deref(), Some("15"), "dispatch through the JS wrapper should apply the JS reducer function");
}

// ─── js_use_ref ───

#[wasm_bindgen_test]
fn js_use_ref_object_identity_is_stable_and_current_survives_rerenders() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();
	let seen_refs: Rc<RefCell<Vec<Object>>> = Rc::new(RefCell::new(Vec::new()));
	let seen_refs_for_comp = seen_refs.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		let obj = js_use_ref(JsValue::from_str("init"));
		seen_refs_for_comp.borrow_mut().push(obj.clone());
		if tick == 0 {
			// Mutate `.current` on the very first render; a later render
			// reusing the same slot should still see this, not the initial value.
			let _ = Reflect::set(&obj, &"current".into(), &JsValue::from_str("mutated"));
		}
		VNode::text("x")
	});
	root.render(VNode::component("RefComp", comp, vec![])).unwrap();

	let setter = setter_slot.borrow().clone().unwrap();
	setter(1); // force a second render of the same instance
	flush_rerenders();

	assert_eq!(seen_refs.borrow().len(), 2, "expected two renders of the same instance");
	let first: JsValue = seen_refs.borrow()[0].clone().into();
	let second: JsValue = seen_refs.borrow()[1].clone().into();
	assert!(js_sys::Object::is(&first, &second), "useRef's returned object should be the same identity across re-renders of the same hook slot");

	let current = Reflect::get(&second, &"current".into()).unwrap();
	assert_eq!(current.as_string().as_deref(), Some("mutated"), "a mutation to .current on one render should still be visible on a later render");
}

// ─── js_use_memo ───

#[wasm_bindgen_test]
fn js_use_memo_only_calls_the_js_factory_when_deps_actually_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let call_count = Rc::new(RefCell::new(0u32));
	let call_count_for_comp = call_count.clone();
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		// Same dep for tick 0 and 1, a different dep from tick 2 onward.
		let dep_str = if tick < 2 { "same" } else { "changed" };
		let deps: JsValue = Array::of1(&JsValue::from_str(dep_str)).into();

		let cc = call_count_for_comp.clone();
		let factory: js_sys::Function = Closure::wrap(Box::new(move || -> JsValue {
			*cc.borrow_mut() += 1;
			JsValue::from_f64(1.0)
		}) as Box<dyn Fn() -> JsValue>)
		.into_js_value()
		.unchecked_into();

		let _ = js_use_memo(&factory, deps);
		VNode::text(tick.to_string())
	});
	root.render(VNode::component("MemoHook", comp, vec![])).unwrap();
	assert_eq!(*call_count.borrow(), 1, "factory should run once on first render");

	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();
	assert_eq!(*call_count.borrow(), 1, "factory should not re-run when deps are unchanged");

	setter(2);
	flush_rerenders();
	assert_eq!(*call_count.borrow(), 2, "factory should re-run once deps actually change");
}

// ─── js_use_callback ───

#[wasm_bindgen_test]
fn js_use_callback_keeps_the_first_functions_identity_until_deps_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();
	let seen: Rc<RefCell<Vec<JsValue>>> = Rc::new(RefCell::new(Vec::new()));
	let seen_for_comp = seen.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		let dep_str = if tick < 2 { "same" } else { "changed" };
		let deps: JsValue = Array::of1(&JsValue::from_str(dep_str)).into();

		// A distinct JS function value built fresh every render (as a real
		// caller's inline `() => {...}` would be), tagged with the render
		// index so identity is easy to compare across renders.
		let marker = tick;
		let f: js_sys::Function = Closure::wrap(Box::new(move || -> i32 { marker }) as Box<dyn Fn() -> i32>).into_js_value().unchecked_into();

		let result = js_use_callback(&f, deps);
		seen_for_comp.borrow_mut().push(result);
		VNode::text(tick.to_string())
	});
	root.render(VNode::component("CallbackHook", comp, vec![])).unwrap();

	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();
	setter(2);
	flush_rerenders();

	assert_eq!(seen.borrow().len(), 3);
	let first = seen.borrow()[0].clone();
	let second = seen.borrow()[1].clone();
	let third = seen.borrow()[2].clone();

	assert!(js_sys::Object::is(&first, &second), "useCallback should keep returning the first render's function while deps are unchanged");
	assert!(!js_sys::Object::is(&first, &third), "useCallback should return the new render's function once deps actually change");
}

// ─── js_use_id ───

#[wasm_bindgen_test]
fn js_use_id_is_stable_across_rerenders_and_distinct_across_instances() {
	let container_a = make_container();
	let container_b = make_container();
	let mut root_a = Root::new(container_a.clone());
	let mut root_b = Root::new(container_b.clone());

	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();
	let ids_a: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
	let ids_a_for_comp = ids_a.clone();

	let comp_a = ComponentFn::infallible(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		ids_a_for_comp.borrow_mut().push(js_use_id());
		VNode::text(tick.to_string())
	});
	root_a.render(VNode::component("IdCompA", comp_a, vec![])).unwrap();
	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();

	assert_eq!(ids_a.borrow().len(), 2);
	assert_eq!(ids_a.borrow()[0], ids_a.borrow()[1], "useId should return the same id across re-renders of the same instance");

	let comp_b = ComponentFn::infallible(move |_props: Props| VNode::text(js_use_id()));
	root_b.render(VNode::component("IdCompB", comp_b, vec![])).unwrap();
	let id_b = container_b.text_content().unwrap_or_default();

	assert_ne!(ids_a.borrow()[0], id_b, "useId should return distinct ids for distinct component instances");
}

// ─── createContext / useContext ───

#[wasm_bindgen_test]
fn context_provider_and_use_context_thread_the_value_through_the_js_object_shape() {
	let ctx = js_create_context(JsValue::from_str("default")).expect("createContext should succeed");
	let provider = Reflect::get(&ctx, &"Provider".into()).unwrap();
	assert!(provider.is_function(), "context object should expose a Provider function");
	assert!(Reflect::get(&ctx, &"Consumer".into()).unwrap().is_function(), "context object should expose a Consumer function");
	assert!(Reflect::get(&ctx, &"useContext".into()).unwrap().is_function(), "context object should expose a useContext function");

	let container = make_container();
	let mut root = Root::new(container.clone());

	let ctx_for_child = ctx.clone();
	let child = ComponentFn::infallible(move |_props: Props| {
		let value = js_use_context(&ctx_for_child).expect("useContext should succeed for a well-shaped context object");
		VNode::text(value.as_string().unwrap_or_default())
	});
	// The Provider closure's only job is to stash the value and hand back
	// its children; calling it directly (the way `createElement(ctx.Provider,
	// { value }, children)` would under the hood) is enough to set the
	// context value for the Consumer/useContext side to pick up.
	let provider_fn: js_sys::Function = provider.unchecked_into();
	let props = Object::new();
	let _ = Reflect::set(&props, &"value".into(), &JsValue::from_str("provided"));
	let _ = provider_fn.call1(&JsValue::NULL, &props.into());

	root.render(VNode::component("Child", child, vec![])).unwrap();
	assert_eq!(container.text_content().as_deref(), Some("provided"), "useContext should see the value set by the Provider");
}

#[wasm_bindgen_test]
fn use_context_errors_on_null_or_a_shapeless_object() {
	assert!(js_use_context(&JsValue::NULL).is_err(), "useContext(null) should error rather than panic");
	assert!(js_use_context(&JsValue::UNDEFINED).is_err(), "useContext(undefined) should error rather than panic");

	let shapeless = Object::new();
	let shapeless: JsValue = shapeless.into();
	assert!(js_use_context(&shapeless).is_err(), "an object without a useContext method should error, not panic or return a bogus value");
}

// ─── memo(): skipped-render path mints a fresh vnode box, not a reused one ───

fn wrap_as_js_component(f: impl Fn(JsValue) -> JsValue + 'static) -> js_sys::Function {
	Closure::wrap(Box::new(f) as Box<dyn Fn(JsValue) -> JsValue>).into_js_value().unchecked_into()
}

#[wasm_bindgen_test]
fn memo_skipped_render_returns_a_freshly_boxed_vnode_id_each_time() {
	// `js_memo`'s cached `prev_result` is replayed through a *fresh*
	// `vnode_to_js` box on every skip (see the comment above `prev_result`
	// in bindings.rs) rather than re-handing out the previous JS wrapper
	// object, to avoid a double-free/double-consume if the same skip
	// result were read twice. Call the raw wrapper function directly
	// (bypassing render/reconciliation) and check the `__id` marker
	// differs across two skipped calls with shallow-equal props.
	let child_fn = wrap_as_js_component(|_props: JsValue| -> JsValue {
		create_element(&JsValue::from_str("div"), &JsValue::NULL, JsValue::NULL).expect("createElement should succeed")
	});
	let memoized = js_memo(&child_fn, JsValue::UNDEFINED).expect("memo() should succeed");
	let wrapper_fn: js_sys::Function = memoized.unchecked_into();

	let props_1 = Object::new();
	let _ = Reflect::set(&props_1, &"label".into(), &JsValue::from_str("same"));
	let ret_1 = wrapper_fn.call1(&JsValue::NULL, &props_1.into()).expect("call should succeed");
	let id_1 = Reflect::get(&ret_1, &"__id".into()).unwrap().as_f64();

	// Shallow-equal props: this call takes the skip path.
	let props_2 = Object::new();
	let _ = Reflect::set(&props_2, &"label".into(), &JsValue::from_str("same"));
	let ret_2 = wrapper_fn.call1(&JsValue::NULL, &props_2.into()).expect("call should succeed");
	let id_2 = Reflect::get(&ret_2, &"__id".into()).unwrap().as_f64();

	assert!(id_1.is_some() && id_2.is_some(), "both results should be tagged vnode wrapper objects");
	assert_ne!(id_1, id_2, "a skipped render should mint a fresh vnode box/id each call, not re-hand out the same one (which would double-consume)");
}
