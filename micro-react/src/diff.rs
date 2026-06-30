// ─── diff.rs ─────────────────────────────────────────────────────────────────
//
// Reconciler — walks old and new VNode trees, patches the DOM.
//
// Algorithm: Preact-style skew diff with keyed matching.
//
// Entry points:
//   diff_node()        – diff a single vnode against its old counterpart
//   diff_children()    – diff a list of children (skew algorithm)
//   rerender_component() – re-render a dirty component in place
//
// ─────────────────────────────────────────────────────────────────────────────

use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{Document, Element, Node, Text};
use js_sys::{Function, Object, Reflect};
use std::rc::Rc;

use crate::vnode::{
    VNode, VNodeInner, Props, PropVal, Children, ComponentFn,
    NodeRef, FLAG_INSERT, FLAG_MATCHED,
};
use crate::hooks::{ComponentInst, with_inst};
use crate::events::{set_event_handler, parse_event_prop};

const SVG_NS:  &str = "http://www.w3.org/2000/svg";
const MATH_NS: &str = "http://www.w3.org/1998/Math/MathML";
const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

// ─────────────────────────────────────────────────────────────────────────────
// Internal component tree node (wraps ComponentInst in Rc<RefCell>)
// ─────────────────────────────────────────────────────────────────────────────
use std::cell::RefCell;

/// Every function-component vnode gets one of these.
pub struct ComponentNode {
    pub inst: Rc<RefCell<ComponentInst>>,
    pub render: ComponentFn,
    pub last_vnode: Option<VNode>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Thread-local render depth guard
// ─────────────────────────────────────────────────────────────────────────────
thread_local! {
    static RENDER_DEPTH: RefCell<u32> = RefCell::new(0);
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

// ─────────────────────────────────────────────────────────────────────────────
// diff_node — main recursive entry
// ─────────────────────────────────────────────────────────────────────────────

pub fn diff_node(
    parent_dom: &Node,
    new_vnode: &mut VNode,
    old_vnode: Option<&VNode>,
    ns: &str,
) -> Result<Option<Node>, JsValue> {
    guard_depth()?;
    let result = diff_node_inner(parent_dom, new_vnode, old_vnode, ns);
    release_depth();
    result
}

fn diff_node_inner(
    parent_dom: &Node,
    new_vnode: &mut VNode,
    old_vnode: Option<&VNode>,
    ns: &str,
) -> Result<Option<Node>, JsValue> {
    match &new_vnode.inner {
        VNodeInner::Null => {
            new_vnode._dom = None;
            Ok(None)
        }

        VNodeInner::Text(text) => {
            let text = text.clone();
            // Reuse existing text node if possible
            if let Some(old) = old_vnode {
                if let Some(existing) = &old._dom {
                    if let Ok(txt) = existing.clone().dyn_into::<Text>() {
                        if txt.data() != text { txt.set_data(&text); }
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
        VNodeInner::Portal { .. }  => diff_portal(new_vnode, old_vnode, ns),

        VNodeInner::Element { .. } => diff_element(parent_dom, new_vnode, old_vnode, ns),

        VNodeInner::Component { .. } => diff_component(parent_dom, new_vnode, old_vnode, ns),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fragment
// ─────────────────────────────────────────────────────────────────────────────

fn diff_fragment(
    parent_dom: &Node,
    new_vnode: &mut VNode,
    old_vnode: Option<&VNode>,
    ns: &str,
) -> Result<Option<Node>, JsValue> {
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

// ─────────────────────────────────────────────────────────────────────────────
// Portal
// ─────────────────────────────────────────────────────────────────────────────

fn diff_portal(
    new_vnode: &mut VNode,
    old_vnode: Option<&VNode>,
    ns: &str,
) -> Result<Option<Node>, JsValue> {
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

// ─────────────────────────────────────────────────────────────────────────────
// Element
// ─────────────────────────────────────────────────────────────────────────────

fn diff_element(
    parent_dom: &Node,
    new_vnode: &mut VNode,
    old_vnode: Option<&VNode>,
    ns: &str,
) -> Result<Option<Node>, JsValue> {
    let (tag, props, children, ref_, _template) = match &new_vnode.inner {
        VNodeInner::Element { template, props, children, ref_, .. } => (
            template.tag.clone(),
            props.clone(),
            children.0.clone(),
            ref_.clone(),
            template.clone(),
        ),
        _ => unreachable!(),
    };

    // Namespace propagation
    let ns = effective_ns(&tag, ns);

    let old_elem = old_vnode.and_then(|o| {
        o._dom.clone().and_then(|n| n.dyn_into::<Element>().ok())
    });

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

// ─────────────────────────────────────────────────────────────────────────────
// Component
// ─────────────────────────────────────────────────────────────────────────────

fn diff_component(
    parent_dom: &Node,
    new_vnode: &mut VNode,
    old_vnode: Option<&VNode>,
    ns: &str,
) -> Result<Option<Node>, JsValue> {
    let (render, props) = match &new_vnode.inner {
        VNodeInner::Component { render, props, .. } => (render.clone(), props.clone()),
        _ => unreachable!(),
    };

    // Reuse the component instance across re-renders: the old vnode (matched
    // by diff_children via type+key) carries the live instance from its own
    // mount, so grab it instead of starting fresh. This is what lets hooks
    // (state, refs, effects) survive across renders.
    let reused_inst: Option<Rc<RefCell<ComponentInst>>> = old_vnode.and_then(|o| match &o.inner {
        VNodeInner::Component { inst, .. } => inst.0.borrow().clone(),
        _ => None,
    });

    let inst_rc: Rc<RefCell<ComponentInst>> = match reused_inst {
        Some(inst) => inst,
        None => Rc::new(RefCell::new(ComponentInst::new())),
    };

    // The previous output of *this* instance (not the matched old vnode
    // itself — that's a stand-in for "did we mount before", the real old
    // tree to diff against lives on the instance).
    let old_rendered = inst_rc.borrow().last_vnode.clone();

    {
        let mut inst = inst_rc.borrow_mut();
        inst.depth = new_vnode._depth;
        inst.parent_dom = parent_dom.clone().dyn_into::<Element>().ok();
        inst.reset_hooks();
        inst.dirty = false;
    }

    let inst_ptr = inst_rc.as_ptr() as *mut ComponentInst;

    // Run render function with this component as the current instance
    let render_result = with_inst(inst_ptr, || render.call(props.clone()));

    let mut rendered = render_result;
    rendered._depth = new_vnode._depth + 1;

    let dom = diff_node(parent_dom, &mut rendered, old_rendered.as_ref(), ns)?;
    new_vnode._dom = dom.clone();

    // Persist everything a future setState-triggered re-render needs.
    {
        let mut inst = inst_rc.borrow_mut();
        inst.render_fn = Some(render);
        inst.last_props = props;
        inst.last_parent_dom = Some(parent_dom.clone());
        inst.last_ns = ns.to_string();
        inst.last_vnode = Some(rendered);
    }

    // Stash the (possibly newly-created) instance on the new vnode so the
    // *next* render can find it via old_vnode.
    if let VNodeInner::Component { inst: slot, .. } = &new_vnode.inner {
        *slot.0.borrow_mut() = Some(inst_rc);
    }

    Ok(dom)
}

// ─────────────────────────────────────────────────────────────────────────────
// rerender_component — called by the scheduler for dirty instances
// ─────────────────────────────────────────────────────────────────────────────

pub fn rerender_component(inst: *mut ComponentInst) {
    // Safety: single-threaded WASM, inst is kept alive by the vnode tree
    // (held via the Rc stashed in the matched Component vnode's inst slot).
    unsafe {
        (*inst).dirty = false;
        (*inst).reset_hooks();
    }

    let (render_fn, props, parent_node, ns, old_rendered, depth) = unsafe {
        let i = &*inst;
        let render_fn = match &i.render_fn { Some(r) => r.clone(), None => return };
        let parent_node = match &i.last_parent_dom { Some(p) => p.clone(), None => return };
        (render_fn, i.last_props.clone(), parent_node, i.last_ns.clone(), i.last_vnode.clone(), i.depth)
    };

    let mut rendered = with_inst(inst, || render_fn.call(props));
    rendered._depth = depth + 1;

    if diff_node(&parent_node, &mut rendered, old_rendered.as_ref(), &ns).is_ok() {
        unsafe { (*inst).last_vnode = Some(rendered); }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// diff_children — Preact skew algorithm
// ─────────────────────────────────────────────────────────────────────────────

pub fn diff_children(
    parent_dom: &Node,
    new_children: &mut Vec<VNode>,
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
        // Only host elements (tag strings) and text nodes are single,
        // directly-insertable DOM nodes. Function components have no DOM
        // node of their own, and Fragments/Portals represent *multiple*
        // independent top-level nodes -- flagging them for direct insertion
        // makes the insert step below move only their borrowed "first
        // child" stand-in dom, yanking it away from its already-correctly-
        // placed siblings (mirrors the JS reconciler's exclusion of
        // function/symbol-typed children here).
        let is_insertable = matches!(cv.inner, VNodeInner::Element { .. } | VNodeInner::Text(_));

        let is_mounting = idx < 0;
        if is_mounting {
            if new_len > old_children.len() { skew -= 1; }
            else if new_len < old_children.len() { skew += 1; }
            if is_insertable { new_children[i]._flags |= FLAG_INSERT; }
        } else if idx != skewed {
            if idx == skewed - 1 {
                skew -= 1;
            } else if idx == skewed + 1 {
                skew += 1;
            } else {
                if idx > skewed { skew -= 1; } else { skew += 1; }
                if is_insertable { new_children[i]._flags |= FLAG_INSERT; }
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

        // The skew algorithm above only flags FLAG_INSERT for vnodes that
        // were *directly* host Elements/Text before diffing — Component
        // (and Fragment/Portal) wrappers are excluded there because their
        // pre-diff shape doesn't reflect what they actually render to.
        //
        // But a Component's rendered output is only ever attached to the
        // DOM by *this* loop (diff_element / diff_component never insert
        // anything themselves — they just record `_dom`). So if we only
        // trust the pre-diff FLAG_INSERT, a Component child's freshly
        // created DOM node is built but never appended anywhere, leaving
        // the page blank. To fix this without re-introducing the
        // "move only the fragment's stand-in first child" bug the original
        // exclusion was guarding against, decide insertion from the
        // *post-diff* reality: insert/move whenever the node produced isn't
        // already attached under `parent_dom`, in addition to the explicit
        // skew-reorder flag.
        let already_attached = cv
            ._dom
            .as_ref()
            .and_then(|d| d.parent_node())
            .map_or(false, |p| p.is_same_node(Some(parent_dom)));
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

// ─────────────────────────────────────────────────────────────────────────────
// find_match — bidirectional search centred on `skewed_index`
// ─────────────────────────────────────────────────────────────────────────────

fn find_match(
    new_vn: &VNode,
    old_children: &[VNode],
    skewed_index: usize,
    matched: &[bool],
) -> i32 {
    let key  = new_vn.key();
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
        if lo >= 0 { lo -= 1; } else { hi += 1; }

        if ci < 0 || ci >= n as i32 { continue; }
        let old = &old_children[ci as usize];
        if !matched[ci as usize] && old.key() == key && old.type_tag() == type_ {
            return ci;
        }
    }

    -1
}

// ─────────────────────────────────────────────────────────────────────────────
// unmount_vnode — run cleanups, detach refs, remove DOM
// ─────────────────────────────────────────────────────────────────────────────

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
        VNodeInner::Component { .. } => {
            // Component inst cleanup would go here
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

// ─────────────────────────────────────────────────────────────────────────────
// apply_props — set/remove DOM attributes and event handlers
// ─────────────────────────────────────────────────────────────────────────────

const BLOCKED_ATTRS: &[&str] = &["srcdoc"];
const URL_ATTRS: &[&str] = &["href", "src", "action", "formaction", "poster", "data", "cite"];
const BOOL_ATTRS: &[&str] = &[
    // NOTE: "checked" is intentionally excluded — it's handled below via
    // input.set_checked() so the live DOM *property* (not just the
    // attribute) stays in sync on re-renders. Leaving it in this list
    // shadowed the dedicated "checked" match arm further down, since this
    // check runs first.
    "disabled", "selected", "readonly", "multiple", "autofocus",
    "autoplay", "controls", "loop", "muted", "open", "required", "reversed",
    "hidden",
];
const SAFE_URL_PREFIXES: &[&str] = &["https://", "http://", "mailto:", "tel:", "#", "/", "./", "../"];

fn is_safe_url(val: &str) -> bool {
    let trimmed = val.trim();
    SAFE_URL_PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

fn apply_props(
    dom: &Element,
    new_props: &Props,
    old_props: &Props,
    ns: &str,
) -> Result<(), JsValue> {
    // Remove props that vanished
    for (k, old_val) in old_props {
        if k == "children" || k == "key" || k == "ref" { continue; }
        let still_present = new_props.iter().any(|(nk, _)| nk == k);
        if !still_present {
            remove_prop(dom, k, old_val, ns)?;
        }
    }
    // Set / update props
    for (k, new_val) in new_props {
        if k == "children" || k == "key" || k == "ref" { continue; }
        let old_val = old_props.iter().find(|(ok, _)| ok == k).map(|(_, v)| v);
        set_prop(dom, k, new_val, old_val, ns)?;
    }
    Ok(())
}

fn set_prop(
    dom: &Element,
    key: &str,
    value: &PropVal,
    old_value: Option<&PropVal>,
    ns: &str,
) -> Result<(), JsValue> {
    if BLOCKED_ATTRS.contains(&key) { return Ok(()); }

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

    // style — accepts either a CSS string or a JS style object
    // (`style={{ fontSize: '1rem', marginTop: '.5rem' }}`, the form used
    // throughout this app). Previously only PropVal::Str was handled here
    // (via prop_str), and an object value had already been collapsed to
    // PropVal::Null upstream besides — both are fixed now: js_val_to_prop_val
    // preserves the object as PropVal::Js, and here we convert it to real
    // CSS text (camelCase keys -> kebab-case properties) instead of dropping it.
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
        PropVal::Js(_) => {} // arbitrary objects/arrays aren't valid DOM attribute values
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
        if ns == "svg" { dom.remove_attribute("class")?; }
        else { dom.unchecked_ref::<web_sys::HtmlElement>().set_class_name(""); }
        return Ok(());
    }
    if key == "style" {
        dom.unchecked_ref::<web_sys::HtmlElement>().style().set_css_text("");
        return Ok(());
    }
    dom.remove_attribute(key)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn prop_str(v: &PropVal) -> String {
    match v {
        PropVal::Str(s)  => s.clone(),
        PropVal::Bool(b) => b.to_string(),
        PropVal::Num(n)  => n.to_string(),
        _                => String::new(),
    }
}

/// Convert a JS style object (`{ fontSize: '1rem', marginTop: '.5rem' }`)
/// into a CSS text string (`font-size: 1rem; margin-top: .5rem;`), the way
/// React does for `style={{...}}`. camelCase keys become kebab-case
/// properties; numeric values are passed through as-is (callers in this app
/// only ever use unit-suffixed strings or pixel-implicit numbers, mirroring
/// React's behavior of treating bare numbers as px for most properties —
/// kept simple here since this app never relies on the unitless exceptions).
fn js_style_obj_to_css_text(obj: &JsValue) -> String {
    if !obj.is_object() { return String::new(); }
    let o = match obj.dyn_ref::<Object>() {
        Some(o) => o,
        None => return String::new(),
    };
    let mut out = String::new();
    for key in Object::keys(o).iter() {
        let key_str = match key.as_string() { Some(s) => s, None => continue };
        let val = match Reflect::get(obj, &key) { Ok(v) => v, Err(_) => continue };
        if val.is_null() || val.is_undefined() { continue; }
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
    if s.starts_with("--") { return s.to_string(); }
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 { out.push('-'); }
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
        "svg"  => "svg".to_string(),
        "math" => "math".to_string(),
        "foreignObject" => "html".to_string(),
        _ => current_ns.to_string(),
    }
}

fn ns_uri(ns: &str) -> Option<&str> {
    match ns {
        "svg"  => Some(SVG_NS),
        "math" => Some(MATH_NS),
        "html" | "" => None,
        _ => None,
    }
}

fn document() -> Document {
    web_sys::window().expect("no window").document().expect("no document")
}