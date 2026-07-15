//! Microtask-batched rerender scheduler (architecture mirrors Preact's).
//! setState/dispatch push into DIRTY_QUEUE, flushed depth-first on the
//! next microtask.

use std::{
	cell::RefCell,
	rc::{Rc, Weak},
};
use wasm_bindgen::{JsCast, prelude::*};

use crate::hooks::{ComponentInst, HookSlot};

thread_local! {
	/// Components waiting for a synchronous re-render. Stored as `Weak`
	/// since a component may unmount while sitting in the queue.
	static DIRTY_QUEUE: RefCell<Vec<Weak<RefCell<ComponentInst>>>> = const { RefCell::new(Vec::new()) };

	/// True while a microtask flush is already scheduled.
	static FLUSH_PENDING: RefCell<bool> = const { RefCell::new(false) };

	/// Pending useEffect callbacks (run asynchronously, after paint).
	pub(crate) static PENDING_EFFECTS: RefCell<Vec<EffectSlot>> = const { RefCell::new(Vec::new()) };

	/// Pending useLayoutEffect callbacks (run synchronously, before paint).
	pub(crate) static PENDING_LAYOUT_EFFECTS: RefCell<Vec<EffectSlot>> = const { RefCell::new(Vec::new()) };

	/// Cached microtask-flush callback, built once and reused (a one-shot
	/// `Closure::once` would panic on the second, re-entrant flush).
	static FLUSH_CB: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// Points back to the component + hook index so a flush can retrieve the
/// pending callback/cleanup that schedule_effect_inner() stored on the hook.
pub struct EffectSlot {
	pub inst: Weak<RefCell<ComponentInst>>,
	pub idx: usize,
}

/// Mark a component instance as dirty and schedule a flush. Takes a `Weak`
/// so a setState call after unmount becomes a no-op instead of touching freed memory.
pub fn enqueue_render(inst: Weak<RefCell<ComponentInst>>) {
	let Some(rc) = inst.upgrade() else { return };

	let (already_dirty, unmounted) = {
		let i = rc.borrow();
		(i.dirty, i.unmounted)
	};
	if already_dirty || unmounted {
		return;
	}

	rc.borrow_mut().dirty = true;
	DIRTY_QUEUE.with(|q| q.borrow_mut().push(Rc::downgrade(&rc)));
	schedule_flush();
}

/// Queue a useEffect slot.
pub fn enqueue_effect(slot: EffectSlot) {
	PENDING_EFFECTS.with(|q| q.borrow_mut().push(slot));
}

/// Queue a useLayoutEffect slot.
pub fn enqueue_layout_effect(slot: EffectSlot) {
	PENDING_LAYOUT_EFFECTS.with(|q| q.borrow_mut().push(slot));
}

fn schedule_flush() {
	let already = FLUSH_PENDING.with(|f| {
		let was = *f.borrow();
		*f.borrow_mut() = true;
		was
	});
	if already {
		return;
	}

	let cb = FLUSH_CB.with(|slot| {
		let mut slot = slot.borrow_mut();
		if slot.is_none() {
			let closure = Closure::wrap(Box::new(|| {
				FLUSH_PENDING.with(|f| *f.borrow_mut() = false);
				flush_rerenders();
			}) as Box<dyn Fn()>);
			// Leak intentionally: lives for the app's entire lifetime.
			*slot = Some(closure.into_js_value().unchecked_into::<js_sys::Function>());
		}
		slot.as_ref().expect("slot was just set above").clone()
	});

	// queueMicrotask is not in web-sys yet; call via js-sys.
	let fn_ = js_sys::Function::new_no_args("queueMicrotask(arguments[0])");
	let _ = fn_.call1(&JsValue::NULL, &cb);
}

/// Same as `schedule_flush`, but yields to the browser's macrotask queue
/// (`setTimeout(0)`) instead of a microtask. Used only when
/// `flush_rerenders` bails out of a runaway loop: re-queuing via
/// `queueMicrotask` wouldn't actually give control back to the browser,
/// since microtasks scheduled from within microtask processing still run
/// before the next paint/event — a `setState`-storming component would
/// keep re-triggering flushes forever with the page never becoming
/// responsive. `setTimeout` breaks that chain.
fn schedule_flush_deferred() {
	FLUSH_PENDING.with(|f| *f.borrow_mut() = true);

	let cb = FLUSH_CB.with(|slot| {
		let mut slot = slot.borrow_mut();
		if slot.is_none() {
			let closure = Closure::wrap(Box::new(|| {
				FLUSH_PENDING.with(|f| *f.borrow_mut() = false);
				flush_rerenders();
			}) as Box<dyn Fn()>);
			*slot = Some(closure.into_js_value().unchecked_into::<js_sys::Function>());
		}
		slot.as_ref().expect("slot was just set above").clone()
	});

	let fn_ = js_sys::Function::new_no_args("setTimeout(arguments[0], 0)");
	let _ = fn_.call1(&JsValue::NULL, &cb);
}

/// Hard cap on how many components a single `flush_rerenders` call will
/// render before bailing out. Exists to stop a component that dirties
/// itself unconditionally (e.g. calling its own setState every render, or
/// from a layout effect that always fires) from spinning the drain loop
/// forever and hanging the page. `pub` so tests can assert the exact
/// bailout point.
pub const MAX_FLUSH_ITERATIONS: u32 = 1000;

pub fn flush_rerenders() {
	let mut iterations: u32 = 0;
	// Drain the dirty queue, depth-sorted (parents before children).
	loop {
		iterations += 1;
		if iterations > MAX_FLUSH_ITERATIONS {
			crate::console_warn!(
				"[micro-react] flush_rerenders bailed out after {} renders in a single flush. \
				 This usually means a component is unconditionally re-dirtying itself (e.g. calling \
				 its own setState every render, or from an effect with no guard/dependency check). \
				 Remaining pending updates were deferred instead of spinning forever — check your \
				 components for setState calls that aren't gated by a condition or a dependency array.",
				MAX_FLUSH_ITERATIONS
			);
			// Defer the rest to a fresh macrotask rather than looping
			// forever or immediately re-queuing a microtask (which
			// wouldn't yield control back to the browser at all).
			schedule_flush_deferred();
			return;
		}

		let rc = DIRTY_QUEUE.with(|q| {
			let mut q = q.borrow_mut();
			// Drop any entries whose component has since unmounted before
			// picking the next one to run.
			loop {
				if q.is_empty() {
					return None;
				}
				let idx = q
					.iter()
					.enumerate()
					.filter_map(|(i, w)| w.upgrade().map(|rc| (i, rc.borrow().depth)))
					.min_by_key(|(_, depth)| *depth)
					.map(|(i, _)| i);
				match idx {
					Some(i) => {
						let w = q.swap_remove(i);
						if let Some(rc) = w.upgrade() {
							return Some(rc);
						}
					}
					None => {
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
}

pub fn run_layout_effects() {
	let slots: Vec<EffectSlot> = PENDING_LAYOUT_EFFECTS.with(|q| std::mem::take(&mut *q.borrow_mut()));
	for slot in slots {
		run_one_effect(slot.inst, slot.idx, true);
	}
}

pub fn run_effects() {
	let slots: Vec<EffectSlot> = PENDING_EFFECTS.with(|q| std::mem::take(&mut *q.borrow_mut()));
	for slot in slots {
		run_one_effect(slot.inst, slot.idx, false);
	}
}

/// Run the cleanup + pending callback on hooks[idx], then store the
/// returned cleanup back. `inst` is `Weak` since the component may have
/// unmounted by the time this runs.
fn run_one_effect(inst: Weak<RefCell<ComponentInst>>, idx: usize, is_layout: bool) {
	let Some(rc) = inst.upgrade() else { return };
	if rc.borrow().unmounted {
		return;
	}

	let (old_cleanup, pending) = {
		let mut i = rc.borrow_mut();
		if idx >= i.hooks.len() {
			return;
		}
		match &mut i.hooks[idx] {
			HookSlot::Effect { cleanup, pending, .. } if !is_layout => (cleanup.take(), pending.take()),
			HookSlot::LayoutEffect { cleanup, pending, .. } if is_layout => (cleanup.take(), pending.take()),
			_ => (None, None),
		}
	};

	if let Some(c) = old_cleanup {
		let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(c));
	}

	if let Some(p) = pending {
		let new_cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(p)).ok().flatten();
		// Still holding `rc`, so the instance can't be freed, but the hook
		// slot could be reset by a re-entrant render; re-check the index.
		let mut i = rc.borrow_mut();
		if idx < i.hooks.len() {
			match &mut i.hooks[idx] {
				HookSlot::Effect { cleanup, .. } if !is_layout => {
					*cleanup = new_cleanup;
				}
				HookSlot::LayoutEffect { cleanup, .. } if is_layout => {
					*cleanup = new_cleanup;
				}
				_ => {}
			}
		}
	}
}
