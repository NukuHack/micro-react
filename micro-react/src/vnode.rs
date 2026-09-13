//! `VNode` tree + fluent element builder. A Template stores only the static
//! skeleton (tag + static attrs) of an Element; dynamic values live in
//! `holes`/`props` and are resolved at diff time.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use wasm_bindgen::JsValue;
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
			(Self::Str(a), Self::Str(b)) => a == b,
			(Self::Bool(a), Self::Bool(b)) => a == b,
			(Self::Num(a), Self::Num(b)) => a == b,
			(Self::Null, Self::Null) => true,
			(Self::Callback(a), Self::Callback(b)) => js_sys::Object::is(a.as_ref(), b.as_ref()),
			(Self::Js(a), Self::Js(b)) => js_sys::Object::is(a, b),
			_ => false,
		}
	}
}

impl From<&str> for PropVal {
	fn from(s: &str) -> Self {
		Self::Str(s.to_string())
	}
}
impl From<String> for PropVal {
	fn from(s: String) -> Self {
		Self::Str(s)
	}
}
impl From<bool> for PropVal {
	fn from(b: bool) -> Self {
		Self::Bool(b)
	}
}
impl From<f64> for PropVal {
	fn from(n: f64) -> Self {
		Self::Num(n)
	}
}
impl From<i32> for PropVal {
	fn from(n: i32) -> Self {
		Self::Num(f64::from(n))
	}
}
impl From<usize> for PropVal {
	fn from(n: usize) -> Self {
		Self::Num(n as f64)
	}
}
impl From<JsCallback> for PropVal {
	fn from(f: JsCallback) -> Self {
		Self::Callback(f)
	}
}

/// A JS function value used for event handlers.
#[derive(Clone, Debug)]
pub struct JsCallback(pub js_sys::Function);
impl AsRef<JsValue> for JsCallback {
	fn as_ref(&self) -> &JsValue {
		self.0.as_ref()
	}
}
impl From<js_sys::Function> for JsCallback {
	fn from(f: js_sys::Function) -> Self {
		Self(f)
	}
}
impl From<&js_sys::Function> for JsCallback {
	fn from(f: &js_sys::Function) -> Self {
		Self(f.clone())
	}
}

// ─── Template — the static part of an Element, cached on the vnode ───
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
	pub id: u64,
	pub tag: String,
}

impl Template {
	pub fn new(tag: impl Into<String>) -> Self {
		Self { id: next_id(), tag: tag.into() }
	}
}

// ─── Props — a thin ordered map ───
pub type Props = Vec<(String, PropVal)>;
pub type Key = Option<String>;

// ─── Children helper ───
#[derive(Clone, Debug)]
pub struct Children(pub Vec<VNode>);

impl Children {
	#[must_use]
	pub const fn len(&self) -> usize {
		self.0.len()
	}
	#[must_use]
	pub const fn is_empty(&self) -> bool {
		self.0.is_empty()
	}
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
	Fragment { children: Children, key: Key },
	/// A function component call.
	Component {
		name: String,
		render: ComponentFn,
		props: Props,
		/// Raw JSX children, kept alongside (not only inside) `render`'s
		/// closure so callers like `Routes` can walk a component tree
		/// (e.g. nested `<Route>`s) without invoking any component function.
		children: Vec<VNode>,
		key: Key,
		/// Holds the live `ComponentInst` once mounted, so the next render
		/// can reuse it and let hooks survive across re-renders.
		inst: ComponentInstSlot,
	},
	/// Portal — render children into a different DOM container.
	Portal { container: Element, children: Children },
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
	pub(crate) dom_node: Option<web_sys::Node>,
	pub(crate) depth: u32,
	pub(crate) order_index: i32,
	pub(crate) flags: u8,
}

pub const FLAG_INSERT: u8 = 1 << 0;
pub const FLAG_MATCHED: u8 = 1 << 1;

impl VNode {
	fn new(inner: VNodeInner) -> Self {
		Self { inner, original: next_id(), dom_node: None, depth: 0, order_index: -1, flags: 0 }
	}

	#[must_use]
	pub fn null() -> Self {
		Self::new(VNodeInner::Null)
	}

	pub fn text(s: impl Into<String>) -> Self {
		Self::new(VNodeInner::Text(s.into()))
	}

	/// Start building an element: `VNode::tag("div")`.
	pub fn tag(tag: impl Into<String>) -> ElementBuilder {
		ElementBuilder::new(&tag.into())
	}

	#[must_use]
	pub fn fragment(children: Vec<Self>) -> Self {
		Self::new(VNodeInner::Fragment { children: Children(children), key: None })
	}

	pub fn fragment_keyed(key: impl Into<String>, children: Vec<Self>) -> Self {
		Self::new(VNodeInner::Fragment { children: Children(children), key: Some(key.into()) })
	}

	/// Render `children` into a different DOM `container` than the one the
	/// portal vnode itself sits in. No JS-facing binding constructs this
	/// yet (see `bindings.rs`'s `create_element`), so Rust callers/tests
	/// build it directly via this constructor.
	#[must_use]
	pub fn portal(container: Element, children: Vec<Self>) -> Self {
		Self::new(VNodeInner::Portal { container, children: Children(children) })
	}

	pub fn component(name: impl Into<String>, render: ComponentFn, props: Props) -> Self {
		Self::new(VNodeInner::Component { name: name.into(), render, props, children: Vec::new(), key: None, inst: ComponentInstSlot::new() })
	}

	/// Attaches raw JSX children to a `Component` vnode after construction
	/// (mirrors `with_key`). Used by `createElement`/`html!` so callers like
	/// `Routes` can walk a component tree (e.g. nested `<Route>`s) without
	/// invoking any component function.
	#[must_use]
	pub fn with_children(mut self, children: Vec<Self>) -> Self {
		if let VNodeInner::Component { children: c, .. } = &mut self.inner {
			*c = children;
		}
		self
	}

	/// Set this vnode's key after construction. Needed for `Component`
	/// vnodes, which have no builder step to pass a key through, so a
	/// `key` prop (e.g. `h(ErrorBoundary, { key })`) would otherwise be dropped.
	#[must_use]
	pub fn with_key(mut self, key: Option<String>) -> Self {
		match &mut self.inner {
			VNodeInner::Element { key: k, .. } | VNodeInner::Fragment { key: k, .. } | VNodeInner::Component { key: k, .. } => *k = key,
			_ => {}
		}
		self
	}

	#[must_use]
	pub fn key(&self) -> Option<&str> {
		match &self.inner {
			VNodeInner::Element { key, .. } | VNodeInner::Fragment { key, .. } | VNodeInner::Component { key, .. } => key.as_deref(),
			_ => None,
		}
	}

	#[must_use]
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
	#[must_use]
	pub fn new() -> Self {
		Self { node: std::rc::Rc::new(std::cell::RefCell::new(None)), sync: None }
	}
	/// Create a `NodeRef` that calls `sync` (with the new node, or `None` on
	/// unmount) every time the DOM node it's attached to changes.
	pub fn with_sync(sync: impl Fn(Option<web_sys::Node>) + 'static) -> Self {
		Self { node: std::rc::Rc::new(std::cell::RefCell::new(None)), sync: Some(std::rc::Rc::new(sync)) }
	}
	pub(crate) fn set(&self, node: Option<web_sys::Node>) {
		self.node.borrow_mut().clone_from(&node);
		if let Some(f) = &self.sync {
			f(node);
		}
	}
}

// ─── ComponentFn — an `Fn(Props) -> Result<VNode, JsValue>` wrapped in Rc so it's Clone ───
// A component can "throw" by returning `Err`, which propagates up to the
// nearest ErrorBoundary (see `diff::diff_component` /
// `hooks::report_to_nearest_boundary`), mirroring React's fiber walk.
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
		Self(std::rc::Rc::new(f))
	}
	/// Convenience constructor for the common case of a component that
	/// never throws.
	pub fn infallible(f: impl Fn(Props) -> VNode + 'static) -> Self {
		Self(std::rc::Rc::new(move |props| Ok(f(props))))
	}
	pub fn call(&self, props: Props) -> Result<VNode, JsValue> {
		(self.0)(props)
	}
}

// ─── ComponentInstSlot: where the diff engine stashes a Component vnode's live ComponentInst so hooks persist across re-renders ───
#[derive(Clone, Default)]
pub struct ComponentInstSlot(pub std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<crate::hooks::ComponentInst>>>>>);

impl ComponentInstSlot {
	#[must_use]
	pub fn new() -> Self {
		Self(std::rc::Rc::new(std::cell::RefCell::new(None)))
	}
}

impl fmt::Debug for ComponentInstSlot {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "<ComponentInstSlot>")
	}
}

// ─── ElementBuilder — fluent builder that produces a VNode::Element ───
#[derive(Clone, Debug)]
pub struct ElementBuilder {
	template: Template,
	props: Props,
	children: Vec<VNode>,
	key: Key,
	ref_: Option<NodeRef>,
}

impl ElementBuilder {
	#[must_use]
	pub fn new(tag: &str) -> Self {
		Self { template: Template::new(tag), props: Vec::new(), children: Vec::new(), key: None, ref_: None }
	}

	/// Set any attribute, e.g. `.attr("className", "foo")`.
	#[must_use]
	pub fn attr(mut self, name: impl Into<String>, value: impl Into<PropVal>) -> Self {
		self.props.push((name.into(), value.into()));
		self
	}

	/// Set an event handler. `name` should be React-style camelCase, e.g. "onClick".
	#[must_use]
	pub fn on(self, name: impl Into<String>, handler: js_sys::Function) -> Self {
		self.attr(name.into(), PropVal::Callback(JsCallback(handler)))
	}

	/// Set a `key` for keyed reconciliation.
	#[must_use]
	pub fn key(mut self, k: impl Into<String>) -> Self {
		self.key = Some(k.into());
		self
	}

	/// Attach a `NodeRef`.
	#[must_use]
	pub fn ref_(mut self, r: NodeRef) -> Self {
		self.ref_ = Some(r);
		self
	}

	#[must_use]
	pub fn child(mut self, c: VNode) -> Self {
		self.children.push(c);
		self
	}

	#[must_use]
	pub fn children(mut self, cs: impl IntoIterator<Item = VNode>) -> Self {
		self.children.extend(cs);
		self
	}

	#[must_use]
	pub fn text(self, t: impl Into<String>) -> Self {
		self.child(VNode::text(t))
	}

	#[must_use]
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
	fn from(b: ElementBuilder) -> Self {
		b.build()
	}
}
