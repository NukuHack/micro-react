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
pub(crate) fn scan_tag_name_end(chars: &[char], start: usize) -> usize {
	let n = chars.len();
	let mut j = start;
	while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_' | ':')) {
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
pub(crate) fn skip_js_string(chars: &[char], i: usize) -> Option<usize> {
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
pub(crate) fn skip_js_comment(chars: &[char], i: usize) -> Option<usize> {
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
pub(crate) fn find_matching_brace(chars: &[char], open: usize) -> Option<usize> {
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
		let src = r#"{ if (x) { doThing("}"); } // } trailing comment
}"#;
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
}
