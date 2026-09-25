//! Entry point for the lower-risk half of the DOM/browser-backed tests.
//!
//! `wasm-pack test --headless --firefox` launches a fresh browser context
//! (and a fresh wasm module instance) per *test binary*. Everything in one
//! binary shares that single instance, and nothing resets the crate's
//! module-level state (the vnode id counter, `VNODE_STORE`, the module
//! loader cache, context listener maps, ...) between individual
//! `#[wasm_bindgen_test]` functions — they only get a clean slate at the
//! start of a binary.
//!
//! That's harmless for tests that don't lean on precise DOM node identity
//! or ordering surviving across many renders. It's the exact thing that
//! bit us in tests/browser_reconciliation.rs, so that half lives in its
//! own binary/session instead — see the doc comment there for which
//! modules moved and why. Everything below is the "doesn't hammer that
//! machinery" half: parsing/matching logic, hook wiring, bindings
//! conversions, and DOM-touching tests that don't depend on node identity
//! surviving repeated mount/unmount/reorder cycles.
//!
//! Split a module back out into its own top-level `tests/*.rs` file only
//! if it genuinely needs an isolated context (e.g. something that mutates
//! global/shared browser state in a way that would leak across tests).

use wasm_bindgen_test::wasm_bindgen_test_configure;

wasm_bindgen_test_configure!(run_in_browser);

#[path = "browser/bindings.rs"]
mod bindings;
#[path = "browser/bindings_gaps.rs"]
mod bindings_gaps;
#[path = "browser/bindings_gaps2.rs"]
mod bindings_gaps2;
#[path = "browser/bindings_gaps3.rs"]
mod bindings_gaps3;
#[path = "browser/context_unit.rs"]
mod context_unit;
#[path = "browser/events_dom.rs"]
mod events_dom;
#[path = "browser/events_unit.rs"]
mod events_unit;
#[path = "browser/hooks_scheduler.rs"]
mod hooks_scheduler;
#[path = "browser/module_loading.rs"]
mod module_loading;
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
