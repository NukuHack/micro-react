//! Tests for `events::parse_event_prop` — pure string logic with no
//! JS/DOM calls of its own, but run here via `wasm-bindgen-test` (like
//! the rest of `tests/`) so `build.sh`'s single
//! `wasm-pack test --headless --firefox` step picks them up alongside
//! everything else. `set_event_handler` (the DOM-touching half of this
//! module) is covered separately in `tests/events_dom.rs`.

use wasm_bindgen_test::*;

use micro_react::events::parse_event_prop;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn simple_click() {
	assert_eq!(parse_event_prop("onClick"), Some(("click".to_string(), false)));
}

#[wasm_bindgen_test]
fn multi_word_event_name() {
	assert_eq!(parse_event_prop("onMouseEnter"), Some(("mouseenter".to_string(), false)));
	assert_eq!(parse_event_prop("onDoubleClick"), Some(("doubleclick".to_string(), false)));
}

#[wasm_bindgen_test]
fn capture_suffix_is_stripped_and_flagged() {
	assert_eq!(parse_event_prop("onClickCapture"), Some(("click".to_string(), true)));
	assert_eq!(parse_event_prop("onMouseEnterCapture"), Some(("mouseenter".to_string(), true)));
}

#[wasm_bindgen_test]
fn missing_on_prefix_returns_none() {
	assert_eq!(parse_event_prop("click"), None);
	assert_eq!(parse_event_prop("className"), None);
	assert_eq!(parse_event_prop(""), None);
}

#[wasm_bindgen_test]
fn bare_on_with_nothing_after_returns_none() {
	assert_eq!(parse_event_prop("on"), None);
}

#[wasm_bindgen_test]
fn bare_on_capture_with_nothing_between_returns_none() {
	// "on" + "Capture" -> rest after stripping "Capture" suffix is empty.
	assert_eq!(parse_event_prop("onCapture"), None);
}

#[wasm_bindgen_test]
fn event_name_that_happens_to_contain_capture_as_prefix_is_not_over_stripped() {
	// "onCaptureClick" ends with "Click", not "Capture", so the
	// capture-suffix check should not touch it.
	assert_eq!(parse_event_prop("onCaptureClick"), Some(("captureclick".to_string(), false)));
}

#[wasm_bindgen_test]
fn single_char_event_name() {
	assert_eq!(parse_event_prop("onX"), Some(("x".to_string(), false)));
}

#[wasm_bindgen_test]
fn case_insensitive_prefix_is_not_matched_when_lowercase_on() {
	// "on" prefix check is case-sensitive: "On" (capital O) is not "on".
	assert_eq!(parse_event_prop("OnClick"), None);
}
