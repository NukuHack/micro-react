//! Tests for `router::Pattern` (compile + matches).
//!
//! `Pattern::matches` builds and executes a `js_sys::RegExp`, which calls
//! into a real JS engine — under a plain `cargo test --lib` target that
//! panics ("not supported outside wasm"), so like `tests/reconciler.rs`
//! these run via `wasm-bindgen-test` in an actual (headless) browser:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox
//!
//! `Pattern`'s fields are private, so these go through the crate's public
//! surface (`compile` + `matches`) exactly as `router.rs`'s own
//! `js_router` does, rather than reaching into internals.

use micro_react::router::Pattern;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn static_path_matches_exactly() {
	let p = Pattern::compile("/about");
	let params = p.matches("/about").expect("should match");
	assert!(params.is_empty());
}

#[wasm_bindgen_test]
fn static_path_does_not_match_other_paths() {
	let p = Pattern::compile("/about");
	assert!(p.matches("/contact").is_none());
	assert!(p.matches("/about/team").is_none());
	assert!(p.matches("/abou").is_none());
}

#[wasm_bindgen_test]
fn root_path_matches_root() {
	let p = Pattern::compile("/");
	assert!(p.matches("/").is_some());
}

#[wasm_bindgen_test]
fn single_param_segment_is_captured() {
	let p = Pattern::compile("/users/:id");
	let params = p.matches("/users/42").expect("should match");
	assert_eq!(params.get("id"), Some(&"42".to_string()));
}

#[wasm_bindgen_test]
fn multiple_param_segments_are_all_captured() {
	let p = Pattern::compile("/users/:userId/posts/:postId");
	let params = p.matches("/users/7/posts/99").expect("should match");
	assert_eq!(params.get("userId"), Some(&"7".to_string()));
	assert_eq!(params.get("postId"), Some(&"99".to_string()));
}

#[wasm_bindgen_test]
fn param_segment_does_not_match_across_slashes() {
	let p = Pattern::compile("/users/:id");
	// "42/extra" should not be captured as a single :id segment.
	assert!(p.matches("/users/42/extra").is_none());
}

#[wasm_bindgen_test]
fn wildcard_matches_anything_including_nested_segments() {
	let p = Pattern::compile("/files/*");
	assert!(p.matches("/files/a").is_some());
	assert!(p.matches("/files/a/b/c.txt").is_some());
	assert!(p.matches("/files/").is_some());
}

#[wasm_bindgen_test]
fn trailing_slash_is_tolerated() {
	let p = Pattern::compile("/about");
	assert!(p.matches("/about/").is_some());
}

#[wasm_bindgen_test]
fn mixed_static_and_param_segments() {
	let p = Pattern::compile("/blog/:year/:slug");
	let params = p.matches("/blog/2024/hello-world").expect("should match");
	assert_eq!(params.get("year"), Some(&"2024".to_string()));
	assert_eq!(params.get("slug"), Some(&"hello-world".to_string()));
}

#[wasm_bindgen_test]
fn regex_special_characters_in_static_segments_are_escaped() {
	// A literal "." in a path segment must not act as a regex wildcard.
	let p = Pattern::compile("/v1.0/status");
	assert!(p.matches("/v1.0/status").is_some());
	// If "." weren't escaped, "v1X0" would also match — it must not.
	assert!(p.matches("/v1X0/status").is_none());
}

#[wasm_bindgen_test]
fn empty_pattern_matches_only_root() {
	let p = Pattern::compile("");
	assert!(p.matches("/").is_some());
	assert!(p.matches("/x").is_none());
}
