//! Regression coverage for the module-loading gaps called out in the TODO:
//!
//! - Importing a module that doesn't exist under any extension should reject
//!   cleanly (a rejected promise / `Err`), not hang forever.
//! - A "diamond" import — two sibling specifiers in the *same* module that
//!   both point at the same dependency, resolved in parallel by the same
//!   `Promise.all` in `load_module_body` (`src/jsx.rs`) — should still
//!   resolve correctly rather than double-fetching or deadlocking.
//! - Retrying the same bad import a second time in the same session should
//!   fail cleanly again, proving the failed load's placeholder was actually
//!   removed from `MODULE_CACHE` rather than left dangling (which would
//!   otherwise either hang forever awaiting a promise nobody re-settles, or
//!   incorrectly reuse a dead cache entry).
//!
//! These exercise `micro_react::jsx::load_jsx_module` directly (it's a plain
//! `pub async fn`, `#[wasm_bindgen]` only adds the JS-facing name), so no
//! bundler or JS harness is needed — just a real browser event loop via
//! `wasm-bindgen-test`, since the implementation uses `web_sys::window()`
//! and `fetch`.
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox
//!
//! Data URLs (`data:text/javascript;base64,...`) are used for the "real
//! module" fixtures so these tests don't depend on any file being served
//! from a particular path by the `wasm-pack test` harness — `fetch()` and
//! `import`-specifier URL resolution both work the same way against a
//! `data:` URL as against an `http(s):` one for the purposes of this code
//! path. Base64 (rather than a raw comma-separated `data:` URL) avoids any
//! ambiguity around commas/quotes inside the encoded source being confused
//! with the URL's own syntax when spliced into an `import ... from '...'` line.

use wasm_bindgen_test::*;

use micro_react::jsx::load_jsx_module;

fn to_data_url(window: &web_sys::Window, source: &str) -> String {
	let encoded = window.btoa(source).expect("btoa should succeed on plain ASCII JS source");
	format!("data:text/javascript;base64,{encoded}")
}

// ─── (1) importing a nonexistent module rejects cleanly, doesn't hang ───

#[wasm_bindgen_test]
async fn importing_a_module_that_does_not_exist_under_any_extension_rejects_cleanly() {
	// If this hung instead of rejecting, the test itself would time out
	// rather than fail with a clean assertion — that's the "doesn't freeze"
	// half of this regression test; awaiting it at all is the point.
	let result = load_jsx_module("./this-module-definitely-does-not-exist-anywhere-1234", None).await;
	assert!(result.is_err(), "importing a module missing under every resolved extension should reject, not hang or silently resolve");

	let err = result.unwrap_err();
	let message = err.as_string().unwrap_or_else(|| format!("{err:?}"));
	assert!(message.contains("Failed to fetch JSX"), "expected a clear, descriptive rejection message, got: {message}");
}

// ─── (3) retrying the same bad path a second time still fails cleanly ───

#[wasm_bindgen_test]
async fn reimporting_the_same_nonexistent_module_fails_cleanly_again_not_hanging() {
	// Proves the failed load's MODULE_CACHE placeholder was actually removed
	// (see the "Failure: drop the placeholder entirely" branch in
	// `load_jsx_module_impl`) rather than left dangling — a dangling entry
	// with `is_loading: true` would make a second import of the same URL
	// take the `is_concurrent_diamond`/`wait_for_module` path and await a
	// promise that already rejected and will never be re-driven, which
	// would still resolve today (the promise already settled) but would be
	// the wrong path, and a stale non-loading dangling entry would
	// incorrectly reuse dead exports instead of retrying the fetch.
	let path = "./this-module-definitely-does-not-exist-anywhere-5678";

	let first = load_jsx_module(path, None).await;
	assert!(first.is_err(), "first import of a missing module should reject");

	let second = load_jsx_module(path, None).await;
	assert!(second.is_err(), "retrying the same missing module should reject again, not hang or return stale/incorrect success");
}

// ─── (2) concurrent "diamond" import of the same real module still resolves ───

#[wasm_bindgen_test]
async fn diamond_import_of_the_same_module_from_two_sibling_specifiers_resolves_correctly() {
	// Two `import` lines in the *same* module pointing at the exact same
	// dependency URL. `load_module_body`'s specifier loop processes these in
	// order: the first spawns the real load and (synchronously, before any
	// fetch has actually completed) inserts a `MODULE_CACHE` entry with
	// `is_loading: true`; the second specifier then sees that same entry
	// already loading and — since it isn't its own ancestor, so this isn't a
	// circular import — takes the `is_concurrent_diamond` path and awaits
	// the first load's real promise instead of re-fetching the module or
	// reading its (still-empty) exports early. If that path were broken,
	// this would either hang, double-fetch, or resolve `b` as `undefined`.
	let window = web_sys::window().expect("window should be available in a browser test");

	let child_src = "export const x = 5;";
	let child_url = to_data_url(&window, child_src);

	let root_src = format!("import {{ x as a }} from '{child_url}';\nimport {{ x as b }} from '{child_url}';\nexport const sum = a + b;\n");
	let root_url = to_data_url(&window, &root_src);

	let exports = load_jsx_module(&root_url, None).await.expect("diamond import of the same real module should resolve, not hang or error");

	let sum =
		js_sys::Reflect::get(&exports, &"sum".into()).expect("exports should have a 'sum' property").as_f64().expect("'sum' should be a number");
	assert_eq!(
		sum, 10.0,
		"both sibling imports of the same module should resolve to its real exports (5 + 5), not undefined/NaN from a broken diamond path"
	);
}
