//! Integration tests for `NodeRef` DOM sync (`ElementBuilder::ref_` /
//! `vnode::NodeRef`), driven through `Root::render` exactly like real
//! `ref="${...}"` usage in `html\`\``.
//!
//! `tests/browser/vnode_unit.rs` claims this is "covered by other
//! wasm-bindgen-test files (tests/browser/reconciler.rs, tests/browser/events_dom.rs)" —
//! it isn't; neither file references `ref_`/`NodeRef` at all. This file
//! closes that gap. Needs a real DOM, so like the rest of the
//! `wasm-bindgen-test` suite:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;

use micro_react::render::Root;
use micro_react::vnode::{ComponentFn, NodeRef, Props, VNode};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

// ─── Basic mount / unmount sync ───

#[wasm_bindgen_test]
fn node_ref_is_none_before_mount() {
	let node_ref = NodeRef::new();
	assert!(node_ref.node.borrow().is_none());
}

#[wasm_bindgen_test]
fn node_ref_points_at_the_mounted_dom_node() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	root.render(VNode::tag("div").ref_(node_ref.clone()).text("hi").build()).unwrap();

	let current = node_ref.node.borrow().clone().expect("ref should be set after mount");
	let el: web_sys::Element = current.dyn_into().unwrap();
	assert_eq!(el.tag_name(), "DIV");
	assert!(container.first_child().unwrap().is_same_node(Some(el.as_ref())), "ref should point at the actual rendered DOM node");
}

#[wasm_bindgen_test]
fn node_ref_is_cleared_on_full_unmount() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	root.render(VNode::tag("div").ref_(node_ref.clone()).build()).unwrap();
	assert!(node_ref.node.borrow().is_some());

	root.unmount();
	assert!(node_ref.node.borrow().is_none(), "ref should be cleared to None on unmount");
}

#[wasm_bindgen_test]
fn node_ref_is_cleared_when_conditionally_removed_from_tree() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	// First render: element present with a ref attached.
	root.render(VNode::fragment(vec![VNode::tag("span").ref_(node_ref.clone()).key("a").build()])).unwrap();
	assert!(node_ref.node.borrow().is_some());

	// Second render: the ref'd element is gone entirely (not just replaced).
	root.render(VNode::fragment(vec![])).unwrap();
	assert!(node_ref.node.borrow().is_none(), "ref should be cleared when its element leaves the tree, not just on full unmount");
}

// ─── Re-render behavior ───

#[wasm_bindgen_test]
fn node_ref_survives_prop_only_rerender_without_changing_identity() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	root.render(VNode::tag("div").ref_(node_ref.clone()).attr("data-n", "1").build()).unwrap();
	let first = node_ref.node.borrow().clone().unwrap();

	root.render(VNode::tag("div").ref_(node_ref.clone()).attr("data-n", "2").build()).unwrap();
	let second = node_ref.node.borrow().clone().unwrap();

	assert!(first.is_same_node(Some(&second)), "same-tag re-render should reuse the DOM node, so the ref target's identity shouldn't change");
}

#[wasm_bindgen_test]
fn node_ref_tracks_unkeyed_tag_change() {
	// Regression test for a fixed bug: `find_match` (diff.rs) matches
	// unkeyed children by `(key, type_tag)`, and an Element's `type_tag()`
	// is its tag name. So a `div` -> `span` swap doesn't diff as an
	// in-place update — it's treated as "remove the old node, insert a new
	// one", and the two halves run as independent steps in
	// `diff_children`:
	//   1. the new `<span>` mounts first (`ref_.set(Some(new_span))`)
	//   2. the old, now-unmatched `<div>` is unmounted afterwards
	// Since the ref is the same `Rc`-shared `NodeRef` on both vnodes, step
	// 2 used to unconditionally clobber step 1 back to `None`, even though
	// the `<span>` was genuinely mounted. `unmount_vnode` now only clears a
	// ref if it still points at *that* vnode's own DOM node, so a ref
	// reassigned to a fresher node during the same commit survives.
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	root.render(VNode::tag("div").ref_(node_ref.clone()).build()).unwrap();
	assert!(node_ref.node.borrow().is_some());

	root.render(VNode::tag("span").ref_(node_ref.clone()).build()).unwrap();

	// The span is in the DOM...
	let el = container.first_element_child().expect("span should be mounted");
	assert_eq!(el.tag_name(), "SPAN");
	// ...and the ref points at it.
	let current = node_ref.node.borrow().clone().expect("ref should track the new element after an unkeyed tag change");
	assert!(current.is_same_node(Some(el.as_ref())), "ref should point at the actual mounted <span>, not a stale/cleared value");
}

#[wasm_bindgen_test]
fn node_ref_tracks_keyed_tag_change() {
	// Same bug/fix as `node_ref_tracks_unkeyed_tag_change`, but with an
	// explicit key. `find_match` requires *both* the key and `type_tag`
	// (tag name, for Elements) to match, so a keyed element that changes
	// tag while keeping its key still fails to match and goes through the
	// exact same mount-then-unmount two-phase path — worth covering
	// separately since it's easy to assume keys alone are enough to keep
	// a vnode "the same" across a diff.
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	root.render(VNode::fragment(vec![VNode::tag("div").ref_(node_ref.clone()).key("a").build()])).unwrap();
	assert!(node_ref.node.borrow().is_some());

	root.render(VNode::fragment(vec![VNode::tag("span").ref_(node_ref.clone()).key("a").build()])).unwrap();

	let el = container.first_element_child().expect("span should be mounted");
	assert_eq!(el.tag_name(), "SPAN");
	let current = node_ref.node.borrow().clone().expect("ref should track the new element after a keyed tag change");
	assert!(current.is_same_node(Some(el.as_ref())), "ref should point at the actual mounted <span>, not a stale/cleared value");
}

#[wasm_bindgen_test]
fn node_ref_still_clears_on_genuine_unmount_after_unkeyed_tag_change_fix() {
	// Companion to the test above: make sure the fix (only clearing a ref
	// if it still points at the unmounting vnode's own DOM node) didn't
	// accidentally disable clearing altogether. A real removal — no
	// replacement claiming the ref afterward — must still null it out.
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();

	root.render(VNode::fragment(vec![VNode::tag("div").ref_(node_ref.clone()).key("a").build()])).unwrap();
	assert!(node_ref.node.borrow().is_some());

	root.render(VNode::fragment(vec![])).unwrap();
	assert!(node_ref.node.borrow().is_none(), "ref should still be cleared when its element is genuinely removed, not replaced");
}

#[wasm_bindgen_test]
fn node_ref_with_sync_tracks_unkeyed_tag_change() {
	// Same scenario as `node_ref_tracks_unkeyed_tag_change`, but for the
	// callback-ref (`with_sync`) flavor, whose closure is also shared
	// across the old/new vnodes via the same `Rc<NodeRef>`. The fix must
	// not make the sync callback fire a spurious `None` for the clobbered
	// case, nor skip the real `None` firing on a genuine unmount.
	let container = make_container();
	let mut root = Root::new(container.clone());

	let calls: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(vec![])); // true = attached, false = detached
	let calls_for_sync = calls.clone();
	let node_ref = NodeRef::with_sync(move |node| {
		calls_for_sync.borrow_mut().push(node.is_some());
	});

	root.render(VNode::tag("div").ref_(node_ref.clone()).build()).unwrap();
	root.render(VNode::tag("span").ref_(node_ref.clone()).build()).unwrap();

	let current = node_ref.node.borrow().clone().expect("ref should track the new element after an unkeyed tag change");
	let el: web_sys::Element = current.dyn_into().unwrap();
	assert_eq!(el.tag_name(), "SPAN");

	// Mount fired true, and the clobbered unmount step must NOT have fired
	// a spurious `false` in between — only a genuine unmount fires `false`.
	assert_eq!(*calls.borrow(), vec![true], "sync callback should only have fired once (mount), not a spurious detach during the tag swap");

	root.unmount();
	assert_eq!(calls.borrow().last(), Some(&false), "sync callback should still fire false on a genuine unmount after the swap");
}

// ─── Callback-style refs (`NodeRef::with_sync`) ───

#[wasm_bindgen_test]
fn node_ref_with_sync_fires_on_mount_and_unmount() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	let calls: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(vec![])); // true = attached, false = detached
	let calls_for_sync = calls.clone();
	let node_ref = NodeRef::with_sync(move |node| {
		calls_for_sync.borrow_mut().push(node.is_some());
	});

	root.render(VNode::tag("div").ref_(node_ref.clone()).build()).unwrap();
	root.unmount();

	let log = calls.borrow();
	assert_eq!(log.first(), Some(&true), "sync callback should fire with Some(node) on mount");
	assert_eq!(log.last(), Some(&false), "sync callback should fire with None on unmount");
}

// ─── Ref inside a component (the realistic `useRef`-adjacent path) ───

#[wasm_bindgen_test]
fn node_ref_attached_from_within_a_component_tracks_its_element() {
	let container = make_container();
	let mut root = Root::new(container.clone());
	let node_ref = NodeRef::new();
	let node_ref_for_comp = node_ref.clone();

	let comp = ComponentFn::infallible(move |_props: Props| VNode::tag("button").ref_(node_ref_for_comp.clone()).text("click me").build());
	root.render(VNode::component("RefComp", comp, vec![])).unwrap();

	let current = node_ref.node.borrow().clone().expect("ref should be set for a component-rendered element");
	assert_eq!(current.dyn_ref::<web_sys::Element>().unwrap().tag_name(), "BUTTON");
}
