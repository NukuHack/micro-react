//! Additional coverage for `vnode`'s less-exercised "inner stuff":
//! `with_children`, `ElementBuilder::on`/`ref_`, `Portal`, `ComponentInstSlot`,
//! the `FLAG_*` reconciler-bookkeeping constants, `PropVal::Callback`/`Js`
//! equality, `ComponentFn::new`'s error path, and `with_key` on `Fragment`.
//!
//! Complements `tests/browser/vnode_unit.rs`, which covers the more commonly-hit
//! constructors/builders. Runs via `wasm-bindgen-test` (like the rest of
//! `tests/`) since a few cases touch real JS functions/DOM elements:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox

use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

use micro_react::vnode::{ComponentFn, ComponentInstSlot, FLAG_INSERT, FLAG_MATCHED, JsCallback, NodeRef, PropVal, VNode, VNodeInner};

fn make_element() -> web_sys::Element {
	web_sys::window().unwrap().document().unwrap().create_element("div").unwrap()
}

// ── with_children ──

#[wasm_bindgen_test]
fn with_children_attaches_raw_jsx_children_to_component_vnode() {
	let render = ComponentFn::infallible(|_| VNode::null());
	let v = VNode::component("Layout", render, vec![]).with_children(vec![VNode::text("a"), VNode::text("b")]);
	match v.inner {
		VNodeInner::Component { children, .. } => assert_eq!(children.len(), 2),
		_ => panic!("expected component vnode"),
	}
}

#[wasm_bindgen_test]
fn with_children_is_a_noop_on_non_component_vnodes() {
	// Only `Component` has a raw-children slot; other variants should just
	// ignore the call rather than panic.
	let v = VNode::text("x").with_children(vec![VNode::text("ignored")]);
	match v.inner {
		VNodeInner::Text(s) => assert_eq!(s, "x"),
		_ => panic!("expected text vnode unchanged"),
	}

	let el = VNode::tag("div").build().with_children(vec![VNode::text("ignored")]);
	match el.inner {
		VNodeInner::Element { children, .. } => assert!(children.is_empty()),
		_ => panic!("expected element vnode unchanged"),
	}
}

#[wasm_bindgen_test]
fn with_children_replaces_any_previously_set_children() {
	let render = ComponentFn::infallible(|_| VNode::null());
	let v = VNode::component("Layout", render, vec![])
		.with_children(vec![VNode::text("first")])
		.with_children(vec![VNode::text("second"), VNode::text("third")]);
	match v.inner {
		VNodeInner::Component { children, .. } => {
			assert_eq!(children.len(), 2);
			match &children[0].inner {
				VNodeInner::Text(s) => assert_eq!(s, "second"),
				_ => panic!("expected text child"),
			}
		}
		_ => panic!("expected component vnode"),
	}
}

// ── with_key on Fragment ──

#[wasm_bindgen_test]
fn with_key_sets_key_on_fragment_vnode() {
	let v = VNode::fragment(vec![VNode::text("a")]).with_key(Some("frag-1".to_string()));
	assert_eq!(v.key(), Some("frag-1"));
}

#[wasm_bindgen_test]
fn with_key_can_clear_a_previously_set_key() {
	let v = VNode::tag("li").key("row-1").build().with_key(None);
	assert_eq!(v.key(), None);
}

// ── ElementBuilder::on / ref_ ──

#[wasm_bindgen_test]
fn element_builder_on_sets_a_callback_prop() {
	let f = js_sys::Function::new_no_args("return 1;");
	let v = VNode::tag("button").on("onClick", f.clone()).build();
	match v.inner {
		VNodeInner::Element { props, .. } => {
			assert_eq!(props.len(), 1);
			assert_eq!(props[0].0, "onClick");
			match &props[0].1 {
				PropVal::Callback(cb) => assert!(js_sys::Object::is(cb.as_ref(), f.as_ref())),
				_ => panic!("expected a Callback prop"),
			}
		}
		_ => panic!("expected element vnode"),
	}
}

#[wasm_bindgen_test]
fn element_builder_ref_attaches_a_node_ref() {
	let node_ref = NodeRef::new();
	let v = VNode::tag("input").ref_(node_ref).build();
	match v.inner {
		VNodeInner::Element { ref_, .. } => assert!(ref_.is_some()),
		_ => panic!("expected element vnode"),
	}
}

// ── Portal ──

#[wasm_bindgen_test]
fn portal_vnode_inner_holds_its_container_and_children() {
	// `VNode` itself can't be built via struct-literal syntax from outside
	// the crate (its reconciler-bookkeeping fields are `pub(crate)`, and
	// there's no public constructor for a bare `Portal`), but `VNodeInner`
	// is a plain `pub enum` whose variant fields are exactly as visible as
	// the enum, so its data shape is still directly testable on its own.
	let container = make_element();
	let inner = VNodeInner::Portal { container: container.clone(), children: micro_react::vnode::Children(vec![VNode::text("x"), VNode::text("y")]) };
	match inner {
		VNodeInner::Portal { container: c, children } => {
			assert_eq!(c.tag_name(), container.tag_name());
			assert_eq!(children.len(), 2);
		}
		_ => panic!("expected portal vnode"),
	}
}

// ── ComponentInstSlot ──

#[wasm_bindgen_test]
fn component_inst_slot_new_starts_empty() {
	let slot = ComponentInstSlot::new();
	assert!(slot.0.borrow().is_none());
}

#[wasm_bindgen_test]
fn component_inst_slot_default_also_starts_empty() {
	let slot = ComponentInstSlot::default();
	assert!(slot.0.borrow().is_none());
}

#[wasm_bindgen_test]
fn fresh_component_vnode_gets_its_own_empty_inst_slot() {
	let render = ComponentFn::infallible(|_| VNode::null());
	let v = VNode::component("Widget", render, vec![]);
	match v.inner {
		VNodeInner::Component { inst, .. } => assert!(inst.0.borrow().is_none()),
		_ => panic!("expected component vnode"),
	}
}

// ── FLAG_INSERT / FLAG_MATCHED ──

#[wasm_bindgen_test]
fn reconciler_flags_are_distinct_single_bits() {
	assert_ne!(FLAG_INSERT, FLAG_MATCHED);
	assert_eq!(FLAG_INSERT & FLAG_MATCHED, 0);
	assert_eq!(FLAG_INSERT.count_ones(), 1);
	assert_eq!(FLAG_MATCHED.count_ones(), 1);
}

#[wasm_bindgen_test]
fn reconciler_flags_can_be_combined_and_cleared_with_bit_ops() {
	let mut flags: u8 = 0;
	flags |= FLAG_INSERT;
	flags |= FLAG_MATCHED;
	assert_eq!(flags & FLAG_INSERT, FLAG_INSERT);
	assert_eq!(flags & FLAG_MATCHED, FLAG_MATCHED);
	flags &= !(FLAG_INSERT | FLAG_MATCHED);
	assert_eq!(flags, 0);
}

// ── PropVal::Callback / PropVal::Js equality ──

#[wasm_bindgen_test]
fn propval_callback_eq_uses_object_identity_not_structural_equality() {
	let f1 = js_sys::Function::new_no_args("return 1;");
	let f2 = js_sys::Function::new_no_args("return 1;");
	let a = PropVal::Callback(JsCallback(f1.clone()));
	let b = PropVal::Callback(JsCallback(f1.clone()));
	let c = PropVal::Callback(JsCallback(f2));
	assert_eq!(a, b, "same underlying JS function should be equal");
	assert_ne!(a, c, "structurally-identical but distinct JS functions should not be equal");
}

#[wasm_bindgen_test]
fn propval_js_eq_uses_object_identity() {
	let obj1 = js_sys::Object::new();
	let obj2 = js_sys::Object::new();
	let a = PropVal::Js(obj1.clone().into());
	let b = PropVal::Js(obj1.into());
	let c = PropVal::Js(obj2.into());
	assert_eq!(a, b);
	assert_ne!(a, c);
}

#[wasm_bindgen_test]
fn propval_js_eq_is_false_against_other_variants() {
	let obj = js_sys::Object::new();
	let a = PropVal::Js(obj.into());
	assert_ne!(a, PropVal::Null);
	assert_ne!(a, PropVal::Str("x".to_string()));
}

// ── ComponentFn::new error path ──

#[wasm_bindgen_test]
fn component_fn_new_can_return_err() {
	let f = ComponentFn::new(|_props| Err(JsValue::from_str("boom")));
	let err = f.call(vec![]).unwrap_err();
	assert_eq!(err.as_string().as_deref(), Some("boom"));
}

#[wasm_bindgen_test]
fn component_fn_call_propagates_props_through_to_a_fallible_component() {
	let f = ComponentFn::new(|props| if props.is_empty() { Err(JsValue::from_str("no props")) } else { Ok(VNode::text("ok")) });
	assert!(f.call(vec![]).is_err());
	assert!(f.call(vec![("a".to_string(), PropVal::Bool(true))]).is_ok());
}

// ── NodeRef::with_sync fires on set() ──
// `set()` is `pub(crate)`, so this exercises it indirectly by mounting a
// component that attaches a `ref_` and checking the sync callback observes
// a real DOM node — closer to how the reconciler actually drives it than
// constructing a NodeRef in isolation.

#[wasm_bindgen_test]
fn node_ref_sync_observes_the_mounted_dom_node() {
	use std::cell::RefCell;
	use std::rc::Rc;

	let container = make_element();
	web_sys::window().unwrap().document().unwrap().body().unwrap().append_child(&container).unwrap();
	let mut root = micro_react::render::Root::new(container);

	let seen: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
	let seen_clone = seen.clone();
	let node_ref = NodeRef::with_sync(move |n| seen_clone.borrow_mut().push(n.is_some()));

	let v = VNode::tag("span").ref_(node_ref).text("hi").build();
	root.render(v).unwrap();

	assert_eq!(seen.borrow().as_slice(), &[true]);
}
