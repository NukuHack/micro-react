//! Rewrites JSX source text into the `` html`...` `` tagged-template calls
//! that `html_template::compile` already knows how to handle. This is a
//! pure syntax transform — `{expr}` holes are copied verbatim, never
//! parsed as JS — so it stays additive to the existing render pipeline.
//!
//! Scope: plain `.jsx`, not `.tsx`. JSX roots are detected structurally
//! (any `<Tag ...>`/`<>` that scans as a balanced element), not only ones
//! preceded by `return`/`(`, which means a stray `<` in a JS comparison
//! can rarely be misread as a tag start; see `looks_like_jsx_start`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

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

/// Synthetic attribute name prefix used to smuggle a JSX spread
/// (`{...expr}`) through the `html`` sentinel-HTML pipeline. Attribute
/// *names* in that pipeline must be static text (see the comment on
/// `build_case_map` in html_template.rs) so a bare `...expr` can't be
/// represented directly; instead it's rewritten as a normal
/// `name="${expr}"` attribute using this reserved name prefix, and
/// `html_template::compile_node` recognizes the prefix and treats the
/// value as a props object to merge rather than a literal attribute.
/// The char offset of the opening `{` is appended to keep multiple
/// spreads on the same tag unique; DOM attribute order (and therefore
/// override order) still matches source order either way.
pub(crate) const SPREAD_ATTR_PREFIX: &str = "__mrspread-";

/// Renders a JSX attribute section (`from` is just after the tag name) up
/// to its terminating `>` or self-closing `/>`, converting `{expr}` holes
/// into `${expr}` and leaving quoted attribute values untouched. A bare
/// `{...expr}` spread attribute is rewritten into a synthetic
/// `__mrspread-N="${expr}"` attribute (see `SPREAD_ATTR_PREFIX`).
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
				let inner: String = chars[i + 1..close].iter().collect();
				if let Some(expr) = inner.trim_start().strip_prefix("...") {
					// `{...expr}` — a spread, not a `name={value}` pair.
					// Rewritten as an ordinary attribute so it survives the
					// sentinel-HTML round trip; see `SPREAD_ATTR_PREFIX`.
					out.push_str(&format!("{SPREAD_ATTR_PREFIX}{i}=\"${{"));
					out.push_str(&transpile_jsx_str(expr.trim())?);
					out.push_str("}\"");
				} else {
					out.push_str("${");
					out.push_str(&transpile_jsx_str(&inner)?);
					out.push('}');
				}
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

/// True if a JSX `{...}` hole's inner text is nothing but whitespace and JS
/// comments, as in `{/* comment */}`. Real JSX treats such holes as valid
/// children that render nothing; naively splicing them into an `` html`` ``
/// template as `${/* comment */}` produces an empty template expression,
/// which is a JS syntax error, so these holes must be dropped entirely.
fn is_comment_only_hole(inner: &str) -> bool {
	let chars: Vec<char> = inner.chars().collect();
	let n = chars.len();
	let mut i = 0;
	while i < n {
		if chars[i].is_whitespace() {
			i += 1;
			continue;
		}
		match skip_js_comment(&chars, i) {
			Some(next) => i = next,
			None => return false,
		}
	}
	true
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
			let inner: String = chars[i + 1..close].iter().collect();
			if !is_comment_only_hole(&inner) {
				out.push_str("${");
				out.push_str(&transpile_jsx_str(&inner)?);
				out.push('}');
			}
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
#[wasm_bindgen(js_name = jsx)]
pub fn transpile_jsx(source: &str) -> Result<JsValue, JsValue> {
	transpile_jsx_str(source).map(|s| JsValue::from_str(&s)).map_err(|e| JsValue::from_str(&e.to_string()))
}

thread_local! {
	static MODULE_CACHE: RefCell<HashMap<String, JsxModuleRecord>> = RefCell::new(HashMap::new());

	/// There's no global `AsyncFunction` binding to grab directly (unlike
	/// `Function`), so it's derived once via `(async function(){}).constructor`
	/// and reused for every module — every module body is executed as an
	/// async function so it can use top-level `await`.
	static ASYNC_FUNCTION_CTOR: js_sys::Function = {
		let getter = js_sys::Function::new_no_args("return (async function(){}).constructor;");
		js_sys::Reflect::apply(&getter, &JsValue::UNDEFINED, &js_sys::Array::new())
			.ok()
			.and_then(|v| v.dyn_into::<js_sys::Function>().ok())
			.expect("AsyncFunction constructor should always be obtainable")
	};
}
struct JsxModuleRecord {
	exports: JsValue,
	is_loading: bool,
	/// The in-flight (or already-settled) load's own promise. True circular
	/// imports still grab `exports` directly without awaiting (see
	/// `is_circular` in `load_module_body`) to break the deadlock, but a
	/// merely concurrent, non-circular import of the same URL awaits this
	/// instead of polling `is_loading` — which correctly propagates a
	/// rejection if the real load fails, rather than waiting forever for a
	/// flag that a failed load will never flip.
	promise: js_sys::Promise,
}

/// Extensions tried, in order, for an extensionless import specifier —
/// mirroring the "extension is optional if obvious" resolution real React
/// tooling (Vite/webpack/Node's resolver) does at build time. We can't do
/// that statically here since modules are fetched at runtime, so instead we
/// just probe: request the bare URL, and if that 404s, request each
/// candidate extension in turn and use the first one that resolves.
const RESOLVE_EXTENSIONS: [&str; 4] = [".js", ".jsx", ".ts", ".tsx"];

/// True if the URL's final path segment already has a `.ext` (so we should
/// NOT guess further extensions — e.g. `foo.css?url` or `foo.json`).
fn path_has_extension(js_url: &web_sys::Url) -> bool {
	let pathname = js_url.pathname();
	let last_segment = pathname.rsplit('/').next().unwrap_or("");
	last_segment.contains('.')
}

/// Fetches `base_url` as-is; if that 404s and the path looks extensionless,
/// retries with each of `RESOLVE_EXTENSIONS` appended (query/hash preserved)
/// until one succeeds. Returns the successful `(Response, url actually
/// fetched)`, or the original 404 error if every attempt failed.
async fn fetch_resolving_extension(window: &web_sys::Window, base_url: &str) -> Result<(web_sys::Response, String), JsValue> {
	let resp_val = JsFuture::from(window.fetch_with_str(base_url)).await?;
	let resp: web_sys::Response = resp_val.dyn_into()?;
	if resp.ok() {
		return Ok((resp, base_url.to_string()));
	}

	let parsed = web_sys::Url::new(base_url)?;
	if path_has_extension(&parsed) {
		return Ok((resp, base_url.to_string()));
	}

	let pathname = parsed.pathname();
	let mut last_resp = resp;
	for ext in RESOLVE_EXTENSIONS {
		let candidate = web_sys::Url::new(base_url)?;
		candidate.set_pathname(&format!("{pathname}{ext}"));
		let candidate_url = candidate.href();

		let resp_val = JsFuture::from(window.fetch_with_str(&candidate_url)).await?;
		let resp: web_sys::Response = resp_val.dyn_into()?;
		if resp.ok() {
			return Ok((resp, candidate_url));
		}
		last_resp = resp;
	}

	Ok((last_resp, base_url.to_string()))
}

/// True if `url`'s final path segment is a `.css` file (query/hash ignored,
/// so `./x.css?raw` etc. still counts).
fn is_css_url(url: &web_sys::Url) -> bool {
	let pathname = url.pathname();
	pathname.rsplit('/').next().unwrap_or("").to_ascii_lowercase().ends_with(".css")
}

thread_local! {
	static INJECTED_STYLESHEETS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Handles a side-effect stylesheet import (`import './styles.css'`) by
/// injecting a `<link rel="stylesheet">` into `<head>`, instead of trying to
/// fetch, transpile, and execute the file as JS. Deduplicated by absolute
/// URL, so importing the same stylesheet from several modules only adds one
/// `<link>`. Deliberately doesn't wait for the network fetch to finish —
/// real bundlers (Vite, webpack) don't block JS module evaluation on CSS
/// loading either, they just guarantee the tag is in the document.
fn inject_stylesheet(window: &web_sys::Window, href: &str) -> Result<(), JsValue> {
	let already_injected = INJECTED_STYLESHEETS.with(|set| !set.borrow_mut().insert(href.to_string()));
	if already_injected {
		return Ok(());
	}

	let document = window.document().ok_or_else(|| JsValue::from_str("No document available"))?;
	let head = document.head().ok_or_else(|| JsValue::from_str("No <head> available"))?;

	let link: web_sys::HtmlLinkElement = document.create_element("link")?.dyn_into()?;
	link.set_rel("stylesheet");
	link.set_href(href);
	head.append_child(&link)?;
	Ok(())
}

/// Recursively loads, transpiles, and executes a JSX module in the browser.
#[wasm_bindgen(js_name = loadJsx)]
pub async fn load_jsx_module(url: &str, base_url: Option<String>) -> Result<JsValue, JsValue> {
	load_jsx_module_impl(url, base_url, Vec::new()).await
}

/// Waits for a module that's already loading elsewhere (a concurrent,
/// non-circular "diamond" import — e.g. two sibling modules resolved in
/// parallel by the same `Promise.all` both importing the same dependency)
/// to actually finish, by awaiting the exact same promise the real loader
/// is awaiting. This resolves the instant the real load does (there's no
/// polling delay), and — critically — rejects if the real load fails,
/// instead of waiting forever for a completion flag that a failed load
/// would never set.
async fn wait_for_module(child_url: &str) -> Result<JsValue, JsValue> {
	let promise = MODULE_CACHE.with(|cache| cache.borrow().get(child_url).map(|rec| rec.promise.clone()));
	match promise {
		Some(promise) => JsFuture::from(promise).await,
		None => Err(JsValue::from_str(&format!("module '{child_url}' is no longer loading (its load likely already failed and was cleared)"))),
	}
}

/// Real body of [`load_jsx_module`]. `ancestors` is the chain of absolute
/// URLs currently being loaded further up this particular import branch —
/// used to tell a genuine circular import (child is its own ancestor, so it
/// must get the pre-allocated `exports` reference to break the deadlock)
/// apart from a merely concurrent one (child is loading because a sibling
/// branch also depends on it, so it's correct — and necessary, to avoid
/// reading its exports before they're populated — to just await its promise).
async fn load_jsx_module_impl(url: &str, base_url: Option<String>, ancestors: Vec<String>) -> Result<JsValue, JsValue> {
	let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;

	// 1. Resolve relative paths to absolute URLs
	let absolute_url = if let Some(base) = base_url {
		web_sys::Url::new_with_base(url, &base)?.href()
	} else {
		let current_href = window.location().href()?;
		web_sys::Url::new_with_base(url, &current_href)?.href()
	};

	// 2. Handle caching & break circular dependency deadlocks
	let cached_exports = MODULE_CACHE.with(|cache| cache.borrow().get(&absolute_url).filter(|rec| !rec.is_loading).map(|rec| rec.exports.clone()));
	if let Some(exports) = cached_exports {
		return Ok(exports);
	}

	// Pre-allocate the exports object. Circular imports will get this exact reference instantly.
	let exports = js_sys::Object::new();

	let window_for_body = window.clone();
	let absolute_url_for_body = absolute_url.clone();
	let exports_for_body = exports.clone();
	let promise = wasm_bindgen_futures::future_to_promise(async move {
		load_module_body(&window_for_body, &absolute_url_for_body, &exports_for_body, ancestors).await
	});

	MODULE_CACHE.with(|cache| {
		cache
			.borrow_mut()
			.insert(absolute_url.clone(), JsxModuleRecord { exports: exports.clone().into(), is_loading: true, promise: promise.clone() });
	});

	let result = JsFuture::from(promise).await;

	MODULE_CACHE.with(|cache| {
		let mut cache = cache.borrow_mut();
		match &result {
			// Success: flip the placeholder live so anyone waiting on it (or
			// checking it for a future circular import) sees the real exports.
			Ok(_) => {
				if let Some(rec) = cache.get_mut(&absolute_url) {
					rec.is_loading = false;
				}
			}
			// Failure: drop the placeholder entirely, so a later fresh import
			// of the same URL retries instead of reusing a dead entry.
			Err(_) => {
				cache.remove(&absolute_url);
			}
		}
	});

	result
}

/// Fetches, prepares, resolves dependencies for, transpiles, and executes
/// a module's body. Split out from `load_jsx_module_impl` purely so the
/// caller can uniformly clean up the module cache on any failure here,
/// regardless of which step produced it.
async fn load_module_body(
	window: &web_sys::Window,
	absolute_url: &str,
	exports: &js_sys::Object,
	ancestors: Vec<String>,
) -> Result<JsValue, JsValue> {
	// 3. Fetch the raw JSX file, guessing an extension if the bare specifier 404s.
	let (resp, resolved_url) = fetch_resolving_extension(window, absolute_url).await?;
	if !resp.ok() {
		return Err(JsValue::from_str(&format!(
			"Failed to fetch JSX from '{}' (also tried {}): {} {}",
			absolute_url,
			RESOLVE_EXTENSIONS.iter().map(|e| format!("{absolute_url}{e}")).collect::<Vec<_>>().join(", "),
			resp.status(),
			resp.status_text()
		)));
	}
	let src_val = JsFuture::from(resp.text()?).await?;
	let src = src_val.as_string().unwrap_or_default();

	// 4. Prepare module (Strips import/export and parses import lines using Rust)
	let (code, specifiers) = crate::module_prep::prepare_module_str(&src);

	// 5. Recursively resolve all dependencies in parallel
	let promises = js_sys::Array::new();
	for spec in specifiers {
		let from = spec.from.clone();
		let absolute_url_clone = absolute_url.to_string();
		let default_name = spec.default_name.clone();
		let namespace_name = spec.namespace_name.clone();
		let named = spec.named.clone();

		let child_url = web_sys::Url::new_with_base(&from, &absolute_url_clone)?.href();

		// CSS specifiers are never JS modules — fetch/transpile/execute
		// doesn't apply. What to do instead depends entirely on *how* it
		// was imported, since that's the importer telling us what they
		// want:
		//   import './x.css'            → side effect only: inject a
		//                                  <link rel="stylesheet"> for them.
		//   import url from './x.css'   → they want the URL (e.g. to build
		//   import url from './x.css?url'  their own <link>, pass to a
		//                                  Worker, etc.) — hand back the
		//                                  resolved URL as `default` and
		//                                  don't touch the DOM ourselves.
		if is_css_url(&web_sys::Url::new(&child_url)?) {
			let is_bare_import = default_name.is_none() && namespace_name.is_none() && named.is_empty();
			let child_exports = js_sys::Object::new();
			if is_bare_import {
				inject_stylesheet(window, &child_url)?;
			} else {
				js_sys::Reflect::set(&child_exports, &"default".into(), &JsValue::from_str(&child_url))?;
			}

			let result_obj = js_sys::Object::new();
			js_sys::Reflect::set(&result_obj, &"exports".into(), &child_exports)?;
			js_sys::Reflect::set(&result_obj, &"default_name".into(), &default_name.into())?;
			js_sys::Reflect::set(&result_obj, &"namespace_name".into(), &namespace_name.into())?;
			js_sys::Reflect::set(&result_obj, &"named".into(), &js_sys::Array::new())?;
			promises.push(&js_sys::Promise::resolve(&result_obj));
			continue;
		}

		let is_loading = MODULE_CACHE.with(|cache| cache.borrow().get(&child_url).is_some_and(|rec| rec.is_loading));
		let is_circular = is_loading && ancestors.contains(&child_url);
		let is_concurrent_diamond = is_loading && !is_circular;

		if is_circular {
			// Break the deadlock! Grab the pre-allocated reference without awaiting
			let child_exports = MODULE_CACHE.with(|cache| cache.borrow().get(&child_url).unwrap().exports.clone());
			let result_obj = js_sys::Object::new();
			js_sys::Reflect::set(&result_obj, &"exports".into(), &child_exports)?;
			js_sys::Reflect::set(&result_obj, &"default_name".into(), &default_name.into())?;
			js_sys::Reflect::set(&result_obj, &"namespace_name".into(), &namespace_name.into())?;

			let js_named = js_sys::Array::new();
			for (local, exported) in named {
				let pair = js_sys::Array::new();
				pair.push(&JsValue::from_str(&local));
				pair.push(&JsValue::from_str(&exported));
				js_named.push(&pair);
			}
			js_sys::Reflect::set(&result_obj, &"named".into(), &js_named)?;

			promises.push(&js_sys::Promise::resolve(&result_obj));
		} else if is_concurrent_diamond {
			// Not a cycle — some sibling branch just happens to be loading
			// the same module right now. Wait for it to actually finish
			// instead of grabbing its still-empty exports early.
			let fut = async move {
				let child_exports = wait_for_module(&child_url).await?;
				let result_obj = js_sys::Object::new();
				js_sys::Reflect::set(&result_obj, &"exports".into(), &child_exports)?;
				js_sys::Reflect::set(&result_obj, &"default_name".into(), &default_name.into())?;
				js_sys::Reflect::set(&result_obj, &"namespace_name".into(), &namespace_name.into())?;

				let js_named = js_sys::Array::new();
				for (local, exported) in named {
					let pair = js_sys::Array::new();
					pair.push(&JsValue::from_str(&local));
					pair.push(&JsValue::from_str(&exported));
					js_named.push(&pair);
				}
				js_sys::Reflect::set(&result_obj, &"named".into(), &js_named)?;
				Ok(result_obj.into())
			};
			promises.push(&wasm_bindgen_futures::future_to_promise(fut));
		} else {
			// Spawn standard Rust async future to fetch the child module
			let mut child_ancestors = ancestors.clone();
			child_ancestors.push(absolute_url.to_string());
			let fut = async move {
				let child_exports = load_jsx_module_impl(&from, Some(absolute_url_clone), child_ancestors).await?;
				let result_obj = js_sys::Object::new();
				js_sys::Reflect::set(&result_obj, &"exports".into(), &child_exports)?;
				js_sys::Reflect::set(&result_obj, &"default_name".into(), &default_name.into())?;
				js_sys::Reflect::set(&result_obj, &"namespace_name".into(), &namespace_name.into())?;

				let js_named = js_sys::Array::new();
				for (local, exported) in named {
					let pair = js_sys::Array::new();
					pair.push(&JsValue::from_str(&local));
					pair.push(&JsValue::from_str(&exported));
					js_named.push(&pair);
				}
				js_sys::Reflect::set(&result_obj, &"named".into(), &js_named)?;
				Ok(result_obj.into())
			};
			promises.push(&wasm_bindgen_futures::future_to_promise(fut));
		}
	}

	// Await all parallel loads
	let resolved_array_val = JsFuture::from(js_sys::Promise::all(&promises)).await?;
	let resolved_array: js_sys::Array = resolved_array_val.dyn_into()?;

	// Bind imports matching the shape
	let imports = js_sys::Object::new();
	for val in resolved_array.iter() {
		let obj: js_sys::Object = val.dyn_into()?;
		let child_exports = js_sys::Reflect::get(&obj, &"exports".into())?;
		let default_name = js_sys::Reflect::get(&obj, &"default_name".into())?;
		let namespace_name = js_sys::Reflect::get(&obj, &"namespace_name".into())?;
		let named: js_sys::Array = js_sys::Reflect::get(&obj, &"named".into())?.dyn_into()?;

		if !namespace_name.is_null() && !namespace_name.is_undefined() {
			js_sys::Reflect::set(&imports, &namespace_name, &child_exports)?;
		}

		if !default_name.is_null() && !default_name.is_undefined() {
			let default_val = js_sys::Reflect::get(&child_exports, &"default".into())?;
			js_sys::Reflect::set(&imports, &default_name, &default_val)?;
		}

		for pair_val in named.iter() {
			let pair: js_sys::Array = pair_val.dyn_into()?;
			let local = pair.get(0);
			let exported = pair.get(1);
			let val = js_sys::Reflect::get(&child_exports, &exported)?;
			js_sys::Reflect::set(&imports, &local, &val)?;
		}
	}

	// 6. Transpile JSX syntax into html`...` calls
	let js_code = crate::jsx::transpile_jsx_str(&code).map_err(|e| JsValue::from_str(&e.to_string()))?;

	// 7. Map arguments and execute via the AsyncFunction constructor, so the
	// module body can use top-level `await`.
	let param_names = js_sys::Object::keys(&imports);
	let param_values = js_sys::Object::values(&imports);

	let fn_body = format!("{}\nreturn exports;\n//# sourceURL={}", js_code, resolved_url);

	let args = js_sys::Array::new();
	args.push(&"exports".into());
	for name in param_names.iter() {
		args.push(&name);
	}
	args.push(&fn_body.into());

	let func: js_sys::Function = ASYNC_FUNCTION_CTOR.with(|ctor| js_sys::Reflect::construct(ctor, &args))?.dyn_into()?;

	let call_args = js_sys::Array::new();
	call_args.push(exports);
	for val in param_values.iter() {
		call_args.push(&val);
	}

	// Calling an async function returns a promise immediately; await it so
	// we (and any module importing this one) don't move on until the whole
	// body — including anything after a top-level `await` — has actually run.
	let result = js_sys::Reflect::apply(&func, &JsValue::UNDEFINED, &call_args)?;
	let promise: js_sys::Promise = result.dyn_into()?;
	JsFuture::from(promise).await?;

	Ok(exports.clone().into())
}
