// ─── context.rs ──────────────────────────────────────────────────────────────
//
// Context API (mirrors React.createContext).
//
// Because WASM is single-threaded, contexts are stored in a thread-local
// global map keyed by a monotonic u64 id.  The Provider function component
// sets the value; useContext reads it and subscribes to changes.
//
// ─────────────────────────────────────────────────────────────────────────────

use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::vnode::{VNode};

// ─────────────────────────────────────────────────────────────────────────────
// Global context registry
// ─────────────────────────────────────────────────────────────────────────────

static CTX_ID_SEQ: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Maps context_id → current value (as a type-erased `Rc<dyn Any>`)
    static CTX_VALUES: RefCell<HashMap<u64, Rc<dyn Any>>> = RefCell::new(HashMap::new());

    /// Maps context_id → list of waker callbacks
    static CTX_LISTENERS: RefCell<HashMap<u64, Vec<Rc<dyn Fn()>>>> =
        RefCell::new(HashMap::new());
}

// ─────────────────────────────────────────────────────────────────────────────
// Context<T>
// ─────────────────────────────────────────────────────────────────────────────

/// A context object created by `Context::new(default_value)`.
/// Clone this to share the context across components.
#[derive(Clone)]
pub struct Context<T: Clone + 'static> {
    pub id: u64,
    pub default_value: T,
}

impl<T: Clone + 'static> Context<T> {
    /// Create a new context with the given default value.
    pub fn new(default_value: T) -> Self {
        Context {
            id: CTX_ID_SEQ.fetch_add(1, Ordering::Relaxed),
            default_value,
        }
    }

    /// Get the current value from the registry (or default).
    pub fn current_value(&self) -> T {
        CTX_VALUES.with(|m| {
            m.borrow()
                .get(&self.id)
                .and_then(|v| v.downcast_ref::<T>())
                .cloned()
                .unwrap_or_else(|| self.default_value.clone())
        })
    }

    /// Set the current value (called by the Provider component).
    pub fn set_value(&self, value: T) {
        let rc: Rc<dyn Any> = Rc::new(value);
        CTX_VALUES.with(|m| {
            m.borrow_mut().insert(self.id, rc);
        });
        self.notify_listeners();
    }

    /// Subscribe to value changes.  Returns a de-registration closure.
    pub fn subscribe(&self, listener: Rc<dyn Fn()>) -> Box<dyn FnOnce()> {
        let id = self.id;
        CTX_LISTENERS.with(|m| {
            m.borrow_mut()
                .entry(id)
                .or_insert_with(Vec::new)
                .push(listener.clone());
        });
        Box::new(move || {
            CTX_LISTENERS.with(|m| {
                if let Some(v) = m.borrow_mut().get_mut(&id) {
                    v.retain(|f| !Rc::ptr_eq(f, &listener));
                }
            });
        })
    }

    fn notify_listeners(&self) {
        let listeners: Vec<Rc<dyn Fn()>> = CTX_LISTENERS.with(|m| {
            m.borrow().get(&self.id).cloned().unwrap_or_default()
        });
        for f in listeners { f(); }
    }

    /// Build a Provider VNode that wraps children with a new context value.
    ///
    /// Usage:
    /// ```rust
    /// let ctx = MY_CTX.clone();
    /// ctx.provide(42, vec![child_vnode])
    /// ```
    pub fn provide(&self, value: T, children: Vec<VNode>) -> VNode {
        let ctx = self.clone();
        VNode::component(
            "Context.Provider",
            crate::vnode::ComponentFn::new(move |_props| {
                ctx.set_value(value.clone());
                VNode::fragment(children.clone())
            }),
            vec![],
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// useContext hook
// ─────────────────────────────────────────────────────────────────────────────

/// Read the current value of `ctx` and re-render this component when it changes.
pub fn use_context<T: Clone + 'static>(ctx: &Context<T>) -> T {
    use crate::hooks::{current_inst, use_effect_nodrop, DepVal};
    use crate::scheduler::enqueue_render;

    let inst = current_inst();
    let value = ctx.current_value();

    // Subscribe to future changes: when the context updates, re-render this component.
    let ctx_id = ctx.id;
    let waker: Rc<dyn Fn()> = Rc::new(move || {
        enqueue_render(inst);
    });

    // Register the subscription in a useEffect (runs once, cleans up on unmount)
    let ctx_clone = ctx.clone();
    use_effect_nodrop(move || {
        let unsub = ctx_clone.subscribe(waker);
        // TODO: wire unsub as cleanup
        let _ = unsub;
    }, Some(vec![DepVal(format!("ctx_{}", ctx_id))]));

    value
}
