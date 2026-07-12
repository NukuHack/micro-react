//! Reconciler: walks old/new VNode trees and patches the DOM using a
//! Preact-style skew diff with keyed matching. Entry points: diff_node(),
//! diff_children(), rerender_component().

use js_sys::{Function, Object, Reflect};
use std::rc::Rc;
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{Document, Element, Node, Text};

use crate::events::{parse_event_prop, set_event_handler};
use crate::hooks::{unmount_inst, with_inst, ComponentInst};
use crate::vnode::{Children, ComponentFn, NodeRef, PropVal, Props, VNode, VNodeInner, FLAG_INSERT, FLAG_MATCHED};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const MATH_NS: &str = "http://www.w3.org/1998/Math/MathML";

// ─── Internal component tree node (wraps ComponentInst in Rc<RefCell>) ───
use std::cell::RefCell;

/// Every function-component vnode gets one of these.
pub struct ComponentNode {
	pub inst: Rc<RefCell<ComponentInst>>,
	pub render: ComponentFn,
	pub last_vnode: Option<VNode>,
}

// ─── Thread-local render depth guard ───
thread_local! {
	static RENDER_DEPTH: RefCell<u32> = const { RefCell::new(0) };
}

const MAX_RENDER_DEPTH: u32 = 256;

fn guard_depth() -> Result<(), JsValue> {
	RENDER_DEPTH.with(|d| {
		let v = *d.borrow();
		if v >= MAX_RENDER_DEPTH {
			Err(JsValue::from_str("[MicroReact] Max render depth exceeded"))
		} else {
			*d.borrow_mut() = v + 1;
			Ok(())
		}
	})
}
fn release_depth() {
	RENDER_DEPTH.with(|d| *d.borrow_mut() -= 1);
}

/// RAII guard around a `guard_depth()` increment, so a panic unwinding
/// through the stack still releases it instead of permanently leaking RENDER_DEPTH.
struct DepthGuard;
impl Drop for DepthGuard {
	fn drop(&mut self) {
		release_depth();
	}
}

/// Extract a human-readable message from a caught panic payload.
fn panic_message(e: &(dyn std::any::Any + Send + 'static)) -> String {
	if let Some(s) = e.downcast_ref::<&str>() {
		s.to_string()
	} else if let Some(s) = e.downcast_ref::<String>() {
		s.clone()
	} else {
		"unknown panic".to_string()
	}
}

// ─── diff_node — main recursive entry ───

pub fn diff_node(parent_dom: &Node, new_vnode: &mut VNode, old_vnode: Option<&VNode>, ns: &str) -> Result<Option<Node>, JsValue> {
	guard_depth()?;
	let _guard = DepthGuard;
	diff_node_inner(parent_dom, new_vnode, old_vnode, ns)
}

fn diff_node_inner(parent_dom: &Node, new_vnode: &mut VNode, old_vnode: Option<&VNode>, ns: &str) -> Result<Option<Node>, JsValue> {
	match &new_vnode.inner {
		VNodeInner::Null => {
			// A component can render `null` after a throw (createElement
			// substitutes VNode::null() on error). Unmount any old subtree properly instead of just dropping our _dom, so hooks/effects don't leak.
			if let Some(old) = old_vnode {
				if !matches!(old.inner, VNodeInner::Null) {
					unmount_vnode(old, false);
				}
			}
			new_vnode._dom = None;
			Ok(None)
		}

		VNodeInner::Text(text) => {
			let text = text.clone();
			// Reuse existing text node if possible
			if let Some(old) = old_vnode {
				if let Some(existing) = &old._dom {
					if let Ok(txt) = existing.clone().dyn_into::<Text>() {
						if txt.data() != text {
							txt.set_data(&text);
						}
						new_vnode._dom = Some(txt.into());
						return Ok(new_vnode._dom.clone());
					}
				}
			}
			let doc = document();
			let txt = doc.create_text_node(&text);
			let node: Node = txt.into();
			new_vnode._dom = Some(node.clone());
			Ok(Some(node))
		}

		VNodeInner::Fragment { .. } => diff_fragment(parent_dom, new_vnode, old_vnode, ns),
		VNodeInner::Portal { .. } => diff_portal(new_vnode, old_vnode, ns),

		VNodeInner::Element { .. } => diff_element(parent_dom, new_vnode, old_vnode, ns),

		VNodeInner::Component { .. } => diff_component(parent_dom, new_vnode, old_vnode, ns),
	}
}

// ─── Fragment ───

fn diff_fragment(parent_dom: &Node, new_vnode: &mut VNode, old_vnode: Option<&VNode>, ns: &str) -> Result<Option<Node>, JsValue> {
	let children: Vec<VNode> = match &new_vnode.inner {
		VNodeInner::Fragment { children, .. } => children.0.clone(),
		_ => unreachable!(),
	};

	let old_children = old_vnode
		.and_then(|o| match &o.inner {
			VNodeInner::Fragment { children, .. } => Some(children.0.clone()),
			_ => None,
		})
		.unwrap_or_default();

	let mut new_children = children;
	diff_children(parent_dom, &mut new_children, &old_children, ns, None)?;

	new_vnode._dom = new_children.first().and_then(|c| c._dom.clone());
	if let VNodeInner::Fragment { children: c, .. } = &mut new_vnode.inner {
		*c = Children(new_children);
	}
	Ok(new_vnode._dom.clone())
}

// ─── Portal ───

fn diff_portal(new_vnode: &mut VNode, old_vnode: Option<&VNode>, ns: &str) -> Result<Option<Node>, JsValue> {
	let (container, children) = match &new_vnode.inner {
		VNodeInner::Portal { container, children } => (container.clone(), children.0.clone()),
		_ => unreachable!(),
	};
	let old_children = old_vnode
		.and_then(|o| match &o.inner {
			VNodeInner::Portal { children, .. } => Some(children.0.clone()),
			_ => None,
		})
		.unwrap_or_default();

	let container_node: Node = container.clone().into();
	let mut new_children = children;
	diff_children(&container_node, &mut new_children, &old_children, ns, None)?;

	if let VNodeInner::Portal { children: c, .. } = &mut new_vnode.inner {
		*c = Children(new_children);
	}
	new_vnode._dom = None;
	Ok(None)
}

// ─── Element ───

fn diff_element(parent_dom: &Node, new_vnode: &mut VNode, old_vnode: Option<&VNode>, ns: &str) -> Result<Option<Node>, JsValue> {
	let (tag, props, children, ref_, _template) = match &new_vnode.inner {
		VNodeInner::Element { template, props, children, ref_, .. } => {
			(template.tag.clone(), props.clone(), children.0.clone(), ref_.clone(), template.clone())
		}
		_ => unreachable!(),
	};

	// Namespace propagation
	let ns = effective_ns(&tag, ns);

	let old_elem = old_vnode.and_then(|o| o._dom.clone().and_then(|n| n.dyn_into::<Element>().ok()));

	let old_props = old_vnode
		.and_then(|o| match &o.inner {
			VNodeInner::Element { props, .. } => Some(props.clone()),
			_ => None,
		})
		.unwrap_or_default();

	// Reuse or create DOM element
	let dom: Element = match old_elem {
		Some(e) if e.local_name() == tag => e,
		_other => {
			// Unmount old tree if replacing a different element
			if let Some(old) = old_vnode {
				unmount_vnode(old, true);
			}
			let doc = document();
			if let Some(ns) = ns_uri(&ns) {
				doc.create_element_ns(Some(ns), &tag)?
			} else {
				doc.create_element(&tag)?
			}
		}
	};

	let old_was_element = matches!(old_vnode.map(|o| &o.inner), Some(VNodeInner::Element { .. }));

	// If the previous vnode wasn't an Element, `old_props` is empty but the
	// DOM element still carries real attributes. apply_props only removes
	// attributes listed in `old_props`, so strip the element bare first.
	if old_vnode.is_some() && !old_was_element {
		let attrs = dom.attributes();
		let mut stale_names = Vec::with_capacity(attrs.length() as usize);
		for i in 0..attrs.length() {
			if let Some(a) = attrs.item(i) {
				stale_names.push(a.name());
			}
		}
		for name in stale_names {
			let _ = dom.remove_attribute(&name);
		}
	}

	// Apply props
	apply_props(&dom, &props, &old_props, &ns)?;

	// Handle children or dangerouslySetInnerHTML
	let has_inner_html = props.iter().any(|(k, _)| k == "dangerouslySetInnerHTML.__html");
	let new_children = if has_inner_html {
		// dangerouslySetInnerHTML — children handled by the prop setter
		vec![]
	} else {
		let dom_node: Node = dom.clone().into();
		let old_children = old_vnode
			.and_then(|o| match &o.inner {
				VNodeInner::Element { children, .. } => Some(children.0.clone()),
				_ => None,
			})
			.unwrap_or_default();
		// Same reasoning as the attribute-stripping above: `old_children` is
		// empty but the DOM element has real leftover children, so diffing
		// against an empty list would append rather than replace. Wipe first.
		if old_vnode.is_some() && !old_was_element {
			dom_node.set_text_content(None);
		}
		let mut ch = children;
		let child_ns = if tag == "foreignObject" { "html".to_string() } else { ns.clone() };
		diff_children(&dom_node, &mut ch, &old_children, &child_ns, None)?;
		ch
	};

	// Attach ref
	if let Some(r) = &ref_ {
		r.set(Some(dom.clone().into()));
	}

	let dom_node: Node = dom.clone().into();
	new_vnode._dom = Some(dom_node.clone());

	if let VNodeInner::Element { children: c, .. } = &mut new_vnode.inner {
		*c = Children(new_children);
	}

	Ok(Some(dom_node))
}

// ─── Component ───

fn diff_component(parent_dom: &Node, new_vnode: &mut VNode, old_vnode: Option<&VNode>, ns: &str) -> Result<Option<Node>, JsValue> {
	let (render, props) = match &new_vnode.inner {
		VNodeInner::Component { render, props, .. } => (render.clone(), props.clone()),
		_ => unreachable!(),
	};

	// Reuse the component instance across re-renders (matched by diff_children
	// via type+key), so hooks (state, refs, effects) survive across renders.
	let reused_inst: Option<Rc<RefCell<ComponentInst>>> = old_vnode.and_then(|o| match &o.inner {
		VNodeInner::Component { inst, .. } => inst.0.borrow().clone(),
		_ => None,
	});

	let inst_rc: Rc<RefCell<ComponentInst>> = match reused_inst {
		Some(inst) => inst,
		None => Rc::new(RefCell::new(ComponentInst::new())),
	};

	// The instance's own previous output, not the matched old vnode itself
	// (which is just a stand-in for "did we mount before").
	let old_rendered = inst_rc.borrow().last_vnode.clone();

	let my_generation = {
		let mut inst = inst_rc.borrow_mut();
		inst.depth = new_vnode._depth;
		inst.parent_dom = parent_dom.clone().dyn_into::<Element>().ok();
		inst.reset_hooks();
		inst.dirty = false;
		inst.render_generation += 1;
		inst.render_generation
	};

	// Raw pointer is only sound for this synchronous call, while inst_rc is
	// alive on the stack. Anything that outlives it captures inst_weak instead.
	let inst_ptr = inst_rc.as_ptr();
	let inst_weak = Rc::downgrade(&inst_rc);

	// A component "throws" via Err from its render function (what a thrown
	// JS exception becomes crossing the binding). catch_unwind is only a
	// secondary net; on wasm32 a panic traps the instance, so it's best-effort.
	let render_call_result: Result<VNode, JsValue> =
		match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| with_inst(inst_ptr, inst_weak, || render.call(props.clone())))) {
			Ok(result) => result,
			Err(panic_payload) => {
				let msg = panic_message(&*panic_payload);
				crate::console_error!("[micro-react] component render panicked: {}", msg);
				Err(JsValue::from_str(&msg))
			}
		};

	let render_result = match render_call_result {
		Ok(vnode) => vnode,
		Err(err) => {
			// Mirrors React: an error propagates to the nearest ErrorBoundary;
			// with none, it's uncaught, so log it and render nothing for
			// this subtree instead of unmounting the whole tree.
			if !crate::hooks::report_to_nearest_boundary(&inst_rc, err.clone()) {
				crate::console_error!(
					"[micro-react] uncaught error in component render (no boundary above): {}",
					crate::bindings::stringify_thrown(&err)
				);
			}
			VNode::null()
		}
	};

	let mut rendered = render_result;
	rendered._depth = new_vnode._depth + 1;

	// Persist render_fn/props/parent_dom/ns before diffing children: a first
	// mount whose child throws immediately needs render_fn set already, or
	// the boundary's forced rerender no-ops and the fallback appears late.
	{
		let mut inst = inst_rc.borrow_mut();
		inst.render_fn = Some(render.clone());
		inst.last_props = props.clone();
		inst.last_parent_dom = Some(parent_dom.clone());
		inst.last_ns = ns.to_string();
		// Persist the ambient boundary (ComponentInst::nearest_boundary) so a
		// later, independent re-render of this instance can still find its
		// ancestor boundary via report_to_nearest_boundary.
		inst.nearest_boundary = crate::hooks::current_boundary();
	}

	// If an ancestor ErrorBoundary already absorbed this throw
	// (BOUNDARY_ABSORBED), it repurposed our old DOM node for its fallback;
	// diffing `null` against `old_rendered` now would delete it, so skip.
	if crate::hooks::take_boundary_absorbed() {
		{
			let mut inst = inst_rc.borrow_mut();
			if inst.render_generation == my_generation {
				inst.render_fn = Some(render);
				inst.last_props = props;
				inst.last_parent_dom = Some(parent_dom.clone());
				inst.last_ns = ns.to_string();
				inst.last_vnode = Some(rendered);
			}
		}
		// Reflect the instance's true current DOM, not this call's discarded
		// `rendered`, so the parent's vnode tree (used for key matching)
		// still points at real, live DOM instead of going stale.
		new_vnode._dom = inst_rc.borrow().last_vnode.as_ref().and_then(|v| v._dom.clone());
		if let VNodeInner::Component { inst: slot, .. } = &new_vnode.inner {
			*slot.0.borrow_mut() = Some(inst_rc);
		}
		return Ok(None);
	}

	// If this component registered as an error boundary, make it visible on
	// the boundary stack while diffing its own subtree, the only window a descendant's failure can report to it.
	let is_boundary = inst_rc.borrow().error_setter.is_some();
	if is_boundary {
		crate::hooks::push_boundary(Rc::downgrade(&inst_rc));
	}
	// catch_unwind here too: the render call only protects the render fn
	// itself. The reconciliation that follows can also panic (e.g. tearing
	// down a thrown child), and previously had no safety net at all.
	let dom = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| diff_node(parent_dom, &mut rendered, old_rendered.as_ref(), ns))) {
		Ok(res) => res,
		Err(e) => {
			let msg = panic_message(&*e);
			crate::console_error!("[micro-react] reconciliation panicked: {}", msg);
			if !crate::hooks::report_to_nearest_boundary(&inst_rc, JsValue::from_str(&msg)) {
				crate::console_error!("[micro-react] uncaught reconciliation panic (no boundary above): {}", msg);
			}
			Ok(None)
		}
	};
	if is_boundary {
		crate::hooks::pop_boundary();
	}
	// A reconciliation panic above may itself have been absorbed by a
	// boundary, which already replaced the DOM this call was about to
	// commit — take the same "nothing left to do" exit here too.
	if crate::hooks::take_boundary_absorbed() {
		{
			let mut inst = inst_rc.borrow_mut();
			if inst.render_generation == my_generation {
				inst.render_fn = Some(render);
				inst.last_props = props;
				inst.last_parent_dom = Some(parent_dom.clone());
				inst.last_ns = ns.to_string();
				inst.last_vnode = Some(rendered);
			}
		}
		new_vnode._dom = inst_rc.borrow().last_vnode.as_ref().and_then(|v| v._dom.clone());
		if let VNodeInner::Component { inst: slot, .. } = &new_vnode.inner {
			*slot.0.borrow_mut() = Some(inst_rc);
		}
		return Ok(None);
	}
	let dom = dom?;

	// Persist everything a future setState-triggered re-render needs.
	// Guard against staleness: a reentrant re-render (a child panicking into
	// a boundary) may already have written a fresher last_vnode — don't clobber it.
	{
		let mut inst = inst_rc.borrow_mut();
		if inst.render_generation == my_generation {
			inst.render_fn = Some(render);
			inst.last_props = props;
			inst.last_parent_dom = Some(parent_dom.clone());
			inst.last_ns = ns.to_string();
			inst.last_vnode = Some(rendered);
		}
	}

	// Same reasoning as the absorbed-path above: reflect the instance's true
	// current DOM rather than this call's possibly-stale `dom`, so the
	// parent's tree always has an accurate reference for this slot.
	new_vnode._dom = inst_rc.borrow().last_vnode.as_ref().and_then(|v| v._dom.clone());

	// Stash the (possibly newly-created) instance on the new vnode so the
	// *next* render can find it via old_vnode.
	if let VNodeInner::Component { inst: slot, .. } = &new_vnode.inner {
		*slot.0.borrow_mut() = Some(inst_rc);
	}

	Ok(dom)
}

// ─── rerender_component — called by the scheduler for dirty instances ───

pub fn rerender_component(inst_rc: Rc<RefCell<ComponentInst>>) {
	let my_generation = {
		let mut i = inst_rc.borrow_mut();
		i.dirty = false;
		i.reset_hooks();
		i.render_generation += 1;
		i.render_generation
	};

	let (render_fn, props, parent_node, ns, old_rendered, depth) = {
		let i = inst_rc.borrow();
		let render_fn = match &i.render_fn {
			Some(r) => r.clone(),
			None => return,
		};
		let parent_node = match &i.last_parent_dom {
			Some(p) => p.clone(),
			None => return,
		};
		(render_fn, i.last_props.clone(), parent_node, i.last_ns.clone(), i.last_vnode.clone(), i.depth)
	};

	// Same reasoning as diff_component: raw pointer valid only for this call.
	let inst_ptr = inst_rc.as_ptr();
	let inst_weak = Rc::downgrade(&inst_rc);

	let render_call_result: Result<VNode, JsValue> =
		match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| with_inst(inst_ptr, inst_weak, || render_fn.call(props)))) {
			Ok(result) => result,
			Err(panic_payload) => {
				let msg = panic_message(&*panic_payload);
				crate::console_error!("[micro-react] component render panicked: {}", msg);
				Err(JsValue::from_str(&msg))
			}
		};
	let mut rendered = match render_call_result {
		Ok(vnode) => vnode,
		Err(err) => {
			if !crate::hooks::report_to_nearest_boundary(&inst_rc, err.clone()) {
				crate::console_error!(
					"[micro-react] uncaught error in component render (no boundary above): {}",
					crate::bindings::stringify_thrown(&err)
				);
			} else {
				// BOUNDARY_ABSORBED was set, but this path skips the
				// take_boundary_absorbed() check below, so it would leak
				// into an unrelated later boundary's first mount otherwise.
				crate::hooks::take_boundary_absorbed();
			}
			return;
		}
	};
	rendered._depth = depth + 1;

	// Same reasoning as in diff_component: if an ancestor ErrorBoundary
	// already absorbed this throw, our old DOM node was repurposed by its
	// fallback; diffing `null` against it now would tear it back out.
	if crate::hooks::take_boundary_absorbed() {
		let mut inst = inst_rc.borrow_mut();
		if inst.render_generation == my_generation {
			inst.last_vnode = Some(rendered);
		}
		return;
	}

	let is_boundary = inst_rc.borrow().error_setter.is_some();
	if is_boundary {
		crate::hooks::push_boundary(Rc::downgrade(&inst_rc));
	}
	// See the matching catch_unwind in diff_component: reconciliation itself
	// (not just render) needs to be panic-safe, since this is the call that
	// runs when an ErrorBoundary's setError() swaps in its fallback UI.
	let diff_result =
		match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| diff_node(&parent_node, &mut rendered, old_rendered.as_ref(), &ns))) {
			Ok(res) => res,
			Err(e) => {
				let msg = panic_message(&*e);
				crate::console_error!("[micro-react] reconciliation panicked: {}", msg);
				if !crate::hooks::report_to_nearest_boundary(&inst_rc, JsValue::from_str(&msg)) {
					crate::console_error!("[micro-react] uncaught reconciliation panic (no boundary above): {}", msg);
				}
				Ok(None)
			}
		};
	if is_boundary {
		crate::hooks::pop_boundary();
	}
	// Same follow-up check as in diff_component: a reconciliation panic
	// above may itself have been absorbed by a boundary that already
	// replaced the DOM this call was about to commit. Don't clobber it.
	if crate::hooks::take_boundary_absorbed() {
		let mut inst = inst_rc.borrow_mut();
		if inst.render_generation == my_generation {
			inst.last_vnode = Some(rendered);
		}
		return;
	}

	match diff_result {
		// Same staleness guard as diff_component: this render may have
		// synchronously triggered a reentrant rerender_component of this
		// instance (a child panicking into setError); don't clobber it.
		Ok(_) => {
			let mut inst = inst_rc.borrow_mut();
			if inst.render_generation == my_generation {
				inst.last_vnode = Some(rendered);
			}
		}
		Err(e) => {
			crate::console_error!("[micro-react] re-render failed: {:?}", e);
		}
	}
}

// ─── diff_children — Preact skew algorithm ───

pub fn diff_children(
	parent_dom: &Node,
	new_children: &mut [VNode],
	old_children: &[VNode],
	ns: &str,
	_excess_dom: Option<Node>,
) -> Result<(), JsValue> {
	let new_len = new_children.len();
	let mut skew: i32 = 0;
	let mut matched: Vec<bool> = vec![false; old_children.len()];

	// Phase 1: match old→new
	let mut match_indices: Vec<i32> = vec![-1; new_len];
	for i in 0..new_len {
		let cv = &new_children[i];
		let skewed = (i as i32) + skew;
		let idx = find_match(cv, old_children, skewed as usize, &matched);
		match_indices[i] = idx;
		if idx >= 0 {
			matched[idx as usize] = true;
		}
		// Only host elements and text nodes are single, directly-insertable
		// DOM nodes; components/fragments/portals are excluded (mirrors the JS reconciler).
		let is_insertable = matches!(cv.inner, VNodeInner::Element { .. } | VNodeInner::Text(_));

		let is_mounting = idx < 0;
		if is_mounting {
			if new_len > old_children.len() {
				skew -= 1;
			} else if new_len < old_children.len() {
				skew += 1;
			}
			if is_insertable {
				new_children[i]._flags |= FLAG_INSERT;
			}
		} else if idx != skewed {
			if idx == skewed - 1 {
				skew -= 1;
			} else if idx == skewed + 1 {
				skew += 1;
			} else {
				if idx > skewed {
					skew -= 1;
				} else {
					skew += 1;
				}
				if is_insertable {
					new_children[i]._flags |= FLAG_INSERT;
				}
			}
		}
	}

	// Phase 2: diff each child
	let mut old_dom: Option<Node> = old_children.first().and_then(|c| c._dom.clone());

	for i in 0..new_len {
		let cv = &mut new_children[i];
		let idx = match_indices[i];
		cv._index = i as i32;

		let old_vn = if idx >= 0 { old_children.get(idx as usize) } else { None };

		let result_dom = diff_node(parent_dom, cv, old_vn, ns)?;

		// Components are excluded from pre-diff FLAG_INSERT since their shape
		// doesn't reflect what they render; use post-diff attachment instead.
		let already_attached = cv._dom.as_ref().and_then(|d| d.parent_node()).is_some_and(|p| p.is_same_node(Some(parent_dom)));
		let should_insert = (cv._flags & FLAG_INSERT) != 0 || !already_attached;

		if should_insert {
			if let Some(dom) = &cv._dom {
				parent_dom.insert_before(dom, old_dom.as_ref())?;
			}
		}
		if let Some(dom) = &cv._dom {
			old_dom = dom.next_sibling();
		}

		cv._flags &= !(FLAG_INSERT | FLAG_MATCHED);
	}

	// Phase 3: unmount leftover old children
	for (i, old) in old_children.iter().enumerate() {
		if !matched[i] {
			unmount_vnode(old, false);
		}
	}

	Ok(())
}

// ─── find_match — bidirectional search centred on `skewed_index` ───

fn find_match(new_vn: &VNode, old_children: &[VNode], skewed_index: usize, matched: &[bool]) -> i32 {
	let key = new_vn.key();
	let type_ = new_vn.type_tag();

	// Check centred position first
	if let Some(old) = old_children.get(skewed_index) {
		if !matched[skewed_index] && old.key() == key && old.type_tag() == type_ {
			return skewed_index as i32;
		}
	}

	// Bidirectional search
	let n = old_children.len();
	let mut lo = if skewed_index > 0 { skewed_index as i32 - 1 } else { -1 };
	let mut hi = skewed_index as i32 + 1;

	while lo >= 0 || hi < n as i32 {
		let ci = if lo >= 0 { lo } else { hi };
		if lo >= 0 {
			lo -= 1;
		} else {
			hi += 1;
		}

		if ci < 0 || ci >= n as i32 {
			continue;
		}
		let old = &old_children[ci as usize];
		if !matched[ci as usize] && old.key() == key && old.type_tag() == type_ {
			return ci;
		}
	}

	-1
}

// ─── unmount_vnode — run cleanups, detach refs, remove DOM ───

pub fn unmount_vnode(vnode: &VNode, skip_remove: bool) {
	if let Some(ref_) = vnode_ref(vnode) {
		ref_.set(None);
	}

	match &vnode.inner {
		VNodeInner::Element { children, .. } => {
			for child in &children.0 {
				// For host elements, removing the parent removes all DOM children.
				unmount_vnode(child, true);
			}
		}
		VNodeInner::Fragment { children, .. } | VNodeInner::Portal { children, .. } => {
			for child in &children.0 {
				// Fragments/Portals have no host DOM node, must remove individually.
				unmount_vnode(child, skip_remove || matches!(&vnode.inner, VNodeInner::Element { .. }));
			}
		}
		VNodeInner::Component { inst, .. } => {
			// Take the instance out (not just clone the Rc), so dropping it
			// here frees the ComponentInst and Weak-holding closures correctly see it as gone.
			if let Some(inst_rc) = inst.0.borrow_mut().take() {
				// Run effect cleanups and flip `unmounted` so any
				// already-queued scheduler entries become no-ops.
				unmount_inst(&mut inst_rc.borrow_mut());

				// Recurse into what this component last rendered so nested
				// elements/components get torn down too, not just this component's own top-level DOM.
				let last_rendered = inst_rc.borrow().last_vnode.clone();
				if let Some(rendered) = last_rendered {
					unmount_vnode(&rendered, true);
				}
				// inst_rc drops here, freeing the ComponentInst now that
				// nothing else needs it synchronously.
			}
		}
		_ => {}
	}

	if !skip_remove {
		if let Some(dom) = &vnode._dom {
			if let Some(parent) = dom.parent_node() {
				let _ = parent.remove_child(dom);
			}
		}
	}
}

fn vnode_ref(vnode: &VNode) -> Option<&NodeRef> {
	match &vnode.inner {
		VNodeInner::Element { ref_, .. } => ref_.as_ref(),
		_ => None,
	}
}

// ─── apply_props — set/remove DOM attributes and event handlers ───

const BLOCKED_ATTRS: &[&str] = &["srcdoc"];
const URL_ATTRS: &[&str] = &["href", "src", "action", "formaction", "poster", "data", "cite"];
const BOOL_ATTRS: &[&str] = &[
	// NOTE: "checked" is intentionally excluded here; it's handled below via
	// input.set_checked() so the live DOM property stays in sync on re-renders.
	"disabled",
	"selected",
	"readonly",
	"multiple",
	"autofocus",
	"autoplay",
	"controls",
	"loop",
	"muted",
	"open",
	"required",
	"reversed",
	"hidden",
];
const SAFE_URL_PREFIXES: &[&str] = &["https://", "http://", "mailto:", "tel:", "#", "/", "./", "../"];

fn is_safe_url(val: &str) -> bool {
	let trimmed = val.trim();
	if trimmed.is_empty() {
		return true;
	}
	if SAFE_URL_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
		return true;
	}
	// No allowlist prefix matched. That's fine if the value has no URI
	// scheme at all (a bare relative reference like "a.png" is inert);
	// only block values embedding an actual, non-allowlisted scheme.
	let scheme_end = trimmed.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')));
	!matches!(scheme_end, Some(i) if i > 0 && trimmed.as_bytes()[i] == b':')
}

fn apply_props(dom: &Element, new_props: &Props, old_props: &Props, ns: &str) -> Result<(), JsValue> {
	// Remove props that vanished
	for (k, old_val) in old_props {
		if k == "children" || k == "key" || k == "ref" {
			continue;
		}
		let still_present = new_props.iter().any(|(nk, _)| nk == k);
		if !still_present {
			remove_prop(dom, k, old_val, ns)?;
		}
	}
	// Set / update props
	for (k, new_val) in new_props {
		if k == "children" || k == "key" || k == "ref" {
			continue;
		}
		let old_val = old_props.iter().find(|(ok, _)| ok == k).map(|(_, v)| v);
		set_prop(dom, k, new_val, old_val, ns)?;
	}
	Ok(())
}

fn set_prop(dom: &Element, key: &str, value: &PropVal, old_value: Option<&PropVal>, ns: &str) -> Result<(), JsValue> {
	if BLOCKED_ATTRS.contains(&key) {
		return Ok(());
	}

	// dangerouslySetInnerHTML
	if key == "dangerouslySetInnerHTML.__html" {
		if let PropVal::Str(html) = value {
			dom.set_inner_html(html);
		}
		return Ok(());
	}

	// className
	if key == "className" {
		let s = prop_str(value);
		if ns == "svg" {
			dom.set_attribute("class", &s)?;
		} else {
			let el: &web_sys::HtmlElement = dom.unchecked_ref();
			el.set_class_name(&s);
		}
		return Ok(());
	}

	// style — accepts either a CSS string or a JS style object; js_val_to_prop_val
	// preserves objects as PropVal::Js and we convert them to CSS text (camelCase -> kebab-case) here.
	if key == "style" {
		let el: &web_sys::HtmlElement = dom.unchecked_ref();
		let css_text = match value {
			PropVal::Js(obj) => js_style_obj_to_css_text(obj),
			_ => prop_str(value),
		};
		el.style().set_css_text(&css_text);
		return Ok(());
	}

	// Events: onClick, onMouseEnter, etc.
	if let Some((event_name, capture)) = parse_event_prop(key) {
		let old_fn = old_value.and_then(prop_fn);
		let new_fn = prop_fn(value);
		set_event_handler(dom, &event_name, capture, new_fn, old_fn);
		return Ok(());
	}

	// URL attrs — sanitise
	if URL_ATTRS.contains(&key) {
		let s = prop_str(value);
		let safe = if is_safe_url(&s) { s } else { "#".to_string() };
		dom.set_attribute(key, &safe)?;
		return Ok(());
	}

	// Boolean attrs
	if BOOL_ATTRS.contains(&key) {
		match value {
			PropVal::Bool(true) | PropVal::Str(_) => dom.set_attribute(key, "")?,
			_ => dom.remove_attribute(key)?,
		}
		return Ok(());
	}

	// Special input props
	match key {
		"value" => {
			let s = prop_str(value);
			if let Ok(input) = dom.clone().dyn_into::<web_sys::HtmlInputElement>() {
				input.set_value(&s);
			} else if let Ok(ta) = dom.clone().dyn_into::<web_sys::HtmlTextAreaElement>() {
				ta.set_value(&s);
			} else {
				dom.set_attribute("value", &s)?;
			}
			return Ok(());
		}
		"checked" => {
			let b = matches!(value, PropVal::Bool(true));
			if let Ok(input) = dom.clone().dyn_into::<web_sys::HtmlInputElement>() {
				input.set_checked(b);
			}
			return Ok(());
		}
		"htmlFor" => {
			dom.set_attribute("for", &prop_str(value))?;
			return Ok(());
		}
		_ => {}
	}

	// Generic attr
	match value {
		PropVal::Null => dom.remove_attribute(key)?,
		PropVal::Bool(false) => dom.remove_attribute(key)?,
		PropVal::Str(s) => dom.set_attribute(key, s)?,
		PropVal::Bool(true) => dom.set_attribute(key, "")?,
		PropVal::Num(n) => dom.set_attribute(key, &n.to_string())?,
		PropVal::Callback(_) => {} // ignore non-event function props
		PropVal::Js(_) => {}       // arbitrary objects/arrays aren't valid DOM attribute values
	}
	Ok(())
}

fn remove_prop(dom: &Element, key: &str, old_val: &PropVal, ns: &str) -> Result<(), JsValue> {
	if let Some((event_name, capture)) = parse_event_prop(key) {
		let old_fn = prop_fn(old_val);
		set_event_handler(dom, &event_name, capture, None, old_fn);
		return Ok(());
	}
	if key == "className" {
		if ns == "svg" {
			dom.remove_attribute("class")?;
		} else {
			dom.unchecked_ref::<web_sys::HtmlElement>().set_class_name("");
		}
		return Ok(());
	}
	if key == "style" {
		dom.unchecked_ref::<web_sys::HtmlElement>().style().set_css_text("");
		return Ok(());
	}
	dom.remove_attribute(key)?;
	Ok(())
}

// ─── Helpers ───

fn prop_str(v: &PropVal) -> String {
	match v {
		PropVal::Str(s) => s.clone(),
		PropVal::Bool(b) => b.to_string(),
		PropVal::Num(n) => n.to_string(),
		_ => String::new(),
	}
}

/// Convert a JS style object (`{ fontSize: '1rem' }`) into CSS text
/// (`font-size: 1rem;`), the way React does for `style={{...}}`.
fn js_style_obj_to_css_text(obj: &JsValue) -> String {
	if !obj.is_object() {
		return String::new();
	}
	let o = match obj.dyn_ref::<Object>() {
		Some(o) => o,
		None => return String::new(),
	};
	let mut out = String::new();
	for key in Object::keys(o).iter() {
		let key_str = match key.as_string() {
			Some(s) => s,
			None => continue,
		};
		let val = match Reflect::get(obj, &key) {
			Ok(v) => v,
			Err(_) => continue,
		};
		if val.is_null() || val.is_undefined() {
			continue;
		}
		let val_str = if let Some(s) = val.as_string() {
			s
		} else if let Some(n) = val.as_f64() {
			// React treats bare numbers as px for most props; good enough here.
			format!("{}px", n)
		} else {
			continue;
		};
		out.push_str(&camel_to_kebab(&key_str));
		out.push_str(": ");
		out.push_str(&val_str);
		out.push_str("; ");
	}
	out
}

fn camel_to_kebab(s: &str) -> String {
	// CSS custom properties (`--foo`) pass through untouched.
	if s.starts_with("--") {
		return s.to_string();
	}
	let mut out = String::new();
	for (i, c) in s.chars().enumerate() {
		if c.is_ascii_uppercase() {
			if i != 0 {
				out.push('-');
			}
			out.push(c.to_ascii_lowercase());
		} else {
			out.push(c);
		}
	}
	out
}

fn prop_fn(v: &PropVal) -> Option<&Function> {
	match v {
		PropVal::Callback(cb) => Some(&cb.0),
		_ => None,
	}
}

fn effective_ns(tag: &str, current_ns: &str) -> String {
	match tag {
		"svg" => "svg".to_string(),
		"math" => "math".to_string(),
		"foreignObject" => "html".to_string(),
		_ => current_ns.to_string(),
	}
}

fn ns_uri(ns: &str) -> Option<&str> {
	match ns {
		"svg" => Some(SVG_NS),
		"math" => Some(MATH_NS),
		"html" | "" => None,
		_ => None,
	}
}

fn document() -> Document {
	web_sys::window().expect("no window").document().expect("no document")
}

// ─── Tests for pure (non-DOM) helper logic ───
// `cargo test --lib` covers these; DOM-dependent reconciler behavior is
// covered separately in `tests/reconciler.rs` via wasm-bindgen-test.
#[cfg(test)]
mod helper_tests {
	use super::*;

	#[test]
	fn camel_to_kebab_basic() {
		assert_eq!(camel_to_kebab("fontSize"), "font-size");
		assert_eq!(camel_to_kebab("backgroundColor"), "background-color");
		assert_eq!(camel_to_kebab("color"), "color");
	}

	#[test]
	fn camel_to_kebab_leaves_css_custom_properties_alone() {
		// `--my-Var` is a CSS custom property name; it must pass through
		// untouched (no case conversion), unlike a normal camelCase prop.
		assert_eq!(camel_to_kebab("--myVar"), "--myVar");
	}

	#[test]
	fn is_safe_url_allows_expected_schemes() {
		assert!(is_safe_url("https://example.com"));
		assert!(is_safe_url("http://example.com"));
		assert!(is_safe_url("mailto:a@b.com"));
		assert!(is_safe_url("tel:+123"));
		assert!(is_safe_url("#anchor"));
		assert!(is_safe_url("/relative/path"));
		assert!(is_safe_url("./relative"));
		assert!(is_safe_url("../relative"));
	}

	#[test]
	fn is_safe_url_allows_bare_relative_references() {
		// Regression test: these have no URI scheme (nothing before a ":"),
		// so they were previously rejected/swapped for "#" just for lacking
		// a leading "/", "./", or "../".
		assert!(is_safe_url("a.png"));
		assert!(is_safe_url("img/a.png"));
		assert!(is_safe_url("style.css"));
		assert!(is_safe_url("?x=1"));
		assert!(is_safe_url(""));
		assert!(is_safe_url("   "));
	}

	#[test]
	fn is_safe_url_still_rejects_unknown_schemes_without_a_leading_slash() {
		// A value with an actual, non-allowlisted scheme must still be
		// rejected — the bare-relative-reference carve-out must not
		// accidentally swallow this case.
		assert!(!is_safe_url("javascript:alert(1)"));
		assert!(!is_safe_url("custom-scheme:payload"));
	}

	#[test]
	fn is_safe_url_rejects_script_and_unknown_schemes() {
		// The whole point of this allowlist is blocking `javascript:` URLs
		// (and similar) from an untrusted href/src/action prop.
		assert!(!is_safe_url("javascript:alert(1)"));
		assert!(!is_safe_url("data:text/html,<script>alert(1)</script>"));
		assert!(!is_safe_url("vbscript:msgbox(1)"));
	}

	#[test]
	fn is_safe_url_trims_whitespace_before_checking() {
		// Browsers tolerate leading whitespace/control chars before a scheme,
		// so trim first or `"  javascript:..."` slips through as
		// "unrecognized" and falls through to unsafe acceptance.
		assert!(is_safe_url("  https://example.com"));
		assert!(!is_safe_url("  javascript:alert(1)"));
	}

	#[test]
	fn effective_ns_switches_into_svg_and_math() {
		assert_eq!(effective_ns("svg", "html"), "svg");
		assert_eq!(effective_ns("math", "html"), "math");
		assert_eq!(effective_ns("foreignObject", "svg"), "html");
		assert_eq!(effective_ns("div", "svg"), "svg"); // inherits current ns
	}
}
