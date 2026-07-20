//! Low-level, allocation-light character scanning helpers shared by
//! `html_template` (scanning the compiled sentinel HTML string) and `jsx`
//! (scanning raw JSX/JS source text). Kept dependency-free and generic so
//! neither caller needs to know about the other's file format.

// ───────────────────────── HTML-flavored scanning ─────────────────────────

/// If `chars[i..]` starts an HTML comment (`<!--`), returns the index just
/// past the closing `-->`. Used to skip comment bodies so tag-like text
/// inside them isn't mistaken for a real tag.
pub(crate) fn skip_html_comment(chars: &[char], i: usize) -> Option<usize> {
	if !chars[i..].starts_with(&['<', '!', '-', '-']) {
		return None;
	}
	let mut j = i + 4;
	let n = chars.len();
	while j < n && !chars[j..].starts_with(&['-', '-', '>']) {
		j += 1;
	}
	Some((j + 3).min(n))
}

/// If `chars[i]` starts a `<!...>` declaration (doctype and similar, but
/// not a comment), returns the index just past the closing `>`.
pub(crate) fn skip_html_doctype(chars: &[char], i: usize) -> Option<usize> {
	if chars.get(i) != Some(&'<') || chars.get(i + 1) != Some(&'!') {
		return None;
	}
	let n = chars.len();
	let mut j = i;
	while j < n && chars[j] != '>' {
		j += 1;
	}
	Some((j + 1).min(n))
}

/// Scans a tag name starting at `start` (right after `<` or `</`), returning
/// the index one past the last name character.
pub fn scan_tag_name_end(chars: &[char], start: usize) -> usize {
	let n = chars.len();
	let mut j = start;
	while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_' | ':' | '.')) {
		j += 1;
	}
	j
}

/// Result of scanning forward from just after a tag name to the tag's end.
pub(crate) struct TagEnd {
	/// Index one past the closing `>` (or past `/>` for a self-closing tag).
	pub end: usize,
	pub self_closing: bool,
}

/// Quote-aware scan from `from` (typically right after a tag name) to the
/// end of the tag, so a `/` or `>` inside a `"..."`/`'...'` attribute value
/// isn't mistaken for tag syntax.
pub(crate) fn scan_html_tag_end(chars: &[char], from: usize) -> TagEnd {
	let n = chars.len();
	let mut k = from;
	let mut in_quote: Option<char> = None;
	let mut self_closing = false;

	while k < n {
		let ch = chars[k];
		match in_quote {
			Some(q) => {
				if ch == q {
					in_quote = None;
				}
				k += 1;
			}
			None => match ch {
				'"' | '\'' => {
					in_quote = Some(ch);
					k += 1;
				}
				'>' => break,
				'/' if chars.get(k + 1) == Some(&'>') => {
					self_closing = true;
					k += 1; // now indexes '>'
					break;
				}
				_ => k += 1,
			},
		}
	}
	let end = if k < n { k + 1 } else { n };
	TagEnd { end, self_closing }
}

// ────────────────────────── JS-flavored scanning ──────────────────────────

/// If `chars[i]` opens a JS string or template literal (`'`, `"`, `` ` ``),
/// returns the index just past its matching close, correctly skipping
/// backslash-escaped quote characters. Template literals may contain
/// `${...}` holes with arbitrary nested code (including more strings and
/// braces); those are skipped via [`find_matching_brace`] rather than by
/// naively scanning for the next backtick.
pub fn skip_js_string(chars: &[char], i: usize) -> Option<usize> {
	let quote = *chars.get(i)?;
	if !matches!(quote, '\'' | '"' | '`') {
		return None;
	}
	let n = chars.len();
	let mut j = i + 1;
	while j < n {
		match chars[j] {
			'\\' => j += 2,
			c if c == quote => return Some(j + 1),
			'$' if quote == '`' && chars.get(j + 1) == Some(&'{') => {
				j = find_matching_brace(chars, j + 1).map(|close| close + 1).unwrap_or(n);
			}
			_ => j += 1,
		}
	}
	Some(n)
}

/// If `chars[i..]` starts a `//` or `/* */` JS comment, returns the index
/// just past its end (end-of-line for `//`, past `*/` for block comments —
/// or end-of-input if unterminated).
pub fn skip_js_comment(chars: &[char], i: usize) -> Option<usize> {
	let n = chars.len();
	if chars[i..].starts_with(&['/', '/']) {
		let mut j = i + 2;
		while j < n && chars[j] != '\n' {
			j += 1;
		}
		return Some(j);
	}
	if chars[i..].starts_with(&['/', '*']) {
		let mut j = i + 2;
		while j < n && !chars[j..].starts_with(&['*', '/']) {
			j += 1;
		}
		return Some((j + 2).min(n));
	}
	None
}

/// Finds the index of the `}` matching the `{` at `open` (which must point
/// at a literal `{`), skipping over nested braces, JS strings/template
/// literals, and comments so characters inside them can't corrupt the
/// balance count. Returns `None` if the input ends before the brace closes.
pub fn find_matching_brace(chars: &[char], open: usize) -> Option<usize> {
	debug_assert_eq!(chars.get(open), Some(&'{'));
	let n = chars.len();
	let mut depth = 0usize;
	let mut i = open;
	while i < n {
		if let Some(next) = skip_js_comment(chars, i) {
			i = next;
			continue;
		}
		if let Some(next) = skip_js_string(chars, i) {
			i = next;
			continue;
		}
		match chars[i] {
			'{' => depth += 1,
			'}' => {
				depth -= 1;
				if depth == 0 {
					return Some(i);
				}
			}
			_ => {}
		}
		i += 1;
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn skip_html_comment_finds_end() {
		let chars: Vec<char> = "<!-- hi </div> -->rest".chars().collect();
		assert_eq!(skip_html_comment(&chars, 0), Some(18));
	}

	#[test]
	fn skip_html_comment_none_for_non_comment() {
		let chars: Vec<char> = "<div>".chars().collect();
		assert_eq!(skip_html_comment(&chars, 0), None);
	}

	#[test]
	fn scan_html_tag_end_detects_self_closing() {
		let chars: Vec<char> = "<img src=\"a/b\"/>rest".chars().collect();
		let name_end = scan_tag_name_end(&chars, 1);
		let tag_end = scan_html_tag_end(&chars, name_end);
		assert!(tag_end.self_closing);
		assert_eq!(chars[tag_end.end - 1], '>');
	}

	#[test]
	fn scan_html_tag_end_ignores_slash_in_quotes() {
		let chars: Vec<char> = "<a href='a/b/c'>text</a>".chars().collect();
		let name_end = scan_tag_name_end(&chars, 1);
		let tag_end = scan_html_tag_end(&chars, name_end);
		assert!(!tag_end.self_closing);
	}

	#[test]
	fn find_matching_brace_skips_strings_and_comments() {
		let src = "{ if (x) { doThing(\"}\"); } // } trailing comment\n}";
		let chars: Vec<char> = src.chars().collect();
		let close = find_matching_brace(&chars, 0).expect("brace should match");
		assert_eq!(chars[close], '}');
		assert_eq!(close, chars.len() - 1);
	}

	#[test]
	fn find_matching_brace_handles_template_literal_holes() {
		let src = "{ `a${ {} }b` }";
		let chars: Vec<char> = src.chars().collect();
		let close = find_matching_brace(&chars, 0).expect("brace should match");
		assert_eq!(close, chars.len() - 1);
	}

	#[test]
	fn find_matching_brace_none_when_unterminated() {
		let chars: Vec<char> = "{ still open".chars().collect();
		assert_eq!(find_matching_brace(&chars, 0), None);
	}

	#[test]
	fn find_matching_brace_handles_escaped_quotes_in_strings() {
		// The escaped quote inside the string must not be mistaken for the
		// string's terminator, which would otherwise throw off brace counting.
		let src = r#"{ f("a\"}b"); }"#;
		let chars: Vec<char> = src.chars().collect();
		let close = find_matching_brace(&chars, 0).expect("brace should match");
		assert_eq!(close, chars.len() - 1);
	}

	#[test]
	fn find_matching_brace_single_quoted_string_with_braces() {
		let src = "{ f('} not a close') }";
		let chars: Vec<char> = src.chars().collect();
		let close = find_matching_brace(&chars, 0).expect("brace should match");
		assert_eq!(close, chars.len() - 1);
	}

	#[test]
	fn find_matching_brace_nested_object_literal() {
		let src = "{ style: { color: 'red' } }";
		let chars: Vec<char> = src.chars().collect();
		let close = find_matching_brace(&chars, 0).expect("brace should match");
		assert_eq!(close, chars.len() - 1);
	}

	#[test]
	fn find_matching_brace_skips_block_comment_containing_brace() {
		let src = "{ /* } */ x }";
		let chars: Vec<char> = src.chars().collect();
		let close = find_matching_brace(&chars, 0).expect("brace should match");
		assert_eq!(close, chars.len() - 1);
	}

	// ── skip_html_doctype ──

	#[test]
	fn skip_html_doctype_finds_end_of_doctype() {
		let chars: Vec<char> = "<!DOCTYPE html>rest".chars().collect();
		assert_eq!(skip_html_doctype(&chars, 0), Some(15));
	}

	#[test]
	fn skip_html_doctype_none_for_ordinary_tag() {
		let chars: Vec<char> = "<div>".chars().collect();
		assert_eq!(skip_html_doctype(&chars, 0), None);
	}

	#[test]
	fn skip_html_doctype_none_for_comment() {
		// `<!--` is a comment, not a doctype-like declaration; callers try
		// `skip_html_comment` first, but this should still refuse to match it.
		let chars: Vec<char> = "<!-- not a doctype -->".chars().collect();
		assert_eq!(skip_html_doctype(&chars, 0), Some(chars.len()));
	}

	#[test]
	fn skip_html_doctype_unterminated_stops_at_input_end() {
		let chars: Vec<char> = "<!DOCTYPE html".chars().collect();
		assert_eq!(skip_html_doctype(&chars, 0), Some(chars.len()));
	}

	// ── skip_js_string ──

	#[test]
	fn skip_js_string_double_quoted() {
		let chars: Vec<char> = r#""hello" rest"#.chars().collect();
		assert_eq!(skip_js_string(&chars, 0), Some(7));
	}

	#[test]
	fn skip_js_string_single_quoted() {
		let chars: Vec<char> = "'hello' rest".chars().collect();
		assert_eq!(skip_js_string(&chars, 0), Some(7));
	}

	#[test]
	fn skip_js_string_handles_escaped_quote() {
		let chars: Vec<char> = r#""a\"b" rest"#.chars().collect();
		let end = skip_js_string(&chars, 0).expect("should find end");
		let s: String = chars[0..end].iter().collect();
		assert_eq!(s, r#""a\"b""#);
	}

	#[test]
	fn skip_js_string_none_for_non_quote_start() {
		let chars: Vec<char> = "not a string".chars().collect();
		assert_eq!(skip_js_string(&chars, 0), None);
	}

	#[test]
	fn skip_js_string_template_literal_with_hole() {
		let chars: Vec<char> = "`a${b}c` rest".chars().collect();
		assert_eq!(skip_js_string(&chars, 0), Some(8));
	}

	#[test]
	fn skip_js_string_template_literal_hole_containing_braces() {
		let chars: Vec<char> = "`a${ {} }c` rest".chars().collect();
		let end = skip_js_string(&chars, 0).expect("should find end");
		assert_eq!(chars[end - 1], '`');
	}

	#[test]
	fn skip_js_string_unterminated_reaches_end_of_input() {
		let chars: Vec<char> = "\"no closing quote".chars().collect();
		assert_eq!(skip_js_string(&chars, 0), Some(chars.len()));
	}

	// ── skip_js_comment ──

	#[test]
	fn skip_js_comment_line_comment_stops_before_newline() {
		let chars: Vec<char> = "// hi\nrest".chars().collect();
		assert_eq!(skip_js_comment(&chars, 0), Some(5));
	}

	#[test]
	fn skip_js_comment_block_comment_finds_close() {
		let chars: Vec<char> = "/* hi */rest".chars().collect();
		assert_eq!(skip_js_comment(&chars, 0), Some(8));
	}

	#[test]
	fn skip_js_comment_unterminated_block_reaches_end() {
		let chars: Vec<char> = "/* never closes".chars().collect();
		assert_eq!(skip_js_comment(&chars, 0), Some(chars.len()));
	}

	#[test]
	fn skip_js_comment_none_for_plain_division() {
		let chars: Vec<char> = "a / b".chars().collect();
		assert_eq!(skip_js_comment(&chars, 2), None);
	}

	#[test]
	fn skip_js_comment_line_comment_directly_abutting_tag() {
		// No space between the comment's end and the following `<tag>`.
		let chars: Vec<char> = "// note\n<div>".chars().collect();
		let end = skip_js_comment(&chars, 0).expect("should find end of line comment");
		assert_eq!(chars[end], '\n');
		let tag_start = end + 1;
		assert_eq!(chars[tag_start], '<');
		let name_end = scan_tag_name_end(&chars, tag_start + 1);
		let name: String = chars[tag_start + 1..name_end].iter().collect();
		assert_eq!(name, "div");
	}

	#[test]
	fn skip_js_comment_block_comment_directly_abutting_tag() {
		// No space between the `*/` and the following `<tag>`.
		let chars: Vec<char> = "/* note */<span>".chars().collect();
		let end = skip_js_comment(&chars, 0).expect("should find end of block comment");
		assert_eq!(chars[end], '<');
		let name_end = scan_tag_name_end(&chars, end + 1);
		let name: String = chars[end + 1..name_end].iter().collect();
		assert_eq!(name, "span");
	}
}

#[cfg(test)]
mod dotted_name_tests {
	use super::*;

	#[test]
	fn scan_tag_name_end_includes_dots() {
		let chars: Vec<char> = "Context.Provider>".chars().collect();
		assert_eq!(scan_tag_name_end(&chars, 0), 16);
	}
}
