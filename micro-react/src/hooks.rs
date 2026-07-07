// ─── hooks.rs ────────────────────────────────────────────────────────────────

use std::{cell::RefCell, rc::{Rc, Weak}};
use wasm_bindgen::prelude::*;

use crate::scheduler::{enqueue_render, enqueue_effect, enqueue_layout_effect, EffectSlot};
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

    /// Bumped at the start of every render of this instance (initial mount,
    /// rerender_component, or a reentrant render triggered mid-diff by a
    /// synchronous setState). Used so a render that is still "in flight" when
    /// a nested/reentrant render for the *same* instance completes can detect
    /// that it is stale and avoid clobbering the fresher `last_vnode` (etc.)
    /// the reentrant render just committed. See diff_component / rerender_component.
    pub render_generation: u64,

    // ── Re-render bookkeeping: what a setState-triggered re-render needs to actually re-invoke and patch, not just reset hooks ──
    /// The component's render closure, captured at (re)mount time.
    pub render_fn: Option<ComponentFn>,
    /// Props from the most recent render (re-renders triggered by this
    /// instance's own setState reuse the same props its parent gave it).
    pub last_props: Props,
    /// The DOM node this component's output was last mounted under.
    pub last_parent_dom: Option<web_sys::Node>,
    /// Namespace ("html" | "svg" | "math") in effect for that subtree.
    pub last_ns: String,
    /// The vnode tree this instance rendered last time, used as the "old"
    /// side of the diff on the next render.
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
            render_generation: 0,
            render_fn: None,
            last_props: Vec::new(),
            last_parent_dom: None,
            last_ns: String::new(),
            last_vnode: None,
        }
    }
    pub fn reset_hooks(&mut self) { self.hook_idx = 0; }
}

// ─── HookSlot ───
pub enum HookSlot {
    State   { value: Rc<RefCell<Box<dyn std::any::Any>>> },
    Reducer { value: Rc<RefCell<Box<dyn std::any::Any>>> },
    Effect  {
        deps:    Option<Vec<DepVal>>,
        cleanup: Option<Box<dyn FnOnce()>>,
        pending: Option<Box<dyn FnOnce() -> Option<Box<dyn FnOnce()>>>>,
    },
    LayoutEffect {
        deps:    Option<Vec<DepVal>>,
        cleanup: Option<Box<dyn FnOnce()>>,
        pending: Option<Box<dyn FnOnce() -> Option<Box<dyn FnOnce()>>>>,
    },
    Ref  { value: crate::vnode::NodeRef },
    Memo { value: Rc<dyn std::any::Any>, deps: Option<Vec<DepVal>> },
    Id   { value: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DepVal(pub String);

impl DepVal {
    pub fn of<T: std::fmt::Debug>(v: &T) -> Self { DepVal(format!("{:?}", v)) }
    pub fn js(_v: &JsValue) -> Self { DepVal("js".to_string()) }
}

// ─── Current component (thread-local dispatcher) ───
thread_local! {
    pub(crate) static CURRENT_INST: RefCell<Option<*mut ComponentInst>> = RefCell::new(None);
    // Weak handle to the same instance as CURRENT_INST. Anything that
    // outlives the render (closures, effects) must upgrade() this instead of dereferencing the raw pointer.
    pub(crate) static CURRENT_WEAK: RefCell<Option<Weak<RefCell<ComponentInst>>>> = RefCell::new(None);
}

pub fn current_inst() -> *mut ComponentInst {
    CURRENT_INST.with(|c| c.borrow().expect("hook called outside component"))
}

/// A `Weak` handle to the component instance currently rendering. Use this
/// instead of `current_inst()` in closures that outlive the render call.
pub fn current_weak() -> Weak<RefCell<ComponentInst>> {
    CURRENT_WEAK.with(|c| c.borrow().clone().expect("hook called outside component"))
}

pub fn with_inst<R>(
    inst: *mut ComponentInst,
    weak: Weak<RefCell<ComponentInst>>,
    f: impl FnOnce() -> R,
) -> R {
    let prev = CURRENT_INST.with(|c| *c.borrow());
    let prev_weak = CURRENT_WEAK.with(|c| c.borrow().clone());
    CURRENT_INST.with(|c| *c.borrow_mut() = Some(inst));
    CURRENT_WEAK.with(|c| *c.borrow_mut() = Some(weak));
    let result = f();
    CURRENT_INST.with(|c| *c.borrow_mut() = prev);
    CURRENT_WEAK.with(|c| *c.borrow_mut() = prev_weak);
    result
}

// ─── Error boundary stack: tracks currently-rendering ErrorBoundary instances (innermost last) ───
thread_local! {
    static BOUNDARY_STACK: RefCell<Vec<Weak<RefCell<ComponentInst>>>> = RefCell::new(Vec::new());
}

pub fn push_boundary(inst: Weak<RefCell<ComponentInst>>) {
    BOUNDARY_STACK.with(|s| s.borrow_mut().push(inst));
}

pub fn pop_boundary() {
    BOUNDARY_STACK.with(|s| { s.borrow_mut().pop(); });
}

/// Hand a render failure to the nearest live ErrorBoundary ancestor.
/// Returns true if a boundary accepted it, false if the caller should fall back to logging.
pub fn report_to_nearest_boundary(err: JsValue) -> bool {
    // Collect the setter and drop the BOUNDARY_STACK borrow before calling
    // it, since the re-render it triggers may re-entrantly push/pop the stack.
    let setter = BOUNDARY_STACK.with(|s| {
        for weak in s.borrow().iter().rev() {
            if let Some(inst_rc) = weak.upgrade() {
                // Same reasoning, one level in: clone the setter out before calling it.
                let setter = inst_rc.borrow().error_setter.clone();
                if setter.is_some() {
                    return setter;
                }
            }
        }
        None
    });

    match setter {
        Some(setter) => { setter(err); true }
        None => false,
    }
}

// ─── helper: get &hooks safely through raw ptr ───────────────────────────────
// SAFETY: WASM is single-threaded; inst is valid for the duration of a render.
macro_rules! hooks_ref {
    ($inst:expr) => { unsafe { &(&(*$inst).hooks) } }
}
// hooks_get_mut: get a mutable reference to a hook slot by index via raw ptr
// Using a function instead of indexing through a temporary &mut Vec reference.
#[inline(always)]
unsafe fn hooks_get_mut(inst: *mut ComponentInst, idx: usize) -> &'static mut HookSlot {
    &mut (&mut (*inst).hooks)[idx]
}
macro_rules! hook_idx {
    ($inst:expr) => { unsafe { (*$inst).hook_idx } }
}
macro_rules! hook_idx_inc {
    ($inst:expr) => { unsafe { (*$inst).hook_idx += 1; } }
}
macro_rules! hooks_push {
    ($inst:expr, $slot:expr) => { unsafe { (*$inst).hooks.push($slot); } }
}
macro_rules! hooks_len {
    ($inst:expr) => { unsafe { (*$inst).hooks.len() } }
}

// ─── useState ───
pub fn use_state<T: Clone + 'static>(initial: T) -> (T, Rc<dyn Fn(T)>) {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
    hook_idx_inc!(inst);

    if hooks_len!(inst) <= idx {
        let val: Box<dyn std::any::Any> = Box::new(initial.clone());
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
    let setter: Rc<dyn Fn(T)> = Rc::new(move |next: T| {
        *value_rc.borrow_mut() = Box::new(next);
        enqueue_render(weak.clone());
    });

    (current, setter)
}

// ─── useState with functional updater ───
pub fn use_state_fn<T: Clone + 'static>(initial: T) -> (T, Rc<dyn Fn(Box<dyn FnOnce(T) -> T>)>) {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
    hook_idx_inc!(inst);

    if hooks_len!(inst) <= idx {
        let val: Box<dyn std::any::Any> = Box::new(initial);
        hooks_push!(inst, HookSlot::State { value: Rc::new(RefCell::new(val)) });
    }

    let value_rc = match &hooks_ref!(inst)[idx] {
        HookSlot::State { value } => value.clone(),
        _ => panic!("hook type mismatch"),
    };

    let current = value_rc.borrow().downcast_ref::<T>().unwrap().clone();

    let weak = current_weak();
    let setter: Rc<dyn Fn(Box<dyn FnOnce(T) -> T>)> = Rc::new(move |f: Box<dyn FnOnce(T) -> T>| {
        let old  = value_rc.borrow().downcast_ref::<T>().unwrap().clone();
        let next = f(old);
        *value_rc.borrow_mut() = Box::new(next);
        enqueue_render(weak.clone());
    });

    (current, setter)
}

// ─── useState — cell-exposing variant: returns the hook's live cell so JS functional updates resolve against current value, not a stale snapshot ───
pub fn use_state_cell<T: Clone + 'static>(
    initial: T,
) -> (T, Rc<RefCell<Box<dyn std::any::Any>>>, Rc<dyn Fn(T)>) {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
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
/// value and dispatcher, same reasoning as `use_state_cell` — lets a caller
/// (e.g. the JS bindings) key a cache off the cell's stable address instead
/// of rebuilding a JS-facing wrapper every render.
pub fn use_reducer_cell<S, A>(
    reducer: impl Fn(S, A) -> S + 'static,
    initial: S,
) -> (S, Rc<RefCell<Box<dyn std::any::Any>>>, Rc<dyn Fn(A)>)
where S: Clone + 'static, A: 'static
{
    let inst = current_inst();
    let idx  = hook_idx!(inst);
    hook_idx_inc!(inst);

    if hooks_len!(inst) <= idx {
        let val: Box<dyn std::any::Any> = Box::new(initial);
        hooks_push!(inst, HookSlot::Reducer { value: Rc::new(RefCell::new(val)) });
    }

    let value_rc = match &hooks_ref!(inst)[idx] {
        HookSlot::Reducer { value } => value.clone(),
        _ => panic!("hook type mismatch"),
    };

    let current  = value_rc.borrow().downcast_ref::<S>().unwrap().clone();
    let reducer  = Rc::new(reducer);

    let weak = current_weak();
    let cell_for_dispatch = value_rc.clone();
    let dispatch: Rc<dyn Fn(A)> = Rc::new(move |action: A| {
        let old  = cell_for_dispatch.borrow().downcast_ref::<S>().unwrap().clone();
        let next = reducer(old, action);
        *cell_for_dispatch.borrow_mut() = Box::new(next);
        enqueue_render(weak.clone());
    });

    (current, value_rc, dispatch)
}

pub fn use_reducer<S, A>(
    reducer: impl Fn(S, A) -> S + 'static,
    initial: S,
) -> (S, Rc<dyn Fn(A)>)
where S: Clone + 'static, A: 'static
{
    let (current, _cell, dispatch) = use_reducer_cell(reducer, initial);
    (current, dispatch)
}

// ─── useEffect / useLayoutEffect (shared inner) ───
fn schedule_effect_inner(
    is_layout: bool,
    callback: Box<dyn FnOnce() -> Option<Box<dyn FnOnce()>>>,
    deps: Option<Vec<DepVal>>,
) {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
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
            HookSlot::Effect { deps: prev, .. } | HookSlot::LayoutEffect { deps: prev, .. } => {
                match (prev, &deps) {
                    (None, _) | (_, None) => true,
                    (Some(a), Some(b))    => a != b,
                }
            }
            _ => true,
        }
    };

    if !changed { return; }

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

pub fn use_effect(
    callback: impl FnOnce() -> Box<dyn FnOnce()> + 'static,
    deps: Option<Vec<DepVal>>,
) {
    let boxed: Box<dyn FnOnce() -> Option<Box<dyn FnOnce()>>> =
        Box::new(move || Some(callback()));
    schedule_effect_inner(false, boxed, deps);
}

pub fn use_effect_nodrop(
    callback: impl FnOnce() + 'static,
    deps: Option<Vec<DepVal>>,
) {
    let boxed: Box<dyn FnOnce() -> Option<Box<dyn FnOnce()>>> =
        Box::new(move || { callback(); None });
    schedule_effect_inner(false, boxed, deps);
}

pub fn use_layout_effect(
    callback: impl FnOnce() -> Box<dyn FnOnce()> + 'static,
    deps: Option<Vec<DepVal>>,
) {
    let boxed: Box<dyn FnOnce() -> Option<Box<dyn FnOnce()>>> =
        Box::new(move || Some(callback()));
    schedule_effect_inner(true, boxed, deps);
}

// ─── useRef ───
pub fn use_ref() -> crate::vnode::NodeRef {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
    hook_idx_inc!(inst);

    if hooks_len!(inst) <= idx {
        hooks_push!(inst, HookSlot::Ref { value: crate::vnode::NodeRef::new() });
    }

    match &hooks_ref!(inst)[idx] {
        HookSlot::Ref { value } => value.clone(),
        _ => panic!("hook type mismatch"),
    }
}

// ─── useMemo / useCallback ───
pub fn use_memo<T: Clone + 'static>(
    factory: impl FnOnce() -> T,
    deps: Option<Vec<DepVal>>,
) -> T {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
    hook_idx_inc!(inst);

    let changed = if hooks_len!(inst) <= idx {
        true
    } else {
        match &hooks_ref!(inst)[idx] {
            HookSlot::Memo { deps: prev, .. } => {
                match (prev, &deps) {
                    (None, _) | (_, None) => true,
                    (Some(a), Some(b))    => a != b,
                }
            }
            _ => true,
        }
    };

    if changed {
        let val: Rc<dyn std::any::Any> = Rc::new(factory());
        let slot = HookSlot::Memo { value: val, deps };
        if hooks_len!(inst) <= idx {
            hooks_push!(inst, slot);
        } else {
            unsafe { (&mut (*inst).hooks)[idx] = slot; }
        }
    }

    match &hooks_ref!(inst)[idx] {
        HookSlot::Memo { value, .. } => value.downcast_ref::<T>().unwrap().clone(),
        _ => panic!("hook type mismatch"),
    }
}

pub fn use_callback<F: Clone + 'static>(f: F, deps: Option<Vec<DepVal>>) -> F {
    use_memo(move || f, deps)
}

// ─── useId ───
static ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn use_id() -> String {
    let inst = current_inst();
    let idx  = hook_idx!(inst);
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

// ─── useDeferredValue ───
pub fn use_deferred_value<T: Clone + PartialEq + 'static>(value: T) -> T {
    let (deferred, set) = use_state(value.clone());
    use_effect_nodrop({
        let value = value.clone();
        move || {
            crate::scheduler::start_transition(move || { set(value); });
        }
    }, Some(vec![DepVal("deferred".to_string())]));
    deferred
}

// ─── Unmount — run all effect cleanups ───
pub fn unmount_inst(inst: &mut ComponentInst) {
    inst.unmounted = true;
    for slot in &mut inst.hooks {
        match slot {
            HookSlot::Effect      { cleanup, .. }
            | HookSlot::LayoutEffect { cleanup, .. } => {
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
