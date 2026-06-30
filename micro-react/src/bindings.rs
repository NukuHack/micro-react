// ─── bindings.rs ─────────────────────────────────────────────────────────────
//
// wasm-bindgen public surface — JS-callable exports.
//
// ─────────────────────────────────────────────────────────────────────────────

use wasm_bindgen::{prelude::*, JsCast};
use js_sys::{Array, Function, Object, Reflect};
use web_sys::Element;
use std::cell::RefCell;
use std::rc::Rc;

use crate::vnode::{VNode, VNodeInner, PropVal, JsCallback, ComponentFn, Props, NodeRef};
use crate::render::Root;
use crate::hooks::{
    use_state, use_state_fn, use_reducer, use_effect_nodrop, use_layout_effect,
    use_ref, use_memo, use_callback, use_id, DepVal, ComponentInst, with_inst, current_inst,
};
use crate::scheduler::{flush_sync as rs_flush_sync, start_transition as rs_start_transition, enqueue_render};
use crate::context::{Context, use_context};

// ─────────────────────────────────────────────────────────────────────────────
// Root handle (JS-visible)
// ─────────────────────────────────────────────────────────────────────────────

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

#[wasm_bindgen(js_name = createRoot)]
pub fn create_root(container: Element) -> Result<JsRoot, JsValue> {
    crate::console_log!("[micro-react] createRoot()");
    let root = Root::new(container);
    Ok(JsRoot { inner: RefCell::new(root) })
}

#[wasm_bindgen(js_name = render)]
pub fn render(vnode: JsValue, container: Element) -> Result<JsRoot, JsValue> {
    let vnode = js_to_vnode(&vnode)?;
    crate::console_log!("[micro-react] render() mounting to container");
    let mut root = Root::new(container);
    root.render(vnode)?;
    Ok(JsRoot { inner: RefCell::new(root) })
}

// ─────────────────────────────────────────────────────────────────────────────
// createElement
// ─────────────────────────────────────────────────────────────────────────────

#[wasm_bindgen(js_name = createElement)]
pub fn create_element(
    type_: &JsValue,
    props: &JsValue,
    children: JsValue,
) -> Result<JsValue, JsValue> {
    // `children` arrives here as whatever the JS caller passed as the 3rd
    // positional argument. wasm-bindgen-generated export shims have fixed
    // arity (exactly the 3 declared params), so a JS-side variadic call like
    // `createElement(type, props, child1, child2, child3)` (the React-style
    // calling convention script.js/the JS API use, collecting via `...args`)
    // silently truncates to just `child1` at this boundary — args 4+ never
    // reach wasm at all. That can only be fixed where the call is made (see
    // the `h` wrapper in index.html, which must bundle children into a real
    // array before calling in). Here we just make sure that a single
    // non-array child (e.g. `createElement('h1', null, 'text')`, or the
    // already-truncated single-arg case before that JS fix lands) isn't
    // silently dropped on the floor by treating any non-null/undefined,
    // non-array value as a one-element children array.
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
        Reflect::get(props, &"key".into()).ok().and_then(|v| v.as_string())
    } else { None };

    // `ref` used to be silently dropped here, so `useRef`/callback refs never
    // received the live DOM node. Extract it and turn it into a NodeRef whose
    // `sync` callback writes back into the JS ref object (or calls the
    // callback-ref function) — see `js_ref_to_node_ref` below.
    let node_ref: Option<NodeRef> = if props.is_object() {
        Reflect::get(props, &"ref".into()).ok().and_then(|v| js_ref_to_node_ref(&v))
    } else { None };

    let mut rust_props: Props = Vec::new();
    let dummy = js_sys::Object::new();
    if props.is_object() && !props.is_null() {
        let obj = props.dyn_ref::<js_sys::Object>().unwrap_or(&dummy);
        let keys = js_sys::Object::keys(obj);
        for k in keys.iter() {
            let k_str = k.as_string().unwrap_or_default();
            if k_str == "key" || k_str == "ref" { continue; }
            let val = Reflect::get(props, &k)?;
            rust_props.push((k_str, js_val_to_prop_val(&val)));
        }
    }

    let mut child_vnodes: Vec<VNode> = Vec::new();
    for child in children.iter() {
        if let Ok(vn) = js_to_vnode(&child) {
            if !matches!(vn.inner, VNodeInner::Null) {
                child_vnodes.push(vn);
            }
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
        if let Some(k) = key { builder = builder.key(k); }
        if let Some(r) = node_ref { builder = builder.ref_(r); }
        builder.children(child_vnodes).build()
    } else if type_.is_function() {
        let fn_: Function = type_.clone().dyn_into().unwrap();
        let fn_name = Reflect::get(&fn_, &"name".into())
            .ok().and_then(|v| v.as_string())
            .unwrap_or_else(|| "Anonymous".to_string());

        VNode::component(
            fn_name,
            ComponentFn::new(move |props| {
                let js_props = props_to_js_object(&props);
                if !child_vnodes.is_empty() {
                    let children_val = children_to_js(&child_vnodes);
                    let _ = Reflect::set(&js_props, &"children".into(), &children_val);
                }
                match fn_.call1(&JsValue::NULL, &js_props) {
                    Ok(result) => js_to_vnode(&result).unwrap_or_else(|_| VNode::null()),
                    Err(_) => VNode::null(),
                }
            }),
            rust_props,
        )
    } else {
        VNode::null()
    };

    vnode_to_js(vnode)
}

/// Convert a JS `ref` value — either a callback function `(node) => {}` or a
/// ref object `{ current }` (the shape returned by `useRef`) — into a
/// `NodeRef` whose `sync` callback keeps it updated with the live DOM node.
/// Returns `None` for `null`/`undefined`/anything else.
fn js_ref_to_node_ref(ref_val: &JsValue) -> Option<NodeRef> {
    if ref_val.is_null() || ref_val.is_undefined() { return None; }

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

// ─────────────────────────────────────────────────────────────────────────────
// Fragment symbol (JS-visible)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the Symbol used as the Fragment type.
#[wasm_bindgen(js_name = getFragment)]
pub fn get_fragment() -> JsValue {
    js_sys::Symbol::for_("MicroReact.Fragment").into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Scheduler exports
// ─────────────────────────────────────────────────────────────────────────────

#[wasm_bindgen(js_name = flushSync)]
pub fn flush_sync(f: &Function) -> Result<(), JsValue> {
    rs_flush_sync(|| { let _ = f.call0(&JsValue::NULL); });
    Ok(())
}

#[wasm_bindgen(js_name = startTransition)]
pub fn start_transition(f: &Function) -> Result<(), JsValue> {
    rs_start_transition(|| { let _ = f.call0(&JsValue::NULL); });
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Hooks  — exposed as bare wasm-bindgen functions
//
// Strategy: hooks must be called from inside a JS component function that was
// invoked from Rust's diffComponent path (which sets CURRENT_INST).  Each JS
// hook binding just delegates to the Rust implementation.
//
// For JS we wrap the Rust `use_state` in a thin binding that:
//   1. calls Rust use_state<JsValue>
//   2. returns a JS Array [value, setter]
// ─────────────────────────────────────────────────────────────────────────────

/// `useState(initialValue)` — returns `[value, setter]`.
/// Supports both `setState(value)` and `setState(prev => next)` functional
/// updaters by keeping a shared cell of the current value alongside the hook
/// slot, so the updater function always sees the latest state.
#[wasm_bindgen(js_name = useState)]
pub fn js_use_state(initial: JsValue) -> Array {
    let shared: Rc<RefCell<JsValue>> = Rc::new(RefCell::new(initial.clone()));
    let shared_init = shared.clone();

    let (value, setter) = use_state::<JsValue>(initial);
    // Keep the shared cell in sync with the latest committed value.
    *shared_init.borrow_mut() = value.clone();

    let setter_fn = {
        let shared = shared.clone();
        Closure::wrap(Box::new(move |next: JsValue| {
            let resolved = if next.is_function() {
                let f: Function = next.unchecked_ref::<Function>().clone();
                let cur = shared.borrow().clone();
                f.call1(&JsValue::NULL, &cur).unwrap_or(JsValue::UNDEFINED)
            } else {
                next
            };
            *shared.borrow_mut() = resolved.clone();
            setter(resolved);
        }) as Box<dyn Fn(JsValue)>)
    };

    let js_fn = setter_fn.into_js_value();
    let arr = Array::new();
    arr.push(&value);
    arr.push(&js_fn);
    arr
}

/// `useState(initialValue)` — full version with functional updater support.
/// Returns `[value, setter]` where setter accepts either a value or `v => v` function.
#[wasm_bindgen(js_name = useStateF)]
pub fn js_use_state_f(initial: JsValue) -> Array {
    // We store the value in a Rc<RefCell> so functional setters can read current value.
    let shared: Rc<RefCell<JsValue>> = Rc::new(RefCell::new(initial.clone()));
    let shared_init = shared.clone();

    let (value, setter) = use_state::<JsValue>(initial);
    // Keep the shared cell in sync
    *shared_init.borrow_mut() = value.clone();

    let setter_fn = {
        let shared = shared.clone();
        let setter = setter.clone();
        Closure::wrap(Box::new(move |next: JsValue| {
            let resolved = if next.is_function() {
                let f: Function = next.unchecked_ref::<Function>().clone();
                let cur = shared.borrow().clone();
                f.call1(&JsValue::NULL, &cur).unwrap_or(JsValue::UNDEFINED)
            } else {
                next
            };
            *shared.borrow_mut() = resolved.clone();
            setter(resolved);
        }) as Box<dyn Fn(JsValue)>)
    };

    let arr = Array::new();
    arr.push(&value);
    arr.push(&setter_fn.into_js_value());
    arr
}

/// `useReducer(reducer, initialState)` — returns `[state, dispatch]`.
#[wasm_bindgen(js_name = useReducer)]
pub fn js_use_reducer(reducer: &Function, initial: JsValue) -> Array {
    let reducer = reducer.clone();
    let shared: Rc<RefCell<JsValue>> = Rc::new(RefCell::new(initial.clone()));

    let (state, dispatch) = use_reducer::<JsValue, JsValue>(
        {
            let reducer = reducer.clone();
            move |state, action| {
                reducer.call2(&JsValue::NULL, &state, &action)
                    .unwrap_or(state)
            }
        },
        initial,
    );

    *shared.borrow_mut() = state.clone();

    let dispatch_fn = {
        let dispatch = dispatch.clone();
        Closure::wrap(Box::new(move |action: JsValue| {
            dispatch(action);
        }) as Box<dyn Fn(JsValue)>)
    };

    let arr = Array::new();
    arr.push(&state);
    arr.push(&dispatch_fn.into_js_value());
    arr
}

/// `useEffect(callback, deps?)` — callback returns an optional cleanup function.
///
/// Previously the cleanup function returned by `callback` was captured and
/// then immediately thrown away (`let _ = f;`), so cleanups never ran on
/// deps-change or unmount — e.g. the Stopwatch demo's
/// `return () => cancelAnimationFrame(rafRef.current)` never fired, leaking
/// a new requestAnimationFrame loop every time the effect re-ran. Delegate to
/// `hooks::use_effect`, which already threads cleanups through correctly
/// (this mirrors what `useEffectWithCleanup`/`useLayoutEffect` already do).
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

/// Full `useEffect` with cleanup support.
#[wasm_bindgen(js_name = useEffectWithCleanup)]
pub fn js_use_effect_cleanup(callback: &Function, deps: JsValue) {
    let callback = callback.clone();
    let rust_deps = js_deps_to_rust(&deps);

    // We use use_layout_effect internally so we can capture the cleanup fn
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

/// `useRef(initialValue?)` — returns a `{ current: value }` JS object.
/// The returned object is stable across renders.
///
/// Previously this discarded the `use_state` setter (`let (ref_obj, _) = ...`),
/// so the hook's stored value was never actually updated after the first
/// render: every render saw `ref_obj` as still `undefined` and manufactured a
/// *brand new* `{ current }` object, resetting `.current` back to `initial`
/// and changing the object's identity each time. Persist it like
/// `useRefStable` does below.
#[wasm_bindgen(js_name = useRef)]
pub fn js_use_ref(initial: JsValue) -> Object {
    let (initialized, set_init) = use_state::<bool>(false);
    let (ref_obj, set_ref) = use_state::<JsValue>(JsValue::NULL);

    if !initialized {
        let obj = Object::new();
        Reflect::set(&obj, &"current".into(), &initial).unwrap();
        let obj_val: JsValue = obj.clone().into();
        set_init(true);
        set_ref(obj_val);
        return obj;
    }

    ref_obj.dyn_into::<Object>().unwrap_or_else(|_| {
        let obj = Object::new();
        Reflect::set(&obj, &"current".into(), &initial).unwrap();
        obj
    })
}

/// Stable useRef — uses a separate state slot to persist the ref object.
#[wasm_bindgen(js_name = useRefStable)]
pub fn js_use_ref_stable(initial: JsValue) -> Object {
    // Use two slots: one for the object itself (lazy-init), one to signal init
    let (initialized, set_init) = use_state::<bool>(false);
    let (ref_obj, set_ref) = use_state::<JsValue>(JsValue::NULL);

    if !initialized {
        let obj = Object::new();
        Reflect::set(&obj, &"current".into(), &initial).unwrap();
        let obj_val: JsValue = obj.clone().into();
        set_init(true);
        set_ref(obj_val);
        return obj;
    }

    ref_obj.dyn_into::<Object>().unwrap_or_else(|_| Object::new())
}

/// `useMemo(factory, deps)` — returns a memoised value.
#[wasm_bindgen(js_name = useMemo)]
pub fn js_use_memo(factory: &Function, deps: JsValue) -> JsValue {
    let factory = factory.clone();
    let rust_deps = js_deps_to_rust(&deps);
    use_memo(
        move || factory.call0(&JsValue::NULL).unwrap_or(JsValue::UNDEFINED),
        rust_deps,
    )
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

// ─────────────────────────────────────────────────────────────────────────────
// Context API
//
// Strategy: we can't expose a Rust Context<T> directly to JS because T is
// generic. Instead we use Context<JsValue> and wrap it in a plain JS object
// that looks like the JS micro-react context shape:
//
//   {
//     Provider:   JSFunction(({ value, children }) => ...),
//     Consumer:   JSFunction(({ children }) => children(value)),
//     useContext: JSFunction(() => currentValue),
//     _id:        number,
//   }
// ─────────────────────────────────────────────────────────────────────────────

/// `createContext(defaultValue)` — returns a JS context object.
#[wasm_bindgen(js_name = createContext)]
pub fn js_create_context(default_value: JsValue) -> Result<JsValue, JsValue> {
    use crate::context::Context;

    // Allocate a new context and leak it into a 'static reference
    // (safe: WASM module lives forever, context is never freed)
    let ctx: &'static Context<JsValue> = Box::leak(Box::new(Context::new(default_value.clone())));
    let ctx_id = ctx.id;

    // Build the JS context object
    let obj = Object::new();

    // _id
    Reflect::set(&obj, &"_id".into(), &JsValue::from_f64(ctx_id as f64))?;

    // Provider component function: function Provider({ value, children }) { ... }
    let ctx_provider = ctx;
    let provider_fn = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
        let value = Reflect::get(&props, &"value".into()).unwrap_or(default_value.clone());
        ctx_provider.set_value(value);
        // Return children
        Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL)
    }) as Box<dyn Fn(JsValue) -> JsValue>);
    Reflect::set(&obj, &"Provider".into(), provider_fn.as_ref())?;
    provider_fn.forget();

    // Consumer render-prop: function Consumer({ children }) { return children(value) }
    let ctx_consumer = ctx;
    let consumer_fn = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
        let value = ctx_consumer.current_value();
        let children = Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL);
        if children.is_function() {
            let f: Function = children.dyn_into().unwrap();
            f.call1(&JsValue::NULL, &value).unwrap_or(JsValue::NULL)
        } else {
            JsValue::NULL
        }
    }) as Box<dyn Fn(JsValue) -> JsValue>);
    Reflect::set(&obj, &"Consumer".into(), consumer_fn.as_ref())?;
    consumer_fn.forget();

    // useContext() hook — reads current value and subscribes to changes
    let ctx_hook = ctx;
    let use_ctx_fn = Closure::wrap(Box::new(move || -> JsValue {
        use_context(ctx_hook)
    }) as Box<dyn Fn() -> JsValue>);
    Reflect::set(&obj, &"useContext".into(), use_ctx_fn.as_ref())?;
    use_ctx_fn.forget();

    Ok(obj.into())
}

// ─────────────────────────────────────────────────────────────────────────────
// memo() HOC
// ─────────────────────────────────────────────────────────────────────────────

/// `memo(Component, compare?)` — wraps a component function to skip re-renders
/// when props are shallowly equal (or compare() returns true).
#[wasm_bindgen(js_name = memo)]
pub fn js_memo(component: &Function, compare: JsValue) -> Result<JsValue, JsValue> {
    let component = component.clone();
    let compare_fn: Option<Function> = compare.dyn_into().ok();

    // We leak the previous props into a thread-local via a JS-side WeakMap.
    // Simpler: use a Rust RefCell inside the closure.
    let prev_props: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
    let prev_result: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));

    let wrapper = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
        let should_skip = if let Some(prev) = prev_props.borrow().as_ref() {
            if let Some(cmp) = &compare_fn {
                cmp.call2(&JsValue::NULL, prev, &props)
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                shallow_equal(prev, &props)
            }
        } else {
            false
        };

        if should_skip {
            if let Some(res) = prev_result.borrow().as_ref() {
                return res.clone();
            }
        }

        let result = component.call1(&JsValue::NULL, &props)
            .unwrap_or(JsValue::NULL);
        *prev_props.borrow_mut() = Some(props);
        *prev_result.borrow_mut() = Some(result.clone());
        result
    }) as Box<dyn Fn(JsValue) -> JsValue>);

    Ok(wrapper.into_js_value())
}

fn shallow_equal(a: &JsValue, b: &JsValue) -> bool {
    if js_sys::Object::is(a, b) { return true; }
    if !a.is_object() || !b.is_object() { return false; }
    let ka = match a.dyn_ref::<Object>() {
        Some(o) => js_sys::Object::keys(o),
        None => return false,
    };
    let kb = match b.dyn_ref::<Object>() {
        Some(o) => js_sys::Object::keys(o),
        None => return false,
    };
    if ka.length() != kb.length() { return false; }
    for k in ka.iter() {
        let va = Reflect::get(a, &k).unwrap_or(JsValue::UNDEFINED);
        let vb = Reflect::get(b, &k).unwrap_or(JsValue::UNDEFINED);
        if !js_sys::Object::is(&va, &vb) { return false; }
    }
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// createRef
// ─────────────────────────────────────────────────────────────────────────────

#[wasm_bindgen(js_name = createRef)]
pub fn js_create_ref() -> Object {
    let obj = Object::new();
    Reflect::set(&obj, &"current".into(), &JsValue::NULL).unwrap();
    obj
}

// ─────────────────────────────────────────────────────────────────────────────
// ErrorBoundary component factory
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a JS function component that acts as an error boundary.
/// Usage: `createElement(ErrorBoundary, { fallback: err => <div>{err.message}</div> }, children)`
#[wasm_bindgen(js_name = createErrorBoundary)]
pub fn js_create_error_boundary() -> JsValue {
    let boundary_fn = Closure::wrap(Box::new(|props: JsValue| -> JsValue {
        // [error, setError] = useState(null)
        let arr = js_use_state_f(JsValue::NULL);
        let error: JsValue = arr.get(0);
        let set_error: JsValue = arr.get(1);

        if !error.is_null() && !error.is_undefined() {
            crate::console_error!("[micro-react] ErrorBoundary caught: {}", js_sys::JsString::from(error.clone()));
            // Render fallback
            let fallback = Reflect::get(&props, &"fallback".into()).unwrap_or(JsValue::NULL);
            if fallback.is_function() {
                let f: Function = fallback.dyn_into().unwrap();
                return f.call1(&JsValue::NULL, &error).unwrap_or(JsValue::NULL);
            }
            return fallback;
        }

        // No error: render children
        Reflect::get(&props, &"children".into()).unwrap_or(JsValue::NULL)
    }) as Box<dyn Fn(JsValue) -> JsValue>);

    boundary_fn.into_js_value()
}

// ─────────────────────────────────────────────────────────────────────────────
// html tagged-template (improved, template-cached)
// ─────────────────────────────────────────────────────────────────────────────

#[wasm_bindgen(js_name = htmlTemplate)]
pub fn html_template(statics: Array, values: Array) -> Result<JsValue, JsValue> {
    let static_parts: Vec<String> = statics.iter()
        .filter_map(|v| v.as_string())
        .collect();
    let cache_key: String = static_parts.join("\x00");

    let _template_id: u64 = {
        thread_local! {
            static STRING_TO_ID: RefCell<std::collections::HashMap<String, u64>> =
                RefCell::new(std::collections::HashMap::new());
        }
        STRING_TO_ID.with(|m| m.borrow().get(&cache_key).copied())
            .unwrap_or_else(|| {
                let id = crate::vnode::next_id();
                thread_local! {
                    static STRING_TO_ID2: RefCell<std::collections::HashMap<String, u64>> =
                        RefCell::new(std::collections::HashMap::new());
                }
                id
            })
    };

    let combined = build_template_vnode(&static_parts, &values)?;
    vnode_to_js(combined)
}

fn build_template_vnode(statics: &[String], values: &Array) -> Result<VNode, JsValue> {
    let mut html = String::new();
    for (i, s) in statics.iter().enumerate() {
        html.push_str(s);
        if i < values.length() as usize {
            let val = values.get(i as u32);
            if is_vnode(&val) {
                html.push_str("<!--HOLE-->");
            } else if val.is_string() {
                html.push_str(&val.as_string().unwrap_or_default());
            } else if let Some(n) = val.as_f64() {
                html.push_str(&n.to_string());
            }
        }
    }
    parse_html_fragment(&html)
}

fn parse_html_fragment(html: &str) -> Result<VNode, JsValue> {
    use web_sys::DomParser;
    let parser = DomParser::new()?;
    let doc = parser.parse_from_string(
        &format!("<root>{}</root>", html),
        web_sys::SupportedType::TextHtml,
    )?;
    let body = doc.body().ok_or("no body")?;
    let root = body.first_child().ok_or("no root")?;
    dom_node_to_vnode(&root)
}

fn dom_node_to_vnode(node: &web_sys::Node) -> Result<VNode, JsValue> {
    use web_sys::Node;
    match node.node_type() {
        Node::TEXT_NODE => {
            let text = node.text_content().unwrap_or_default();
            if text.trim().is_empty() { Ok(VNode::null()) }
            else { Ok(VNode::text(text)) }
        }
        Node::ELEMENT_NODE => {
            let elem: &Element = node.unchecked_ref();
            let tag = elem.local_name();
            let mut builder = VNode::tag(tag);
            let attrs = elem.attributes();
            for i in 0..attrs.length() {
                if let Some(a) = attrs.item(i) {
                    builder = builder.attr(a.name(), a.value());
                }
            }
            let child_nodes = node.child_nodes();
            for i in 0..child_nodes.length() {
                if let Some(child) = child_nodes.item(i) {
                    let child_vn = dom_node_to_vnode(&child)?;
                    if !matches!(child_vn.inner, VNodeInner::Null) {
                        builder = builder.child(child_vn);
                    }
                }
            }
            Ok(builder.build())
        }
        _ => Ok(VNode::null()),
    }
}

fn is_vnode(v: &JsValue) -> bool {
    if !v.is_object() { return false; }
    Reflect::get(v, &"__mrVNode".into())
        .map(|f| f.is_truthy())
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// VNode ↔ JS conversion helpers
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn js_to_vnode(v: &JsValue) -> Result<VNode, JsValue> {
    if v.is_null() || v.is_undefined() { return Ok(VNode::null()); }
    if let Some(s) = v.as_string() { return Ok(VNode::text(s)); }
    if let Some(n) = v.as_f64() { return Ok(VNode::text(n.to_string())); }
    if let Some(b) = v.as_bool() {
        return if b { Ok(VNode::null()) } else { Ok(VNode::null()) };
    }

    // Array → fragment
    if let Ok(arr) = v.clone().dyn_into::<Array>() {
        let children: Vec<VNode> = arr.iter()
            .filter_map(|c| js_to_vnode(&c).ok())
            .filter(|v| !matches!(v.inner, VNodeInner::Null))
            .collect();
        return Ok(VNode::fragment(children));
    }

    if !v.is_object() { return Ok(VNode::null()); }

    let marker = Reflect::get(v, &"__mrVNode".into())?;
    if !marker.is_truthy() { return Ok(VNode::null()); }

    let ptr = Reflect::get(v, &"__ptr".into())?;
    if let Some(n) = ptr.as_f64() {
        let boxed = unsafe { Box::from_raw(n as usize as *mut VNode) };
        let vnode = *boxed;
        return Ok(vnode);
    }

    Ok(VNode::null())
}

pub(crate) fn vnode_to_js(vnode: VNode) -> Result<JsValue, JsValue> {
    let obj = Object::new();
    Reflect::set(&obj, &"__mrVNode".into(), &JsValue::TRUE)?;
    let ptr = Box::into_raw(Box::new(vnode)) as usize as f64;
    Reflect::set(&obj, &"__ptr".into(), &JsValue::from_f64(ptr))?;
    Ok(obj.into())
}

/// Converts collected JSX-style children into the value a JS component sees
/// on `props.children` — a single vnode-marked object if there's exactly one
/// child, or an array of them (which `js_to_vnode` already treats as a
/// fragment) if there are several.
fn children_to_js(children: &[VNode]) -> JsValue {
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

fn js_val_to_prop_val(v: &JsValue) -> PropVal {
    if v.is_null() || v.is_undefined() { return PropVal::Null; }
    if let Some(b) = v.as_bool() { return PropVal::Bool(b); }
    if let Some(n) = v.as_f64() { return PropVal::Num(n); }
    if let Some(s) = v.as_string() { return PropVal::Str(s); }
    if v.is_function() {
        let f: Function = v.clone().dyn_into().unwrap();
        return PropVal::Callback(JsCallback(f));
    }
    // Plain objects and arrays (style objects, routes maps, etc.) — keep the
    // live JsValue instead of dropping it. See the PropVal::Js doc comment.
    if v.is_object() {
        return PropVal::Js(v.clone());
    }
    PropVal::Null
}

fn props_to_js_object(props: &Props) -> JsValue {
    let obj = Object::new();
    for (k, v) in props {
        let js_val = match v {
            PropVal::Str(s)      => JsValue::from_str(s),
            PropVal::Bool(b)     => JsValue::from_bool(*b),
            PropVal::Num(n)      => JsValue::from_f64(*n),
            PropVal::Callback(c) => c.0.clone().into(),
            PropVal::Js(v)       => v.clone(),
            PropVal::Null        => JsValue::NULL,
        };
        let _ = Reflect::set(&obj, &JsValue::from_str(k), &js_val);
    }
    obj.into()
}

/// Convert a JS deps array (or undefined) to a Rust dep vec.
fn js_deps_to_rust(deps: &JsValue) -> Option<Vec<DepVal>> {
    if deps.is_undefined() || deps.is_null() {
        return None; // always re-run
    }
    if let Ok(arr) = deps.clone().dyn_into::<Array>() {
        let v = arr.iter().map(|d| {
            if let Some(s) = d.as_string() { DepVal(s) }
            else if let Some(n) = d.as_f64() { DepVal(n.to_string()) }
            else if let Some(b) = d.as_bool() { DepVal(b.to_string()) }
            else { DepVal("js".to_string()) }
        }).collect();
        Some(v)
    } else {
        None
    }
}