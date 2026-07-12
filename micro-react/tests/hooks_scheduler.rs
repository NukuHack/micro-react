//! Integration tests for the hooks in `src/hooks.rs` (and the scheduler
//! that drives them) driven through `Root::render`, the same way real
//! components exercise them — hooks only work inside `with_inst`, which
//! is set up by the diff engine for `Component` vnodes, and several of
//! them (setState -> reschedule, effects) need a real DOM, so — like
//! `tests/reconciler.rs` — these run via `wasm-bindgen-test` in a
//! headless browser:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox
//!
//! `setState`/`dispatch` schedule their re-render on the next
//! microtask (`scheduler::schedule_flush`), which we don't need to
//! actually await here: `scheduler::flush_rerenders` is itself `pub`,
//! so we call it directly to deterministically drain the dirty queue
//! synchronously, exactly like the microtask callback would.
//!
//! Pure-logic pieces that don't need a live component (Context's plain
//! get/set/subscribe bookkeeping, `parse_event_prop`, VNode builders,
//! `Pattern`) are covered separately with plain `cargo test --lib` unit
//! tests or in `tests/router.rs`.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen_test::*;

use micro_react::context::{use_context, Context};
use micro_react::hooks::*;
use micro_react::render::Root;
use micro_react::scheduler::flush_rerenders;
use micro_react::vnode::{ComponentFn, Props, VNode};

wasm_bindgen_test_configure!(run_in_browser);

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

fn text_of(container: &web_sys::Element) -> String {
	container.text_content().unwrap_or_default()
}

// ─── useState ───

#[wasm_bindgen_test]
fn use_state_initial_value_renders() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let comp = ComponentFn::infallible(|_props: Props| {
		let (count, _set_count) = use_state(0i32);
		VNode::text(count.to_string())
	});
	root.render(VNode::component("Counter", comp, vec![])).unwrap();
	assert_eq!(text_of(&container), "0");
}

#[wasm_bindgen_test]
fn use_state_setter_triggers_rerender_with_new_value() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (count, set_count) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_count);
		VNode::text(count.to_string())
	});
	root.render(VNode::component("Counter", comp, vec![])).unwrap();
	assert_eq!(text_of(&container), "0");

	let setter = setter_slot.borrow().clone().unwrap();
	setter(5);
	flush_rerenders();
	assert_eq!(text_of(&container), "5");
}

#[wasm_bindgen_test]
fn use_state_setter_is_a_noop_after_unmount() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (count, set_count) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_count);
		VNode::text(count.to_string())
	});
	root.render(VNode::component("Counter", comp, vec![])).unwrap();
	root.unmount();

	let setter = setter_slot.borrow().clone().unwrap();
	// Must not panic even though the component instance is gone.
	setter(99);
	flush_rerenders();
}

// ─── useReducer ───

#[wasm_bindgen_test]
fn use_reducer_cell_dispatches_and_rerenders() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let dispatch_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let dispatch_slot_for_comp = dispatch_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (state, _cell, dispatch) = use_reducer_cell(|s: i32, a: i32| s + a, 10i32);
		*dispatch_slot_for_comp.borrow_mut() = Some(dispatch);
		VNode::text(state.to_string())
	});
	root.render(VNode::component("Reducer", comp, vec![])).unwrap();
	assert_eq!(text_of(&container), "10");

	let dispatch = dispatch_slot.borrow().clone().unwrap();
	dispatch(7);
	flush_rerenders();
	assert_eq!(text_of(&container), "17");
}

// ─── useEffect / useLayoutEffect ───

#[wasm_bindgen_test]
fn use_effect_runs_after_mount() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let ran = Rc::new(RefCell::new(false));
	let ran_clone = ran.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let ran = ran_clone.clone();
		use_effect_nodrop(
			move || {
				*ran.borrow_mut() = true;
			},
			Some(vec![]),
		);
		VNode::text("hi")
	});
	root.render(VNode::component("EffectComp", comp, vec![])).unwrap();
	// Root::render runs effects synchronously after diffing.
	assert!(*ran.borrow());
}

#[wasm_bindgen_test]
fn use_effect_cleanup_runs_on_unmount() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let cleaned = Rc::new(RefCell::new(false));
	let cleaned_clone = cleaned.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let cleaned = cleaned_clone.clone();
		use_effect(
			move || {
				let cleaned = cleaned.clone();
				Box::new(move || {
					*cleaned.borrow_mut() = true;
				}) as Box<dyn FnOnce()>
			},
			Some(vec![]),
		);
		VNode::text("hi")
	});
	root.render(VNode::component("CleanupComp", comp, vec![])).unwrap();
	assert!(!*cleaned.borrow());

	root.unmount();
	assert!(*cleaned.borrow());
}

#[wasm_bindgen_test]
fn use_effect_does_not_rerun_when_deps_are_unchanged() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let run_count = Rc::new(RefCell::new(0));
	let run_count_clone = run_count.clone();
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (_tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		let run_count = run_count_clone.clone();
		use_effect_nodrop(
			move || {
				*run_count.borrow_mut() += 1;
			},
			Some(vec![]),
		);
		VNode::text("hi")
	});
	root.render(VNode::component("StableEffectComp", comp, vec![])).unwrap();
	assert_eq!(*run_count.borrow(), 1);

	let setter = setter_slot.borrow().clone().unwrap();
	setter(1); // forces a re-render, but the effect's deps (empty vec) are unchanged
	flush_rerenders();
	assert_eq!(*run_count.borrow(), 1, "effect with unchanged deps should not rerun");
}

#[wasm_bindgen_test]
fn use_layout_effect_runs_synchronously_after_render() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let ran = Rc::new(RefCell::new(false));
	let ran_clone = ran.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let ran = ran_clone.clone();
		use_layout_effect(
			move || {
				*ran.borrow_mut() = true;
				Box::new(|| {}) as Box<dyn FnOnce()>
			},
			Some(vec![]),
		);
		VNode::text("hi")
	});
	root.render(VNode::component("LayoutComp", comp, vec![])).unwrap();
	assert!(*ran.borrow(), "layout effects run synchronously inside Root::render");
}

// ─── useMemo / useCallback ───

#[wasm_bindgen_test]
fn use_memo_recomputes_only_when_deps_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let compute_count = Rc::new(RefCell::new(0));
	let compute_count_clone = compute_count.clone();
	let dep_slot: Rc<RefCell<i32>> = Rc::new(RefCell::new(1));
	let dep_slot_for_comp = dep_slot.clone();
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (_tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		let dep = *dep_slot_for_comp.borrow();
		let compute_count = compute_count_clone.clone();
		let _memoized = use_memo(
			move || {
				*compute_count.borrow_mut() += 1;
				dep * 2
			},
			Some(vec![DepVal(dep.to_string())]),
		);
		VNode::text("x")
	});
	root.render(VNode::component("MemoComp", comp, vec![])).unwrap();
	assert_eq!(*compute_count.borrow(), 1);

	// Re-render with the same dep: setState forces a re-render, but the
	// memo's dep value hasn't changed, so factory must not re-run.
	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();
	assert_eq!(*compute_count.borrow(), 1, "memo should not recompute when deps are unchanged");
}

#[wasm_bindgen_test]
fn use_memo_recomputes_when_deps_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let compute_count = Rc::new(RefCell::new(0));
	let compute_count_clone = compute_count.clone();
	let dep_slot: Rc<RefCell<i32>> = Rc::new(RefCell::new(1));
	let dep_slot_for_comp = dep_slot.clone();
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (_tick, set_tick) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_tick);
		let dep = *dep_slot_for_comp.borrow();
		let compute_count = compute_count_clone.clone();
		let _memoized = use_memo(
			move || {
				*compute_count.borrow_mut() += 1;
				dep * 2
			},
			Some(vec![DepVal(dep.to_string())]),
		);
		VNode::text("x")
	});
	root.render(VNode::component("MemoComp2", comp, vec![])).unwrap();
	assert_eq!(*compute_count.borrow(), 1);

	*dep_slot.borrow_mut() = 2; // change the dep the memo depends on
	let setter = setter_slot.borrow().clone().unwrap();
	setter(1);
	flush_rerenders();
	assert_eq!(*compute_count.borrow(), 2, "memo should recompute when deps change");
}

#[wasm_bindgen_test]
fn use_callback_returns_a_working_closure() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let comp = ComponentFn::infallible(|_props: Props| {
		let cb = use_callback(|| 42i32, Some(vec![]));
		VNode::text(cb().to_string())
	});
	root.render(VNode::component("CbComp", comp, vec![])).unwrap();
	assert_eq!(text_of(&container), "42");
}

// ─── useId ───

#[wasm_bindgen_test]
fn use_id_is_unique_per_component_and_stable_format() {
	let container_a = make_container();
	let container_b = make_container();
	let mut root_a = Root::new(container_a.clone());
	let mut root_b = Root::new(container_b.clone());

	let make_comp = || {
		ComponentFn::infallible(|_props: Props| {
			let id = use_id();
			VNode::text(id)
		})
	};
	root_a.render(VNode::component("IdComp", make_comp(), vec![])).unwrap();
	root_b.render(VNode::component("IdComp", make_comp(), vec![])).unwrap();

	let id_a = text_of(&container_a);
	let id_b = text_of(&container_b);
	assert_ne!(id_a, id_b);
	assert!(id_a.starts_with("mr-"));
	assert!(id_b.starts_with("mr-"));
}

// ─── Context / use_context ───

#[wasm_bindgen_test]
fn use_context_reads_provided_value_and_rerenders_on_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let ctx: Context<i32> = Context::new(1);
	ctx.set_value(1);

	let ctx_for_comp = ctx.clone();
	let comp = ComponentFn::infallible(move |_props: Props| {
		let value = use_context(&ctx_for_comp);
		VNode::text(value.to_string())
	});
	root.render(VNode::component("CtxReader", comp, vec![])).unwrap();
	assert_eq!(text_of(&container), "1");

	ctx.set_value(2);
	flush_rerenders();
	assert_eq!(text_of(&container), "2");
}

#[wasm_bindgen_test]
fn use_context_multiple_independent_consumers_all_rerender_on_change() {
	// Two separate roots (two separate component trees) subscribed to the
	// same context: a value change must reach *both*, not just the one
	// that happens to render first.
	let container_a = make_container();
	let container_b = make_container();
	let mut root_a = Root::new(container_a.clone());
	let mut root_b = Root::new(container_b.clone());
	let ctx: Context<i32> = Context::new(0);
	ctx.set_value(10);

	let make_comp = |ctx: Context<i32>| {
		ComponentFn::infallible(move |_props: Props| {
			let value = use_context(&ctx);
			VNode::text(value.to_string())
		})
	};
	root_a.render(VNode::component("A", make_comp(ctx.clone()), vec![])).unwrap();
	root_b.render(VNode::component("B", make_comp(ctx.clone()), vec![])).unwrap();
	assert_eq!(text_of(&container_a), "10");
	assert_eq!(text_of(&container_b), "10");

	ctx.set_value(20);
	flush_rerenders();
	assert_eq!(text_of(&container_a), "20", "consumer A should pick up the new context value");
	assert_eq!(text_of(&container_b), "20", "consumer B should pick up the new context value independently of A");
}

#[wasm_bindgen_test]
fn use_context_unsubscribes_on_unmount_and_does_not_panic_on_later_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let ctx: Context<i32> = Context::new(0);
	ctx.set_value(1);

	let ctx_for_comp = ctx.clone();
	let comp = ComponentFn::infallible(move |_props: Props| {
		let value = use_context(&ctx_for_comp);
		VNode::text(value.to_string())
	});
	root.render(VNode::component("UnmountCtx", comp, vec![])).unwrap();
	assert_eq!(text_of(&container), "1");

	root.unmount();

	// Must not panic: the subscribed waker held a Weak to the (now-gone)
	// component instance, so notifying it after unmount has to be a no-op,
	// not a dangling-pointer dereference.
	ctx.set_value(2);
	flush_rerenders();
}
