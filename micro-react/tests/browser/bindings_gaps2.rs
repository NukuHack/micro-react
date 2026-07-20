//! Third pass at the remaining `src/bindings.rs` TODO gaps not covered by
//! `bindings.rs`/`bindings_gaps.rs`:
//!
//! - `js_use_effect`/`js_use_layout_effect` called with real JS `Function`
//!   values, exercising the cleanup-function marshalling layer on top of
//!   the underlying Rust hooks (already covered without JS involved in
//!   `hooks_scheduler.rs`).
//! - `record_create_context_call`'s running call count (the leak-count
//!   warning itself fires through `console.warn`, which the suite has no
//!   spy hook for yet — see the TODO — so this checks the counter it's
//!   driven by instead of the console output).
//! - `js_create_error_boundary`'s `in_progress` re-entrancy guard, and that
//!   the guard resets after a call so later, separate calls still run.

use js_sys::{Array, Object, Reflect};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::{js_create_error_boundary, js_use_effect, js_use_layout_effect};
use micro_react::context::record_create_context_call;
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

fn js_effect_callback(on_run: impl Fn() -> Option<js_sys::Function> + 'static) -> js_sys::Function {
	Closure::wrap(Box::new(move || -> JsValue { on_run().map(JsValue::from).unwrap_or(JsValue::UNDEFINED) }) as Box<dyn Fn() -> JsValue>)
		.into_js_value()
		.unchecked_into()
}

// ─── js_use_effect: JS callback + JS cleanup marshalling ───

#[wasm_bindgen_test]
fn js_use_effect_runs_the_js_callback_and_its_js_cleanup_on_dep_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let effect_runs = Rc::new(RefCell::new(0u32));
	let cleanup_runs = Rc::new(RefCell::new(0u32));
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let effect_runs_for_comp = effect_runs.clone();
	let cleanup_runs_for_comp = cleanup_runs.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);

		let effect_runs = effect_runs_for_comp.clone();
		let cleanup_runs = cleanup_runs_for_comp.clone();
		let callback = js_effect_callback(move || {
			*effect_runs.borrow_mut() += 1;
			let cleanup_runs = cleanup_runs.clone();
			let cleanup: js_sys::Function = Closure::wrap(Box::new(move || {
				*cleanup_runs.borrow_mut() += 1;
			}) as Box<dyn Fn()>)
			.into_js_value()
			.unchecked_into();
			Some(cleanup)
		});

		let dep_str = if tick < 2 { "same" } else { "changed" };
		let deps: JsValue = Array::of1(&JsValue::from_str(dep_str)).into();
		js_use_effect(&callback, deps);
		VNode::text(tick.to_string())
	});
	root.render(VNode::component("JsEffectComp", comp, vec![])).unwrap();
	assert_eq!(*effect_runs.borrow(), 1, "the JS effect callback should run once after first mount");
	assert_eq!(*cleanup_runs.borrow(), 0);

	let setter = setter_slot.borrow().clone().unwrap();
	setter(1); // same dep -> effect should not rerun
	flush_rerenders();
	assert_eq!(*effect_runs.borrow(), 1, "unchanged deps should not rerun the JS effect callback");
	assert_eq!(*cleanup_runs.borrow(), 0);

	setter(2); // changed dep -> previous JS cleanup should run, then the effect reruns
	flush_rerenders();
	assert_eq!(*cleanup_runs.borrow(), 1, "the JS cleanup function returned by the previous effect call should have run");
	assert_eq!(*effect_runs.borrow(), 2, "changed deps should rerun the JS effect callback");
}

// ─── js_use_layout_effect: same marshalling layer, synchronous timing ───

#[wasm_bindgen_test]
fn js_use_layout_effect_runs_the_js_callback_synchronously_and_its_js_cleanup_on_unmount() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let effect_runs = Rc::new(RefCell::new(0u32));
	let cleanup_runs = Rc::new(RefCell::new(0u32));
	let effect_runs_for_comp = effect_runs.clone();
	let cleanup_runs_for_comp = cleanup_runs.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let effect_runs = effect_runs_for_comp.clone();
		let cleanup_runs = cleanup_runs_for_comp.clone();
		let callback = js_effect_callback(move || {
			*effect_runs.borrow_mut() += 1;
			let cleanup_runs = cleanup_runs.clone();
			let cleanup: js_sys::Function = Closure::wrap(Box::new(move || {
				*cleanup_runs.borrow_mut() += 1;
			}) as Box<dyn Fn()>)
			.into_js_value()
			.unchecked_into();
			Some(cleanup)
		});
		js_use_layout_effect(&callback, JsValue::UNDEFINED);
		VNode::text("hi")
	});
	root.render(VNode::component("JsLayoutEffectComp", comp, vec![])).unwrap();
	// Root::render runs layout effects synchronously after diffing, same as
	// the Rust-only `use_layout_effect_runs_synchronously_after_render` test.
	assert_eq!(*effect_runs.borrow(), 1, "the JS layout-effect callback should have run synchronously by the time render() returns");

	root.unmount();
	assert_eq!(*cleanup_runs.borrow(), 1, "the JS cleanup function should run on unmount");
}

// ─── record_create_context_call: running total across calls ───

#[wasm_bindgen_test]
fn record_create_context_call_returns_a_monotonically_increasing_running_total() {
	// This is process-global (a thread-local static counter shared across
	// every test in this binary), so assert on the *delta* across two calls
	// made back to back rather than an absolute value.
	let before = record_create_context_call();
	let after = record_create_context_call();
	assert_eq!(after, before + 1, "each call should increment the running total by exactly one");
	// A third call in a row should keep climbing, which is the condition
	// `js_create_context` uses to decide whether to warn (`call_count > 1`);
	// the console.warn call itself needs a console-spy hook the suite
	// doesn't have, so this checks the counter that decision is based on.
	let third = record_create_context_call();
	assert_eq!(third, after + 1);
	assert!(third > 1, "repeated calls should trip the `call_count > 1` condition that createContext warns on");
}

// ─── js_create_error_boundary: in_progress re-entrancy guard ───

#[wasm_bindgen_test]
fn error_boundary_reentrant_call_is_short_circuited_by_the_in_progress_guard() {
	// The guard exists for exactly this shape of call: the same boundary
	// closure invoked again, synchronously, before its first call returns
	// (the doc comment above `in_progress` in bindings.rs describes this
	// happening via a synchronous re-render triggered from `js_use_state`).
	// Reproduce the reentrancy directly with a `children` accessor property
	// whose getter calls the very same boundary function again while the
	// outer call is still on the stack, rather than trying to reproduce the
	// exact scheduler condition that provokes it in production.
	let boundary_val = js_create_error_boundary();
	let boundary_fn: js_sys::Function = boundary_val.unchecked_into();
	let boundary_fn_for_call = boundary_fn.clone();
	let boundary_fn_for_getter = boundary_fn.clone();

	let reentrant_result: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
	let reentrant_result_for_getter = reentrant_result.clone();

	let comp = ComponentFn::new(move |_props: Props| {
		let props_obj = Object::new();

		let boundary_fn_inner = boundary_fn_for_getter.clone();
		let reentrant_result_inner = reentrant_result_for_getter.clone();
		let getter = Closure::wrap(Box::new(move || -> JsValue {
			let inner_props: JsValue = Object::new().into();
			let ret = boundary_fn_inner.call1(&JsValue::NULL, &inner_props).unwrap_or(JsValue::UNDEFINED);
			*reentrant_result_inner.borrow_mut() = Some(ret);
			JsValue::from_str("reentrant-child")
		}) as Box<dyn Fn() -> JsValue>);

		let descriptor = Object::new();
		Reflect::set(&descriptor, &"get".into(), getter.as_ref().unchecked_ref()).unwrap();
		Reflect::set(&descriptor, &"configurable".into(), &JsValue::TRUE).unwrap();
		Reflect::define_property(&props_obj, &JsValue::from_str("children"), &descriptor).unwrap();
		getter.forget(); // only needs to live for the duration of this call

		let result = boundary_fn_for_call.call1(&JsValue::NULL, &props_obj.into())?;
		Ok(VNode::text(result.as_string().unwrap_or_default()))
	});

	let container = make_container();
	let mut root = Root::new(container.clone());
	root.render(VNode::component("BoundaryReentrancyHost", comp, vec![])).unwrap();

	let inner = reentrant_result.borrow().clone().expect("the children getter (and thus the reentrant call) should have fired");
	assert!(
		inner.is_null(),
		"a reentrant call into the same boundary instance while the first call is in progress must return NULL, not re-run js_use_state"
	);
	assert_eq!(container.text_content().as_deref(), Some("reentrant-child"), "the outer (non-reentrant) call should still complete normally");
}

#[wasm_bindgen_test]
fn error_boundary_guard_resets_after_each_call_so_a_later_separate_call_still_runs() {
	// Sanity check that the guard is per-call rather than "sticky": two
	// separate, sequential (non-nested) invocations of the same boundary
	// instance must both run for real; a guard that never reset after the
	// first call would silently short-circuit every call after it forever.
	let container = make_container();
	let mut root = Root::new(container.clone());

	let boundary_val = js_create_error_boundary();
	let boundary_fn: js_sys::Function = boundary_val.unchecked_into();
	let boundary_fn_for_comp = boundary_fn.clone();

	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::new(move |_props: Props| {
		let (tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);

		let props_obj = Object::new();
		let _ = Reflect::set(&props_obj, &"children".into(), &JsValue::from_str(&format!("tick-{tick}")));
		let result = boundary_fn_for_comp.call1(&JsValue::NULL, &props_obj.into())?;
		Ok(VNode::text(result.as_string().unwrap_or_default()))
	});

	root.render(VNode::component("BoundaryResetHost", comp, vec![])).unwrap();
	assert_eq!(container.text_content().as_deref(), Some("tick-0"));

	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();
	assert_eq!(
		container.text_content().as_deref(),
		Some("tick-1"),
		"a later, separate (non-reentrant) call to the same boundary instance must still run for real after the guard resets"
	);
}
