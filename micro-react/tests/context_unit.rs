//! Tests for `context::Context` — pure Rust logic (thread-local map + Rc
//! callbacks) with no JS/DOM calls of its own, but run here via
//! `wasm-bindgen-test` (like the rest of `tests/`) so `build.sh`'s single
//! `wasm-pack test --headless --firefox` step picks them up alongside
//! everything else, rather than needing a separate `cargo test --lib`
//! invocation just for this file.
//!
//! `use_context` itself needs a live `ComponentInst` (via `current_weak()`
//! / `use_effect_nodrop`), so it's covered by the component-level
//! integration tests in `tests/hooks_scheduler.rs` instead.

use std::cell::Cell;
use std::rc::Rc;
use wasm_bindgen_test::*;

use micro_react::context::Context;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn new_context_returns_default_value() {
	let ctx = Context::new(42i32);
	assert_eq!(ctx.current_value(), 42);
}

#[wasm_bindgen_test]
fn set_value_then_current_value_returns_new_value() {
	let ctx = Context::new("default".to_string());
	ctx.set_value("updated".to_string());
	assert_eq!(ctx.current_value(), "updated");
}

#[wasm_bindgen_test]
fn each_context_instance_has_a_unique_id() {
	let a = Context::new(1i32);
	let b = Context::new(1i32);
	assert_ne!(a.id, b.id);
}

#[wasm_bindgen_test]
fn cloned_context_shares_the_same_id_and_storage() {
	let ctx = Context::new(0i32);
	let clone = ctx.clone();
	assert_eq!(ctx.id, clone.id);
	clone.set_value(99);
	// Reading through the original handle sees the value set via the clone.
	assert_eq!(ctx.current_value(), 99);
}

#[wasm_bindgen_test]
fn two_independent_contexts_do_not_leak_values_into_each_other() {
	let a = Context::new(1i32);
	let b = Context::new(100i32);
	a.set_value(5);
	assert_eq!(a.current_value(), 5);
	assert_eq!(b.current_value(), 100); // untouched, still its own default
}

#[wasm_bindgen_test]
fn subscribe_listener_is_called_on_set_value() {
	let ctx = Context::new(0i32);
	let called = Rc::new(Cell::new(false));
	let called_clone = called.clone();
	let _unsub = ctx.subscribe(Rc::new(move || called_clone.set(true)));

	assert!(!called.get());
	ctx.set_value(1);
	assert!(called.get());
}

#[wasm_bindgen_test]
fn subscribe_listener_fires_once_per_set_value_call() {
	let ctx = Context::new(0i32);
	let count = Rc::new(Cell::new(0));
	let count_clone = count.clone();
	let _unsub = ctx.subscribe(Rc::new(move || count_clone.set(count_clone.get() + 1)));

	ctx.set_value(1);
	ctx.set_value(2);
	ctx.set_value(3);
	assert_eq!(count.get(), 3);
}

#[wasm_bindgen_test]
fn unsubscribe_stops_further_notifications() {
	let ctx = Context::new(0i32);
	let count = Rc::new(Cell::new(0));
	let count_clone = count.clone();
	let unsub = ctx.subscribe(Rc::new(move || count_clone.set(count_clone.get() + 1)));

	ctx.set_value(1);
	assert_eq!(count.get(), 1);

	unsub();
	ctx.set_value(2);
	// No further increments after unsubscribing.
	assert_eq!(count.get(), 1);
}

#[wasm_bindgen_test]
fn unsubscribing_one_listener_does_not_affect_another() {
	let ctx = Context::new(0i32);
	let count_a = Rc::new(Cell::new(0));
	let count_b = Rc::new(Cell::new(0));
	let (ca, cb) = (count_a.clone(), count_b.clone());

	let unsub_a = ctx.subscribe(Rc::new(move || ca.set(ca.get() + 1)));
	let _unsub_b = ctx.subscribe(Rc::new(move || cb.set(cb.get() + 1)));

	unsub_a();
	ctx.set_value(1);

	assert_eq!(count_a.get(), 0);
	assert_eq!(count_b.get(), 1);
}

#[wasm_bindgen_test]
fn multiple_listeners_on_same_context_all_fire() {
	let ctx = Context::new(0i32);
	let count = Rc::new(Cell::new(0));

	let c1 = count.clone();
	let _unsub1 = ctx.subscribe(Rc::new(move || c1.set(c1.get() + 1)));
	let c2 = count.clone();
	let _unsub2 = ctx.subscribe(Rc::new(move || c2.set(c2.get() + 1)));
	let c3 = count.clone();
	let _unsub3 = ctx.subscribe(Rc::new(move || c3.set(c3.get() + 1)));

	ctx.set_value(1);
	assert_eq!(count.get(), 3);
}

#[wasm_bindgen_test]
fn set_value_with_no_listeners_does_not_panic() {
	let ctx = Context::new(0i32);
	ctx.set_value(1); // just shouldn't panic
	assert_eq!(ctx.current_value(), 1);
}
