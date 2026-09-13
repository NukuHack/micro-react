//! micro-react: a React-like UI runtime in Rust/WASM.
//! See each module for its role: vnode, diff, hooks, context, events,
//! router, render, scheduler, bindings (the JS-facing surface).

#![cfg_attr(docsrs, feature(doc_cfg))]
//#![warn(missing_docs)]
#![warn(
	clippy::pedantic, // strict-er clippy and optionalizations
	clippy::nursery, // strict-er clippy and optionalizations
	clippy::let_unit_value, // function that returns ()
	clippy::print_stdout, // side effect
	clippy::print_stderr, // side effect
	unsafe_code, // unsafe code
	clippy::panic,
	clippy::unwrap_used,
	clippy::expect_used,
)]
#![allow(
    clippy::cast_possible_truncation, // num as other num
    clippy::cast_possible_wrap, // num as other num
    clippy::cast_sign_loss, // signed to unsigned
    clippy::cast_precision_loss, // float precision loss
	clippy::too_long_first_doc_paragraph, // should be long ...
	clippy::wildcard_imports, // should only be used in testing
	clippy::format_push_string, // it's fine in this scale

	clippy::trivially_copy_pass_by_ref, // should be removed
	clippy::struct_excessive_bools, // should be removed
	clippy::significant_drop_tightening, // should be removed
	clippy::implicit_hasher, // we will use hashset, nothing else

	clippy::similar_names, // similar ...
	clippy::too_many_lines, // will correct it when i correct file lengths

	clippy::new_without_default, // pre-existing crate convention
)]
#![warn(
    missing_debug_implementations, // nice to debug all
	unused_qualifications, // it's useless
    rust_2018_idioms,    // Still useful for backward compatibility patterns
    rust_2021_compatibility, // Warns about things that changed in 2021
    rust_2024_compatibility,  // Warns about things that changed in 2024 (when stable)
)]

use wasm_bindgen::prelude::*;

#[macro_use]
pub mod log;
pub mod bindings;
pub mod context;
pub mod diff;
pub mod events;
pub mod hooks;
pub mod html_template;
pub mod jsx;
pub mod module_prep;
pub mod render;
pub mod router;
pub mod scan;
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
