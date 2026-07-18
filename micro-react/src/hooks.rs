//! React-style hooks (useState, useEffect, useMemo, etc.) backed by a
//! per-component slot vector. Hook order must stay stable across renders,
//! matching the same rule React itself imposes on hook calls.

use std::{
	cell::RefCell,
	rc::{Rc, Weak},
};
use wasm_bindgen::prelude::*;

use crate::scheduler::{EffectSlot, enqueue_effect, enqueue_layout_effect, enqueue_render};
use crate::vnode::{ComponentFn, Props, VNode};

// ─── ComponentInst ───
pub struct ComponentInst {
	pub hooks: Vec<HookSlot>,
	pub hook_idx: usize,
	pub dirty: bool,
	pub unmounted: bool,
	pub depth: u32,
	pub parent_dom: Option<web_sys::Element>,
	pub error_setter: Option<Rc<dyn Fn(JsValue)>>,

	/// The nearest ancestor ErrorBoundary as of this instance's last full
	/// diff pass (see `hooks::current_boundary` / `report_to_nearest_boundary`).
	/// Persisted (unlike `BOUNDARY_STACK`, which only reflects the *current*
	/// render-call window) so a failure from this component's own later,
	/// independent re-render — triggered by its own setState outside of any
	/// boundary's active render pass — can still find the right boundary.
	pub nearest_boundary: Option<Weak<RefCell<ComponentInst>>>,

	/// Bumped at the start of every render of this instance. Lets a
	/// reentrant render (triggered mid-diff by a synchronous setState)
	/// detect that an outer, now-stale render shouldn't clobber it.
	pub render_generation: u64,

	// ── Re-render bookkeeping: what a setState-triggered re-render needs ──
	pub render_fn: Option<ComponentFn>,
	pub last_props: Props,
	pub last_parent_dom: Option<web_sys::Node>,
	pub last_ns: String,
	pub last_vnode: Option<VNode>,
}

impl ComponentInst {
	pub fn new() -> Self {
		ComponentInst {
			hooks: Vec::new(),
			hook_idx: 0,
			dirty: false,
			unmounted: false,
			depth: 0,
			parent_dom: None,
			error_setter: None,
			nearest_boundary: None,
			render_generation: 0,
			render_fn: None,
			last_props: Vec::new(),
			last_parent_dom: None,
			last_ns: String::new(),
			last_vnode: None,
		}
	}
	pub fn reset_hooks(&mut self) {
		self.hook_idx = 0;
	}
}

/// Type-erased cell backing `use_state`/`use_reducer` storage.
pub type AnyCell = Rc<RefCell<Box<dyn std::any::Any>>>;

/// An effect cleanup callback, run before the next effect or on unmount.
pub type CleanupFn = Box<dyn FnOnce()>;

/// A not-yet-run effect body; returns an optional cleanup to store.
pub type PendingEffectFn = Box<dyn FnOnce() -> Option<CleanupFn>>;

// ─── HookSlot ───
pub enum HookSlot {
	State { value: AnyCell },
	Reducer { value: AnyCell },
	Effect { deps: Option<Vec<DepVal>>, cleanup: Option<CleanupFn>, pending: Option<PendingEffectFn> },
	LayoutEffect { deps: Option<Vec<DepVal>>, cleanup: Option<CleanupFn>, pending: Option<PendingEffectFn> },
	Ref { value: crate::vnode::NodeRef },
	RefVal { value: AnyCell },
	Memo { value: Rc<dyn std::any::Any>, deps: Option<Vec<DepVal>> },
	Id { value: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DepVal(pub String);

// ─── Current component (thread-local dispatcher) ───
thread_local! {
	pub(crate) static CURRENT_INST: RefCell<Option<*mut ComponentInst>> = const { RefCell::new(None) };
	// Weak handle to the same instance as CURRENT_INST. Anything that
	// outlives the render must upgrade() this instead of using the raw pointer.
	pub(crate) static CURRENT_WEAK: RefCell<Option<Weak<RefCell<ComponentInst>>>> = const { RefCell::new(None) };
}

pub fn current_inst() -> *mut ComponentInst {
	CURRENT_INST.with(|c| c.borrow().expect("hook called outside component"))
}

/// A `Weak` handle to the component instance currently rendering. Use this
/// instead of `current_inst()` in closures that outlive the render call.
pub fn current_weak() -> Weak<RefCell<ComponentInst>> {
	CURRENT_WEAK.with(|c| c.borrow().clone().expect("hook called outside component"))
}

pub fn with_inst<R>(inst: *mut ComponentInst, weak: Weak<RefCell<ComponentInst>>, f: impl FnOnce() -> R) -> R {
	let prev = CURRENT_INST.with(|c| *c.borrow());
	let prev_weak = CURRENT_WEAK.with(|c| c.borrow().clone());
	CURRENT_INST.with(|c| *c.borrow_mut() = Some(inst));
	CURRENT_WEAK.with(|c| *c.borrow_mut() = Some(weak));
	let result = f();
	CURRENT_INST.with(|c| *c.borrow_mut() = prev);
	CURRENT_WEAK.with(|c| *c.borrow_mut() = prev_weak);
	result
}

// ─── Error boundary stack: currently-rendering ErrorBoundary instances (innermost last) ───
thread_local! {
	static BOUNDARY_STACK: RefCell<Vec<Weak<RefCell<ComponentInst>>>> = const { RefCell::new(Vec::new()) };

	/// Set when a throw was just absorbed by a synchronous ancestor
	/// ErrorBoundary re-render, so the still-unwinding failing component
	/// knows not to unmount/diff the DOM node the fallback just took over.
	static BOUNDARY_ABSORBED: RefCell<bool> = const { RefCell::new(false) };
}

/// See `BOUNDARY_ABSORBED` above. Read-and-clear.
pub fn take_boundary_absorbed() -> bool {
	BOUNDARY_ABSORBED.with(|f| {
		let v = *f.borrow();
		*f.borrow_mut() = false;
		v
	})
}

pub fn push_boundary(inst: Weak<RefCell<ComponentInst>>) {
	BOUNDARY_STACK.with(|s| s.borrow_mut().push(inst));
}

pub fn pop_boundary() {
	BOUNDARY_STACK.with(|s| {
		s.borrow_mut().pop();
	});
}

/// The boundary currently on top of `BOUNDARY_STACK`, i.e. the nearest
/// ancestor ErrorBoundary that is actively diffing its subtree right now.
/// Called by `diff_component` on *every* component (not just failing ones)
/// to persist onto `ComponentInst::nearest_boundary`, so the association
/// with the ancestor boundary survives beyond this one render-call window —
/// see the doc comment on that field for why that matters.
pub fn current_boundary() -> Option<Weak<RefCell<ComponentInst>>> {
	BOUNDARY_STACK.with(|s| s.borrow().last().cloned())
}

/// Hand a render/reconciliation failure to the nearest live ErrorBoundary
/// ancestor of `origin` — the component instance whose render (or whose
/// subtree's reconciliation) just failed. Returns true if a boundary
/// accepted it, false if the caller should fall back to logging.
///
/// Takes `origin` explicitly rather than reading `CURRENT_INST`: this is
/// called from `diff_component`/`rerender_component` *after* `with_inst`
/// has already returned (and thus already reset `CURRENT_INST` back to
/// whatever it was before), so `CURRENT_INST` no longer reliably points at
/// the failing component by the time this runs — every call site already
/// has the right instance sitting in scope as `inst_rc`, so just use that
/// directly instead of going through fragile ambient state.
pub fn report_to_nearest_boundary(origin: &Rc<RefCell<ComponentInst>>, err: JsValue) -> bool {
	// Prefer `origin`'s own persisted `nearest_boundary`: this lets a later,
	// independent re-render (its own setState) still find its ancestor
	// boundary, since BOUNDARY_STACK would be empty in that situation.
	let from_origin = origin.borrow().nearest_boundary.clone().and_then(|w| w.upgrade());

	let target = from_origin.filter(|inst_rc| inst_rc.borrow().error_setter.is_some()).or_else(|| {
		// Fall back to the dynamic call-stack view: covers a first-ever
		// render, where `nearest_boundary` isn't populated yet but we're
		// still inside the ancestor boundary's own diff_node call.
		BOUNDARY_STACK.with(|s| {
			for weak in s.borrow().iter().rev() {
				if let Some(inst_rc) = weak.upgrade()
					&& inst_rc.borrow().error_setter.is_some()
				{
					return Some(inst_rc);
				}
			}
			None
		})
	});

	let Some(inst_rc) = target else { return false };

	let setter = inst_rc.borrow().error_setter.clone();
	let Some(setter) = setter else { return false };
	setter(err);

	// The setter above only schedules a re-render for the next microtask,
	// which isn't guaranteed to run before paint. Force it now so the
	// fallback UI appears in this same synchronous pass.
	crate::diff::rerender_component(inst_rc);

	// The boundary's DOM subtree has now been replaced by the fallback UI;
	// tell the still-unwinding failing component not to touch it.
	BOUNDARY_ABSORBED.with(|f| *f.borrow_mut() = true);

	true
}

// ─── helper: get &hooks safely through raw ptr ───
// SAFETY: WASM is single-threaded; inst is valid for the duration of a render.
macro_rules! hooks_ref {
	($inst:expr) => {
		unsafe { &(*$inst).hooks }
	};
}
#[inline(always)]
unsafe fn hooks_get_mut(inst: *mut ComponentInst, idx: usize) -> &'static mut HookSlot {
	unsafe { &mut (&mut (*inst).hooks)[idx] }
}
macro_rules! hook_idx {
	($inst:expr) => {
		unsafe { (*$inst).hook_idx }
	};
}
macro_rules! hook_idx_inc {
	($inst:expr) => {
		unsafe {
			(*$inst).hook_idx += 1;
		}
	};
}
macro_rules! hooks_push {
	($inst:expr, $slot:expr) => {
		unsafe {
			(*$inst).hooks.push($slot);
		}
	};
}
macro_rules! hooks_len {
	($inst:expr) => {
		unsafe { (*$inst).hooks.len() }
	};
}

// ─── useState ───
pub fn use_state<T: Clone + 'static>(initial: T) -> (T, Rc<dyn Fn(T)>) {
	let (current, _cell, setter) = use_state_cell(initial);
	(current, setter)
}

// ─── useState — cell-exposing variant: exposes the hook's live cell so JS
// functional updates resolve against the current value, not a stale snapshot ───
pub fn use_state_cell<T: Clone + 'static>(initial: T) -> (T, AnyCell, Rc<dyn Fn(T)>) {
	let inst = current_inst();
	let idx = hook_idx!(inst);
	hook_idx_inc!(inst);

	if hooks_len!(inst) <= idx {
		let val: Box<dyn std::any::Any> = Box::new(initial);
		hooks_push!(inst, HookSlot::State { value: Rc::new(RefCell::new(val)) });
	}

	let value_rc = match &hooks_ref!(inst)[idx] {
		HookSlot::State { value } => value.clone(),
		_ => panic!("hook type mismatch at {}", idx),
	};

	let current = value_rc.borrow().downcast_ref::<T>().expect("state type mismatch").clone();

	// Capture a Weak, not the raw pointer: this setter is often stashed in
	// handlers/timers that may fire after the component has unmounted.
	let weak = current_weak();
	let cell_for_setter = value_rc.clone();
	let setter: Rc<dyn Fn(T)> = Rc::new(move |next: T| {
		*cell_for_setter.borrow_mut() = Box::new(next);
		enqueue_render(weak.clone());
	});

	(current, value_rc, setter)
}

// ─── useReducer ───

/// Cell-exposing variant: returns the hook's live backing cell alongside the
/// value and dispatcher, so a caller (e.g. the JS bindings) can key a cache
/// off the cell's stable address.
pub fn use_reducer_cell<S, A>(reducer: impl Fn(S, A) -> S + 'static, initial: S) -> (S, AnyCell, Rc<dyn Fn(A)>)
where
	S: Clone + 'static,
	A: 'static,
{
	let inst = current_inst();
	let idx = hook_idx!(inst);
	hook_idx_inc!(inst);

	if hooks_len!(inst) <= idx {
		let val: Box<dyn std::any::Any> = Box::new(initial);
		hooks_push!(inst, HookSlot::Reducer { value: Rc::new(RefCell::new(val)) });
	}

	let value_rc = match &hooks_ref!(inst)[idx] {
		HookSlot::Reducer { value } => value.clone(),
		_ => panic!("hook type mismatch"),
	};

	let current = value_rc.borrow().downcast_ref::<S>().expect("state type set by this hook").clone();
	let reducer = Rc::new(reducer);

	let weak = current_weak();
	let cell_for_dispatch = value_rc.clone();
	let dispatch: Rc<dyn Fn(A)> = Rc::new(move |action: A| {
		let old = cell_for_dispatch.borrow().downcast_ref::<S>().expect("state type set by this hook").clone();
		let next = reducer(old, action);
		*cell_for_dispatch.borrow_mut() = Box::new(next);
		enqueue_render(weak.clone());
	});

	(current, value_rc, dispatch)
}

// ─── useEffect / useLayoutEffect (shared inner) ───
fn schedule_effect_inner(is_layout: bool, callback: PendingEffectFn, deps: Option<Vec<DepVal>>) {
	let inst = current_inst();
	let idx = hook_idx!(inst);
	hook_idx_inc!(inst);

	if hooks_len!(inst) <= idx {
		let slot = if is_layout {
			HookSlot::LayoutEffect { deps: None, cleanup: None, pending: None }
		} else {
			HookSlot::Effect { deps: None, cleanup: None, pending: None }
		};
		hooks_push!(inst, slot);
	}

	let changed = {
		match &hooks_ref!(inst)[idx] {
			HookSlot::Effect { deps: prev, .. } | HookSlot::LayoutEffect { deps: prev, .. } => match (prev, &deps) {
				(None, _) | (_, None) => true,
				(Some(a), Some(b)) => a != b,
			},
			_ => true,
		}
	};

	if !changed {
		return;
	}

	// SAFETY: single-threaded WASM, inst outlives this borrow
	let slot = unsafe { hooks_get_mut(inst, idx) };
	// Effects run later, so the slot must carry a Weak: the component may
	// have unmounted (and its ComponentInst freed) by the time this runs.
	let weak = current_weak();
	match slot {
		HookSlot::Effect { deps: d, pending: p, .. } => {
			*d = deps;
			*p = Some(callback);
			enqueue_effect(EffectSlot { inst: weak, idx });
		}
		HookSlot::LayoutEffect { deps: d, pending: p, .. } => {
			*d = deps;
			*p = Some(callback);
			enqueue_layout_effect(EffectSlot { inst: weak, idx });
		}
		_ => {}
	}
}

pub fn use_effect(callback: impl FnOnce() -> Box<dyn FnOnce()> + 'static, deps: Option<Vec<DepVal>>) {
	let boxed: PendingEffectFn = Box::new(move || Some(callback()));
	schedule_effect_inner(false, boxed, deps);
}

pub fn use_effect_nodrop(callback: impl FnOnce() + 'static, deps: Option<Vec<DepVal>>) {
	let boxed: PendingEffectFn = Box::new(move || {
		callback();
		None
	});
	schedule_effect_inner(false, boxed, deps);
}

pub fn use_layout_effect(callback: impl FnOnce() -> Box<dyn FnOnce()> + 'static, deps: Option<Vec<DepVal>>) {
	let boxed: PendingEffectFn = Box::new(move || Some(callback()));
	schedule_effect_inner(true, boxed, deps);
}

// ─── useMemo / useCallback ───
pub fn use_memo<T: Clone + 'static>(factory: impl FnOnce() -> T, deps: Option<Vec<DepVal>>) -> T {
	let inst = current_inst();
	let idx = hook_idx!(inst);
	hook_idx_inc!(inst);

	let changed = if hooks_len!(inst) <= idx {
		true
	} else {
		match &hooks_ref!(inst)[idx] {
			HookSlot::Memo { deps: prev, .. } => match (prev, &deps) {
				(None, _) | (_, None) => true,
				(Some(a), Some(b)) => a != b,
			},
			_ => true,
		}
	};

	if changed {
		let val: Rc<dyn std::any::Any> = Rc::new(factory());
		let slot = HookSlot::Memo { value: val, deps };
		if hooks_len!(inst) <= idx {
			hooks_push!(inst, slot);
		} else {
			unsafe {
				(&mut (*inst).hooks)[idx] = slot;
			}
		}
	}

	match &hooks_ref!(inst)[idx] {
		HookSlot::Memo { value, .. } => value.downcast_ref::<T>().expect("memo type set by this hook").clone(),
		_ => panic!("hook type mismatch"),
	}
}

pub fn use_callback<F: Clone + 'static>(f: F, deps: Option<Vec<DepVal>>) -> F {
	use_memo(move || f, deps)
}

// ─── useRef ───

/// A stable, mutable, type-erased cell — the general-purpose `useRef`
/// (as opposed to `HookSlot::Ref`'s `NodeRef`, which is specifically for
/// DOM-node refs attached via the `ref` prop). Unlike `useState`, getting
/// or writing this cell never touches the scheduler, so calling it never
/// triggers a re-render — this is what fixes `useRef` causing a spurious
/// extra render on every component's first mount.
pub fn use_ref_cell<T: 'static>(initial: impl FnOnce() -> T) -> AnyCell {
	let inst = current_inst();
	let idx = hook_idx!(inst);
	hook_idx_inc!(inst);

	if hooks_len!(inst) <= idx {
		let val: Box<dyn std::any::Any> = Box::new(initial());
		hooks_push!(inst, HookSlot::RefVal { value: Rc::new(RefCell::new(val)) });
	}

	match &hooks_ref!(inst)[idx] {
		HookSlot::RefVal { value } => value.clone(),
		_ => panic!("hook type mismatch at {}", idx),
	}
}

// ─── useId ───
static ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn use_id() -> String {
	let inst = current_inst();
	let idx = hook_idx!(inst);
	hook_idx_inc!(inst);

	if hooks_len!(inst) <= idx {
		let id = format!("mr-{}", ID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
		hooks_push!(inst, HookSlot::Id { value: id });
	}

	match &hooks_ref!(inst)[idx] {
		HookSlot::Id { value } => value.clone(),
		_ => panic!("hook type mismatch"),
	}
}

// ─── Unmount — run all effect cleanups ───
pub fn unmount_inst(inst: &mut ComponentInst) {
	inst.unmounted = true;
	for slot in &mut inst.hooks {
		match slot {
			HookSlot::Effect { cleanup, .. } | HookSlot::LayoutEffect { cleanup, .. } => {
				if let Some(f) = cleanup.take() {
					let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
				}
			}
			HookSlot::State { value } | HookSlot::Reducer { value } => {
				crate::bindings::evict_setter_cache(value);
			}
			_ => {}
		}
	}
}
