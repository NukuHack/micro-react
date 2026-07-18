//! Tests for `events::set_event_handler`, which installs/removes the
//! Preact-style logical-clock proxy listener on real DOM elements and
//! must dispatch/suppress handlers correctly — needs a real DOM + JS
//! function values, so these run via `wasm-bindgen-test` in a headless
//! browser (see `tests/browser/reconciler.rs` for the invocation).

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_test::*;

use micro_react::events::set_event_handler;

fn make_button() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("button").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

fn counting_handler() -> (js_sys::Function, Rc<RefCell<u32>>) {
	let count = Rc::new(RefCell::new(0));
	let count_clone = count.clone();
	let closure = Closure::wrap(Box::new(move |_e: web_sys::Event| {
		*count_clone.borrow_mut() += 1;
	}) as Box<dyn Fn(web_sys::Event)>);
	let f: js_sys::Function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
	closure.forget();
	(f, count)
}

fn dispatch_click(el: &web_sys::Element) {
	let ev = web_sys::Event::new("click").unwrap();
	let _ = el.dispatch_event(&ev);
}

#[wasm_bindgen_test]
fn handler_fires_on_dispatched_event() {
	let el = make_button();
	let (handler, count) = counting_handler();
	set_event_handler(&el, "click", false, Some(&handler), None);

	dispatch_click(&el);
	assert_eq!(*count.borrow(), 1);
}

#[wasm_bindgen_test]
fn handler_fires_once_per_dispatch() {
	let el = make_button();
	let (handler, count) = counting_handler();
	set_event_handler(&el, "click", false, Some(&handler), None);

	dispatch_click(&el);
	dispatch_click(&el);
	dispatch_click(&el);
	assert_eq!(*count.borrow(), 3);
}

#[wasm_bindgen_test]
fn removing_handler_stops_it_from_firing() {
	let el = make_button();
	let (handler, count) = counting_handler();
	set_event_handler(&el, "click", false, Some(&handler), None);
	dispatch_click(&el);
	assert_eq!(*count.borrow(), 1);

	// Remove: handler = None, old_handler = Some(previous).
	set_event_handler(&el, "click", false, None, Some(&handler));
	dispatch_click(&el);
	// Still 1 — the removed handler must not fire again.
	assert_eq!(*count.borrow(), 1);
}

#[wasm_bindgen_test]
fn replacing_handler_only_fires_the_new_one() {
	let el = make_button();
	let (old_handler, old_count) = counting_handler();
	let (new_handler, new_count) = counting_handler();

	set_event_handler(&el, "click", false, Some(&old_handler), None);
	dispatch_click(&el);
	assert_eq!(*old_count.borrow(), 1);

	// Replace: handler = Some(new), old_handler = Some(old) so no second
	// proxy is installed — the map entry is swapped instead.
	set_event_handler(&el, "click", false, Some(&new_handler), Some(&old_handler));
	dispatch_click(&el);

	assert_eq!(*old_count.borrow(), 1, "old handler must not fire again");
	assert_eq!(*new_count.borrow(), 1, "new handler should now fire");
}

#[wasm_bindgen_test]
fn capture_and_bubble_handlers_on_the_same_event_name_are_independent() {
	let el = make_button();
	let (bubble_handler, bubble_count) = counting_handler();
	let (capture_handler, capture_count) = counting_handler();

	set_event_handler(&el, "click", false, Some(&bubble_handler), None);
	set_event_handler(&el, "click", true, Some(&capture_handler), None);

	dispatch_click(&el);

	assert_eq!(*bubble_count.borrow(), 1);
	assert_eq!(*capture_count.borrow(), 1);

	// Removing the capture handler must not affect the bubble one.
	set_event_handler(&el, "click", true, None, Some(&capture_handler));
	dispatch_click(&el);
	assert_eq!(*bubble_count.borrow(), 2);
	assert_eq!(*capture_count.borrow(), 1);
}

#[wasm_bindgen_test]
fn different_event_names_do_not_interfere() {
	let el = make_button();
	let (click_handler, click_count) = counting_handler();
	let (focus_handler, focus_count) = counting_handler();

	set_event_handler(&el, "click", false, Some(&click_handler), None);
	set_event_handler(&el, "focus", false, Some(&focus_handler), None);

	dispatch_click(&el);
	assert_eq!(*click_count.borrow(), 1);
	assert_eq!(*focus_count.borrow(), 0);
}
