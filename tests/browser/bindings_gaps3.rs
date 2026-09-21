//! Fourth pass: the two `src/bindings.rs`/`src/context.rs` test-coverage gaps
//! left open by `bindings_gaps2.rs`:
//!
//! - `record_create_context_call`'s leak-count warning actually reaching
//!   `console.warn` (as opposed to just the counter it's driven by, which
//!   `bindings_gaps2.rs` already covers) — done here via a real
//!   `console.warn` spy, monkey-patched on the global `console` object for
//!   the duration of the test and restored on drop.
//! - `ResetOnDrop` clearing `js_create_error_boundary`'s `in_progress` guard
//!   specifically after a panic, not just a normal return (already covered).
//!   Written but `#[ignore]`d for the same reason as
//!   `error_boundary_still_cannot_catch_a_genuine_rust_panic` in
//!   `reconciler.rs`: `std::panic::catch_unwind` does not actually catch on
//!   the stable wasm32-unknown-unknown toolchain this project targets, so
//!   deliberately panicking here would trap (abort) the whole test binary
//!   rather than exercise the guard. See that test's `#[ignore]` message for
//!   the full explanation; the reasoning is identical here.

use js_sys::{Object, Reflect};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::js_create_context;

// ─── console.warn spy scaffolding ───

/// Monkey-patches `console.warn` with a closure that records every message
/// (stringified via `format!("{:?}", ...)`-free `JsValue::as_string`, falling
/// back to the value's own string coercion for non-string args) into a
/// shared `Vec`, and restores the original function when dropped — so a
/// panicking assertion inside the test still can't leave `console.warn`
/// patched for whichever test runs next in this shared browser context.
struct ConsoleWarnSpy {
	console: JsValue,
	original_warn: JsValue,
	messages: Rc<RefCell<Vec<String>>>,
	_closure: Closure<dyn Fn(JsValue)>,
}

impl ConsoleWarnSpy {
	fn install() -> Self {
		let global = js_sys::global();
		let console = Reflect::get(&global, &"console".into()).expect("global console object should exist in a browser test context");
		let original_warn = Reflect::get(&console, &"warn".into()).expect("console.warn should exist");

		let messages: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
		let messages_for_closure = messages.clone();
		let closure = Closure::wrap(Box::new(move |arg: JsValue| {
			let text = arg.as_string().unwrap_or_else(|| format!("{arg:?}"));
			messages_for_closure.borrow_mut().push(text);
		}) as Box<dyn Fn(JsValue)>);

		Reflect::set(&console, &"warn".into(), closure.as_ref().unchecked_ref()).expect("should be able to replace console.warn");

		ConsoleWarnSpy { console, original_warn, messages, _closure: closure }
	}

	fn messages(&self) -> Vec<String> {
		self.messages.borrow().clone()
	}
}

impl Drop for ConsoleWarnSpy {
	fn drop(&mut self) {
		// Best-effort restore; if this ever fails there's nothing more to do
		// from a `Drop` impl, and leaving the original in place (rather than
		// panicking in a drop) is the safer failure mode either way.
		let _ = Reflect::set(&self.console, &"warn".into(), &self.original_warn);
	}
}

// ─── record_create_context_call's warning: does it actually reach console.warn? ───

#[wasm_bindgen_test]
fn create_context_warns_via_console_warn_once_call_count_exceeds_one() {
	// `CTX_CREATE_COUNT` is a process-global counter shared across every test
	// in this binary (see `bindings_gaps2.rs`'s equivalent test), so by the
	// time this test runs it may already be well past 1 from earlier tests —
	// meaning the very first `js_create_context` call below could already be
	// the one that trips `call_count > 1` and warns. That's fine: this test
	// only needs *some* call within it to warn, not specifically the first.
	let spy = ConsoleWarnSpy::install();

	let _ctx_a = js_create_context(JsValue::from_str("a")).expect("createContext should succeed");
	let _ctx_b = js_create_context(JsValue::from_str("b")).expect("createContext should succeed");

	let messages = spy.messages();
	assert!(!messages.is_empty(), "expected at least one console.warn call once createContext's running total exceeded 1, got none");
	assert!(
		messages.iter().any(|m| m.contains("createContext") && m.contains("leaks")),
		"expected the leak-warning message to mention createContext and leaking, got: {messages:?}"
	);
}

#[wasm_bindgen_test]
fn create_context_warning_is_restored_after_spy_is_dropped() {
	// Sanity check on the spy scaffolding itself: once it's dropped,
	// console.warn should be back to whatever it was before (not left
	// pointing at the spy closure, which would silently swallow every
	// subsequent warning in later tests sharing this browser context).
	let global = js_sys::global();
	let console = Reflect::get(&global, &"console".into()).unwrap();
	let before = Reflect::get(&console, &"warn".into()).unwrap();

	{
		let _spy = ConsoleWarnSpy::install();
		let patched = Reflect::get(&console, &"warn".into()).unwrap();
		assert!(!JsValue::eq(&before, &patched), "console.warn should be replaced while the spy is installed");
	}

	let after = Reflect::get(&console, &"warn".into()).unwrap();
	assert!(JsValue::eq(&before, &after), "console.warn should be restored to its original value once the spy is dropped");
}

// ─── js_create_error_boundary: in_progress guard resets on panic, not just normal return ───

#[wasm_bindgen_test]
#[ignore = "std::panic::catch_unwind does not actually catch panics on wasm32-unknown-unknown \
            with the stable wasm-pack/wasm-bindgen toolchain: without nightly -Z build-std plus \
            the wasm exception-handling target feature, a panic traps (aborts) the whole wasm \
            instance instead of unwinding, so deliberately panicking here would take the whole \
            test binary down (see 'RuntimeError: unreachable executed') rather than exercise \
            `ResetOnDrop`'s `Drop` impl running mid-unwind. This mirrors \
            `error_boundary_still_cannot_catch_a_genuine_rust_panic` in `reconciler.rs` exactly, \
            down to the reason: `ResetOnDrop` is correct Rust (`Drop::drop` does run during an \
            unwind on targets where unwinding actually happens, e.g. native) and this test would \
            pass there today — it's specifically this target/toolchain combination that can't \
            observe it. Re-enable once the toolchain gains stable wasm unwinding support."]
fn error_boundary_in_progress_guard_resets_on_panic_not_just_normal_return() {
	use micro_react::bindings::js_create_error_boundary;
	use micro_react::render::Root;
	use micro_react::vnode::{ComponentFn, Props, VNode};

	// Drive the panic through a `children` accessor getter (the same trick
	// `bindings_gaps2.rs`'s reentrancy test uses), so the panic originates
	// from genuinely calling into the real `js_create_error_boundary_inner`
	// via `Reflect::get(&props, "children")`, not from a hand-rolled stand-in.
	let doc = web_sys::window().unwrap().document().unwrap();
	let container = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&container).unwrap();
	let mut root = Root::new(container.clone());

	let boundary_val = js_create_error_boundary();
	let boundary_fn: js_sys::Function = boundary_val.unchecked_into();
	let boundary_fn_for_comp = boundary_fn.clone();

	let comp = ComponentFn::new(move |_props: Props| {
		let props_obj = Object::new();
		let getter =
			Closure::wrap(Box::new(move || -> JsValue { panic!("boom: simulated failure while reading children") }) as Box<dyn Fn() -> JsValue>);
		let descriptor = Object::new();
		Reflect::set(&descriptor, &"get".into(), getter.as_ref().unchecked_ref()).unwrap();
		Reflect::set(&descriptor, &"configurable".into(), &JsValue::TRUE).unwrap();
		Reflect::define_property(&props_obj, &JsValue::from_str("children"), &descriptor).unwrap();
		getter.forget();

		// This panics inside `js_create_error_boundary_inner`, unwinding
		// through `ResetOnDrop`'s `Drop` impl before reaching here.
		let result = boundary_fn_for_comp.call1(&JsValue::NULL, &props_obj.into())?;
		Ok(VNode::text(result.as_string().unwrap_or_default()))
	});

	// Expected to trap the wasm instance on this toolchain; see the
	// `#[ignore]` message above. On a toolchain with real unwinding, this
	// `render` call returns normally (diff.rs's own catch_unwind converts the
	// panic to a logged error), and the assertions below verify the guard.
	let _ = root.render(VNode::component("BoundaryPanicHost", comp, vec![]));

	// If `ResetOnDrop` ran during the unwind, `in_progress` is back to
	// `false`, so a fresh, separate, non-reentrant call must still run for
	// real rather than being short-circuited to NULL forever.
	let props_obj2 = Object::new();
	Reflect::set(&props_obj2, &"children".into(), &JsValue::from_str("after-panic")).unwrap();
	let ret = boundary_fn.call1(&JsValue::NULL, &props_obj2.into()).unwrap();
	assert_eq!(
		ret.as_string().as_deref(),
		Some("after-panic"),
		"the in_progress guard must reset after a panic, not just after a normal return, or every call after the first panic would be silently dropped to NULL forever"
	);
}
