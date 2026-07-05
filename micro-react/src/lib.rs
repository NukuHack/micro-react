// ─── micro-react-wasm ───
// A React-like UI runtime written in Rust/WASM.
// See each module for its role (vnode, diff, hooks, context, events, router, render, scheduler, bindings).
// ────────────────────────

#![allow(clippy::new_without_default)]
#![allow(dead_code, unused_variables, unused_imports)]

use wasm_bindgen::prelude::*;

#[macro_use]
pub mod log;
pub mod bindings;
pub mod context;
pub mod diff;
pub mod events;
pub mod hooks;
pub mod render;
pub mod router;
pub mod scheduler;
pub mod vnode;

// Re-export the most useful types at crate root
pub use vnode::{Children, Key, Props, Template, VNode, VNodeInner};
pub use render::Root;

/// Called by the WASM module init (wasm-bindgen generated glue runs this).
#[wasm_bindgen(start)]
pub fn wasm_start() {
    console_error_panic_hook::set_once();
    console_log!("[micro-react] wasm module initialized");
}
