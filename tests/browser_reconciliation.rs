//! Entry point for the reconciliation-sensitive DOM/browser-backed tests.
//!
//! These modules were pulled out of `tests/browser.rs` because they're the
//! ones that actually depend on things that persist as module-level state
//! for the lifetime of one wasm module instance — the vnode id counter,
//! `VNODE_STORE`, ref bookkeeping — surviving *correctly* across dozens of
//! mount/unmount/reorder cycles from other, unrelated tests that ran
//! earlier in the same session:
//!
//! - `reconciler`, `html_template`: keyed-list reordering that asserts on
//!   exact DOM node identity and final child order after a diff.
//! - `refs_dom`: ref callbacks that must fire exactly once per genuine
//!   mount/unmount, not on every reconciliation pass.
//! - `portals`: DOM nodes moved between containers, another case that
//!   cares about node identity surviving a diff intact.
//! - `render_root`, `imperative_handle_and_suspense`: full
//!   `Root::render`/`Root::unmount` cycles and imperative-handle identity
//!   across renders.
//!
//! A handful of these tests failed once bundled into the single merged
//! `tests/browser.rs` binary (all keyed-order/ref-identity failures) despite
//! looking correct in isolation — consistent with leaked state from
//! unrelated tests running first in the same instance, not with a real
//! diffing bug. Giving this group its own binary/session keeps that
//! interaction from recurring while still cutting session spin-ups from
//! ~20 down to 2. If a new test here starts failing only when run after
//! specific *other* tests in this file (not in isolation), that's a signal
//! this group still needs splitting further, rather than being merged back
//! into `tests/browser.rs`.

use wasm_bindgen_test::wasm_bindgen_test_configure;

wasm_bindgen_test_configure!(run_in_browser);

#[path = "browser/html_template.rs"]
mod html_template;
#[path = "browser/imperative_handle_and_suspense.rs"]
mod imperative_handle_and_suspense;
#[path = "browser/portals.rs"]
mod portals;
#[path = "browser/reconciler.rs"]
mod reconciler;
#[path = "browser/refs_dom.rs"]
mod refs_dom;
#[path = "browser/render_root.rs"]
mod render_root;
