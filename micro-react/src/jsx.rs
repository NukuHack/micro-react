//! Rewrites JSX source text into the `` html`...` `` tagged-template calls
//! that `html_template::compile` already knows how to handle. This is a
//! pure syntax transform — `{expr}` holes are copied verbatim, never
//! parsed as JS — so it stays additive to the existing render pipeline.
//!
//! Scope: plain `.jsx`, not `.tsx`. JSX roots are detected structurally
//! (any `<Tag ...>`/`<>` that scans as a balanced element), not only ones
//! preceded by `return`/`(`, which means a stray `<` in a JS comparison
//! can rarely be misread as a tag start; see `looks_like_jsx_start`.

use wasm_bindgen::prelude::*;

use crate::scan::{find_matching_brace, scan_tag_name_end, skip_js_comment, skip_js_string};

/// Errors produced while transpiling JSX. Offsets are character indices
/// into the source, not byte offsets, since scanning operates on `Vec<char>`.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum JsxError {
	#[error("unterminated JSX starting near character offset {0}: no matching closing tag found")]
	UnterminatedTag(usize),
	#[error("unbalanced '{{' hole starting at character offset {0}")]
	UnbalancedHole(usize),
	#[error("malformed closing tag at character offset {0}")]
	MalformedClosingTag(usize),
	#[error("mismatched closing tag near character offset {at}: expected </{expected}>, found </{found}>")]
	MismatchedClosingTag { expected: String, found: String, at: usize },
}

/// A JSX-shaped span of the original source, plus its rendered replacement.
struct JsxSpan {
	start: usize,
	end: usize,
	rendered: String,
}

/// JSX component tags start with an uppercase letter (or `_`) by
/// convention; those need a `${Name}` hole so `html_template` resolves the
/// JS binding instead of treating it as a literal custom-element tag name.
/// Lowercase tags (`div`, `span`, ...) are real HTML elements and stay static.
fn is_component_name(name: &str) -> bool {
	name.chars().next().is_some_and(|c| c.is_uppercase() || c == '_')
}

/// Cheap pre-check before committing to a full element parse: the char
/// after `<` must look like a real tag opener (a fragment `>`, or a name
/// immediately followed by whitespace, `/`, or `>`). This rejects most
/// comparison operators (`a < b()`, `a < b.c`) without needing to
/// understand JS expression grammar.
fn looks_like_jsx_start(chars: &[char], i: usize) -> bool {
	match chars.get(i + 1) {
		Some('>') => true,
		Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
			let name_end = scan_tag_name_end(chars, i + 1);
			matches!(chars.get(name_end), Some(c) if c.is_whitespace() || *c == '/' || *c == '>')
		}
		_ => false,
	}
}

/// Renders a JSX attribute section (`from` is just after the tag name) up
/// to its terminating `>` or self-closing `/>`, converting `{expr}` holes
/// into `${expr}` and leaving quoted attribute values untouched.
fn render_jsx_attrs(chars: &[char], from: usize) -> Result<(String, usize, bool), JsxError> {
	let n = chars.len();
	let mut out = String::new();
	let mut i = from;
	let mut in_quote: Option<char> = None;

	while i < n {
		let c = chars[i];
		if let Some(q) = in_quote {
			out.push(c);
			if c == q {
				in_quote = None;
			}
			i += 1;
			continue;
		}
		match c {
			'"' | '\'' => {
				in_quote = Some(c);
				out.push(c);
				i += 1;
			}
			'{' => {
				let close = find_matching_brace(chars, i).ok_or(JsxError::UnbalancedHole(i))?;
				out.push_str("${");
				out.extend(&chars[i + 1..close]);
				out.push('}');
				i = close + 1;
			}
			'/' if chars.get(i + 1) == Some(&'>') => return Ok((out, i + 2, true)),
			'>' => return Ok((out, i + 1, false)),
			_ => {
				out.push(c);
				i += 1;
			}
		}
	}
	Err(JsxError::UnterminatedTag(from))
}

/// Renders JSX children (`from` is just after the opening tag's `>`) up to
/// and including the matching closing tag, recursing into nested elements
/// and converting `{expr}` holes into `${expr}` along the way.
fn parse_children(chars: &[char], from: usize, tag_name: &str, is_fragment: bool) -> Result<(String, usize), JsxError> {
	let n = chars.len();
	let mut out = String::new();
	let mut i = from;

	while i < n {
		let c = chars[i];

		if c == '{' {
			let close = find_matching_brace(chars, i).ok_or(JsxError::UnbalancedHole(i))?;
			out.push_str("${");
			out.extend(&chars[i + 1..close]);
			out.push('}');
			i = close + 1;
			continue;
		}

		if c == '<' {
			if chars.get(i + 1) == Some(&'/') {
				let name_start = i + 2;
				let name_end = scan_tag_name_end(chars, name_start);
				let closing_name: String = chars[name_start..name_end].iter().collect();
				let mut k = name_end;
				while k < n && chars[k].is_whitespace() {
					k += 1;
				}
				if chars.get(k) != Some(&'>') {
					return Err(JsxError::MalformedClosingTag(i));
				}
				if closing_name != tag_name {
					return Err(JsxError::MismatchedClosingTag { expected: tag_name.to_string(), found: closing_name, at: i });
				}
				if is_fragment {
					out.push_str("</${Fragment}>");
				} else if is_component_name(tag_name) {
					out.push_str(&format!("</${{{tag_name}}}>"));
				} else {
					out.push_str(&format!("</{tag_name}>"));
				}
				return Ok((out, k + 1));
			}

			let (rendered, end) = parse_element(chars, i)?;
			out.push_str(&rendered);
			i = end;
			continue;
		}

		if c == '`' {
			// Escaped so the emitted `` html`...` `` template literal isn't
			// terminated early by a backtick that was just ordinary JSX text.
			out.push_str("\\`");
			i += 1;
			continue;
		}

		out.push(c);
		i += 1;
	}

	Err(JsxError::UnterminatedTag(from))
}

/// Parses one JSX element or fragment starting at `chars[start] == '<'`,
/// returning its rendered `html``-template text and the index one past its
/// closing tag.
fn parse_element(chars: &[char], start: usize) -> Result<(String, usize), JsxError> {
	if chars.get(start + 1) == Some(&'>') {
		let (children_rendered, end) = parse_children(chars, start + 2, "", true)?;
		return Ok((format!("<${{Fragment}}>{children_rendered}"), end));
	}

	let name_start = start + 1;
	let name_end = scan_tag_name_end(chars, name_start);
	let name: String = chars[name_start..name_end].iter().collect();

	let mut rendered = String::from("<");
	if is_component_name(&name) {
		rendered.push_str(&format!("${{{name}}}"));
	} else {
		rendered.push_str(&name);
	}

	let (attrs_rendered, tag_end, self_closing) = render_jsx_attrs(chars, name_end)?;
	rendered.push_str(&attrs_rendered);

	if self_closing {
		rendered.push_str("/>");
		return Ok((rendered, tag_end));
	}
	rendered.push('>');

	let (children_rendered, end) = parse_children(chars, tag_end, &name, false)?;
	rendered.push_str(&children_rendered);
	Ok((rendered, end))
}

/// Scans `source` for JSX roots and returns their spans in source order.
fn find_jsx_expressions(chars: &[char]) -> Result<Vec<JsxSpan>, JsxError> {
	let n = chars.len();
	let mut spans = Vec::new();
	let mut i = 0;

	while i < n {
		if let Some(next) = skip_js_comment(chars, i) {
			i = next;
			continue;
		}
		if let Some(next) = skip_js_string(chars, i) {
			i = next;
			continue;
		}
		if chars[i] == '<' && looks_like_jsx_start(chars, i) {
			let (rendered, end) = parse_element(chars, i)?;
			spans.push(JsxSpan { start: i, end, rendered: format!("html`{rendered}`") });
			i = end;
			continue;
		}
		i += 1;
	}

	Ok(spans)
}

/// Core transpile, kept as a plain Rust `Result<String, JsxError>` so it's
/// directly unit-testable without going through `wasm_bindgen`'s `JsValue`.
pub fn transpile_jsx_str(source: &str) -> Result<String, JsxError> {
	let chars: Vec<char> = source.chars().collect();
	let spans = find_jsx_expressions(&chars)?;

	let mut out = String::with_capacity(source.len());
	let mut cursor = 0;
	for span in spans {
		out.extend(&chars[cursor..span.start]);
		out.push_str(&span.rendered);
		cursor = span.end;
	}
	out.extend(&chars[cursor..]);
	Ok(out)
}

/// Transpiles JSX source text into plain JS, splicing `` html`...` `` calls
/// in place of each JSX root and leaving everything else untouched.
#[wasm_bindgen(js_name = transpileJsx)]
pub fn transpile_jsx(source: &str) -> Result<JsValue, JsValue> {
	transpile_jsx_str(source).map(|s| JsValue::from_str(&s)).map_err(|e| JsValue::from_str(&e.to_string()))
}
