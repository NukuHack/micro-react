//! Dedicated tests for `render::Root` (`Root::render`/`Root::unmount`), which
//! per the TODO was previously exercised only incidentally by other DOM test
//! files calling `Root::render` to get a mounted tree — never testing `Root`
//! itself: re-`render()`-ing on the same root with a changed tree, or
//! `unmount()` tearing down effect cleanups for the whole tree, not just a
//! single component (which `hooks_scheduler.rs` already covers).
//!
//! Runs via `wasm-bindgen-test` in a headless browser, same as the rest of
//! `tests/browser/`:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen_test::*;

use micro_react::hooks::use_effect;
use micro_react::render::Root;
use micro_react::vnode::{ComponentFn, Props, VNode};

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

// ─── Re-render on the same root ───

#[wasm_bindgen_test]
fn render_twice_on_same_root_updates_dom_in_place() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	root.render(VNode::tag("div").attr("class", "a").text("first").build()).unwrap();
	assert_eq!(container.text_content().as_deref(), Some("first"));
	let node_before = container.children().item(0).unwrap();

	root.render(VNode::tag("div").attr("class", "a").text("second").build()).unwrap();
	assert_eq!(container.text_content().as_deref(), Some("second"), "second render should update the same tree, not append alongside the first");

	let node_after = container.children().item(0).unwrap();
	assert!(
		node_after.is_same_node(Some(&node_before)),
		"an update pass (same tag/key) should reuse the existing DOM node rather than recreating it"
	);
	assert_eq!(container.children().length(), 1, "expected exactly one child after two renders of an equivalent tree");
}

#[wasm_bindgen_test]
fn render_twice_with_different_root_tag_replaces_the_node() {
	// A changed root tag can't be patched in place; the second render
	// should replace the DOM node, not leave the stale first one behind.
	let container = make_container();
	let mut root = Root::new(container.clone());

	root.render(VNode::tag("div").text("as div").build()).unwrap();
	assert!(container.query_selector("div").unwrap().is_some());

	root.render(VNode::tag("span").text("as span").build()).unwrap();
	assert!(container.query_selector("div").unwrap().is_none(), "the old <div> should be gone after switching root tags");
	assert!(container.query_selector("span").unwrap().is_some(), "expected a fresh <span> for the new root tag");
	assert_eq!(container.text_content().as_deref(), Some("as span"));
	assert_eq!(container.children().length(), 1, "should not accumulate old nodes across renders");
}

// ─── unmount() tears down the whole tree, not just one component ───

#[wasm_bindgen_test]
fn unmount_clears_the_container() {
	let container = make_container();
	let mut root = Root::new(container.clone());

	root.render(VNode::fragment(vec![VNode::tag("p").text("one").build(), VNode::tag("p").text("two").build()])).unwrap();
	assert_eq!(container.children().length(), 2);

	root.unmount();
	assert_eq!(container.inner_html(), "", "unmount() should clear the container's contents");
}

#[wasm_bindgen_test]
fn unmount_runs_effect_cleanups_for_every_component_in_the_tree_not_just_one() {
	// Two sibling components, each registering its own effect cleanup.
	// unmount() on the root should run *both*, not just whichever
	// component happens to be first/last.
	let container = make_container();
	let mut root = Root::new(container.clone());

	let cleaned: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));

	let make_child = |label: &'static str, cleaned: Rc<RefCell<Vec<&'static str>>>| {
		VNode::component(
			label,
			ComponentFn::infallible(move |_props: Props| {
				let cleaned = cleaned.clone();
				use_effect(
					move || {
						let cleaned = cleaned.clone();
						Box::new(move || cleaned.borrow_mut().push(label))
					},
					Some(vec![]),
				);
				VNode::text(label)
			}),
			Vec::new(),
		)
	};

	root.render(VNode::fragment(vec![make_child("left", cleaned.clone()), make_child("right", cleaned.clone())])).unwrap();

	// Effects run asynchronously (after paint) but `Root::render` flushes
	// them synchronously before returning, so both cleanups are already
	// registered by the time `unmount()` runs.
	root.unmount();

	let mut got = cleaned.borrow().clone();
	got.sort_unstable();
	assert_eq!(got, vec!["left", "right"], "unmount() should run effect cleanups for every component in the tree, not just one");
}

#[wasm_bindgen_test]
fn unmount_runs_cleanup_for_nested_descendant_components() {
	// A cleanup several levels deep (not a direct child of the root)
	// should still fire on unmount, exercising the "whole tree" part of
	// the gap specifically (vs. hooks_scheduler.rs's single-component case).
	let container = make_container();
	let mut root = Root::new(container.clone());

	let cleaned = Rc::new(RefCell::new(false));
	let cleaned_for_leaf = cleaned.clone();

	let leaf = VNode::component(
		"Leaf",
		ComponentFn::infallible(move |_props: Props| {
			let cleaned = cleaned_for_leaf.clone();
			use_effect(move || Box::new(move || *cleaned.borrow_mut() = true), Some(vec![]));
			VNode::text("leaf")
		}),
		Vec::new(),
	);

	let middle = VNode::component("Middle", ComponentFn::infallible(move |_props: Props| VNode::tag("div").child(leaf.clone()).build()), Vec::new());

	let outer =
		VNode::component("Outer", ComponentFn::infallible(move |_props: Props| VNode::tag("section").child(middle.clone()).build()), Vec::new());

	root.render(outer).unwrap();
	assert!(!*cleaned.borrow(), "cleanup shouldn't have run yet");

	root.unmount();
	assert!(*cleaned.borrow(), "unmount() should run cleanups for descendant components several levels deep, not just direct children of the root");
}
