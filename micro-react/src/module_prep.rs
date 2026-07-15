use wasm_bindgen::prelude::*;

/// One `import ... from '...'` line found in the source, with its specifier
/// shape preserved so a caller can resolve `from` against whatever modules
/// it already has on hand.
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

/// Matches a single `import {a, b} from '...'` or `import def from '...'`
/// line in full (leading/trailing whitespace and an optional trailing `;`
/// allowed, nothing else), returning its specifier if the whole line fits.
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

pub fn extract_imports(source: &str) -> (String, Vec<ImportSpecifier>) {
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

pub fn rewrite_default_export(source: &str) -> (String, Option<String>) {
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

	let default_name = spec.default_name.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL);
	js_sys::Reflect::set(&obj, &"defaultName".into(), &default_name)?;

	let namespace_name = spec.namespace_name.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL);
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
