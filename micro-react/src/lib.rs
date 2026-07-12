// micro-react: a React-like UI runtime in Rust/WASM.
// See each module for its role: vnode, diff, hooks, context, events,
// router, render, scheduler, bindings (the JS-facing surface).

#![allow(clippy::new_without_default)]
#![allow(unused_variables)]

use wasm_bindgen::prelude::*;

#[macro_use]
pub mod log;
pub mod bindings;
pub mod context;
pub mod diff;
pub mod events;
pub mod hooks;
pub mod html_template;
pub mod render;
pub mod router;
pub mod scheduler;
pub mod vnode;

pub use render::Root;
pub use vnode::{Children, Key, Props, VNode, VNodeInner};

/// Runs once when the WASM module is instantiated.
#[wasm_bindgen(start)]
pub fn wasm_start() {
	console_error_panic_hook::set_once();
	console_log!("[micro-react] wasm module initialized");
}
