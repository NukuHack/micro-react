//! Tests for `vnode` — mostly pure Rust logic (VNode/PropVal/Template/
//! ElementBuilder/ComponentFn/Children/NodeRef construction), run here via
//! `wasm-bindgen-test` (like the rest of `tests/`) so `build.sh`'s single
//! `wasm-pack test --headless --firefox` step picks them up alongside
//! everything else. `PropVal::Callback`/`PropVal::Js`, `NodeRef` DOM sync,
//! and `Portal`'s `Element` field all need a real JS/DOM runtime and are
//! covered by other `wasm-bindgen-test` files (`tests/reconciler.rs`,
//! `tests/events_dom.rs`).

use std::rc::Rc;
use wasm_bindgen_test::*;

use micro_react::vnode::{next_id, Children, ComponentFn, NodeRef, PropVal, Props, Template, VNode, VNodeInner};

wasm_bindgen_test_configure!(run_in_browser);

// ── next_id / Template ──

#[wasm_bindgen_test]
fn next_id_is_monotonically_increasing() {
	let a = next_id();
	let b = next_id();
	assert!(b > a);
}

#[wasm_bindgen_test]
fn template_new_assigns_a_fresh_id_each_time() {
	let t1 = Template::new("div");
	let t2 = Template::new("div");
	assert_eq!(t1.tag, "div");
	assert_ne!(t1.id, t2.id);
}

// ── PropVal conversions + equality ──

#[wasm_bindgen_test]
fn propval_from_str_and_string() {
	assert_eq!(PropVal::from("hi"), PropVal::Str("hi".to_string()));
	assert_eq!(PropVal::from(String::from("hi")), PropVal::Str("hi".to_string()));
}

#[wasm_bindgen_test]
fn propval_from_bool_num_variants() {
	assert_eq!(PropVal::from(true), PropVal::Bool(true));
	assert_eq!(PropVal::from(1.5f64), PropVal::Num(1.5));
	assert_eq!(PropVal::from(3i32), PropVal::Num(3.0));
	assert_eq!(PropVal::from(7usize), PropVal::Num(7.0));
}

#[wasm_bindgen_test]
fn propval_eq_is_false_across_different_variants() {
	assert_ne!(PropVal::Str("1".to_string()), PropVal::Num(1.0));
	assert_ne!(PropVal::Bool(true), PropVal::Null);
}

#[wasm_bindgen_test]
fn propval_null_equals_null() {
	assert_eq!(PropVal::Null, PropVal::Null);
}

// ── Children ──

#[wasm_bindgen_test]
fn children_len_and_is_empty() {
	let empty = Children(vec![]);
	assert_eq!(empty.len(), 0);
	assert!(empty.is_empty());

	let some = Children(vec![VNode::text("a"), VNode::text("b")]);
	assert_eq!(some.len(), 2);
	assert!(!some.is_empty());
}

// ── VNode constructors ──

#[wasm_bindgen_test]
fn null_vnode_type_tag() {
	let v = VNode::null();
	assert_eq!(v.type_tag(), Some("#null"));
	assert!(matches!(v.inner, VNodeInner::Null));
}

#[wasm_bindgen_test]
fn text_vnode_holds_string_and_type_tag() {
	let v = VNode::text("hello");
	assert_eq!(v.type_tag(), Some("#text"));
	match v.inner {
		VNodeInner::Text(s) => assert_eq!(s, "hello"),
		_ => panic!("expected text vnode"),
	}
}

#[wasm_bindgen_test]
fn fragment_vnode_has_no_key_by_default() {
	let v = VNode::fragment(vec![VNode::text("a"), VNode::text("b")]);
	assert_eq!(v.type_tag(), Some("#fragment"));
	assert_eq!(v.key(), None);
	match &v.inner {
		VNodeInner::Fragment { children, .. } => assert_eq!(children.len(), 2),
		_ => panic!("expected fragment vnode"),
	}
}

#[wasm_bindgen_test]
fn fragment_keyed_carries_the_given_key() {
	let v = VNode::fragment_keyed("k1", vec![VNode::text("a")]);
	assert_eq!(v.key(), Some("k1"));
}

#[wasm_bindgen_test]
fn each_vnode_gets_a_unique_original_id() {
	let a = VNode::text("a");
	let b = VNode::text("b");
	assert_ne!(a.original, b.original);
}

// ── with_key / key() across variants ──

#[wasm_bindgen_test]
fn with_key_sets_key_on_component_vnode() {
	let render = ComponentFn::infallible(|_| VNode::null());
	let v = VNode::component("MyComp", render, vec![]).with_key(Some("k".to_string()));
	assert_eq!(v.key(), Some("k"));
	assert_eq!(v.type_tag(), Some("MyComp"));
}

#[wasm_bindgen_test]
fn with_key_on_element_vnode() {
	let v = VNode::tag("div").build().with_key(Some("row-1".to_string()));
	assert_eq!(v.key(), Some("row-1"));
}

#[wasm_bindgen_test]
fn with_key_is_a_noop_on_text_and_null() {
	// Text/Null have no key slot; with_key should not panic, just be ignored.
	let t = VNode::text("x").with_key(Some("k".to_string()));
	assert_eq!(t.key(), None);
	let n = VNode::null().with_key(Some("k".to_string()));
	assert_eq!(n.key(), None);
}

#[wasm_bindgen_test]
fn key_defaults_to_none_when_unset() {
	let v = VNode::tag("div").build();
	assert_eq!(v.key(), None);
}

// ── ElementBuilder ──

#[wasm_bindgen_test]
fn element_builder_build_produces_element_with_tag() {
	let v = VNode::tag("span").build();
	assert_eq!(v.type_tag(), Some("span"));
	match v.inner {
		VNodeInner::Element { template, props, children, key, ref_ } => {
			assert_eq!(template.tag, "span");
			assert!(props.is_empty());
			assert!(children.is_empty());
			assert_eq!(key, None);
			assert!(ref_.is_none());
		}
		_ => panic!("expected element vnode"),
	}
}

#[wasm_bindgen_test]
fn element_builder_attr_appends_props_in_order() {
	let v = VNode::tag("input").attr("type", "text").attr("disabled", true).attr("tabIndex", 3i32).build();
	match v.inner {
		VNodeInner::Element { props, .. } => {
			assert_eq!(props.len(), 3);
			assert_eq!(props[0], ("type".to_string(), PropVal::Str("text".to_string())));
			assert_eq!(props[1], ("disabled".to_string(), PropVal::Bool(true)));
			assert_eq!(props[2], ("tabIndex".to_string(), PropVal::Num(3.0)));
		}
		_ => panic!("expected element vnode"),
	}
}

#[wasm_bindgen_test]
fn element_builder_key_sets_key() {
	let v = VNode::tag("li").key("item-1").build();
	assert_eq!(v.key(), Some("item-1"));
}

#[wasm_bindgen_test]
fn element_builder_child_and_children_accumulate() {
	let v = VNode::tag("ul")
		.child(VNode::tag("li").text("one").build())
		.children(vec![VNode::tag("li").text("two").build(), VNode::tag("li").text("three").build()])
		.build();
	match v.inner {
		VNodeInner::Element { children, .. } => assert_eq!(children.len(), 3),
		_ => panic!("expected element vnode"),
	}
}

#[wasm_bindgen_test]
fn element_builder_text_helper_adds_a_text_child() {
	let v = VNode::tag("p").text("hello").build();
	match v.inner {
		VNodeInner::Element { children, .. } => {
			assert_eq!(children.len(), 1);
			match &children.0[0].inner {
				VNodeInner::Text(s) => assert_eq!(s, "hello"),
				_ => panic!("expected text child"),
			}
		}
		_ => panic!("expected element vnode"),
	}
}

#[wasm_bindgen_test]
fn element_builder_into_vnode_via_from() {
	let builder = VNode::tag("div");
	let v: VNode = builder.into();
	assert_eq!(v.type_tag(), Some("div"));
}

// ── ComponentFn ──

#[wasm_bindgen_test]
fn component_fn_infallible_always_returns_ok() {
	let f = ComponentFn::infallible(|_props| VNode::text("rendered"));
	let result = f.call(vec![]).expect("infallible should not error");
	match result.inner {
		VNodeInner::Text(s) => assert_eq!(s, "rendered"),
		_ => panic!("expected text vnode"),
	}
}

#[wasm_bindgen_test]
fn component_fn_new_can_return_ok() {
	let f = ComponentFn::new(|_props| Ok(VNode::text("ok")));
	assert!(f.call(vec![]).is_ok());
}

#[wasm_bindgen_test]
fn component_fn_receives_the_props_passed_in() {
	let f = ComponentFn::infallible(|props| {
		let count = props.len();
		VNode::text(count.to_string())
	});
	let props: Props = vec![("a".to_string(), PropVal::Bool(true)), ("b".to_string(), PropVal::Null)];
	let result = f.call(props).unwrap();
	match result.inner {
		VNodeInner::Text(s) => assert_eq!(s, "2"),
		_ => panic!("expected text vnode"),
	}
}

#[wasm_bindgen_test]
fn vnode_component_constructor_sets_name_and_props() {
	let render = ComponentFn::infallible(|_| VNode::null());
	let props: Props = vec![("x".to_string(), PropVal::Num(1.0))];
	let v = VNode::component("Widget", render, props);
	assert_eq!(v.type_tag(), Some("Widget"));
	match v.inner {
		VNodeInner::Component { name, props, key, .. } => {
			assert_eq!(name, "Widget");
			assert_eq!(props.len(), 1);
			assert_eq!(key, None);
		}
		_ => panic!("expected component vnode"),
	}
}

// ── NodeRef (non-DOM parts) ──

#[wasm_bindgen_test]
fn node_ref_new_starts_empty() {
	let r = NodeRef::new();
	assert!(r.node.borrow().is_none());
}

#[wasm_bindgen_test]
fn node_ref_with_sync_starts_empty_too() {
	// `set()` (the method that actually populates the node and invokes the
	// sync callback) is `pub(crate)` — only the diff engine is meant to
	// call it, so it's exercised indirectly via `ref_()` on a live
	// component tree in `tests/reconciler.rs` rather than directly here.
	let seen = Rc::new(std::cell::RefCell::new(Vec::<bool>::new()));
	let seen_clone = seen.clone();
	let r = NodeRef::with_sync(move |node| seen_clone.borrow_mut().push(node.is_some()));
	assert!(r.node.borrow().is_none());
	assert!(r.sync.is_some());
}
