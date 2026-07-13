//! Strips `import`/`export` syntax from a fetched `.jsx`/`.js` module body so
//! the result is valid as a `new Function(...)` body (bare `import`/`export`
//! are syntax errors there). Ports the two regex passes that used to live in
//! `index.html` (`extractImports` / `rewriteExports`) into plain char
//! scanning, so `loadJsxModule` only needs to fetch, call `prepareModule`,
//! then `transpileJsx`, and build the `Function`.

use wasm_bindgen::prelude::*;

/// One `import ... from '...'` line found in the source, with its specifier
/// shape preserved so a caller can resolve `from` against whatever modules
/// it already has on hand.
pub struct ImportSpecifier {
	pub named: Vec<String>,
	pub default_name: Option<String>,
	pub from: String,
}

fn is_ident_char(c: char) -> bool {
	c.is_alphanumeric() || c == '_' || c == '$'
}

/// Matches a single `import {a, b} from '...'` or `import def from '...'`
/// line in full (leading/trailing whitespace and an optional trailing `;`
/// allowed, nothing else), returning its specifier if the whole line fits.
fn parse_import_line(line: &str) -> Option<ImportSpecifier> {
	let chars: Vec<char> = line.chars().collect();
	let n = chars.len();
	let mut i = 0;
	while i < n && chars[i].is_whitespace() {
		i += 1;
	}
	if !chars[i..].starts_with(&['i', 'm', 'p', 'o', 'r', 't']) {
		return None;
	}
	i += 6;
	let before_ws = i;
	while i < n && chars[i].is_whitespace() {
		i += 1;
	}
	if i == before_ws {
		return None;
	}

	let (named, default_name, mut i) = if chars.get(i) == Some(&'{') {
		let close = chars[i..].iter().position(|&c| c == '}')? + i;
		let inner: String = chars[i + 1..close].iter().collect();
		let named: Vec<String> = inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
		(named, None, close + 1)
	} else {
		let start = i;
		while i < n && !chars[i].is_whitespace() {
			i += 1;
		}
		if i == start {
			return None;
		}
		(Vec::new(), Some(chars[start..i].iter().collect::<String>()), i)
	};

	let before_ws = i;
	while i < n && chars[i].is_whitespace() {
		i += 1;
	}
	if i == before_ws || !chars[i..].starts_with(&['f', 'r', 'o', 'm']) {
		return None;
	}
	i += 4;
	let before_ws = i;
	while i < n && chars[i].is_whitespace() {
		i += 1;
	}
	if i == before_ws {
		return None;
	}

	let quote = *chars.get(i)?;
	if quote != '\'' && quote != '"' {
		return None;
	}
	i += 1;
	let start = i;
	while i < n && chars[i] != quote {
		i += 1;
	}
	if i >= n || i == start {
		return None;
	}
	let from: String = chars[start..i].iter().collect();
	i += 1;

	while i < n && chars[i].is_whitespace() {
		i += 1;
	}
	if chars.get(i) == Some(&';') {
		i += 1;
	}
	while i < n && chars[i].is_whitespace() {
		i += 1;
	}
	if i != n {
		return None;
	}

	Some(ImportSpecifier { named, default_name, from })
}

/// Removes every whole-line `import ... from '...'` statement, blanking the
/// line in place (matching the JS version's line-anchored regex replace),
/// and collects what each one specified.
fn extract_imports(source: &str) -> (String, Vec<ImportSpecifier>) {
	let mut specifiers = Vec::new();
	let lines: Vec<String> = source
		.split('\n')
		.map(|line| match parse_import_line(line) {
			Some(spec) => {
				specifiers.push(spec);
				String::new()
			}
			None => line.to_string(),
		})
		.collect();
	(lines.join("\n"), specifiers)
}

/// Handles `export default function Name(...)` (keeps the declaration so it
/// can still reference itself by name, deferring the `exports.default`
/// assignment) and `export default <expr>` (rewritten inline). Only the
/// first occurrence is touched, matching the original non-global regexes.
fn rewrite_default_export(source: &str) -> (String, Option<String>) {
	let chars: Vec<char> = source.chars().collect();
	let n = chars.len();
	let mut i = 0;

	while i < n {
		if chars[i..].starts_with(&['e', 'x', 'p', 'o', 'r', 't']) && !chars.get(i + 6).is_some_and(|&c| is_ident_char(c)) {
			let mut j = i + 6;
			let before_ws = j;
			while j < n && chars[j].is_whitespace() {
				j += 1;
			}
			if j > before_ws && chars[j..].starts_with(&['d', 'e', 'f', 'a', 'u', 'l', 't']) && !chars.get(j + 7).is_some_and(|&c| is_ident_char(c)) {
				let mut k = j + 7;
				let before_ws = k;
				while k < n && chars[k].is_whitespace() {
					k += 1;
				}
				if k > before_ws {
					if chars[k..].starts_with(&['f', 'u', 'n', 'c', 't', 'i', 'o', 'n']) && !chars.get(k + 8).is_some_and(|&c| is_ident_char(c)) {
						let mut m = k + 8;
						while m < n && chars[m].is_whitespace() {
							m += 1;
						}
						let name_start = m;
						while m < n && is_ident_char(chars[m]) {
							m += 1;
						}
						if m > name_start {
							let name: String = chars[name_start..m].iter().collect();
							let mut out = String::with_capacity(source.len());
							out.extend(&chars[..i]);
							out.push_str("function ");
							out.extend(&chars[name_start..]);
							return (out, Some(name));
						}
					}
					let mut out = String::with_capacity(source.len());
					out.extend(&chars[..i]);
					out.push_str("exports.default = ");
					out.extend(&chars[k..]);
					return (out, None);
				}
			}
		}
		i += 1;
	}

	(source.to_string(), None)
}

/// Splits `local as alias` on a bare `as` keyword with at least one
/// whitespace character on each side (matching `/\s+as\s+/`, not a literal
/// single space, so tab-indented or double-spaced source still matches and
/// identifiers merely containing "as" — e.g. `gas`, `aslong` — don't).
fn split_as(part: &str) -> Option<(&str, &str)> {
	let bytes = part.as_bytes();
	let mut search_from = 0;
	while let Some(rel) = part[search_from..].find("as") {
		let at = search_from + rel;
		let before_is_ws = at > 0 && bytes[at - 1].is_ascii_whitespace();
		let after_is_ws = bytes.get(at + 2).is_some_and(u8::is_ascii_whitespace);
		if before_is_ws && after_is_ws {
			return Some((part[..at].trim_end(), part[at + 2..].trim_start()));
		}
		search_from = at + 2;
	}
	None
}

/// Rewrites every `export { a, b as c };` re-export line into assignments
/// onto `exports`, recording each so they can be appended after the code.
fn rewrite_named_reexports(source: &str, exported: &mut Vec<String>) -> String {
	source
		.split('\n')
		.map(|line| {
			let trimmed = line.trim();
			let Some(after_export) = trimmed.strip_prefix("export") else { return line.to_string() };
			let after_export = after_export.trim_start();
			let Some(after_brace) = after_export.strip_prefix('{') else { return line.to_string() };
			let Some(close) = after_brace.find('}') else { return line.to_string() };
			let names_part = &after_brace[..close];
			let mut after_close = after_brace[close + 1..].trim_start();
			after_close = after_close.strip_prefix(';').unwrap_or(after_close).trim();
			if !after_close.is_empty() {
				return line.to_string();
			}

			for part in names_part.split(',') {
				let part = part.trim();
				if part.is_empty() {
					continue;
				}
				if let Some((local, alias)) = split_as(part) {
					exported.push(format!("exports.{alias} = {local};"));
				} else {
					exported.push(format!("exports.{part} = {part};"));
				}
			}
			String::new()
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// Strips the `export` keyword from `export const|let|var|function|class
/// Name ...` declarations (every line, unlike the two passes above), keeping
/// the rest of the line untouched and recording each declared name. Accepts
/// any run of whitespace after `export` (tabs, multiple spaces), not just a
/// single literal space, matching the original `/\s+/` regex.
fn rewrite_export_declarations(source: &str, exported: &mut Vec<String>) -> String {
	const KINDS: [&str; 5] = ["const", "let", "var", "function", "class"];
	source
		.split('\n')
		.map(|line| {
			let ws_len = line.len() - line.trim_start().len();
			let (ws, rest) = line.split_at(ws_len);
			let Some(after_export_kw) = rest.strip_prefix("export") else { return line.to_string() };
			if !after_export_kw.starts_with(|c: char| c.is_whitespace()) {
				return line.to_string();
			}
			let gap_len = after_export_kw.len() - after_export_kw.trim_start().len();
			let after_export = &after_export_kw[gap_len..];

			for kind in KINDS {
				let Some(after_kind) = after_export.strip_prefix(kind) else { continue };
				if !after_kind.starts_with(|c: char| c.is_whitespace()) {
					continue;
				}
				let trimmed = after_kind.trim_start();
				let name_end = trimmed.find(|c: char| !is_ident_char(c)).unwrap_or(trimmed.len());
				if name_end == 0 {
					continue;
				}
				let name = &trimmed[..name_end];
				exported.push(format!("exports.{name} = {name};"));
				return format!("{ws}{kind}{after_kind}");
			}
			line.to_string()
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// Core rewrite, kept as plain Rust so it's directly unit-testable.
pub fn rewrite_exports_str(source: &str) -> String {
	let mut exported = Vec::new();
	let (code, default_name) = rewrite_default_export(source);
	if let Some(name) = default_name {
		exported.push(format!("exports.default = {name};"));
	}
	let code = rewrite_named_reexports(&code, &mut exported);
	let mut code = rewrite_export_declarations(&code, &mut exported);

	if !exported.is_empty() {
		code.push('\n');
		code.push_str(&exported.join("\n"));
	}
	code
}

/// Runs both passes: strip `import` lines (collecting their specifiers),
/// then rewrite `export` syntax on what's left.
pub fn prepare_module_str(source: &str) -> (String, Vec<ImportSpecifier>) {
	let (code, specifiers) = extract_imports(source);
	(rewrite_exports_str(&code), specifiers)
}

/// Builds the `{ named: string[], defaultName: string|null, from: string }`
/// object matching the shape `index.html`'s `extractImports` used to return.
fn specifier_to_js(spec: &ImportSpecifier) -> Result<JsValue, JsValue> {
	let obj = js_sys::Object::new();
	let named = js_sys::Array::new();
	for name in &spec.named {
		named.push(&JsValue::from_str(name));
	}
	js_sys::Reflect::set(&obj, &"named".into(), &named)?;
	let default_name = spec.default_name.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL);
	js_sys::Reflect::set(&obj, &"defaultName".into(), &default_name)?;
	js_sys::Reflect::set(&obj, &"from".into(), &JsValue::from_str(&spec.from))?;
	Ok(obj.into())
}

/// `{ code, specifiers }` for a fetched module body, ready to be passed
/// through `transpileJsx` and then into `new Function(...)`.
#[wasm_bindgen(js_name = prepareModule)]
pub fn prepare_module(source: &str) -> Result<JsValue, JsValue> {
	let (code, specifiers) = prepare_module_str(source);

	let specifiers_arr = js_sys::Array::new();
	for spec in &specifiers {
		specifiers_arr.push(&specifier_to_js(spec)?);
	}

	let out = js_sys::Object::new();
	js_sys::Reflect::set(&out, &"code".into(), &JsValue::from_str(&code))?;
	js_sys::Reflect::set(&out, &"specifiers".into(), &specifiers_arr)?;
	Ok(out.into())
}
