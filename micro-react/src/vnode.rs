// VNode tree + fluent element builder. A Template stores only the static
// skeleton (tag + static attrs) of an Element; dynamic values live in
// `holes`/`props` and are resolved at diff time.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use wasm_bindgen::{prelude::*, JsValue};
use web_sys::Element;

// ── monotonic vnode id ──
static VNODE_ID: AtomicU64 = AtomicU64::new(1);
pub fn next_id() -> u64 {
    VNODE_ID.fetch_add(1, Ordering::Relaxed)
}

// ─── Prop value — can hold strings, booleans, numbers, or JS callbacks ───
#[derive(Clone, Debug)]
pub enum PropVal {
    Str(String),
    Bool(bool),
    Num(f64),
    Callback(JsCallback),
    /// Any JS value that isn't a primitive/function/null — plain objects
    /// (`style={{...}}`, `routes={{...}}`) and arrays.
    Js(JsValue),
    Null,
}

impl PartialEq for PropVal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PropVal::Str(a), PropVal::Str(b)) => a == b,
            (PropVal::Bool(a), PropVal::Bool(b)) => a == b,
            (PropVal::Num(a), PropVal::Num(b)) => a == b,
            (PropVal::Null, PropVal::Null) => true,
            (PropVal::Callback(a), PropVal::Callback(b)) => {
                js_sys::Object::is(a.as_ref(), b.as_ref())
            }
            (PropVal::Js(a), PropVal::Js(b)) => js_sys::Object::is(a, b),
            _ => false,
        }
    }
}

impl From<&str> for PropVal {
    fn from(s: &str) -> Self { PropVal::Str(s.to_string()) }
}
impl From<String> for PropVal {
    fn from(s: String) -> Self { PropVal::Str(s) }
}
impl From<bool> for PropVal {
    fn from(b: bool) -> Self { PropVal::Bool(b) }
}
impl From<f64> for PropVal {
    fn from(n: f64) -> Self { PropVal::Num(n) }
}
impl From<i32> for PropVal {
    fn from(n: i32) -> Self { PropVal::Num(n as f64) }
}
impl From<usize> for PropVal {
    fn from(n: usize) -> Self { PropVal::Num(n as f64) }
}
impl From<JsCallback> for PropVal {
    fn from(f: JsCallback) -> Self { PropVal::Callback(f) }
}

/// A JS function value used for event handlers.
#[derive(Clone, Debug)]
pub struct JsCallback(pub js_sys::Function);
impl AsRef<JsValue> for JsCallback {
    fn as_ref(&self) -> &JsValue { self.0.as_ref() }
}
impl From<js_sys::Function> for JsCallback {
    fn from(f: js_sys::Function) -> Self { JsCallback(f) }
}
impl From<&js_sys::Function> for JsCallback {
    fn from(f: &js_sys::Function) -> Self { JsCallback(f.clone()) }
}

// ─── Template — the static part of an Element, cached on the vnode ───
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub id: u64,
    pub tag: String,
}

impl Template {
    pub fn new(tag: impl Into<String>) -> Self {
        Template { id: next_id(), tag: tag.into() }
    }
}

// ─── Props — a thin ordered map ───
pub type Props = Vec<(String, PropVal)>;
pub type Key   = Option<String>;

// ─── Children helper ───
#[derive(Clone, Debug)]
pub struct Children(pub Vec<VNode>);

impl Children {
    pub fn len(&self) -> usize { self.0.len() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

// ─── VNodeInner — the discriminated union ───
#[derive(Clone, Debug)]
pub enum VNodeInner {
    /// Plain DOM element: <tag props…>children</tag>
    Element {
        template: Template,
        /// Full merged props (resolved at diff time).
        props: Props,
        children: Children,
        key: Key,
        ref_: Option<NodeRef>,
    },
    /// Plain text node.
    Text(String),
    /// Fragment (keyable list wrapper).
    Fragment {
        children: Children,
        key: Key,
    },
    /// A function component call.
    Component {
        name: String,
        render: ComponentFn,
        props: Props,
        key: Key,
        /// Holds the live `ComponentInst` once mounted, so the next render
        /// can reuse it and let hooks survive across re-renders.
        inst: ComponentInstSlot,
    },
    /// Portal — render children into a different DOM container.
    Portal {
        container: Element,
        children: Children,
    },
    /// Nothing — renders no DOM nodes.
    Null,
}

// ─── VNode — the public handle ───
#[derive(Clone, Debug)]
pub struct VNode {
    pub inner: VNodeInner,
    /// Monotonically increasing id for bailing out on unchanged subtrees.
    pub original: u64,
    // Reconciler bookkeeping (set by diff engine, not by user).
    pub(crate) _dom: Option<web_sys::Node>,
    pub(crate) _depth: u32,
    pub(crate) _index: i32,
    pub(crate) _flags: u8,
}

pub const FLAG_INSERT:  u8 = 1 << 0;
pub const FLAG_MATCHED: u8 = 1 << 1;

impl VNode {
    fn new(inner: VNodeInner) -> Self {
        VNode {
            inner,
            original: next_id(),
            _dom: None,
            _depth: 0,
            _index: -1,
            _flags: 0,
        }
    }

    pub fn null() -> Self { VNode::new(VNodeInner::Null) }

    pub fn text(s: impl Into<String>) -> Self {
        VNode::new(VNodeInner::Text(s.into()))
    }

    /// Start building an element: `VNode::tag("div")`.
    pub fn tag(tag: impl Into<String>) -> ElementBuilder {
        ElementBuilder::new(tag.into())
    }

    pub fn fragment(children: Vec<VNode>) -> Self {
        VNode::new(VNodeInner::Fragment {
            children: Children(children),
            key: None,
        })
    }

    pub fn fragment_keyed(key: impl Into<String>, children: Vec<VNode>) -> Self {
        VNode::new(VNodeInner::Fragment {
            children: Children(children),
            key: Some(key.into()),
        })
    }

    pub fn component(name: impl Into<String>, render: ComponentFn, props: Props) -> Self {
        VNode::new(VNodeInner::Component {
            name: name.into(),
            render,
            props,
            key: None,
            inst: ComponentInstSlot::new(),
        })
    }

    /// Set this vnode's key after construction. Needed for `Component`
    /// vnodes, which have no builder step to pass a key through, so a
    /// `key` prop (e.g. `h(ErrorBoundary, { key })`) would otherwise be dropped.
    pub fn with_key(mut self, key: Option<String>) -> Self {
        match &mut self.inner {
            VNodeInner::Element  { key: k, .. }
            | VNodeInner::Fragment { key: k, .. }
            | VNodeInner::Component { key: k, .. } => *k = key,
            _ => {}
        }
        self
    }

    pub fn key(&self) -> Option<&str> {
        match &self.inner {
            VNodeInner::Element  { key, .. } => key.as_deref(),
            VNodeInner::Fragment { key, .. } => key.as_deref(),
            VNodeInner::Component{ key, .. } => key.as_deref(),
            _ => None,
        }
    }

    pub fn type_tag(&self) -> Option<&str> {
        match &self.inner {
            VNodeInner::Element { template, .. } => Some(&template.tag),
            VNodeInner::Text(_) => Some("#text"),
            VNodeInner::Fragment { .. } => Some("#fragment"),
            VNodeInner::Null => Some("#null"),
            VNodeInner::Component { name, .. } => Some(name),
            VNodeInner::Portal { .. } => Some("#portal"),
        }
    }
}

// ─── NodeRef: keeps a JS-side `{ current }` ref (or callback ref) in sync with the reconciler ───
#[derive(Clone)]
pub struct NodeRef {
    pub node: std::rc::Rc<std::cell::RefCell<Option<web_sys::Node>>>,
    pub sync: Option<std::rc::Rc<dyn Fn(Option<web_sys::Node>)>>,
}

impl fmt::Debug for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<NodeRef>")
    }
}

impl NodeRef {
    pub fn new() -> Self {
        NodeRef { node: std::rc::Rc::new(std::cell::RefCell::new(None)), sync: None }
    }
    /// Create a NodeRef that calls `sync` (with the new node, or `None` on
    /// unmount) every time the DOM node it's attached to changes.
    pub fn with_sync(sync: impl Fn(Option<web_sys::Node>) + 'static) -> Self {
        NodeRef { node: std::rc::Rc::new(std::cell::RefCell::new(None)), sync: Some(std::rc::Rc::new(sync)) }
    }
    pub(crate) fn set(&self, node: Option<web_sys::Node>) {
        *self.node.borrow_mut() = node.clone();
        if let Some(f) = &self.sync {
            f(node);
        }
    }
}

// ─── ComponentFn — an `Fn(Props) -> Result<VNode, JsValue>` wrapped in Rc so it's Clone ───
//
// A component can "throw" by returning `Err`, exactly like a real React
// component throwing during render becomes a JS exception the reconciler
// catches. `Err` propagates up to the nearest ErrorBoundary ancestor (see
// `diff::diff_component` / `hooks::report_to_nearest_boundary`), the same
// way React walks up the fiber tree to find the nearest boundary.
#[derive(Clone)]
pub struct ComponentFn(pub std::rc::Rc<dyn Fn(Props) -> Result<VNode, JsValue>>);

impl fmt::Debug for ComponentFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<ComponentFn>")
    }
}

impl ComponentFn {
    /// The primary constructor, for a component that may throw.
    pub fn new(f: impl Fn(Props) -> Result<VNode, JsValue> + 'static) -> Self {
        ComponentFn(std::rc::Rc::new(f))
    }
    /// Convenience constructor for the common case of a component that
    /// never throws.
    pub fn infallible(f: impl Fn(Props) -> VNode + 'static) -> Self {
        ComponentFn(std::rc::Rc::new(move |props| Ok(f(props))))
    }
    pub fn call(&self, props: Props) -> Result<VNode, JsValue> {
        (self.0)(props)
    }
}

// ─── ComponentInstSlot: where the diff engine stashes a Component vnode's live ComponentInst so hooks persist across re-renders ───
#[derive(Clone, Default)]
pub struct ComponentInstSlot(
    pub std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<crate::hooks::ComponentInst>>>>>,
);

impl ComponentInstSlot {
    pub fn new() -> Self {
        ComponentInstSlot(std::rc::Rc::new(std::cell::RefCell::new(None)))
    }
}

impl fmt::Debug for ComponentInstSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<ComponentInstSlot>")
    }
}

// ─── ElementBuilder — fluent builder that produces a VNode::Element ───
pub struct ElementBuilder {
    template: Template,
    props: Props,
    children: Vec<VNode>,
    key: Key,
    ref_: Option<NodeRef>,
}

impl ElementBuilder {
    pub fn new(tag: String) -> Self {
        ElementBuilder {
            template: Template::new(&tag),
            props: Vec::new(),
            children: Vec::new(),
            key: None,
            ref_: None,
        }
    }

    /// Set any attribute, e.g. `.attr("className", "foo")`.
    pub fn attr(mut self, name: impl Into<String>, value: impl Into<PropVal>) -> Self {
        self.props.push((name.into(), value.into()));
        self
    }

    /// Set an event handler. `name` should be React-style camelCase, e.g. "onClick".
    pub fn on(self, name: impl Into<String>, handler: js_sys::Function) -> Self {
        self.attr(name.into(), PropVal::Callback(JsCallback(handler)))
    }

    /// Set a `key` for keyed reconciliation.
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.key = Some(k.into());
        self
    }

    /// Attach a NodeRef.
    pub fn ref_(mut self, r: NodeRef) -> Self {
        self.ref_ = Some(r);
        self
    }

    pub fn child(mut self, c: VNode) -> Self {
        self.children.push(c);
        self
    }

    pub fn children(mut self, cs: impl IntoIterator<Item = VNode>) -> Self {
        self.children.extend(cs);
        self
    }

    pub fn text(self, t: impl Into<String>) -> Self {
        self.child(VNode::text(t))
    }

    pub fn build(self) -> VNode {
        VNode::new(VNodeInner::Element {
            template: self.template,
            props: self.props,
            children: Children(self.children),
            key: self.key,
            ref_: self.ref_,
        })
    }
}

/// Allow `.build()` to be omitted in most contexts.
impl From<ElementBuilder> for VNode {
    fn from(b: ElementBuilder) -> VNode { b.build() }
}
