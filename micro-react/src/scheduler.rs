// ─── scheduler.rs ────────────────────────────────────────────────────────────
//
// Microtask-batched rerender scheduler.
//
// Architecture mirrors Preact's:
//   • Components that call setState/dispatch are pushed into DIRTY_QUEUE.
//   • The first push schedules a microtask via queueMicrotask().
//   • The microtask flushes the queue depth-first (parents before children).
//   • Transition updates go into a separate TRANSITION_QUEUE flushed via rAF.
//
// ─────────────────────────────────────────────────────────────────────────────

use std::{
    cell::RefCell,
};
use wasm_bindgen::{prelude::*, JsCast};

use crate::hooks::{ComponentInst, HookSlot};

// ─────────────────────────────────────────────────────────────────────────────
// Thread-local queues (WASM is single-threaded)
// ─────────────────────────────────────────────────────────────────────────────
thread_local! {
    /// Components waiting for a synchronous re-render.
    static DIRTY_QUEUE: RefCell<Vec<*mut ComponentInst>> = RefCell::new(Vec::new());

    /// Components queued for a transition (rendered in rAF).
    static TRANSITION_QUEUE: RefCell<Vec<*mut ComponentInst>> = RefCell::new(Vec::new());

    /// True while a microtask flush is already scheduled.
    static FLUSH_PENDING: RefCell<bool> = RefCell::new(false);

    /// True while we are inside startTransition().
    static IN_TRANSITION: RefCell<bool> = RefCell::new(false);

    /// Pending useEffect callbacks (run asynchronously, after paint).
    pub(crate) static PENDING_EFFECTS: RefCell<Vec<EffectSlot>> = RefCell::new(Vec::new());

    /// Pending useLayoutEffect callbacks (run synchronously, before paint).
    pub(crate) static PENDING_LAYOUT_EFFECTS: RefCell<Vec<EffectSlot>> = RefCell::new(Vec::new());
}

/// An effect slot queued for execution. Carries a pointer back to the
/// component instance + hook index so the flush step can retrieve the
/// real pending callback/cleanup that schedule_effect_inner() stored on
/// the hook itself (see hooks.rs). Earlier this struct tried to carry the
/// boxed closures directly, but the closures were actually being stashed
/// on the HookSlot and an *empty* EffectSlot was enqueued, so run_effects()
/// / run_layout_effects() had nothing to call — effects never fired.
pub struct EffectSlot {
    pub inst: *mut ComponentInst,
    pub idx: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public scheduler API
// ─────────────────────────────────────────────────────────────────────────────

/// Mark a component instance as dirty and schedule a flush.
///
/// # Safety
/// `inst` must point to a valid `ComponentInst` that outlives the flush.
/// In practice, instances are stored in `Rc<RefCell<ComponentInst>>` by the
/// diff engine, so they are kept alive by the vnode tree.
pub fn enqueue_render(inst: *mut ComponentInst) {
    // Safety: single-threaded WASM, no data races possible.
    let already_dirty = unsafe { (*inst).dirty };
    let unmounted     = unsafe { (*inst).unmounted };
    if already_dirty || unmounted { return; }

    unsafe { (*inst).dirty = true; }

    let in_transition = IN_TRANSITION.with(|t| *t.borrow());
    if in_transition {
        TRANSITION_QUEUE.with(|q| q.borrow_mut().push(inst));
    } else {
        DIRTY_QUEUE.with(|q| q.borrow_mut().push(inst));
        schedule_flush();
    }
}

/// Queue a useEffect slot.
pub fn enqueue_effect(slot: EffectSlot) {
    PENDING_EFFECTS.with(|q| q.borrow_mut().push(slot));
}

/// Queue a useLayoutEffect slot.
pub fn enqueue_layout_effect(slot: EffectSlot) {
    PENDING_LAYOUT_EFFECTS.with(|q| q.borrow_mut().push(slot));
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal flush machinery
// ─────────────────────────────────────────────────────────────────────────────

fn schedule_flush() {
    let already = FLUSH_PENDING.with(|f| {
        let was = *f.borrow();
        *f.borrow_mut() = true;
        was
    });
    if already { return; }

    // Queue a microtask via `queueMicrotask()`
    let cb = Closure::once(|| {
        FLUSH_PENDING.with(|f| *f.borrow_mut() = false);
        flush_rerenders();
    });
    let window = web_sys::window().expect("no window");
    // queueMicrotask is not in web-sys yet; call via js-sys
    let fn_ = js_sys::Function::new_no_args(
        "queueMicrotask(arguments[0])"
    );
    let _ = fn_.call1(&JsValue::NULL, cb.as_ref().unchecked_ref::<js_sys::Function>());
    cb.forget(); // leak is intentional — runs once then drops
}

pub fn flush_rerenders() {
    // Drain the dirty queue, depth-sorted (parents before children)
    loop {
        let inst = DIRTY_QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            if q.is_empty() { return None; }
            // Find shallowest (lowest depth)
            let idx = q.iter().enumerate()
                .min_by_key(|(_, p)| { let inst: &ComponentInst = unsafe { &**(*p) }; inst.depth })
                .map(|(i, _)| i)
                .unwrap_or(0);
            Some(q.swap_remove(idx))
        });
        let Some(inst) = inst else { break };
        // Safety: same single-thread guarantee
        if unsafe { (*inst).dirty && !(*inst).unmounted } {
            crate::diff::rerender_component(inst);
        }
    }

    run_layout_effects();
    run_effects();

    // Flush transition queue in a rAF
    let has_transitions = TRANSITION_QUEUE.with(|q| !q.borrow().is_empty());
    if has_transitions {
        let cb = Closure::once(|| {
            let insts: Vec<*mut ComponentInst> = TRANSITION_QUEUE.with(|q| {
                std::mem::take(&mut *q.borrow_mut())
            });
            for inst in insts {
                if unsafe { !(*inst).unmounted } {
                    unsafe { (*inst).dirty = true; }
                    crate::diff::rerender_component(inst);
                }
            }
            run_layout_effects();
            run_effects();
        });
        let window = web_sys::window().expect("no window");
        let _ = window.request_animation_frame(cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

pub fn run_layout_effects() {
    let slots: Vec<EffectSlot> = PENDING_LAYOUT_EFFECTS.with(|q| {
        std::mem::take(&mut *q.borrow_mut())
    });
    for slot in slots {
        run_one_effect(slot.inst, slot.idx, true);
    }
}

pub fn run_effects() {
    let slots: Vec<EffectSlot> = PENDING_EFFECTS.with(|q| {
        std::mem::take(&mut *q.borrow_mut())
    });
    for slot in slots {
        run_one_effect(slot.inst, slot.idx, false);
    }
}

/// Run the cleanup + pending callback stored on hooks[idx] of `inst`,
/// then store whatever cleanup the callback returned back onto the hook.
///
/// # Safety
/// `inst` must still be a live ComponentInst (true for the lifetime of a
/// mounted component — the same assumption already relied on by
/// `enqueue_render`'s use of a raw `*mut ComponentInst` across a
/// microtask boundary).
fn run_one_effect(inst: *mut ComponentInst, idx: usize, is_layout: bool) {
    unsafe {
        if (*inst).unmounted { return; }

        let hooks: &mut Vec<HookSlot> = &mut (*inst).hooks;
        if idx >= hooks.len() { return; }

        let (old_cleanup, pending) = match &mut hooks[idx] {
            HookSlot::Effect { cleanup, pending, .. } if !is_layout => {
                (cleanup.take(), pending.take())
            }
            HookSlot::LayoutEffect { cleanup, pending, .. } if is_layout => {
                (cleanup.take(), pending.take())
            }
            _ => (None, None),
        };

        if let Some(c) = old_cleanup {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(c));
        }

        if let Some(p) = pending {
            let new_cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(p))
                .ok()
                .flatten();
            let hooks: &mut Vec<HookSlot> = &mut (*inst).hooks;
            match &mut hooks[idx] {
                HookSlot::Effect { cleanup, .. } if !is_layout => { *cleanup = new_cleanup; }
                HookSlot::LayoutEffect { cleanup, .. } if is_layout => { *cleanup = new_cleanup; }
                _ => {}
            }
        }
    }
}

/// Run `f` synchronously and flush all pending rerenders before returning.
pub fn flush_sync(f: impl FnOnce()) {
    f();
    flush_rerenders();
}

/// Execute `f` in "transition" mode — updates are deferred to rAF.
pub fn start_transition(f: impl FnOnce()) {
    IN_TRANSITION.with(|t| *t.borrow_mut() = true);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    IN_TRANSITION.with(|t| *t.borrow_mut() = false);
}
