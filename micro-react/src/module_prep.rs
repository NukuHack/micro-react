use wasm_bindgen::prelude::*;

/// One `import ... from '...'` line found in the source, with its specifier
/// shape preserved so a caller can resolve `from` against whatever modules
/// it already has on hand.
///
/// A specifier with `named`, `default_name`, and `namespace_name` all
/// empty/`None` is a bare, side-effect-only import (`import './x.css';`) —
/// there's no runtime binding at all, just a request to load `from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSpecifier {
	// Holds pairs of: (local_alias, exported_name)
	pub named: Vec<(String, String)>,
	pub default_name: Option<String>,
	pub namespace_name: Option<String>,
	pub from: String,
}

fn is_ident_char(c: char) -> bool {
	c.is_alphanumeric() || c == '_' || c == '$'
}

/// Rewrites a *relative* dynamic `import('./x.jsx')` / `import("../x.jsx")`
/// call into `__mrDynImport("<base_url>", './x.jsx')`, so it resolves
/// against the module's own real URL instead of whatever script the
/// browser considers "currently running" — see `jsx.rs` for why that's
/// not the same thing once the code is executing inside the
/// `AsyncFunction` module bodies are run through.
///
/// Deliberately narrow: only a dynamic import whose *literal* specifier
/// starts with `./` or `../` is rewritten. That's exactly (and only) the
/// case that's actually broken — a real bundler (Vite, webpack) resolves
/// those against the file that wrote them, which is also what `__mrDynImport`
/// now does. A dynamic import of an absolute URL, or of a variable
/// (`import(someUrl)`, e.g. loading a third-party module from a CDN) already
/// works fine as a native `import()` and is left completely alone.
#[must_use]
pub fn rewrite_dynamic_imports(source: &str, base_url: &str) -> String {
	let chars: Vec<char> = source.chars().collect();
	let total = chars.len();
	let mut out = String::with_capacity(source.len());
	let mut cursor = 0;

	while cursor < total {
		let starts_here = chars[cursor..].starts_with(&['i', 'm', 'p', 'o', 'r', 't']);
		let preceded_by_ident = cursor > 0 && is_ident_char(chars[cursor - 1]);
		if !starts_here || preceded_by_ident {
			out.push(chars[cursor]);
			cursor += 1;
			continue;
		}

		let mut after = cursor + 6;
		while after < total && chars[after].is_whitespace() {
			after += 1;
		}
		let is_call = chars.get(after) == Some(&'(');
		if !is_call {
			out.push(chars[cursor]);
			cursor += 1;
			continue;
		}

		let mut arg_start = after + 1;
		while arg_start < total && chars[arg_start].is_whitespace() {
			arg_start += 1;
		}
		let quote = chars.get(arg_start).copied();
		let is_relative = matches!(quote, Some('\'' | '"'))
			&& (chars[arg_start + 1..].starts_with(&['.', '/']) || chars[arg_start + 1..].starts_with(&['.', '.', '/']));

		if !is_relative {
			out.push(chars[cursor]);
			cursor += 1;
			continue;
		}

		out.push_str("__mrDynImport(");
		out.push('"');
		out.push_str(&escape_js_string(base_url));
		out.push('"');
		out.push_str(", ");
		cursor = after + 1; // resume right after the "(" — the original specifier argument follows untouched
	}

	out
}

fn escape_js_string(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for c in s.chars() {
		match c {
			'\\' => out.push_str("\\\\"),
			'"' => out.push_str("\\\""),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			_ => out.push(c),
		}
	}
	out
}

/// Matches a single `import {a, b} from '...'`, `import def from '...'`, or
/// bare `import '...'` line in full (leading/trailing whitespace and an
/// optional trailing `;` allowed, nothing else), returning its specifier if
/// the whole line fits.
pub fn parse_import_line(line: &str) -> Option<ImportSpecifier> {
	let trimmed = line.trim();
	if !trimmed.starts_with("import") {
		return None;
	}

	// Ensure "import" is matched as a whole word
	let rest = &trimmed[6..];
	if !rest.starts_with(char::is_whitespace) {
		return None;
	}
	let mut rest = rest.trim();

	let mut default_name = None;
	let mut namespace_name = None;
	let mut named = Vec::new();

	let is_ident_char = |c: char| c.is_alphanumeric() || c == '_' || c == '$';

	// Side-effect-only import (`import './x.css';`): no bindings and no
	// `from` keyword at all, just a bare specifier string. Must be checked
	// before the default-import branch below, which would otherwise see
	// the leading quote, find zero identifier characters, and bail out
	// with `None` — leaving the (illegal, once run through `new
	// Function`) `import` statement untouched in the transpiled code.
	if rest.starts_with('\'') || rest.starts_with('"') {
		let quote = rest.chars().next()?;
		let after_quote = &rest[1..];
		let close_quote_idx = after_quote.find(quote)?;
		let from = after_quote[..close_quote_idx].to_string();
		let trailing = after_quote[close_quote_idx + 1..].trim();
		let trailing = trailing.strip_prefix(';').unwrap_or(trailing).trim();
		if !trailing.is_empty() {
			return None;
		}
		return Some(ImportSpecifier { named: Vec::new(), default_name: None, namespace_name: None, from });
	}

	// TS type-only imports (`import type { Foo } from '...'` / `import
	// type Foo from '...'`) carry no runtime binding at all. Without this
	// check, "type" would fall through to the default-import branch below
	// and get silently (and wrongly) extracted as `default_name: "type"`.
	// Real TS distinguishes this from a default import that happens to be
	// named `type` (`import type from '...'`, legal since `type` isn't a
	// reserved word) by checking whether the *next* token is `from` — if
	// so, `type` is the binding itself, not the type-only modifier.
	if let Some(after_type) = rest.strip_prefix("type")
		&& after_type.starts_with(char::is_whitespace)
	{
		let after_type = after_type.trim_start();
		if !after_type.starts_with("from") || after_type[4..].starts_with(is_ident_char) {
			// Genuine `import type ...`: not a value-level import this
			// parser (or the runtime module loader) should act on. Bail
			// out and leave the line untouched rather than guessing.
			return None;
		}
	}

	// 1. Parse default import if it exists
	if !rest.starts_with('{') && !rest.starts_with('*') {
		let ident_len = rest.chars().take_while(|&c| is_ident_char(c)).count();
		if ident_len == 0 {
			return None;
		}
		let ident = &rest[..ident_len];
		default_name = Some(ident.to_string());
		rest = rest[ident_len..].trim();

		// Handle mixed imports comma separator (e.g., import Foo, { bar } ...)
		if rest.starts_with(',') {
			rest = rest[1..].trim();
		}
	}

	// 2. Parse namespace wildcard (* as ns) or named imports ({ a, b })
	if rest.starts_with('*') {
		rest = rest[1..].trim();
		if !rest.starts_with("as") {
			return None;
		}
		rest = &rest[2..];
		if !rest.starts_with(char::is_whitespace) {
			return None;
		}
		rest = rest.trim();
		let ident_len = rest.chars().take_while(|&c| is_ident_char(c)).count();
		if ident_len == 0 {
			return None;
		}
		namespace_name = Some(rest[..ident_len].to_string());
		rest = rest[ident_len..].trim();
	} else if rest.starts_with('{') {
		let close_idx = rest.find('}')?;
		let inner = &rest[1..close_idx];
		for part in inner.split(',') {
			let p = part.trim();
			if p.is_empty() {
				continue;
			}
			let words: Vec<&str> = p.split_whitespace().collect();
			if words.len() == 3 && words[1] == "as" {
				named.push((words[2].to_string(), words[0].to_string()));
			} else if let Some(&word) = words.first() {
				named.push((word.to_string(), word.to_string()));
			}
		}
		rest = rest[close_idx + 1..].trim();
	}

	// 3. Match "from" keyword
	if !rest.starts_with("from") {
		return None;
	}
	rest = &rest[4..];
	if !rest.starts_with(char::is_whitespace) {
		return None;
	}
	rest = rest.trim();

	// 4. Parse module specifier string
	if rest.is_empty() {
		return None;
	}
	let quote = rest.chars().next()?;
	if quote != '\'' && quote != '"' {
		return None;
	}
	let rest = &rest[1..];
	let close_quote_idx = rest.find(quote)?;
	let from = rest[..close_quote_idx].to_string();
	let trailing = rest[close_quote_idx + 1..].trim();
	let trailing = trailing.strip_prefix(';').unwrap_or(trailing).trim();

	// 5. Ensure there is nothing trailing except an optional semicolon
	if !trailing.is_empty() {
		return None;
	}

	Some(ImportSpecifier { named, default_name, namespace_name, from })
}

/// Specifiers that resolve to identifiers already available in scope (e.g.
/// injected globals) rather than a real fetchable module. An `import ...
/// from` line naming one of these is still stripped from the source — the
/// statement would be a syntax error left in place, since module bodies run
/// through `new AsyncFunction`, not an actual ES module — but it is dropped
/// before it reaches the specifier list, so `load_module_body` never tries
/// to fetch/resolve it and never rebinds the imported names as parameters.
/// The imported identifiers (`StrictMode`, `createRoot`, `Route`, ...) fall
/// through to whatever is already bound in the surrounding scope instead.
const SKIPPED_SPECIFIERS: &[&str] = &["react", "react-dom/client", "react-router-dom"];

#[must_use]
pub fn extract_imports(source: &str) -> (String, Vec<ImportSpecifier>) {
	let mut specifiers = Vec::new();
	let lines: Vec<String> = source
		.split('\n')
		.map(|line| {
			parse_import_line(line).map_or_else(
				|| line.to_string(),
				|spec| {
					if !SKIPPED_SPECIFIERS.contains(&spec.from.as_str()) {
						specifiers.push(spec);
					}
					String::new()
				},
			)
		})
		.collect();
	(lines.join("\n"), specifiers)
}

#[must_use]
pub fn rewrite_default_export(source: &str) -> (String, Option<String>) {
	let chars: Vec<char> = source.chars().collect();
	let total_chars = chars.len();
	let mut cursor = 0;

	while cursor < total_chars {
		if chars[cursor..].starts_with(&['e', 'x', 'p', 'o', 'r', 't']) && !chars.get(cursor + 6).is_some_and(|&ch| is_ident_char(ch)) {
			let mut next = cursor + 6;
			let before_ws = next;
			while next < total_chars && chars[next].is_whitespace() {
				next += 1;
			}
			if next > before_ws
				&& chars[next..].starts_with(&['d', 'e', 'f', 'a', 'u', 'l', 't'])
				&& !chars.get(next + 7).is_some_and(|&ch| is_ident_char(ch))
			{
				let mut after_default = next + 7;
				let before_ws = after_default;
				while after_default < total_chars && chars[after_default].is_whitespace() {
					after_default += 1;
				}
				if after_default > before_ws {
					if chars[after_default..].starts_with(&['f', 'u', 'n', 'c', 't', 'i', 'o', 'n'])
						&& !chars.get(after_default + 8).is_some_and(|&ch| is_ident_char(ch))
					{
						let mut name_cursor = after_default + 8;
						while name_cursor < total_chars && chars[name_cursor].is_whitespace() {
							name_cursor += 1;
						}
						let name_start = name_cursor;
						while name_cursor < total_chars && is_ident_char(chars[name_cursor]) {
							name_cursor += 1;
						}
						if name_cursor > name_start {
							let name: String = chars[name_start..name_cursor].iter().collect();
							let mut out = String::with_capacity(source.len());
							out.extend(&chars[..cursor]);
							out.push_str("function ");
							out.extend(&chars[name_start..]);
							return (out, Some(name));
						}
					}
					let mut out = String::with_capacity(source.len());
					out.extend(&chars[..cursor]);
					out.push_str("exports.default = ");
					out.extend(&chars[after_default..]);
					return (out, None);
				}
			}
		}
		cursor += 1;
	}

	(source.to_string(), None)
}

pub fn split_as(part: &str) -> Option<(&str, &str)> {
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

pub fn rewrite_named_reexports(source: &str, exported: &mut Vec<String>) -> String {
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

pub fn rewrite_export_declarations(source: &str, exported: &mut Vec<String>) -> String {
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

#[must_use]
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

#[must_use]
pub fn prepare_module_str(source: &str) -> (String, Vec<ImportSpecifier>) {
	let (code, specifiers) = extract_imports(source);
	(rewrite_exports_str(&code), specifiers)
}

fn specifier_to_js(spec: &ImportSpecifier) -> Result<JsValue, JsValue> {
	let obj = js_sys::Object::new();

	let named = js_sys::Array::new();
	for (local, exported) in &spec.named {
		let pair = js_sys::Array::new();
		pair.push(&JsValue::from_str(local));
		pair.push(&JsValue::from_str(exported));
		named.push(&pair);
	}
	js_sys::Reflect::set(&obj, &"named".into(), &named)?;

	let default_name = spec.default_name.as_deref().map_or(JsValue::NULL, JsValue::from_str);
	js_sys::Reflect::set(&obj, &"defaultName".into(), &default_name)?;

	let namespace_name = spec.namespace_name.as_deref().map_or(JsValue::NULL, JsValue::from_str);
	js_sys::Reflect::set(&obj, &"namespaceName".into(), &namespace_name)?;

	js_sys::Reflect::set(&obj, &"from".into(), &JsValue::from_str(&spec.from))?;
	Ok(obj.into())
}

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

#[cfg(test)]
mod tests {
	#![allow(clippy::expect_used, clippy::unwrap_used)]
	use super::*;

	#[test]
	fn parses_bare_side_effect_import() {
		let spec = parse_import_line("import './css/main.css';").expect("should parse");
		assert_eq!(spec.from, "./css/main.css");
		assert!(spec.default_name.is_none());
		assert!(spec.namespace_name.is_none());
		assert!(spec.named.is_empty());
	}

	#[test]
	fn parses_bare_side_effect_import_no_semicolon() {
		let spec = parse_import_line("import '../css/list.css'").expect("should parse");
		assert_eq!(spec.from, "../css/list.css");
	}

	#[test]
	fn parses_default_url_import_with_query() {
		let spec = parse_import_line("import listCssUrl from '../css/list.css?url'").expect("should parse");
		assert_eq!(spec.from, "../css/list.css?url");
		assert_eq!(spec.default_name.as_deref(), Some("listCssUrl"));
	}

	#[test]
	fn bare_import_rejects_trailing_garbage() {
		assert!(parse_import_line("import './x.css' extra").is_none());
	}

	#[test]
	fn rewrites_relative_dynamic_import_single_quote() {
		let out = rewrite_dynamic_imports("const P = lazy(() => import('./pages/Dice.jsx'));", "https://x/src/App.jsx");
		assert_eq!(out, "const P = lazy(() => __mrDynImport(\"https://x/src/App.jsx\", './pages/Dice.jsx'));");
	}

	#[test]
	fn rewrites_relative_dynamic_import_double_quote_and_dotdot() {
		let out = rewrite_dynamic_imports(r#"import("../x.jsx")"#, "https://x/y");
		assert_eq!(out, r#"__mrDynImport("https://x/y", "../x.jsx")"#);
	}

	#[test]
	fn leaves_absolute_and_variable_dynamic_imports_alone() {
		assert_eq!(rewrite_dynamic_imports("import(url)", "b"), "import(url)");
		assert_eq!(rewrite_dynamic_imports("import('https://cdn.example.com/x.js')", "b"), "import('https://cdn.example.com/x.js')");
	}

	#[test]
	fn leaves_import_meta_and_lookalike_identifiers_alone() {
		assert_eq!(rewrite_dynamic_imports("import.meta.url", "b"), "import.meta.url");
		assert_eq!(rewrite_dynamic_imports("const reimport = 1;", "b"), "const reimport = 1;");
		assert_eq!(rewrite_dynamic_imports("importantValue(x)", "b"), "importantValue(x)");
	}

	#[test]
	fn escapes_quotes_and_backslashes_in_base_url() {
		let out = rewrite_dynamic_imports("import('./a.jsx')", "https://x/\"y\\z");
		assert_eq!(out, r#"__mrDynImport("https://x/\"y\\z", './a.jsx')"#);
	}

	#[test]
	fn skipped_specifiers_are_stripped_but_not_resolved() {
		let source = "import { StrictMode } from 'react';\n\
			import { createRoot } from 'react-dom/client';\n\
			import { Route, Routes, BrowserRouter } from 'react-router-dom';\n\
			import { useThing } from './useThing.js';\n\
			const x = 1;";
		let (code, specifiers) = extract_imports(source);

		// None of the three skipped specifiers made it into the list that
		// gets fetched/resolved.
		assert!(specifiers.iter().all(|s| s.from != "react"));
		assert!(specifiers.iter().all(|s| s.from != "react-dom/client"));
		assert!(specifiers.iter().all(|s| s.from != "react-router-dom"));

		// A normal, non-skipped import is still resolved as usual.
		assert!(specifiers.iter().any(|s| s.from == "./useThing.js"));

		// The skipped import lines are gone from the emitted code (each
		// becomes an empty line), so the body stays valid to execute.
		assert!(!code.contains("import"));
		assert!(code.contains("const x = 1;"));
	}
}
