// ─── events.rs ────────────────────────────────────────────────────────────────
//
// Preact logical-clock event proxy.
//
// Problem: if you mount a node INSIDE a click handler, the click event that
// triggered the mount can bubble INTO the newly-mounted node and fire its
// click handler — the so-called "event fire after mount" race.
//
// Solution (from Preact): attach a SINGLE proxy listener per (element × event
// × capture).  The proxy records the time each handler was *attached*
// (`__mrAttached`, via `performance.now()`) and compares it against the
// dispatching event's own `timeStamp` (which uses the same clock).  If the
// attachment happened after the event started dispatching, the event is
// suppressed.
//
// Handlers are stored on a JS object property so the proxy can read them
// without keeping Rust borrows live across async boundaries.
//
// ─────────────────────────────────────────────────────────────────────────────

use wasm_bindgen::{prelude::*, JsCast};
use js_sys::{Function, Reflect};

/// Property names on the DOM element for the listeners map.
const LISTENERS_KEY: &str = "__mrListeners";

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Set or remove an event handler on a DOM element.
///
/// * `elem`       – the DOM element
/// * `event_name` – lowercase event name, e.g. "click"
/// * `capture`    – true for capture phase
/// * `handler`    – `Some(fn)` to set, `None` to remove
/// * `old_handler`– previous handler value (may be null)
pub fn set_event_handler(
    elem: &web_sys::Element,
    event_name: &str,
    capture: bool,
    handler: Option<&Function>,
    old_handler: Option<&Function>,
) {
    let js_elem: &JsValue = elem.as_ref();

    // Lazily create the _listeners map on the element
    let listeners: js_sys::Object = ensure_listeners(js_elem);

    let key = listener_key(event_name, capture);

    match handler {
        Some(h) => {
            // Record the time at which this handler was attached, on the same
            // clock as `Event.timeStamp` (performance.now()-relative), so the
            // proxy can compare them directly.
            let attached_at = web_sys::window()
                .and_then(|w| w.performance())
                .map(|p| p.now())
                .unwrap_or(0.0);
            let _ = Reflect::set(h.as_ref(), &JsValue::from_str("__mrAttached"), &JsValue::from_f64(attached_at));

            // Install the handler in the map
            let _ = Reflect::set(listeners.as_ref(), &JsValue::from_str(&key), h.as_ref());

            // If no proxy was attached before (old_handler was None), add the proxy
            if old_handler.is_none() {
                let proxy = make_proxy(capture);
                let _ = elem.add_event_listener_with_callback_and_bool(
                    event_name,
                    &proxy,
                    capture,
                );
                // Store proxy reference on the listeners map so we can remove it later
                let proxy_key = format!("__proxy_{}", key);
                let _ = Reflect::set(listeners.as_ref(), &JsValue::from_str(&proxy_key), proxy.as_ref());
                // proxy kept alive by the DOM reference stored in listeners map
            }
        }
        None => {
            // Remove from map
            let _ = Reflect::delete_property(&listeners, &JsValue::from_str(&key));

            // Remove the proxy listener if there was an old one
            if old_handler.is_some() {
                let proxy_key = format!("__proxy_{}", key);
                if let Ok(proxy_val) = Reflect::get(listeners.as_ref(), &JsValue::from_str(&proxy_key)) {
                    if !proxy_val.is_null() && !proxy_val.is_undefined() {
                        if let Ok(proxy_fn) = proxy_val.dyn_into::<Function>() {
                            let _ = elem.remove_event_listener_with_callback_and_bool(
                                event_name,
                                &proxy_fn,
                                capture,
                            );
                        }
                    }
                    let _ = Reflect::delete_property(&listeners, &JsValue::from_str(&proxy_key));
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

fn ensure_listeners(elem: &JsValue) -> js_sys::Object {
    match Reflect::get(elem, &JsValue::from_str(LISTENERS_KEY)) {
        Ok(v) if !v.is_undefined() && !v.is_null() => {
            v.dyn_into::<js_sys::Object>().unwrap_or_else(|_| js_sys::Object::new())
        }
        _ => {
            let obj = js_sys::Object::new();
            let _ = Reflect::set(elem, &JsValue::from_str(LISTENERS_KEY), obj.as_ref());
            obj.clone()
        }
    }
}

fn listener_key(event_name: &str, capture: bool) -> String {
    if capture {
        format!("{}_cap", event_name)
    } else {
        event_name.to_string()
    }
}

/// Create a proxy closure for the given phase.
fn make_proxy(capture: bool) -> js_sys::Function {
    // The proxy is a JS function that reads the current handler from
    // `this.__mrListeners[eventName+capture]` and calls it with the clock
    // guard: if the handler was attached *after* this event started
    // dispatching (e.g. because the click that's currently bubbling also
    // mounted the node we're now on), suppress the call. `event.timeStamp`
    // and `performance.now()` (used for `__mrAttached`, see set_event_handler
    // above) share the same clock, so they can be compared directly.
    let code = format!(r#"
        const key = event.type + {capture_str};
        const listeners = this['{listeners_key}'];
        if (!listeners) return;
        const handler = listeners[key];
        if (!handler) return;
        if ((handler['__mrAttached'] || 0) > event.timeStamp) {{
            return;
        }}
        return handler.call(this, event);
    "#,
        capture_str = if capture { "'_cap'" } else { "''" },
        listeners_key = LISTENERS_KEY,
    );

    // Build as: new Function('event', body)
    Function::new_with_args("event", &code)
}

// ─────────────────────────────────────────────────────────────────────────────
// React-style event name normalisation
// ─────────────────────────────────────────────────────────────────────────────

/// Convert a React-style camelCase event prop name to a DOM event name + capture flag.
/// e.g. "onClick"        → ("click", false)
///      "onClickCapture" → ("click", true)
///      "onMouseEnter"   → ("mouseenter", false)
pub fn parse_event_prop(prop: &str) -> Option<(String, bool)> {
    if !prop.starts_with("on") { return None; }
    let rest = &prop[2..]; // strip "on"

    let (rest, capture) = if rest.ends_with("Capture") {
        (&rest[..rest.len() - 7], true)
    } else {
        (rest, false)
    };

    if rest.is_empty() { return None; }

    // Convert camelCase to lowercase: "MouseEnter" → "mouseenter"
    let event_name = rest.to_lowercase();
    Some((event_name, capture))
}
