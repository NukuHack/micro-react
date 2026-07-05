// ─── vnode.rs ───
// VNode tree + template-cached html!() builder.
// A Template stores only the static skeleton (tag + static attrs); dynamic
// holes are patched on re-render without re-parsing. Also exposes a fluent VNode::tag() builder API.
// ────────────────

use std::{
    collections::HashMap,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};
use wasm_bindgen::{prelude::*, JsValue};
use web_sys::Element;

// ── monotonic vnode id (mirrors JS vnodeId counter) ──────────────────────────
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
                // Compare by function identity (pointer)
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

// ─── Template — the static part of an element definition, interned per call site so identical html!() calls share one skeleton ───
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Stable id: address of the static string literal (or a hash).
    pub id: u64,
    /// Element tag name.
    pub tag: String,
    /// Static attributes: name → value (both known at parse time).
    pub static_attrs: Vec<(String, String)>,
    /// Holes: just the attribute names for dynamic slots, in order.
    pub hole_names: Vec<String>,
    /// True if a `key={…}` hole was present.
    pub has_key: bool,
    /// Optional XML namespace ("svg" | "math").
    pub ns: Option<String>,
}

impl Template {
    pub fn new(tag: impl Into<String>) -> Self {
        Template {
            id: next_id(),
            tag: tag.into(),
            static_attrs: Vec::new(),
            hole_names: Vec::new(),
            has_key: false,
            ns: None,
        }
    }
}

// ─── Props — a thin ordered map ───
pub type Props = Vec<(String, PropVal)>;
pub type Key   = Option<String>;

pub fn props_get<'a>(props: &'a Props, key: &str) -> Option<&'a PropVal> {
    props.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

// ─── Children helper (mirrors React.Children) ───
#[derive(Clone, Debug)]
pub struct Children(pub Vec<VNode>);

impl Children {
    pub fn empty() -> Self { Children(Vec::new()) }
    pub fn one(v: VNode) -> Self { Children(vec![v]) }

    pub fn map(&self, f: impl Fn(&VNode) -> VNode) -> Vec<VNode> {
        self.0.iter().map(f).collect()
    }
    pub fn len(&self) -> usize { self.0.len() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

// ─── VNodeInner — the discriminated union ───
#[derive(Clone, Debug)]
pub enum VNodeInner {
    /// Plain DOM element: <tag props…>children</tag>
    Element {
        /// The cached template (static skeleton).
        template: Template,
        /// Dynamic hole values in `template.hole_names` order.
        /// Static attrs are baked into the template.
        holes: Vec<PropVal>,
        /// Full merged props (static + holes, resolved at diff time).
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
        /// can reuse it instead of starting fresh (lets hooks survive across re-renders).
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
    // Reconciler bookkeeping (set by diff engine, not by user)
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

    // ── Null / Text ──────────────────────────────────────────────────────────

    pub fn null() -> Self { VNode::new(VNodeInner::Null) }

    pub fn text(s: impl Into<String>) -> Self {
        VNode::new(VNodeInner::Text(s.into()))
    }

    // ── Element builder entry point ──────────────────────────────────────────

    /// Start building an element.  Idiomatic: `VNode::tag("div")`.
    /// Also callable as `"div".v()` via the `IntoVNode` trait.
    pub fn tag(tag: impl Into<String>) -> ElementBuilder {
        ElementBuilder::new(tag.into())
    }

    // ── Fragment ─────────────────────────────────────────────────────────────

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

    // ── Component ────────────────────────────────────────────────────────────

    pub fn component(name: impl Into<String>, render: ComponentFn, props: Props) -> Self {
        VNode::new(VNodeInner::Component {
            name: name.into(),
            render,
            props,
            key: None,
            inst: ComponentInstSlot::new(),
        })
    }

    // ── Key helper ───────────────────────────────────────────────────────────

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

// ─── NodeRef: a live-DOM-node ref; `sync` keeps a JS-side `{ current }` ref (or callback ref) in sync with the reconciler ───
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
    pub fn current(&self) -> Option<web_sys::Node> {
        self.node.borrow().clone()
    }
    pub(crate) fn set(&self, node: Option<web_sys::Node>) {
        *self.node.borrow_mut() = node.clone();
        if let Some(f) = &self.sync {
            f(node);
        }
    }
}

// ─── ComponentFn — an `Fn(Props) -> VNode` wrapped in Rc so it's Clone ───
#[derive(Clone)]
pub struct ComponentFn(pub std::rc::Rc<dyn Fn(Props) -> VNode>);

impl fmt::Debug for ComponentFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<ComponentFn>")
    }
}

impl ComponentFn {
    pub fn new(f: impl Fn(Props) -> VNode + 'static) -> Self {
        ComponentFn(std::rc::Rc::new(f))
    }
    pub fn call(&self, props: Props) -> VNode {
        (self.0)(props)
    }
}

// ─── ComponentInstSlot: handle the diff engine uses to stash/retrieve a Component vnode's live ComponentInst so hooks persist across re-renders ───
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
// Static attrs go into the Template, dynamic values into holes; finalizing registers a template id so identical shapes skip re-parsing.
pub struct ElementBuilder {
    template: Template,
    holes: Vec<PropVal>,
    props: Props,
    children: Vec<VNode>,
    key: Key,
    ref_: Option<NodeRef>,
}

impl ElementBuilder {
    pub fn new(tag: String) -> Self {
        ElementBuilder {
            template: Template::new(&tag),
            holes: Vec::new(),
            props: Vec::new(),
            children: Vec::new(),
            key: None,
            ref_: None,
        }
    }

    // ── Static attributes (baked into template) ──────────────────────────────

    /// Set a static string attribute (known at "compile time").
    pub fn attr_static(mut self, name: &str, value: &str) -> Self {
        self.template.static_attrs.push((name.to_string(), value.to_string()));
        self.props.push((name.to_string(), PropVal::Str(value.to_string())));
        self
    }

    /// Set any attribute with a dynamic value (stored in holes).
    pub fn attr(mut self, name: impl Into<String>, value: impl Into<PropVal>) -> Self {
        let name = name.into();
        let val  = value.into();
        self.template.hole_names.push(name.clone());
        self.holes.push(val.clone());
        self.props.push((name, val));
        self
    }

    /// Convenience: `class` attribute (dynamic value).
    pub fn class(self, value: impl Into<PropVal>) -> Self {
        self.attr("className", value)
    }

    /// Convenience: `id` attribute.
    pub fn id(self, value: impl Into<PropVal>) -> Self {
        self.attr("id", value)
    }

    /// Convenience: `style` attribute string.
    pub fn style_str(self, value: impl Into<String>) -> Self {
        self.attr("style", PropVal::Str(value.into()))
    }

    /// Set an event handler.
    /// `name` should be the React-style camelCase name, e.g. "onClick".
    pub fn on(self, name: impl Into<String>, handler: js_sys::Function) -> Self {
        self.attr(name.into(), PropVal::Callback(JsCallback(handler)))
    }

    /// Set a `key` (for keyed reconciliation).
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.key = Some(k.into());
        self
    }

    /// Attach a NodeRef.
    pub fn ref_(mut self, r: NodeRef) -> Self {
        self.ref_ = Some(r);
        self
    }

    /// Set `dangerouslySetInnerHTML` (raw HTML string).
    pub fn inner_html(self, html: impl Into<String>) -> Self {
        self.attr("dangerouslySetInnerHTML.__html", html.into())
    }

    // ── Children ─────────────────────────────────────────────────────────────

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

    // ── Finalize ─────────────────────────────────────────────────────────────

    pub fn build(self) -> VNode {
        VNode::new(VNodeInner::Element {
            template: self.template,
            holes: self.holes,
            props: self.props,
            children: Children(self.children),
            key: self.key,
            ref_: self.ref_,
        })
    }
}

/// Allow `.build()` to be omitted in most contexts by implementing `Into<VNode>`.
impl From<ElementBuilder> for VNode {
    fn from(b: ElementBuilder) -> VNode { b.build() }
}

// ─── `IntoVNode` trait — lets `"div".v()` work ───
pub trait IntoVNode {
    /// Start building an element with this tag name.
    fn v(self) -> ElementBuilder;
    /// Create a text node directly.
    fn t(self) -> VNode;
}

impl IntoVNode for &str {
    fn v(self) -> ElementBuilder { ElementBuilder::new(self.to_string()) }
    fn t(self) -> VNode { VNode::text(self) }
}

impl IntoVNode for String {
    fn v(self) -> ElementBuilder { ElementBuilder::new(self) }
    fn t(self) -> VNode { VNode::text(self) }
}

// ─── html!() macro: parses a JSX-ish template at compile time via declarative macros ───
// The parsed skeleton is cached as a static Template; only hole values change between renders.

/// Lightweight compile-time html macro; creates a VNode element with
/// static + dynamic props, e.g. `html!(div class={cls} => [child])`.
#[macro_export]
macro_rules! html {
    // ── Element with children: html!(tag attr=val … => [children…]) ─────────
    ($tag:ident $($attr:ident = $val:expr)* => [$($child:expr),* $(,)?]) => {{
        let mut builder = $crate::vnode::VNode::tag(stringify!($tag));
        $(
            builder = builder.attr(stringify!($attr), $val);
        )*
        $(
            builder = builder.child($child.into());
        )*
        builder.build()
    }};

    // ── Self-closing: html!(tag attr=val …) ─────────────────────────────────
    ($tag:ident $($attr:ident = $val:expr)*) => {{
        let mut builder = $crate::vnode::VNode::tag(stringify!($tag));
        $(
            builder = builder.attr(stringify!($attr), $val);
        )*
        builder.build()
    }};

    // ── Text shorthand: html!("literal string") ──────────────────────────────
    ($text:literal) => {
        $crate::vnode::VNode::text($text)
    };
}

// ─── Template::parse: runtime parser for the JS-side html() tagged-template API ───
// Returns (Template, initial_holes); the Template is cached by id so later renders only re-evaluate dynamic expressions.
impl Template {
    /// Parse a JSX-ish string (without dynamic values) into a static Template.
    /// `static_src` must be the concatenation of the tagged-template statics only.
    pub fn parse(static_src: &str) -> Result<Template, String> {
        // Parse via DOMParser, with dynamic slots replaced by placeholder
        // attrs beforehand and matched back by index; cached in TEMPLATE_REGISTRY.
        let window = web_sys::window().ok_or("no window")?;
        let _doc = window.document().ok_or("no document")?;

        // Use DOMParser
        let parser = web_sys::DomParser::new().map_err(|e| format!("{:?}", e))?;
        let parsed = parser
            .parse_from_string(&format!("<root>{}</root>", static_src), web_sys::SupportedType::TextHtml)
            .map_err(|e| format!("{:?}", e))?;

        let root = parsed
            .body()
            .ok_or("no body")?
            .first_child()
            .ok_or("no root child")?;

        let first_elem = root.first_child().ok_or("empty template")?;
        let elem: web_sys::Element = first_elem.dyn_into()
            .map_err(|_| "root child is not an element")?;

        let tag = elem.local_name();
        let mut static_attrs = Vec::new();

        let attrs = elem.attributes();
        for i in 0..attrs.length() {
            if let Some(a) = attrs.item(i) {
                static_attrs.push((a.name(), a.value()));
            }
        }

        Ok(Template {
            id: next_id(),
            tag,
            static_attrs,
            hole_names: Vec::new(), // filled by caller
            has_key: false,
            ns: None,
        })
    }
}

// ─── Global template registry (id → Template) ───
use once_cell::sync::Lazy;
use std::sync::Mutex;

pub static TEMPLATE_REGISTRY: Lazy<Mutex<HashMap<u64, Template>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register_template(t: Template) -> u64 {
    let id = t.id;
    if let Ok(mut reg) = TEMPLATE_REGISTRY.lock() {
        reg.entry(id).or_insert(t);
    }
    id
}

pub fn get_template(id: u64) -> Option<Template> {
    TEMPLATE_REGISTRY.lock().ok()?.get(&id).cloned()
}
