// ─── scheduler.rs ───
// Microtask-batched rerender scheduler, architecture mirrors Preact's.
// setState/dispatch push into DIRTY_QUEUE, flushed depth-first on the next microtask; transitions flush separately via rAF.
// ────────────────────

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
use wasm_bindgen::{prelude::*, JsCast};

use crate::hooks::{ComponentInst, HookSlot};

// ─── Thread-local queues (WASM is single-threaded) ───
thread_local! {
    /// Components waiting for a synchronous re-render. Stored as `Weak`
    /// since a component may unmount while sitting in the queue.
    static DIRTY_QUEUE: RefCell<Vec<Weak<RefCell<ComponentInst>>>> = RefCell::new(Vec::new());

    /// Components queued for a transition (rendered in rAF).
    static TRANSITION_QUEUE: RefCell<Vec<Weak<RefCell<ComponentInst>>>> = RefCell::new(Vec::new());

    /// True while a microtask flush is already scheduled.
    static FLUSH_PENDING: RefCell<bool> = RefCell::new(false);

    /// True while we are inside startTransition().
    static IN_TRANSITION: RefCell<bool> = RefCell::new(false);

    /// Pending useEffect callbacks (run asynchronously, after paint).
    pub(crate) static PENDING_EFFECTS: RefCell<Vec<EffectSlot>> = RefCell::new(Vec::new());

    /// Pending useLayoutEffect callbacks (run synchronously, before paint).
    pub(crate) static PENDING_LAYOUT_EFFECTS: RefCell<Vec<EffectSlot>> = RefCell::new(Vec::new());

    /// Cached microtask-flush callback, built once and reused.
    static FLUSH_CB: RefCell<Option<js_sys::Function>> = RefCell::new(None);

    /// Cached rAF transition-flush callback, same reasoning as FLUSH_CB.
    static TRANSITION_CB: RefCell<Option<js_sys::Function>> = RefCell::new(None);
}

/// Points back to the component + hook index so a flush can retrieve the
/// pending callback/cleanup that schedule_effect_inner() stored on the hook.
pub struct EffectSlot {
    pub inst: Weak<RefCell<ComponentInst>>,
    pub idx: usize,
}

// ─── Public scheduler API ───

/// Mark a component instance as dirty and schedule a flush. Takes a `Weak`
/// so a setState call after unmount becomes a no-op instead of touching freed memory.
pub fn enqueue_render(inst: Weak<RefCell<ComponentInst>>) {
    let Some(rc) = inst.upgrade() else { return };

    let (already_dirty, unmounted) = {
        let i = rc.borrow();
        (i.dirty, i.unmounted)
    };
    if already_dirty || unmounted { return; }

    rc.borrow_mut().dirty = true;

    let in_transition = IN_TRANSITION.with(|t| *t.borrow());
    if in_transition {
        TRANSITION_QUEUE.with(|q| q.borrow_mut().push(Rc::downgrade(&rc)));
    } else {
        DIRTY_QUEUE.with(|q| q.borrow_mut().push(Rc::downgrade(&rc)));
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

// ─── Internal flush machinery ───

fn schedule_flush() {
    let already = FLUSH_PENDING.with(|f| {
        let was = *f.borrow();
        *f.borrow_mut() = true;
        was
    });
    if already { return; }

    // A one-shot `Closure::once` panics if called twice, which happens
    // when re-entrant enqueue_render() calls overlap; cache a reusable `Fn` closure instead.
    let cb = FLUSH_CB.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let closure = Closure::wrap(Box::new(|| {
                FLUSH_PENDING.with(|f| *f.borrow_mut() = false);
                flush_rerenders();
            }) as Box<dyn Fn()>);
            // Leak intentionally: this closure lives for the app's entire
            // lifetime and is called repeatedly, never dropped.
            *slot = Some(closure.into_js_value().unchecked_into::<js_sys::Function>());
        }
        slot.as_ref().unwrap().clone()
    });

    // queueMicrotask is not in web-sys yet; call via js-sys.
    let fn_ = js_sys::Function::new_no_args("queueMicrotask(arguments[0])");
    let _ = fn_.call1(&JsValue::NULL, &cb);
}

pub fn flush_rerenders() {
    // Drain the dirty queue, depth-sorted (parents before children)
    loop {
        let rc = DIRTY_QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            // Drop any entries whose component has since unmounted (Weak
            // that no longer upgrades) before picking the next one to run.
            loop {
                if q.is_empty() { return None; }
                let idx = q.iter().enumerate()
                    .filter_map(|(i, w)| w.upgrade().map(|rc| (i, rc.borrow().depth)))
                    .min_by_key(|(_, depth)| *depth)
                    .map(|(i, _)| i);
                match idx {
                    Some(i) => {
                        let w = q.swap_remove(i);
                        if let Some(rc) = w.upgrade() { return Some(rc); }
                        // shouldn't happen (we just upgraded it above), but
                        // loop again defensively instead of unwrapping.
                    }
                    None => {
                        // Every remaining entry is dead; drop them all.
                        q.clear();
                        return None;
                    }
                }
            }
        });
        let Some(rc) = rc else { break };
        let should_render = {
            let i = rc.borrow();
            i.dirty && !i.unmounted
        };
        if should_render {
            crate::diff::rerender_component(rc);
        }
    }

    run_layout_effects();
    run_effects();

    // Flush transition queue in a rAF
    let has_transitions = TRANSITION_QUEUE.with(|q| !q.borrow().is_empty());
    if has_transitions {
        // Same reasoning as FLUSH_CB above: reuse one persistent `Fn` closure.
        let cb = TRANSITION_CB.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let closure = Closure::wrap(Box::new(|| {
                    let insts: Vec<Weak<RefCell<ComponentInst>>> = TRANSITION_QUEUE.with(|q| {
                        std::mem::take(&mut *q.borrow_mut())
                    });
                    for weak in insts {
                        let Some(rc) = weak.upgrade() else { continue };
                        if !rc.borrow().unmounted {
                            rc.borrow_mut().dirty = true;
                            crate::diff::rerender_component(rc);
                        }
                    }
                    run_layout_effects();
                    run_effects();
                }) as Box<dyn Fn()>);
                *slot = Some(closure.into_js_value().unchecked_into::<js_sys::Function>());
            }
            slot.as_ref().unwrap().clone()
        });
        let window = web_sys::window().expect("no window");
        let _ = window.request_animation_frame(&cb);
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

/// Run the cleanup + pending callback on hooks[idx], then store the
/// returned cleanup back. `inst` is `Weak` since the component may have unmounted by the time this runs.
fn run_one_effect(inst: Weak<RefCell<ComponentInst>>, idx: usize, is_layout: bool) {
    let Some(rc) = inst.upgrade() else { return };
    if rc.borrow().unmounted { return; }

    let (old_cleanup, pending) = {
        let mut i = rc.borrow_mut();
        if idx >= i.hooks.len() { return; }
        match &mut i.hooks[idx] {
            HookSlot::Effect { cleanup, pending, .. } if !is_layout => {
                (cleanup.take(), pending.take())
            }
            HookSlot::LayoutEffect { cleanup, pending, .. } if is_layout => {
                (cleanup.take(), pending.take())
            }
            _ => (None, None),
        }
    };

    if let Some(c) = old_cleanup {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(c));
    }

    if let Some(p) = pending {
        let new_cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(p))
            .ok()
            .flatten();
        // Still holding `rc`, so the instance can't be freed, but the hook
        // slot could be reset by a re-entrant render; re-check the index.
        let mut i = rc.borrow_mut();
        if idx < i.hooks.len() {
            match &mut i.hooks[idx] {
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