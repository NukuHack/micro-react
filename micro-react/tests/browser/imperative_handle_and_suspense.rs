//! Coverage for the remaining "Test coverage gaps" items from the TODO
//! around `useImperativeHandle` and `Suspense`:
//!
//! - A component calling `useImperativeHandle(ref, () => ({ focus, reset }))`
//!   should expose exactly that custom object on the caller's `ref.current`
//!   (not the underlying DOM node), and re-install it when deps change.
//! - Unmounting such a component should null the ref back out.
//! - `<Suspense>` should show its fallback immediately while a child is
//!   pending, then swap to the real children once the thrown promise settles.
//! - `<Suspense>` nested inside `<ErrorBoundary>` must not intercept a real
//!   thrown `Error` from a suspended child — only pending thenables — so the
//!   `ErrorBoundary` above it is the one that ends up showing its fallback.
//! - Several unrelated re-renders while a Suspense child is still pending on
//!   the same promise shouldn't panic or flicker between renders of the
//!   fallback.
//!
//! `useImperativeHandle` is exercised through the real, JS-facing
//! `bindings::js_use_imperative_handle` (it just takes a raw `JsValue` ref
//! and a `Function`, so it's directly callable from a plain Rust
//! `ComponentFn` without needing a JS engine to build the calling
//! component). `Suspense`'s `is_thenable`/retry-on-settle mechanism, however,
//! lives behind a private closure in `bindings.rs` (`js_create_suspense_inner`)
//! that isn't reachable from an external integration test — so, exactly like
//! `reconciler.rs`'s `make_test_boundary` does for `ErrorBoundary`, the tests
//! below reconstruct that same mechanism (`error_setter` + thenable check +
//! retry-on-settle via a real `use_state` hook so the retry actually
//! reschedules a render) using plain Rust closures around the identical
//! `hooks::current_inst`/`error_setter`/`forward_to_ancestor_boundary` plumbing
//! the real implementation uses, rather than a JS-facing stand-in.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use micro_react::bindings::js_use_imperative_handle;
use micro_react::hooks::{current_inst, current_weak, forward_to_ancestor_boundary, use_state};
use micro_react::render::Root;
use micro_react::scheduler::flush_rerenders;
use micro_react::vnode::{ComponentFn, Props, VNode};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

fn make_js_ref_object() -> js_sys::Object {
	let obj = js_sys::Object::new();
	js_sys::Reflect::set(&obj, &"current".into(), &JsValue::NULL).unwrap();
	obj
}

fn ref_current(obj: &js_sys::Object) -> JsValue {
	js_sys::Reflect::get(obj, &"current".into()).unwrap()
}

// ─── useImperativeHandle: exposes the custom handle, not the DOM node ───

#[wasm_bindgen_test]
fn use_imperative_handle_exposes_custom_object_and_updates_when_deps_change() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let ref_obj = make_js_ref_object();
	let handle_build_count = Rc::new(RefCell::new(0u32));
	let handle_build_count_for_comp = handle_build_count.clone();
	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let ref_val: JsValue = ref_obj.clone().into();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let (dep, set_dep) = use_state(0i32);
		*setter_slot_for_comp.borrow_mut() = Some(set_dep);

		let build_count = handle_build_count_for_comp.clone();
		let create_handle: js_sys::Function = Closure::wrap(Box::new(move || -> JsValue {
			*build_count.borrow_mut() += 1;
			let handle = js_sys::Object::new();
			js_sys::Reflect::set(&handle, &"focus".into(), &JsValue::from_str("focus-fn")).unwrap();
			js_sys::Reflect::set(&handle, &"reset".into(), &JsValue::from_str("reset-fn")).unwrap();
			handle.into()
		}) as Box<dyn Fn() -> JsValue>)
		.into_js_value()
		.unchecked_into();

		let deps: JsValue = js_sys::Array::of1(&JsValue::from_f64(dep as f64)).into();
		js_use_imperative_handle(ref_val.clone(), &create_handle, deps);

		VNode::tag("div").text("handle-host").build()
	});

	root.render(VNode::component("ImperativeHandleHost", comp, vec![])).unwrap();

	let current = ref_current(&ref_obj);
	assert!(current.is_object(), "ref.current should be the custom handle object, not null/undefined, got: {current:?}");
	let is_dom_node = current.dyn_ref::<web_sys::Node>().is_some();
	assert!(!is_dom_node, "ref.current should be the custom handle, not the underlying DOM node");
	assert_eq!(
		js_sys::Reflect::get(&current, &"focus".into()).unwrap().as_string().as_deref(),
		Some("focus-fn"),
		"ref.current should exactly be the object returned by createHandle, exposing 'focus'"
	);
	assert_eq!(js_sys::Reflect::get(&current, &"reset".into()).unwrap().as_string().as_deref(), Some("reset-fn"));
	assert_eq!(*handle_build_count.borrow(), 1, "createHandle should have run exactly once so far");

	// Re-render with unchanged deps: the handle should not be rebuilt.
	let set_dep = setter_slot.borrow().clone().unwrap();
	set_dep(0);
	flush_rerenders();
	assert_eq!(*handle_build_count.borrow(), 1, "unchanged deps should not reinstall the imperative handle");

	// Re-render with changed deps: the handle should be rebuilt and reinstalled.
	set_dep(1);
	flush_rerenders();
	assert_eq!(*handle_build_count.borrow(), 2, "changed deps should reinstall the imperative handle");
	let current_after = ref_current(&ref_obj);
	assert!(current_after.is_object(), "ref.current should still be a fresh handle object after deps changed");
}

#[wasm_bindgen_test]
fn use_imperative_handle_nulls_the_ref_out_on_unmount() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let ref_obj = make_js_ref_object();
	let ref_val: JsValue = ref_obj.clone().into();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let create_handle: js_sys::Function =
			Closure::wrap(Box::new(move || -> JsValue { js_sys::Object::new().into() }) as Box<dyn Fn() -> JsValue>).into_js_value().unchecked_into();
		js_use_imperative_handle(ref_val.clone(), &create_handle, JsValue::UNDEFINED);
		VNode::tag("div").text("handle-host").build()
	});

	root.render(VNode::component("ImperativeHandleUnmountHost", comp, vec![])).unwrap();
	assert!(ref_current(&ref_obj).is_object(), "handle should be installed after mount");

	root.unmount();
	assert!(ref_current(&ref_obj).is_null(), "ref.current should be nulled back out on unmount, matching a real ref's teardown semantics");
}

// ─── Suspense mechanics (reconstructed the same way reconciler.rs's ErrorBoundary tests are) ───

/// True if `v` is a thenable (has a callable `.then`) — same convention
/// `bindings::is_thenable` uses to distinguish a suspend signal from a real error.
fn is_thenable(v: &JsValue) -> bool {
	if !v.is_object() {
		return false;
	}
	js_sys::Reflect::get(v, &"then".into()).map(|t| t.is_function()).unwrap_or(false)
}

/// Builds a Suspense-shaped boundary around whatever `make_child` produces,
/// mirroring `js_create_suspense_inner`'s mechanism: register `error_setter`
/// on `current_inst`, treat a thenable as "suspend" (render `fallback_text`
/// and retry once it settles), and forward anything else to whatever real
/// `ErrorBoundary` sits above this one.
fn make_test_suspense(name: &'static str, fallback_text: &'static str, make_child: impl Fn() -> VNode + 'static) -> VNode {
	VNode::component(
		name,
		ComponentFn::infallible(move |_props: Props| {
			let (caught, set_caught) = use_state::<Option<JsValue>>(None);

			{
				let inst_ptr = current_inst();
				let inst_weak = current_weak();
				let set_caught_for_setter = set_caught.clone();
				let setter: Rc<dyn Fn(JsValue)> = Rc::new(move |value: JsValue| {
					if is_thenable(&value) {
						set_caught_for_setter(Some(value.clone()));
						if let Ok(then_fn) = js_sys::Reflect::get(&value, &"then".into()).and_then(|t| t.dyn_into::<js_sys::Function>()) {
							let set_caught_retry = set_caught_for_setter.clone();
							let retry = Closure::once_into_js(move |_: JsValue| {
								set_caught_retry(None);
							});
							let _ = then_fn.call2(&value, &retry, &retry);
						}
					} else if let Some(inst_rc) = inst_weak.upgrade() {
						forward_to_ancestor_boundary(&inst_rc, value);
					}
				});
				// SAFETY: single-threaded WASM; inst_ptr is valid for this render.
				unsafe {
					(*inst_ptr).error_setter = Some(setter);
				}
			}

			if caught.is_some() { VNode::tag("div").attr("class", "suspense-fallback").text(fallback_text).build() } else { make_child() }
		}),
		Vec::new(),
	)
}

/// Same shape as `reconciler.rs::make_test_boundary`, duplicated locally so
/// this file doesn't need to depend on that test module's internals.
fn make_test_error_boundary(name: &'static str, fallback_text: &'static str, make_child: impl Fn() -> VNode + 'static) -> VNode {
	VNode::component(
		name,
		ComponentFn::infallible(move |_props: Props| {
			let (caught, set_caught) = use_state::<Option<String>>(None);
			{
				let inst_ptr = current_inst();
				let set_caught_for_setter = set_caught.clone();
				let setter: Rc<dyn Fn(JsValue)> = Rc::new(move |err: JsValue| {
					set_caught_for_setter(err.as_string().or_else(|| Some(format!("{err:?}"))));
				});
				// SAFETY: single-threaded WASM; inst_ptr is valid for this render.
				unsafe {
					(*inst_ptr).error_setter = Some(setter);
				}
			}
			if caught.is_some() { VNode::tag("div").attr("class", "eb-fallback").text(fallback_text).build() } else { make_child() }
		}),
		Vec::new(),
	)
}

/// A promise (constructed once) plus a matching resolver function, so a test
/// can control exactly when the "async work" a suspending child depends on settles.
fn make_controllable_promise() -> (js_sys::Promise, js_sys::Function) {
	let resolve_slot: Rc<RefCell<Option<js_sys::Function>>> = Rc::new(RefCell::new(None));
	let resolve_slot_for_executor = resolve_slot.clone();
	// `Promise::new`'s executor runs synchronously during construction (per
	// the Promise spec), so `resolve_slot` is guaranteed populated by the
	// time `new` returns below.
	let promise = js_sys::Promise::new(&mut move |resolve: js_sys::Function, _reject: js_sys::Function| {
		*resolve_slot_for_executor.borrow_mut() = Some(resolve);
	});
	let resolve = resolve_slot.borrow().clone().expect("Promise executor runs synchronously, so the resolver should already be captured");
	(promise, resolve)
}

#[wasm_bindgen_test]
async fn suspense_shows_fallback_then_renders_children_once_the_pending_promise_resolves() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let (promise, resolve) = make_controllable_promise();
	let resolved_flag = Rc::new(RefCell::new(false));
	let resolved_flag_for_child = resolved_flag.clone();
	let promise_for_child = promise.clone();

	let tree = make_test_suspense("Suspense", "loading...", move || {
		let resolved_flag = resolved_flag_for_child.clone();
		let promise = promise_for_child.clone();
		VNode::component(
			"SuspendingChild",
			ComponentFn::new(move |_props: Props| {
				if *resolved_flag.borrow() {
					Ok(VNode::tag("div").attr("class", "loaded").text("real content").build())
				} else {
					Err(promise.clone().into())
				}
			}),
			Vec::new(),
		)
	});

	root.render(tree).unwrap();
	assert!(container.inner_html().contains("loading..."), "expected the fallback immediately on first mount, got: {}", container.inner_html());
	assert!(!container.inner_html().contains("real content"));

	// Resolve the underlying promise and let the microtask queue actually
	// run its callbacks: ours (which flips resolved_flag) is attached before
	// Suspense's retry .then (see make_test_suspense), so ordering is safe.
	let resolved_flag_for_then = resolved_flag.clone();
	let flag_then = Closure::once_into_js(move |_: JsValue| {
		*resolved_flag_for_then.borrow_mut() = true;
	});
	// Attach our own settle-order-sensitive flag flip through the same promise
	// instance first, via `Reflect`/`call1` (mirroring the exact idiom
	// `load_module_body`/`js_create_suspense_inner` use elsewhere in this
	// codebase) rather than `js_sys::Promise::then`, to avoid depending on
	// that method's exact `Closure` vs `Function` argument typing.
	let then_fn: js_sys::Function = js_sys::Reflect::get(&promise, &"then".into()).unwrap().unchecked_into();
	let flag_then_fn: js_sys::Function = flag_then.unchecked_into();
	let _ = then_fn.call1(&promise, &flag_then_fn);
	resolve.call1(&JsValue::NULL, &JsValue::UNDEFINED).unwrap();

	wasm_bindgen_futures::JsFuture::from(promise.clone()).await.unwrap();
	// One more microtask turn so Suspense's own retry .then (attached after
	// ours, inside the error_setter above) has also had a chance to fire.
	wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::UNDEFINED)).await.unwrap();
	flush_rerenders();

	let html = container.inner_html();
	assert!(html.contains("real content"), "expected the real children to render once the promise resolved, got: {html}");
	assert!(!html.contains("loading..."), "the fallback should be gone once the real content renders, got: {html}");
}

#[wasm_bindgen_test]
fn suspense_does_not_intercept_a_real_error_error_boundary_above_it_catches_instead() {
	// Nest Suspense inside an ErrorBoundary, and have the "suspended" child
	// throw a genuine Err(Error) rather than a thenable. Suspense's
	// `is_thenable` check should hand that straight to the ancestor
	// ErrorBoundary via `forward_to_ancestor_boundary` instead of treating it
	// as a suspend signal.
	let container = make_container();
	let mut root = Root::new(container.clone());

	let tree = make_test_error_boundary("Boundary", "boundary caught it", || {
		make_test_suspense("Suspense", "loading...", || {
			VNode::component(
				"RealErrorChild",
				ComponentFn::new(|_props: Props| Err(JsValue::from(js_sys::Error::new("genuine failure")))),
				Vec::new(),
			)
		})
	});

	root.render(tree).unwrap();

	let html = container.inner_html();
	assert!(html.contains("boundary caught it"), "a real thrown Error past Suspense should be caught by the ErrorBoundary above it, got: {html}");
	assert!(!html.contains("loading..."), "Suspense should not show its own fallback for a real error, only for pending thenables, got: {html}");
}

#[wasm_bindgen_test]
fn multiple_unrelated_rerenders_while_suspense_child_is_pending_do_not_panic_or_flicker() {
	// A sibling with its own independent state, re-rendered several times
	// while the Suspense subtree is still pending on the same
	// never-settling-in-this-test promise. This shouldn't panic (the
	// underlying failure mode this guards: `.then` being effectively
	// re-armed and firing multiple times per throw, e.g. from a suspending
	// child being re-diffed on each unrelated render) and the fallback
	// should still show exactly once, not duplicated or missing.
	let container = make_container();
	let mut root = Root::new(container.clone());

	let (promise, _resolve) = make_controllable_promise();
	let promise_for_child = promise.clone();

	let setter_slot: Rc<RefCell<Option<Rc<dyn Fn(i32)>>>> = Rc::new(RefCell::new(None));
	let setter_slot_for_comp = setter_slot.clone();

	let sibling = VNode::component(
		"UnrelatedSibling",
		ComponentFn::infallible(move |_props: Props| {
			let (count, set_count) = use_state(0i32);
			*setter_slot_for_comp.borrow_mut() = Some(set_count);
			VNode::tag("span").attr("class", "sibling-count").text(count.to_string()).build()
		}),
		Vec::new(),
	);

	let suspense = make_test_suspense("Suspense", "loading...", move || {
		let promise = promise_for_child.clone();
		VNode::component("NeverResolvingChild", ComponentFn::new(move |_props: Props| Err(promise.clone().into())), Vec::new())
	});

	root.render(VNode::fragment(vec![suspense, sibling])).unwrap();
	assert_eq!(count_occurrences(&container.inner_html(), "loading..."), 1, "fallback should appear exactly once on first mount");

	let set_count = setter_slot.borrow().clone().expect("sibling should have registered its setState setter on mount");
	for i in 1..=5 {
		set_count(i);
		flush_rerenders();
	}

	let html = container.inner_html();
	assert_eq!(count_occurrences(&html, "loading..."), 1, "fallback should still show exactly once after several unrelated re-renders, got: {html}");
	assert!(html.contains("sibling-count") && html.contains('5'), "the unrelated sibling should still have re-rendered normally, got: {html}");
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
	haystack.matches(needle).count()
}
