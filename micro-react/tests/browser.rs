//! Single entry point for every DOM/browser-backed test.
//!
//! `wasm-pack test --headless --firefox` launches a fresh browser context
//! per *test binary*, and each file directly under `tests/` compiles to
//! its own binary — so with 10 separate browser-test files that was 10
//! context spin-ups (the slow part) to run a total of well under a
//! second's worth of actual test code.
//!
//! Putting them all behind one binary (this file, with the rest living as
//! submodules under `tests/browser/`) means `wasm-pack test` opens exactly
//! one browser context and runs all of them in it. Split a module back out
//! into its own top-level `tests/*.rs` file only if it genuinely needs an
//! isolated context (e.g. something that mutates global/shared browser
//! state in a way that would leak across tests).

use wasm_bindgen_test::wasm_bindgen_test_configure;

wasm_bindgen_test_configure!(run_in_browser);

#[path = "browser/bindings.rs"]
mod bindings;
#[path = "browser/bindings_gaps.rs"]
mod bindings_gaps;
#[path = "browser/context_unit.rs"]
mod context_unit;
#[path = "browser/events_dom.rs"]
mod events_dom;
#[path = "browser/events_unit.rs"]
mod events_unit;
#[path = "browser/hooks_scheduler.rs"]
mod hooks_scheduler;
#[path = "browser/html_template.rs"]
mod html_template;
#[path = "browser/portals.rs"]
mod portals;
#[path = "browser/reconciler.rs"]
mod reconciler;
#[path = "browser/refs_dom.rs"]
mod refs_dom;
#[path = "browser/render_root.rs"]
mod render_root;
#[path = "browser/router.rs"]
mod router;
#[path = "browser/router_gaps.rs"]
mod router_gaps;
#[path = "browser/router_gaps2.rs"]
mod router_gaps2;
#[path = "browser/vnode_inner_unit.rs"]
mod vnode_inner_unit;
#[path = "browser/vnode_unit.rs"]
mod vnode_unit;
