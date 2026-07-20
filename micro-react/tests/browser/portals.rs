//! Coverage for `diff.rs::diff_portal` / `VNodeInner::Portal`, previously
//! untested anywhere in `tests/` per the TODO: rendering into a foreign
//! container, moving a portal's target container across renders, and
//! unmounting a subtree that contains a portal.
//!
//! There's no JS-facing `createPortal` binding yet (`bindings.rs`'s
//! `create_element` has no path that produces `VNodeInner::Portal`), so
//! these tests build portal vnodes directly via the `VNode::portal`
//! constructor added alongside this file for exactly that reason.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen_test::*;

use micro_react::render::Root;
use micro_react::vnode::{ComponentFn, Props, VNode};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

#[wasm_bindgen_test]
fn portal_renders_children_into_the_foreign_container_not_the_tree_parent() {
	let root_container = make_container();
	let portal_target = make_container();

	let tree = VNode::tag("div").child(VNode::portal(portal_target.clone(), vec![VNode::tag("span").text("in portal").build()])).build();

	let mut root = Root::new(root_container.clone());
	root.render(tree).unwrap();

	assert_eq!(root_container.text_content().as_deref(), Some(""), "portal children should not render into the tree's own parent container");
	assert_eq!(portal_target.text_content().as_deref(), Some("in portal"), "portal children should render into the foreign target container");
}

#[wasm_bindgen_test]
fn portal_target_container_change_across_renders_moves_children() {
	let root_container = make_container();
	let target_a = make_container();
	let target_b = make_container();

	let mut root = Root::new(root_container.clone());
	root.render(VNode::tag("div").child(VNode::portal(target_a.clone(), vec![VNode::text("payload")])).build()).unwrap();
	assert_eq!(target_a.text_content().as_deref(), Some("payload"));
	assert_eq!(target_b.text_content().as_deref(), Some(""));

	// Same tree shape, but the portal's container swaps from A to B.
	root.render(VNode::tag("div").child(VNode::portal(target_b.clone(), vec![VNode::text("payload")])).build()).unwrap();

	assert_eq!(target_a.text_content().as_deref(), Some(""), "old target container should no longer hold the portal's content");
	assert_eq!(target_b.text_content().as_deref(), Some("payload"), "new target container should now hold the portal's content");
}

// `diff_portal`'s container-change handling (see the fix in `diff.rs`) has
// to special-case a *changed* container without breaking the common case of
// the container staying the same across renders — this pins down that a
// same-container re-render still reuses/reorders the existing DOM nodes by
// key instead of tearing them down and recreating them.
#[wasm_bindgen_test]
fn portal_with_unchanged_container_reuses_keyed_dom_nodes_across_renders() {
	let root_container = make_container();
	let target = make_container();

	let mut root = Root::new(root_container.clone());
	root.render(
		VNode::tag("div")
			.child(VNode::portal(target.clone(), vec![VNode::tag("span").key("a").text("A").build(), VNode::tag("span").key("b").text("B").build()]))
			.build(),
	)
	.unwrap();

	let span_a_first = target.query_selector("span:first-child").unwrap().unwrap();

	// Same container, children reordered.
	root.render(
		VNode::tag("div")
			.child(VNode::portal(target.clone(), vec![VNode::tag("span").key("b").text("B").build(), VNode::tag("span").key("a").text("A").build()]))
			.build(),
	)
	.unwrap();

	assert_eq!(target.text_content().as_deref(), Some("BA"), "expected the reordered children to render in their new order");
	let span_a_second = target.query_selector("span:last-child").unwrap().unwrap();
	assert!(
		span_a_first.is_same_node(Some(span_a_second.as_ref())),
		"with the container unchanged, diff_portal should still reuse/reorder the existing keyed DOM node by identity, not recreate it"
	);
}

// The container-change path moves every child, not just a single one — this
// guards against a fix that happened to work for one child but still used a
// stale old-container reference node once more than one child was involved.
#[wasm_bindgen_test]
fn portal_container_change_moves_every_child_and_old_container_ends_up_empty() {
	let root_container = make_container();
	let target_a = make_container();
	let target_b = make_container();

	let mut root = Root::new(root_container.clone());
	root.render(VNode::tag("div").child(VNode::portal(target_a.clone(), vec![VNode::text("one"), VNode::text("two"), VNode::text("three")])).build())
		.unwrap();
	assert_eq!(target_a.text_content().as_deref(), Some("onetwothree"));

	root.render(VNode::tag("div").child(VNode::portal(target_b.clone(), vec![VNode::text("one"), VNode::text("two"), VNode::text("three")])).build())
		.unwrap();

	assert_eq!(target_a.text_content().as_deref(), Some(""), "the old container should be fully emptied once the portal's target changes");
	assert_eq!(target_b.text_content().as_deref(), Some("onetwothree"), "all three children should have moved to the new container, in order");
}

// The container-change path unmounts the abandoned old children directly
// (rather than routing them through diff_children against the new
// container), so it has to run their cleanups itself too — otherwise a
// component nested in the portal would silently leak its effect on a
// container swap the same way the (separately tracked) full-unmount bug does.
#[wasm_bindgen_test]
fn portal_container_change_runs_effect_cleanup_for_children_left_behind() {
	let root_container = make_container();
	let target_a = make_container();
	let target_b = make_container();
	let cleaned_up = Rc::new(RefCell::new(false));
	let cleaned_up_for_comp = cleaned_up.clone();

	let comp = ComponentFn::infallible(move |_props: Props| {
		let cleaned_up = cleaned_up_for_comp.clone();
		micro_react::hooks::use_effect(
			move || {
				let cleaned_up = cleaned_up.clone();
				Box::new(move || {
					*cleaned_up.borrow_mut() = true;
				}) as Box<dyn FnOnce()>
			},
			Some(vec![]),
		);
		VNode::text("inside portal")
	});

	let mut root = Root::new(root_container.clone());
	root.render(VNode::tag("div").child(VNode::portal(target_a.clone(), vec![VNode::component("PortalChild", comp, vec![])])).build()).unwrap();
	assert!(!*cleaned_up.borrow());

	root.render(VNode::tag("div").child(VNode::portal(target_b.clone(), vec![VNode::text("replacement")])).build()).unwrap();

	assert!(
		*cleaned_up.borrow(),
		"a component left behind in the old container when the portal's target changes should still have its effect cleaned up"
	);
	assert_eq!(target_a.text_content().as_deref(), Some(""));
	assert_eq!(target_b.text_content().as_deref(), Some("replacement"));
}

#[wasm_bindgen_test]
fn unmounting_a_tree_with_a_portal_subtree_cleans_up_its_effects_and_content() {
	let root_container = make_container();
	let portal_target = make_container();
	let cleaned_up = Rc::new(RefCell::new(false));
	let cleaned_up_for_comp = cleaned_up.clone();

	// A component that lives *inside* the portal's children, so unmounting
	// the outer tree has to recurse through the Portal variant to reach it.
	let comp = ComponentFn::infallible(move |_props: Props| {
		let cleaned_up = cleaned_up_for_comp.clone();
		micro_react::hooks::use_effect(
			move || {
				let cleaned_up = cleaned_up.clone();
				Box::new(move || {
					*cleaned_up.borrow_mut() = true;
				}) as Box<dyn FnOnce()>
			},
			Some(vec![]),
		);
		VNode::text("inside portal")
	});

	let mut root = Root::new(root_container.clone());
	root.render(VNode::tag("div").child(VNode::portal(portal_target.clone(), vec![VNode::component("PortalChild", comp, vec![])])).build()).unwrap();
	assert_eq!(portal_target.text_content().as_deref(), Some("inside portal"));
	assert!(!*cleaned_up.borrow());

	root.unmount();

	// The key regression this guards against: `unmount_vnode`'s Element arm
	// unconditionally passes `skip_remove: true` down to its children on the
	// (usually valid) assumption that "removing the parent removes all DOM
	// children" — an assumption a Portal's children break, since their DOM
	// actually lives in a different, unrelated container. Effect
	// cleanup must still run for a component nested inside a portal even
	// though its DOM isn't reachable through the removed root container.
	assert!(*cleaned_up.borrow(), "unmounting the outer tree should run effect cleanups for components nested inside a portal");
}
