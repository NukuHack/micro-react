//! wasm-bindgen public surface — the JS-callable exports.

use js_sys::{Array, Error, Function, Object, Reflect, TypeError};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::Element;

use crate::context::use_context;
use crate::hooks::{DepVal, current_inst, use_id, use_layout_effect, use_memo, use_reducer_cell, use_state_cell};
use crate::render::Root;
use crate::vnode::{ComponentFn, JsCallback, NodeRef, PropVal, Props, VNode, VNodeInner};

// Setter-closure cache: each hook has one stable backing cell, so cache
// its JS setter (keyed by cell address) instead of leaking a new closure
// on every render.
thread_local! {
	static SETTER_CACHE: RefCell<std::collections::HashMap<usize, JsValue>> =
		RefCell::new(std::collections::HashMap::new());
}

// ─── VNode boundary-crossing slot map ───
// Raw Box::into_raw/from_raw pointers to JS are unsound (double-read ->
// double-free). VNodes instead live in a slot map keyed by opaque `u64`
// id; remove() is safe to call twice, turning a stale read into a no-op.
thread_local! {
	static VNODE_STORE: RefCell<std::collections::HashMap<u64, VNode>> =
		RefCell::new(std::collections::HashMap::new());
	static VNODE_NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
	// Coarse leak guard: past this size we can't safely reclaim unreachable
	// entries, so we surface the growth loudly instead of failing silently.
	static VNODE_LEAK_WARNED: RefCell<bool> = const { RefCell::new(false) };
}

const VNODE_STORE_WARN_THRESHOLD: usize = 10_000;

/// Property name used to tag a `forwardRef`-wrapped render function. See
/// `js_forward_ref` for why this needs no new element-type kind: the
/// function is still dispatched through the ordinary `type_.is_function()`
/// path in `create_element`/`html_template::render_element`, just called
/// with the caller's `ref` as a second argument when this flag is set.
pub(crate) const FORWARD_REF_MARKER: &str = "__mrForwardRef";

fn next_vnode_id() -> u64 {
	VNODE_NEXT_ID.with(|c| {
		let mut c = c.borrow_mut();
		let id = *c;
		// Wrap away from 0 so 0 can be reserved as "invalid" if ever needed.
		*c = if id == u64::MAX { 1 } else { id + 1 };
		id
	})
}

fn store_vnode(vnode: VNode) -> u64 {
	let id = next_vnode_id();
	VNODE_STORE.with(|s| {
		let mut s = s.borrow_mut();
		s.insert(id, vnode);
		if s.len() > VNODE_STORE_WARN_THRESHOLD {
			let already_warned = VNODE_LEAK_WARNED.with(|w| {
				let mut w = w.borrow_mut();
				let prev = *w;
				*w = true;
				prev
			});
			if !already_warned {
				crate::console_error!(
					"[micro-react] vnode store has grown past {} unconsumed entries — \
                     some vnode returned to JS is never reaching render()/reconciliation \
                     (an early return, an unrendered branch, or a JS-side error before use). \
                     This is a memory leak; check for vnodes computed and then discarded.",
					VNODE_STORE_WARN_THRESHOLD
				);
			}
		}
	});
	id
}

/// Take ownership of the vnode for `id` out of the store. Safe to call more
/// than once for the same id: the first call consumes it, every later call
/// (double-consume, unknown id) returns `None` instead of touching freed
/// memory, so the caller can degrade to a null vnode rather than hit UB.
fn take_vnode(id: u64) -> Option<VNode> {
	VNODE_STORE.with(|s| s.borrow_mut().remove(&id))
}

/// Clone the vnode for `id` without removing it from the store. Unlike
/// `take_vnode`, safe to call any number of times for the same id — for
/// JS-side values that are legitimately inspected across multiple renders
/// (e.g. route config sitting in a `props` object that outlives a single
/// render, re-read whenever internal state re-renders the component) rather
/// than consumed exactly once as normal render output.
fn peek_vnode(id: u64) -> Option<VNode> {
	VNODE_STORE.with(|s| s.borrow().get(&id).cloned())
}

/// Safely turn an arbitrary thrown JS value into a `String`. Goes through
/// `as_string()`/`.message` first since wasm-bindgen's `JsString::from`
/// unchecked-converts and can panic later on a non-string throw.
pub(crate) fn stringify_thrown(v: &JsValue) -> String {
	if let Some(s) = v.as_string() {
		return s;
	}
	if let Some(msg) = Reflect::get(v, &"message".into()).ok().and_then(|m| m.as_string()) {
		return msg;
	}
	format!("{:?}", v)
}

fn cell_key(cell: &Rc<RefCell<Box<dyn std::any::Any>>>) -> usize {
	Rc::as_ptr(cell) as usize
}

/// Returns the cached JS setter for `cell` if one exists, otherwise builds
/// it via `build`, caches it, and returns it.
fn cached_setter(cell: &Rc<RefCell<Box<dyn std::any::Any>>>, build: impl FnOnce() -> JsValue) -> JsValue {
	let key = cell_key(cell);
	if let Some(existing) = SETTER_CACHE.with(|c| c.borrow().get(&key).cloned()) {
		return existing;
	}
	let built = build();
	SETTER_CACHE.with(|c| {
		c.borrow_mut().insert(key, built.clone());
	});
	built
}

/// Called on unmount so a hook's cached setter doesn't linger forever, and
/// so a later unrelated allocation reusing the same freed address can't
/// collide with a stale cache entry.
pub(crate) fn evict_setter_cache(cell: &Rc<RefCell<Box<dyn std::any::Any>>>) {
	let key = cell_key(cell);
	SETTER_CACHE.with(|c| {
		c.borrow_mut().remove(&key);
	});
}

// ─── Root handle (JS-visible) ───

#[wasm_bindgen]
pub struct JsRoot {
	inner: RefCell<Root>,
}

#[wasm_bindgen]
impl JsRoot {
	pub fn render(&self, vnode: JsValue) -> Result<(), JsValue> {
		let vnode = js_to_vnode(&vnode)?;
		crate::console_log!("[micro-react] root render()");
		self.inner.borrow_mut().render(vnode)
	}
	pub fn unmount(&self) {
		crate::console_log!("[micro-react] root unmount()");
		self.inner.borrow_mut().unmount();
	}
}

#[wasm_bindgen(js_name = render)]
pub fn render(vnode: JsValue, container: Element) -> Result<JsRoot, JsValue> {
	let vnode = js_to_vnode(&vnode)?;
	crate::console_log!("[micro-react] render() mounting to container");
	let mut root = Root::new(container);
	root.render(vnode)?;
	Ok(JsRoot { inner: RefCell::new(root) })
}

// ─── createElement ───

#[wasm_bindgen(js_name = createElement)]
pub fn create_element(type_: &JsValue, props: &JsValue, children: JsValue) -> Result<JsValue, JsValue> {
	// wasm-bindgen export shims have fixed arity, so a variadic JS call
	// truncates children after the 1st (the `h` wrapper works around this).
	// Treat any lone non-array value as a one-element children array.
	let children: Array = match children.dyn_into::<Array>() {
		Ok(arr) => arr,
		Err(orig) => {
			if orig.is_null() || orig.is_undefined() {
				Array::new()
			} else {
				let arr = Array::new();
				arr.push(&orig);
				arr
			}
		}
	};
	let children: &Array = &children;
	let key = if props.is_object() {
		Reflect::get(props, &"key".into()).ok().and_then(|v| {
			// Stringify any primitive key type (matching JS/React's own
			// coercion), not just strings, or numeric/boolean keys would
			// collapse to "no key" and break keyed reconciliation.
			if v.is_undefined() || v.is_null() {
				None
			} else if let Some(s) = v.as_string() {
				Some(s)
			} else if let Some(n) = v.as_f64() {
				Some(n.to_string())
			} else {
				v.as_bool().map(|b| b.to_string())
			}
		})
	} else {
		None
	};

	// Extract `ref` and turn it into a NodeRef whose `sync` callback writes
	// back into the JS ref object (or calls the callback-ref function).
	// Also keep the raw, unconverted value around: a `forwardRef`-tagged
	// component wants the caller's actual ref (callback or `{ current }`
	// object) handed to it as a plain argument, not a NodeRef.
	let raw_ref: JsValue = if props.is_object() { Reflect::get(props, &"ref".into()).unwrap_or(JsValue::UNDEFINED) } else { JsValue::UNDEFINED };
	let node_ref: Option<NodeRef> = js_ref_to_node_ref(&raw_ref);

	let mut rust_props: Props = Vec::new();
	let dummy = js_sys::Object::new();
	if props.is_object() && !props.is_null() {
		let obj = props.dyn_ref::<js_sys::Object>().unwrap_or(&dummy);
		let keys = js_sys::Object::keys(obj);
		for k in keys.iter() {
			let k_str = k.as_string().unwrap_or_default();
			if k_str == "key" || k_str == "ref" {
				continue;
			}
			let val = Reflect::get(props, &k)?;
			rust_props.push((k_str, js_val_to_prop_val(&val)));
		}
	}

	let mut child_vnodes: Vec<VNode> = Vec::new();
	for child in children.iter() {
		if let Ok(vn) = js_to_vnode(&child)
			&& !matches!(vn.inner, VNodeInner::Null)
		{
			child_vnodes.push(vn);
		}
	}

	// Special Fragment symbol
	let is_fragment = {
		let frag_sym = js_sys::Symbol::for_("MicroReact.Fragment");
		type_.is_symbol() && js_sys::Object::is(type_, frag_sym.as_ref())
	};

	let vnode = if is_fragment {
		VNode::fragment(child_vnodes)
	} else if let Some(tag) = type_.as_string() {
		let mut builder = VNode::tag(tag);
		for (k, v) in rust_props {
			builder = builder.attr(k, v);
		}
		if let Some(k) = key {
			builder = builder.key(k);
		}
		if let Some(r) = node_ref {
			builder = builder.ref_(r);
		}
		builder.children(child_vnodes).build()
	} else if type_.is_function() {
		let fn_: Function = type_.clone().dyn_into().expect("type_.is_function() checked above");
		let fn_name = Reflect::get(&fn_, &"name".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "Anonymous".to_string());
		let is_forward_ref = Reflect::get(&fn_, &FORWARD_REF_MARKER.into()).map(|v| v.is_truthy()).unwrap_or(false);

		let children_for_fn = child_vnodes.clone();
		let raw_ref_for_fn = raw_ref.clone();
		VNode::component(
			fn_name,
			ComponentFn::new(move |props| {
				let js_props = props_to_js_object(&props);
				if !children_for_fn.is_empty() {
					let children_val = children_to_js(&children_for_fn);
					let _ = Reflect::set(&js_props, &"children".into(), &children_val);
				}
				let result =
					if is_forward_ref { fn_.call2(&JsValue::NULL, &js_props, &raw_ref_for_fn) } else { fn_.call1(&JsValue::NULL, &js_props) };
				match result {
					Ok(result) => Ok(js_to_vnode(&result).unwrap_or_else(|_| VNode::null())),
					// A thrown JS exception becomes a plain `Err`, the same path a
					// Rust component uses to "throw" directly; diff_component /
					// rerender_component (diff.rs) walk up to the nearest boundary.
					Err(err) => Err(err),
				}
			}),
			rust_props,
		)
		.with_children(child_vnodes)
		.with_key(key)
	} else {
		VNode::null()
	};

	vnode_to_js(vnode)
}

/// Convert a JS `ref` (callback function or `{ current }` object) into a
/// `NodeRef` whose `sync` callback keeps it updated with the live DOM node.
pub(crate) fn js_ref_to_node_ref(ref_val: &JsValue) -> Option<NodeRef> {
	if ref_val.is_null() || ref_val.is_undefined() {
		return None;
	}

	if ref_val.is_function() {
		let f: Function = ref_val.clone().dyn_into().ok()?;
		return Some(NodeRef::with_sync(move |node: Option<web_sys::Node>| {
			let arg: JsValue = node.map(Into::into).unwrap_or(JsValue::NULL);
			let _ = f.call1(&JsValue::NULL, &arg);
		}));
	}

	if ref_val.is_object() {
		let obj = ref_val.clone();
		return Some(NodeRef::with_sync(move |node: Option<web_sys::Node>| {
			let val: JsValue = node.map(Into::into).unwrap_or(JsValue::NULL);
			let _ = Reflect::set(&obj, &"current".into(), &val);
		}));
	}

	None
}

/// Returns the Symbol used as the Fragment type.
#[wasm_bindgen(js_name = getFragment)]
pub fn get_fragment() -> JsValue {
	js_sys::Symbol::for_("MicroReact.Fragment").into()
}

// ─── Hooks — exposed as bare wasm-bindgen functions ───
// Each JS hook binding delegates to the Rust implementation. `CURRENT_INST`
// is set by the diff engine's `with_inst` around each component render.

/// `useState(initialValue)` — returns `[value, setter]`. Supports functional
/// updaters (`setState(prev => next)`), resolved against the hook's live
/// cell at call time so they never see a stale snapshot.
#[wasm_bindgen(js_name = useState)]
pub fn js_use_state(initial: JsValue) -> Array {
	let (value, cell, setter) = use_state_cell(initial);

	let js_fn = cached_setter(&cell, || {
		let cell = cell.clone();
		Closure::wrap(Box::new(move |next: JsValue| {
			let resolved = if next.is_function() {
				let f: Function = next.unchecked_ref::<Function>().clone();
				let cur = cell.borrow().downcast_ref::<JsValue>().cloned().unwrap_or(JsValue::UNDEFINED);
				f.call1(&JsValue::NULL, &cur).unwrap_or(JsValue::UNDEFINED)
			} else {
				next
			};
			setter(resolved);
		}) as Box<dyn Fn(JsValue)>)
		.into_js_value()
	});

	let arr = Array::new();
	arr.push(&value);
	arr.push(&js_fn);
	arr
}

/// `useReducer(reducer, initialState)` — returns `[state, dispatch]`.
#[wasm_bindgen(js_name = useReducer)]
pub fn js_use_reducer(reducer: &Function, initial: JsValue) -> Array {
	let reducer = reducer.clone();

	let (state, cell, dispatch) =
		use_reducer_cell::<JsValue, JsValue>(move |state, action| reducer.call2(&JsValue::NULL, &state, &action).unwrap_or(state), initial);

	let js_dispatch = cached_setter(&cell, || {
		let dispatch = dispatch.clone();
		Closure::wrap(Box::new(move |action: JsValue| {
			dispatch(action);
		}) as Box<dyn Fn(JsValue)>)
		.into_js_value()
	});

	let arr = Array::new();
	arr.push(&state);
	arr.push(&js_dispatch);
	arr
}

/// `useEffect(callback, deps?)` — callback returns an optional cleanup function.
#[wasm_bindgen(js_name = useEffect)]
pub fn js_use_effect(callback: &Function, deps: JsValue) {
	let callback = callback.clone();
	let rust_deps = js_deps_to_rust(&deps);

	crate::hooks::use_effect(
		move || {
			let result = callback.call0(&JsValue::NULL).ok();
			let cleanup_fn: Option<Function> = result.and_then(|v| v.dyn_into().ok());
			Box::new(move || {
				if let Some(f) = cleanup_fn {
					let _ = f.call0(&JsValue::NULL);
				}
			}) as Box<dyn FnOnce()>
		},
		rust_deps,
	);
}

/// `useLayoutEffect(callback, deps?)` — fires synchronously after DOM updates.
#[wasm_bindgen(js_name = useLayoutEffect)]
pub fn js_use_layout_effect(callback: &Function, deps: JsValue) {
	let callback = callback.clone();
	let rust_deps = js_deps_to_rust(&deps);
	use_layout_effect(
		move || {
			let result = callback.call0(&JsValue::NULL).ok();
			let cleanup_fn: Option<Function> = result.and_then(|v| v.dyn_into().ok());
			Box::new(move || {
				if let Some(f) = cleanup_fn {
					let _ = f.call0(&JsValue::NULL);
				}
			}) as Box<dyn FnOnce()>
		},
		rust_deps,
	);
}

/// `useRef(initialValue?)` — returns a `{ current: value }` JS object,
/// stable across renders. Backed by `use_ref_cell`, a plain hook slot that
/// never touches the scheduler, so (unlike a `useState`-based
/// implementation) calling this never triggers an extra re-render.
#[wasm_bindgen(js_name = useRef)]
pub fn js_use_ref(initial: JsValue) -> Object {
	let cell = crate::hooks::use_ref_cell(|| {
		let obj = Object::new();
		Reflect::set(&obj, &"current".into(), &initial).expect("setting a plain-object property cannot fail");
		let obj_val: JsValue = obj.into();
		obj_val
	});

	cell.borrow().downcast_ref::<JsValue>().cloned().unwrap_or(JsValue::UNDEFINED).dyn_into::<Object>().unwrap_or_else(|_| Object::new())
}

/// `useMemo(factory, deps)` — returns a memoised value.
#[wasm_bindgen(js_name = useMemo)]
pub fn js_use_memo(factory: &Function, deps: JsValue) -> JsValue {
	let factory = factory.clone();
	let rust_deps = js_deps_to_rust(&deps);
	use_memo(move || factory.call0(&JsValue::NULL).unwrap_or(JsValue::UNDEFINED), rust_deps)
}

/// `useCallback(fn, deps)` — returns a stable function reference.
#[wasm_bindgen(js_name = useCallback)]
pub fn js_use_callback(f: &Function, deps: JsValue) -> JsValue {
	let f = f.clone();
	let rust_deps = js_deps_to_rust(&deps);
	use_memo(move || -> JsValue { f.into() }, rust_deps)
}

/// `useId()` — returns a stable unique string id.
#[wasm_bindgen(js_name = useId)]
pub fn js_use_id() -> String {
	use_id()
}

// ─── Context API: a plain JS object shaped like { Provider, Consumer, useContext, _id } since T can't cross the JS boundary generically ───

/// `createContext(defaultValue)` — returns a JS context object.
#[wasm_bindgen(js_name = createContext)]
pub fn js_create_context(default_value: JsValue) -> Result<JsValue, JsValue> {
	use crate::context::{Context, record_create_context_call};

	let call_count = record_create_context_call();
	if call_count > 1 {
		crate::console_warn!(
			"[micro-react] createContext has been called {} times. Each call leaks a Box and 3 \
			 Closures for the lifetime of the page, which is fine if this is {} distinct contexts \
			 declared at module scope, but leaks unboundedly if createContext is being called from \
			 inside a component body on every render — call it once per context outside your component.",
			call_count,
			call_count
		);
	}

	// Leaked into a 'static reference: safe, the WASM module lives forever
	// and the context is never freed.
	let ctx: &'static Context<JsValue> = Box::leak(Box::new(Context::new(default_value.clone())));
	let ctx_id = ctx.id;

	let obj = Object::new();
	Reflect::set(&obj, &"_id".into(), &JsValue::from_f64(ctx_id as f64))?;

	let ctx_provider = ctx;
	let provider_fn = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
		let value = Reflect::get(&props, &"value".into()).unwrap_or(default_value.clone());
		ctx_provider.set_value(value);
		Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL)
	}) as Box<dyn Fn(JsValue) -> JsValue>);
	Reflect::set(&obj, &"Provider".into(), provider_fn.as_ref())?;
	provider_fn.forget();

	let ctx_consumer = ctx;
	let consumer_fn = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
		let value = ctx_consumer.current_value();
		let children = Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL);
		if children.is_function() {
			let f: Function = children.dyn_into().expect("children.is_function() checked above");
			f.call1(&JsValue::NULL, &value).unwrap_or(JsValue::NULL)
		} else {
			JsValue::NULL
		}
	}) as Box<dyn Fn(JsValue) -> JsValue>);
	Reflect::set(&obj, &"Consumer".into(), consumer_fn.as_ref())?;
	consumer_fn.forget();

	let ctx_hook = ctx;
	let use_ctx_fn = Closure::wrap(Box::new(move || -> JsValue { use_context(ctx_hook) }) as Box<dyn Fn() -> JsValue>);
	Reflect::set(&obj, &"useContext".into(), use_ctx_fn.as_ref())?;
	use_ctx_fn.forget();

	Ok(obj.into())
}

#[wasm_bindgen(js_name = useContext)]
pub fn js_use_context(input: &JsValue) -> Result<JsValue, JsValue> {
	// 1. Replicate the `input?.useContext` check
	if input.is_null() || input.is_undefined() {
		return Err(TypeError::new("useContext: input must have a useContext method").into());
	}

	let use_context_prop = Reflect::get(input, &JsValue::from_str("useContext"))?;
	if !use_context_prop.is_function() {
		return Err(TypeError::new("useContext: input must have a useContext method").into());
	}

	let func: Function = use_context_prop.unchecked_into();

	// 2. Replicate the `try { ... } catch (error)` block
	match func.call0(input) {
		Ok(val) => Ok(val),
		Err(err) => {
			// Attempt to extract the error message string from the caught JS error
			let err_message = err
				.as_string()
				.or_else(|| Reflect::get(&err, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
				.unwrap_or_else(|| "Unknown error".to_string());

			// Throw the wrapped Error back to JavaScript
			Err(Error::new(&format!("useContext: failed to execute useContext on input object - {}", err_message)).into())
		}
	}
}

// ─── memo() HOC ───

/// `memo(Component, compare?)` — wraps a component function to skip
/// re-renders when props are shallowly equal (or `compare()` returns true).
#[wasm_bindgen(js_name = memo)]
pub fn js_memo(component: &Function, compare: JsValue) -> Result<JsValue, JsValue> {
	let component = component.clone();
	let compare_fn: Option<Function> = compare.dyn_into().ok();

	let prev_props: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
	// Cache the Rust VNode, not the JS-wrapped pointer: js_to_vnode() frees
	// its backing box, so re-handing the same JsValue out on a cache hit
	// would double-free. Mint a fresh box via vnode_to_js() every return.
	let prev_result: Rc<RefCell<Option<VNode>>> = Rc::new(RefCell::new(None));

	let wrapper = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
		let should_skip = if let Some(prev) = prev_props.borrow().as_ref() {
			if let Some(cmp) = &compare_fn {
				cmp.call2(&JsValue::NULL, prev, &props).ok().and_then(|v| v.as_bool()).unwrap_or(false)
			} else {
				shallow_equal(prev, &props)
			}
		} else {
			false
		};

		if should_skip && let Some(vn) = prev_result.borrow().as_ref() {
			return vnode_to_js(vn.clone()).unwrap_or(JsValue::NULL);
		}

		let result = component.call1(&JsValue::NULL, &props).unwrap_or(JsValue::NULL);
		*prev_props.borrow_mut() = Some(props);
		let vn = js_to_vnode(&result).unwrap_or_else(|_| VNode::null());
		*prev_result.borrow_mut() = Some(vn.clone());
		vnode_to_js(vn).unwrap_or(JsValue::NULL)
	}) as Box<dyn Fn(JsValue) -> JsValue>);

	Ok(wrapper.into_js_value())
}

// ─── forwardRef() HOC ───

/// `forwardRef(render)` — wraps a `(props, ref) => vnode` render function so
/// it receives the caller's `ref` as a second argument instead of having it
/// silently dropped, which is what happens to a `ref` passed to an ordinary
/// function component (matching React's default behavior there).
///
/// Unlike `memo`, this doesn't need a new wrapper closure or element-type
/// kind: `render` is still just an ordinary function as far as
/// `create_element`/`html_template::render_element`'s `type_.is_function()`
/// dispatch is concerned. Tagging it with `FORWARD_REF_MARKER` is enough —
/// both call sites check the flag and call with `(props, ref)` instead of
/// `(props)` when it's set, passing through the raw ref value (a callback
/// or a `{ current }` object) exactly as the caller wrote it, so the
/// component can attach it to an inner DOM node itself (e.g.
/// `<input ref={ref} />`), which goes through the normal ref pipeline at
/// that point.
///
/// Note this tags (mutates) `render` in place and returns the same
/// reference, rather than producing a distinct wrapper object the way React
/// does — a function is a JS object, so this is enough for our purposes,
/// but it does mean the exact same function value is "forwardRef-aware"
/// everywhere it's used, not just via the value `forwardRef()` returns.
#[wasm_bindgen(js_name = forwardRef)]
pub fn js_forward_ref(render: &Function) -> Result<JsValue, JsValue> {
	Reflect::set(render, &FORWARD_REF_MARKER.into(), &JsValue::TRUE)?;
	Ok(render.clone().into())
}

fn shallow_equal(a: &JsValue, b: &JsValue) -> bool {
	if js_sys::Object::is(a, b) {
		return true;
	}
	if !a.is_object() || !b.is_object() {
		return false;
	}
	let ka = match a.dyn_ref::<Object>() {
		Some(o) => js_sys::Object::keys(o),
		None => return false,
	};
	let kb = match b.dyn_ref::<Object>() {
		Some(o) => js_sys::Object::keys(o),
		None => return false,
	};
	if ka.length() != kb.length() {
		return false;
	}
	for k in ka.iter() {
		let va = Reflect::get(a, &k).unwrap_or(JsValue::UNDEFINED);
		let vb = Reflect::get(b, &k).unwrap_or(JsValue::UNDEFINED);
		if !js_sys::Object::is(&va, &vb) {
			return false;
		}
	}
	true
}

// ─── ErrorBoundary component factory ───

/// Returns a JS function component that acts as an error boundary.
/// Usage: `createElement(ErrorBoundary, { fallback: err => <div>{err.message}</div> }, children)`
#[wasm_bindgen(js_name = createErrorBoundary)]
pub fn js_create_error_boundary() -> JsValue {
	// Re-entrancy guard: js_use_state can trigger a synchronous re-render
	// that re-invokes this closure before the first call returns; skip
	// re-entrant calls and return NULL.
	let in_progress = Rc::new(RefCell::new(false));

	/// Resets `in_progress` on drop (normal return or unwind), so a panic
	/// inside the boundary or its children can't leave the guard stuck.
	struct ResetOnDrop(Rc<RefCell<bool>>);
	impl Drop for ResetOnDrop {
		fn drop(&mut self) {
			*self.0.borrow_mut() = false;
		}
	}

	let boundary_fn = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
		if *in_progress.borrow() {
			return JsValue::NULL;
		}
		*in_progress.borrow_mut() = true;
		let _reset = ResetOnDrop(in_progress.clone());
		js_create_error_boundary_inner(props)
	}) as Box<dyn Fn(JsValue) -> JsValue>);

	boundary_fn.into_js_value()
}

fn js_create_error_boundary_inner(props: JsValue) -> JsValue {
	let arr = js_use_state(JsValue::NULL);
	let error: JsValue = arr.get(0);
	let set_error: JsValue = arr.get(1);

	// Register this render's setError on the instance so a descendant's
	// failure, discovered after this closure returns, has somewhere to
	// report to (see hooks::report_to_nearest_boundary).
	{
		let inst_ptr = current_inst();
		let setter_fn = set_error.clone();
		let rc_setter: Rc<dyn Fn(JsValue)> = Rc::new(move |err: JsValue| {
			if let Some(f) = setter_fn.dyn_ref::<Function>() {
				let _ = f.call1(&JsValue::NULL, &err);
			}
		});
		// SAFETY: single-threaded WASM; inst_ptr is valid for this render.
		unsafe {
			(*inst_ptr).error_setter = Some(rc_setter);
		}
	}

	if !error.is_null() && !error.is_undefined() {
		crate::console_error!("[micro-react] ErrorBoundary caught: {}", stringify_thrown(&error));
		let fallback = Reflect::get(&props, &"fallback".into()).unwrap_or(JsValue::NULL);
		if fallback.is_function() {
			let f: Function = fallback.dyn_into().expect("fallback.is_function() checked above");
			return f.call1(&JsValue::NULL, &error).unwrap_or(JsValue::NULL);
		}
		return fallback;
	}

	Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL)
}

// ─── Boot convenience: bundle the non-standard exports ───

/// Bundles the values that aren't plain `wasm-bindgen` exports (`Fragment`
/// is a `Symbol`, `ErrorBoundary` a stateful `Closure`) into one object, so
/// callers get both from a single call right after module init instead of
/// deriving each by hand. `getFragment`/`createErrorBoundary` stay exported
/// too, for callers who want a fresh, independent `ErrorBoundary` instance.
#[wasm_bindgen(js_name = createExtras)]
pub fn js_create_extras() -> Result<JsValue, JsValue> {
	let obj = Object::new();
	Reflect::set(&obj, &"Fragment".into(), &get_fragment())?;
	Reflect::set(&obj, &"ErrorBoundary".into(), &js_create_error_boundary())?;
	Ok(obj.into())
}

// ─── VNode ↔ JS conversion helpers ───

pub(crate) fn js_to_vnode(v: &JsValue) -> Result<VNode, JsValue> {
	if v.is_null() || v.is_undefined() {
		return Ok(VNode::null());
	}
	if let Some(s) = v.as_string() {
		return Ok(VNode::text(s));
	}
	if let Some(n) = v.as_f64() {
		return Ok(VNode::text(n.to_string()));
	}
	if v.as_bool().is_some() {
		return Ok(VNode::null());
	}

	// Array → fragment
	if let Ok(arr) = v.clone().dyn_into::<Array>() {
		let children: Vec<VNode> = arr.iter().filter_map(|c| js_to_vnode(&c).ok()).filter(|v| !matches!(v.inner, VNodeInner::Null)).collect();
		return Ok(VNode::fragment(children));
	}

	if !v.is_object() {
		return Ok(VNode::null());
	}

	let marker = Reflect::get(v, &"__mrVNode".into())?;
	if !marker.is_truthy() {
		return Ok(VNode::null());
	}

	let id_val = Reflect::get(v, &"__id".into())?;
	if let Some(n) = id_val.as_f64() {
		// `n` came off a JS number, so it's an f64; ids are assigned
		// sequentially from 1 in `next_vnode_id`, well within f64's exact
		// integer range for any realistic run, so this round-trip is exact.
		let id = n as u64;
		return Ok(take_vnode(id).unwrap_or_else(|| {
			// Unknown id, or (more likely) this JS vnode was already consumed
			// once (double-read). Unlike the old raw-pointer version this is
			// a safe, loud no-op instead of a use-after-free.
			crate::console_error!(
				"[micro-react] vnode id {} was already consumed or is unknown \
                 (the same rendered value was read more than once). Returning \
                 a null vnode instead of reusing/double-freeing it.",
				id
			);
			VNode::null()
		}));
	}

	Ok(VNode::null())
}

/// Same conversion as `js_to_vnode`, but for callers that may legitimately
/// inspect the same JS vnode wrapper more than once (e.g. reading `<Route
/// element={...}>` out of `props` on every re-render of `<Routes>`, where
/// `props` itself doesn't change even though the component re-renders).
/// Clones the stored vnode instead of removing it, so repeat reads keep
/// working instead of degrading to null after the first one.
pub(crate) fn js_to_vnode_peek(v: &JsValue) -> Result<VNode, JsValue> {
	if v.is_null() || v.is_undefined() {
		return Ok(VNode::null());
	}
	if let Some(s) = v.as_string() {
		return Ok(VNode::text(s));
	}
	if let Some(n) = v.as_f64() {
		return Ok(VNode::text(n.to_string()));
	}
	if v.as_bool().is_some() {
		return Ok(VNode::null());
	}

	if let Ok(arr) = v.clone().dyn_into::<Array>() {
		let children: Vec<VNode> = arr.iter().filter_map(|c| js_to_vnode_peek(&c).ok()).filter(|v| !matches!(v.inner, VNodeInner::Null)).collect();
		return Ok(VNode::fragment(children));
	}

	if !v.is_object() {
		return Ok(VNode::null());
	}

	let marker = Reflect::get(v, &"__mrVNode".into())?;
	if !marker.is_truthy() {
		return Ok(VNode::null());
	}

	let id_val = Reflect::get(v, &"__id".into())?;
	if let Some(n) = id_val.as_f64() {
		let id = n as u64;
		return Ok(peek_vnode(id).unwrap_or_else(|| {
			crate::console_error!(
				"[micro-react] vnode id {} is unknown while peeking \
                 (never stored, or already dropped from the store entirely). \
                 Returning a null vnode.",
				id
			);
			VNode::null()
		}));
	}

	Ok(VNode::null())
}

pub(crate) fn vnode_to_js(vnode: VNode) -> Result<JsValue, JsValue> {
	let obj = Object::new();
	Reflect::set(&obj, &"__mrVNode".into(), &JsValue::TRUE)?;
	let id = store_vnode(vnode);
	// f64 exactly represents all u64 values used here (ids only grow into
	// the billions if a page never reloads for an extremely long time),
	// so this cast is lossless in practice.
	Reflect::set(&obj, &"__id".into(), &JsValue::from_f64(id as f64))?;
	Ok(obj.into())
}

/// Converts collected JSX-style children into what a JS component sees on
/// `props.children`: a single vnode if there's one child, else an array.
pub(crate) fn children_to_js(children: &[VNode]) -> JsValue {
	if children.len() == 1 {
		vnode_to_js(children[0].clone()).unwrap_or(JsValue::NULL)
	} else {
		let arr = Array::new();
		for c in children {
			if let Ok(v) = vnode_to_js(c.clone()) {
				arr.push(&v);
			}
		}
		arr.into()
	}
}

pub(crate) fn js_val_to_prop_val(v: &JsValue) -> PropVal {
	if v.is_null() || v.is_undefined() {
		return PropVal::Null;
	}
	if let Some(b) = v.as_bool() {
		return PropVal::Bool(b);
	}
	if let Some(n) = v.as_f64() {
		return PropVal::Num(n);
	}
	if let Some(s) = v.as_string() {
		return PropVal::Str(s);
	}
	if v.is_function() {
		let f: Function = v.clone().dyn_into().expect("v.is_function() checked above");
		return PropVal::Callback(JsCallback(f));
	}
	// Plain objects and arrays (style objects, routes maps, etc.) — keep
	// the live JsValue instead of dropping it.
	if v.is_object() {
		return PropVal::Js(v.clone());
	}
	PropVal::Null
}

pub(crate) fn props_to_js_object(props: &Props) -> JsValue {
	let obj = Object::new();
	for (k, v) in props {
		let js_val = match v {
			PropVal::Str(s) => JsValue::from_str(s),
			PropVal::Bool(b) => JsValue::from_bool(*b),
			PropVal::Num(n) => JsValue::from_f64(*n),
			PropVal::Callback(c) => c.0.clone().into(),
			PropVal::Js(v) => v.clone(),
			PropVal::Null => JsValue::NULL,
		};
		let _ = Reflect::set(&obj, &JsValue::from_str(k), &js_val);
	}
	obj.into()
}

// ─── Tests for the VNode slot map (the fix for the raw-pointer bug) ───
// Pure Rust logic, no JS/DOM involved, so `cargo test --lib` covers these.
#[cfg(test)]
mod vnode_store_tests {
	use super::*;
	use crate::vnode::VNode;

	fn reset_store() {
		VNODE_STORE.with(|s| s.borrow_mut().clear());
		VNODE_NEXT_ID.with(|c| *c.borrow_mut() = 1);
		VNODE_LEAK_WARNED.with(|w| *w.borrow_mut() = false);
	}

	#[test]
	fn store_then_take_round_trips() {
		reset_store();
		let id = store_vnode(VNode::text("hello"));
		let got = take_vnode(id).expect("vnode should be present after storing");
		match got.inner {
			crate::vnode::VNodeInner::Text(s) => assert_eq!(s, "hello"),
			_ => panic!("expected a text vnode"),
		}
	}

	#[test]
	fn double_consume_is_safe_not_ub() {
		// Core regression test: reading the same "JS vnode" twice must never
		// touch freed memory. Old Box::from_raw approach double-freed here;
		// the second take() is now just a normal, safe `None`.
		reset_store();
		let id = store_vnode(VNode::text("once"));
		assert!(take_vnode(id).is_some(), "first take should succeed");
		assert!(take_vnode(id).is_none(), "second take must NOT return the vnode again");
		assert!(take_vnode(id).is_none(), "third take is still safe and still None");
	}

	#[test]
	fn unknown_id_returns_none_instead_of_panicking() {
		reset_store();
		assert!(take_vnode(999_999).is_none());
	}

	#[test]
	fn ids_are_unique_across_stores() {
		reset_store();
		let id1 = store_vnode(VNode::text("a"));
		let id2 = store_vnode(VNode::text("b"));
		assert_ne!(id1, id2);
		// Each id only unlocks its own vnode.
		let a = take_vnode(id1).unwrap();
		let b = take_vnode(id2).unwrap();
		match (a.inner, b.inner) {
			(crate::vnode::VNodeInner::Text(a), crate::vnode::VNodeInner::Text(b)) => {
				assert_eq!(a, "a");
				assert_eq!(b, "b");
			}
			_ => panic!("expected text vnodes"),
		}
	}

	#[test]
	fn unconsumed_vnodes_stay_out_of_each_others_way() {
		// A discarded vnode (never taken) must not corrupt or block access
		// to other, unrelated vnodes still in the store — it should just
		// sit there as an inert leaked entry.
		reset_store();
		let discarded = store_vnode(VNode::text("discarded"));
		let kept = store_vnode(VNode::text("kept"));
		// Never call take_vnode(discarded) — simulates a JS-side branch
		// that never reads the value it was handed.
		let got = take_vnode(kept).unwrap();
		match got.inner {
			crate::vnode::VNodeInner::Text(s) => assert_eq!(s, "kept"),
			_ => panic!("expected text vnode"),
		}
		// The discarded entry is still sitting in the store (a bounded,
		// visible leak) rather than having corrupted anything.
		assert!(VNODE_STORE.with(|s| s.borrow().contains_key(&discarded)));
	}
}

// ─── Tests for the VNode↔JS conversion helpers (marker protocol, prop-value
// mapping, array-as-fragment). `pub(crate)`, so these live here rather than
// in `tests/`, where they'd be unreachable across the crate boundary. Needs
// real `JsValue`s, so runs under `wasm-bindgen-test`, not plain `cargo test`.
#[cfg(test)]
mod conversion_tests {
	use super::*;
	use crate::vnode::VNodeInner;
	use wasm_bindgen_test::*;

	wasm_bindgen_test_configure!(run_in_browser);

	#[wasm_bindgen_test]
	fn js_to_vnode_null_and_undefined_become_null_vnode() {
		assert!(matches!(js_to_vnode(&JsValue::NULL).unwrap().inner, VNodeInner::Null));
		assert!(matches!(js_to_vnode(&JsValue::UNDEFINED).unwrap().inner, VNodeInner::Null));
	}

	#[wasm_bindgen_test]
	fn js_to_vnode_string_and_number_become_text() {
		match js_to_vnode(&JsValue::from_str("hi")).unwrap().inner {
			VNodeInner::Text(s) => assert_eq!(s, "hi"),
			_ => panic!("expected text vnode"),
		}
		match js_to_vnode(&JsValue::from_f64(42.0)).unwrap().inner {
			VNodeInner::Text(s) => assert_eq!(s, "42"),
			_ => panic!("expected text vnode for a number child"),
		}
	}

	#[wasm_bindgen_test]
	fn js_to_vnode_bool_becomes_null_like_react_conditional_rendering() {
		// `{cond && <X/>}` yields `false` when cond is falsy; React (and
		// this project) renders that as nothing, not the literal text "false".
		assert!(matches!(js_to_vnode(&JsValue::from_bool(false)).unwrap().inner, VNodeInner::Null));
		assert!(matches!(js_to_vnode(&JsValue::from_bool(true)).unwrap().inner, VNodeInner::Null));
	}

	#[wasm_bindgen_test]
	fn js_to_vnode_array_becomes_a_fragment_dropping_null_entries() {
		let arr = Array::new();
		arr.push(&JsValue::from_str("a"));
		arr.push(&JsValue::NULL);
		arr.push(&JsValue::from_str("b"));
		match js_to_vnode(&arr.into()).unwrap().inner {
			VNodeInner::Fragment { children, .. } => {
				assert_eq!(children.len(), 2, "the null entry should be filtered out of the fragment's children");
			}
			other => panic!("expected a fragment, got {other:?}"),
		}
	}

	#[wasm_bindgen_test]
	fn js_to_vnode_rejects_objects_without_the_marker() {
		let plain = Object::new();
		assert!(
			matches!(js_to_vnode(&plain.into()).unwrap().inner, VNodeInner::Null),
			"a plain object without __mrVNode must not be mistaken for a vnode wrapper"
		);
	}

	#[wasm_bindgen_test]
	fn vnode_to_js_round_trips_through_js_to_vnode() {
		let original = VNode::text("round trip");
		let js = vnode_to_js(original).expect("vnode_to_js should succeed");

		// The marker protocol: a stored vnode is a plain object tagged
		// __mrVNode: true with an __id pointing into the slot map.
		let marker = Reflect::get(&js, &"__mrVNode".into()).unwrap();
		assert!(marker.is_truthy(), "vnode_to_js should tag its wrapper with __mrVNode");
		assert!(Reflect::get(&js, &"__id".into()).unwrap().as_f64().is_some(), "vnode_to_js should tag its wrapper with a numeric __id");

		match js_to_vnode(&js).unwrap().inner {
			VNodeInner::Text(s) => assert_eq!(s, "round trip"),
			_ => panic!("expected the round-tripped vnode to still be the original text vnode"),
		}
	}

	#[wasm_bindgen_test]
	fn children_to_js_unwraps_a_single_child_but_arrays_multiple() {
		let one = children_to_js(&[VNode::text("solo")]);
		assert!(!one.is_array(), "a single child should be handed back as one wrapper object, not a one-element array");

		let many = children_to_js(&[VNode::text("a"), VNode::text("b")]);
		let arr: Array = many.dyn_into().expect("multiple children should be an array");
		assert_eq!(arr.length(), 2);
	}

	#[wasm_bindgen_test]
	fn js_val_to_prop_val_covers_every_variant() {
		assert_eq!(js_val_to_prop_val(&JsValue::NULL), PropVal::Null);
		assert_eq!(js_val_to_prop_val(&JsValue::UNDEFINED), PropVal::Null);
		assert_eq!(js_val_to_prop_val(&JsValue::from_bool(true)), PropVal::Bool(true));
		assert_eq!(js_val_to_prop_val(&JsValue::from_f64(3.5)), PropVal::Num(3.5));
		assert_eq!(js_val_to_prop_val(&JsValue::from_str("s")), PropVal::Str("s".to_string()));

		let f: Function = Closure::wrap(Box::new(|| {}) as Box<dyn Fn()>).into_js_value().unchecked_into();
		match js_val_to_prop_val(&f.clone().into()) {
			PropVal::Callback(cb) => assert!(js_sys::Object::is(cb.as_ref(), f.as_ref())),
			other => panic!("expected a Callback PropVal, got {other:?}"),
		}

		let style = Object::new();
		let _ = Reflect::set(&style, &"color".into(), &"red".into());
		match js_val_to_prop_val(&style.clone().into()) {
			PropVal::Js(v) => assert!(js_sys::Object::is(&v, &style.into())),
			other => panic!("expected a Js PropVal for a plain object, got {other:?}"),
		}
	}

	#[wasm_bindgen_test]
	fn props_to_js_object_is_the_inverse_of_js_val_to_prop_val() {
		let props: Props = vec![
			("label".to_string(), PropVal::Str("hi".to_string())),
			("count".to_string(), PropVal::Num(3.0)),
			("on".to_string(), PropVal::Bool(true)),
			("missing".to_string(), PropVal::Null),
		];
		let js = props_to_js_object(&props);
		assert_eq!(Reflect::get(&js, &"label".into()).unwrap().as_string().as_deref(), Some("hi"));
		assert_eq!(Reflect::get(&js, &"count".into()).unwrap().as_f64(), Some(3.0));
		assert_eq!(Reflect::get(&js, &"on".into()).unwrap().as_bool(), Some(true));
		assert!(Reflect::get(&js, &"missing".into()).unwrap().is_null());
	}
}

/// Convert a JS deps array (or undefined) to a Rust dep vec.
fn js_deps_to_rust(deps: &JsValue) -> Option<Vec<DepVal>> {
	if deps.is_undefined() || deps.is_null() {
		return None; // always re-run
	}
	if let Ok(arr) = deps.clone().dyn_into::<Array>() {
		let v = arr
			.iter()
			.map(|d| {
				if let Some(s) = d.as_string() {
					DepVal(s)
				} else if let Some(n) = d.as_f64() {
					DepVal(n.to_string())
				} else if let Some(b) = d.as_bool() {
					DepVal(b.to_string())
				} else {
					// Non-primitive deps must serialize structurally, not
					// collapse to a constant "js" string, or memoized values
					// would never recompute when they actually change.
					js_sys::JSON::stringify(&d).ok().and_then(|s| s.as_string()).map(DepVal).unwrap_or_else(|| DepVal("js".to_string()))
				}
			})
			.collect();
		Some(v)
	} else {
		None
	}
}
