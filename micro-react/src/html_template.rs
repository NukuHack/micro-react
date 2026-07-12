// Implements the `html` tagged-template API: compile the static parts once
// per call-site into a cached skeleton, then substitute live JS values into
// it on every call, so callbacks/refs/keys survive intact.

use std::cell::RefCell;
use std::collections::HashMap;

use js_sys::{Array, Object, Reflect, WeakMap};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{DomParser, Element, Node, SupportedType};

use crate::bindings::{children_to_js, js_ref_to_node_ref, js_to_vnode, js_val_to_prop_val, props_to_js_object, vnode_to_js};
use crate::vnode::{ComponentFn, NodeRef, PropVal, Props, VNode, VNodeInner};

// ─────────────────────────── sentinel tokens ───────────────────────────

/// Private-Use-Area character used to delimit hole tokens inside the
/// sentinel HTML string. It can't appear in real source text, so it's safe
/// to scan for verbatim after parsing.
const MARK: char = '\u{E000}';

fn hole_token(i: usize) -> String {
	format!("{MARK}h{i}{MARK}")
}

/// A hole used as a tag name (`<${Comp} .../>`) is compiled into a
/// synthetic `mr-slot-N` tag, since HTML parsers only produce literal tag
/// names; it round-trips through `DomParser` as an unknown custom element.
fn tag_slot_name(i: usize) -> String {
	format!("mr-slot-{i}")
}

fn tag_slot_index(tag_name: &str) -> Option<usize> {
	tag_name.strip_prefix("mr-slot-").and_then(|s| s.parse().ok())
}

// ───────────────────── self-closing tag expansion ─────────────────────

/// HTML void elements: these have no closing tag and no children by
/// definition, so a trailing `/` on them is just redundant, not meaningful.
const VOID_ELEMENTS: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

fn is_void_element(tag: &str) -> bool {
	VOID_ELEMENTS.contains(&tag.to_ascii_lowercase().as_str())
}

/// Expands JSX-style self-closing syntax (`<tag ... />`) on non-void
/// elements into an explicit `<tag ...></tag>` pair.
///
/// This matters because we parse the sentinel string with `DomParser` in
/// HTML mode, not XML mode: per the HTML parsing spec, a trailing `/` on a
/// non-void, non-foreign element is simply ignored, so `<div class="x" />`
/// parses as an *unclosed* `<div>` that goes on to swallow whatever
/// sibling markup follows as its children — silently wrong. This is
/// especially easy to hit with component holes, since `` `<${Comp} />` ``
/// (no children) is the natural way to write a self-closing component and
/// would otherwise absorb its siblings into the component's props.children.
fn expand_self_closing_tags(html: &str) -> String {
	let chars: Vec<char> = html.chars().collect();
	let n = chars.len();
	let mut out = String::with_capacity(html.len());
	let mut i = 0;

	while i < n {
		let c = chars[i];
		if c != '<' {
			out.push(c);
			i += 1;
			continue;
		}

		// Comments: copy verbatim through "-->" so nothing inside is
		// mistaken for tag syntax.
		if chars[i..].starts_with(&['<', '!', '-', '-']) {
			let start = i;
			i += 4;
			while i < n && !chars[i..].starts_with(&['-', '-', '>']) {
				i += 1;
			}
			i = (i + 3).min(n);
			out.extend(&chars[start..i]);
			continue;
		}

		// Doctype / other `<!...>` declarations: copy verbatim through '>'.
		if i + 1 < n && chars[i + 1] == '!' {
			let start = i;
			while i < n && chars[i] != '>' {
				i += 1;
			}
			i = (i + 1).min(n);
			out.extend(&chars[start..i]);
			continue;
		}

		// Closing tag `</...>`: no self-close logic applies, copy verbatim.
		if i + 1 < n && chars[i + 1] == '/' {
			let start = i;
			while i < n && chars[i] != '>' {
				i += 1;
			}
			i = (i + 1).min(n);
			out.extend(&chars[start..i]);
			continue;
		}

		// Opening tag: capture the tag name.
		let name_start = i + 1;
		let mut j = name_start;
		while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_' | ':')) {
			j += 1;
		}
		if j == name_start {
			// Not actually a tag start (a bare '<' in text) — leave as-is.
			out.push('<');
			i += 1;
			continue;
		}
		let tag_name: String = chars[name_start..j].iter().collect();

		// Scan to the matching '>', tracking quotes so a '/' or '>' inside
		// an attribute value isn't mistaken for tag syntax.
		let mut k = j;
		let mut in_quote: Option<char> = None;
		let mut self_close = false;
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
						self_close = true;
						k += 1; // now indexes '>'
						break;
					}
					_ => k += 1,
				},
			}
		}
		let tag_end = if k < n { k + 1 } else { n };

		if self_close {
			let tag_text: String = chars[i..k].iter().collect();
			let tag_text = tag_text.trim_end_matches('/').trim_end();
			out.push_str(tag_text);
			out.push('>');
			if !is_void_element(&tag_name) {
				out.push_str("</");
				out.push_str(&tag_name);
				out.push('>');
			}
		} else {
			out.extend(&chars[i..tag_end]);
		}
		i = tag_end;
	}

	out
}

// ───────────────────── attribute-name case restoration ─────────────────────

/// Camel-cased DOM/React prop names that HTML's attribute lowercasing
/// (unavoidable once the sentinel string round-trips through `DomParser`)
/// would otherwise flatten into a name `diff::set_prop` doesn't recognize
/// — e.g. `className` -> `classname`, silently dropping the class, or
/// `htmlFor` -> `htmlfor`, silently dropping the `for` attribute. Keyed by
/// the lowercased form actually seen after HTML parsing.
const CASED_ATTR_NAMES: &[(&str, &str)] = &[
	("classname", "className"),
	("htmlfor", "htmlFor"),
	("tabindex", "tabIndex"),
	("readonly", "readOnly"),
	("contenteditable", "contentEditable"),
	("autofocus", "autoFocus"),
	("autocomplete", "autoComplete"),
	("autocapitalize", "autoCapitalize"),
	("autocorrect", "autoCorrect"),
	("autoplay", "autoPlay"),
	("autosave", "autoSave"),
	("spellcheck", "spellCheck"),
	("srcset", "srcSet"),
	("srclang", "srcLang"),
	("minlength", "minLength"),
	("maxlength", "maxLength"),
	("rowspan", "rowSpan"),
	("colspan", "colSpan"),
	("cellpadding", "cellPadding"),
	("cellspacing", "cellSpacing"),
	("usemap", "useMap"),
	("frameborder", "frameBorder"),
	("marginheight", "marginHeight"),
	("marginwidth", "marginWidth"),
	("novalidate", "noValidate"),
	("formnovalidate", "formNoValidate"),
	("acceptcharset", "acceptCharset"),
	("enctype", "encType"),
	("hreflang", "hrefLang"),
	("crossorigin", "crossOrigin"),
	("referrerpolicy", "referrerPolicy"),
	("viewbox", "viewBox"),
	("dangerouslysetinnerhtml", "dangerouslySetInnerHTML"),
	// SVG presentation/animation attributes: the HTML parser's "adjust SVG
	// attributes" step only fixes case for a fixed table of known SVG
	// attribute names, and even that only applies while the parser is
	// actually in the SVG foreign-content insertion mode. Since this crate
	// reads attribute names back out as plain lowercase strings, any of
	// these that slip through un-adjusted need restoring here too.
	("attributename", "attributeName"),
	("attributetype", "attributeType"),
	("basefrequency", "baseFrequency"),
	("calcmode", "calcMode"),
	("clippath", "clipPath"),
	("clippathunits", "clipPathUnits"),
	("contentscripttype", "contentScriptType"),
	("contentstyletype", "contentStyleType"),
	("diffuseconstant", "diffuseConstant"),
	("edgemode", "edgeMode"),
	("externalresourcesrequired", "externalResourcesRequired"),
	("filterres", "filterRes"),
	("filterunits", "filterUnits"),
	("glyphref", "glyphRef"),
	("gradienttransform", "gradientTransform"),
	("gradientunits", "gradientUnits"),
	("kernelmatrix", "kernelMatrix"),
	("kernelunitlength", "kernelUnitLength"),
	("keypoints", "keyPoints"),
	("keysplines", "keySplines"),
	("keytimes", "keyTimes"),
	("lengthadjust", "lengthAdjust"),
	("limitingconeangle", "limitingConeAngle"),
	("markerheight", "markerHeight"),
	("markerunits", "markerUnits"),
	("markerwidth", "markerWidth"),
	("maskcontentunits", "maskContentUnits"),
	("maskunits", "maskUnits"),
	("numoctaves", "numOctaves"),
	("pathlength", "pathLength"),
	("patterncontentunits", "patternContentUnits"),
	("patterntransform", "patternTransform"),
	("patternunits", "patternUnits"),
	("pointsatx", "pointsAtX"),
	("pointsaty", "pointsAtY"),
	("pointsatz", "pointsAtZ"),
	("preservealpha", "preserveAlpha"),
	("preserveaspectratio", "preserveAspectRatio"),
	("primitiveunits", "primitiveUnits"),
	("refx", "refX"),
	("refy", "refY"),
	("repeatcount", "repeatCount"),
	("repeatdur", "repeatDur"),
	("requiredextensions", "requiredExtensions"),
	("requiredfeatures", "requiredFeatures"),
	("specularconstant", "specularConstant"),
	("specularexponent", "specularExponent"),
	("spreadmethod", "spreadMethod"),
	("startoffset", "startOffset"),
	("stddeviation", "stdDeviation"),
	("stitchtiles", "stitchTiles"),
	("surfacescale", "surfaceScale"),
	("systemlanguage", "systemLanguage"),
	("tablevalues", "tableValues"),
	("targetx", "targetX"),
	("targety", "targetY"),
	("textlength", "textLength"),
	("viewtarget", "viewTarget"),
	("xchannelselector", "xChannelSelector"),
	("ychannelselector", "yChannelSelector"),
	("zoomandpan", "zoomAndPan"),
];

/// Scans the *pre-parse* sentinel HTML string for attribute-name tokens
/// (`name=` inside a tag) and records each one's original casing, keyed by
/// its lowercased form. `DomParser` unconditionally lowercases attribute
/// names when it parses this string into a DOM tree (that's just how
/// browsers parse HTML), so by the time `compile_node` reads attributes
/// back off the parsed `Element` via `attributes()`, any camelCase name —
/// `setThemeIdx`, `shouldExplode`, any component prop, not just the
/// `class`/`onclick` DOM ones — has already been flattened to lowercase.
/// This map lets `normalize_attr_name` undo that damage using the literal
/// text the author actually wrote, instead of only handling the one
/// hardcoded `on...Capture` case.
///
/// Attribute *names* in `html\`\`` are always static text (only values can
/// be holes), so scanning the concatenated static HTML is sufficient — no
/// need to look at the live substituted values.
fn build_case_map(html: &str) -> HashMap<String, String> {
	let mut map = HashMap::new();
	let chars: Vec<char> = html.chars().collect();
	let n = chars.len();
	let mut i = 0;
	let mut in_tag = false;
	// Quote char we're currently inside an attribute value for, if any —
	// needed so a `>` or `=` inside a quoted value (`title="a > b"`,
	// `class="x=y"`) can't be mistaken for tag-end or a name/value split.
	let mut in_quote: Option<char> = None;

	while i < n {
		let c = chars[i];

		if let Some(q) = in_quote {
			if c == q {
				in_quote = None;
			}
			i += 1;
			continue;
		}

		if !in_tag {
			if c == '<' {
				// Skip comments/doctype-ish `<!...>` — they have no attrs.
				if i + 1 < n && chars[i + 1] == '!' {
					while i < n && chars[i] != '>' {
						i += 1;
					}
					i += 1;
					continue;
				}
				in_tag = true;
			}
			i += 1;
			continue;
		}

		if c == '"' || c == '\'' {
			in_quote = Some(c);
			i += 1;
			continue;
		}

		if c == '>' {
			in_tag = false;
			i += 1;
			continue;
		}

		if c.is_ascii_alphabetic() {
			let start = i;
			let mut j = i;
			while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_' | ':')) {
				j += 1;
			}
			let mut k = j;
			while k < n && chars[k].is_whitespace() {
				k += 1;
			}
			if k < n && chars[k] == '=' {
				let name: String = chars[start..j].iter().collect();
				let lower = name.to_ascii_lowercase();
				if name != lower {
					map.insert(lower, name);
				}
			}
			i = j;
			continue;
		}

		i += 1;
	}

	map
}

fn normalize_attr_name(lowered: &str, case_map: &HashMap<String, String>) -> String {
	// Prefer the casing the author actually wrote at this call-site — it's
	// ground truth, reconstructed before the HTML parser had a chance to
	// lowercase it. Takes priority over the generic SVG/event heuristics
	// below, which only exist as a fallback for when the source itself
	// used lowercase (e.g. deliberately-lowercase `viewbox`).
	if let Some(original) = case_map.get(lowered) {
		return original.clone();
	}

	for (from, to) in CASED_ATTR_NAMES {
		if *from == lowered {
			return (*to).to_string();
		}
	}
	// Event props (`onclick`, `onclickcapture`, …): the DOM lowers case
	// indiscriminately, but `parse_event_prop` only recognizes a capture
	// listener via a literal trailing "Capture" (exact case) — so a
	// capture suffix lost to lowercasing would silently downgrade to a
	// bubble-phase listener. Restore just that one bit of case; the event
	// name itself is lowercased again downstream regardless, so its case
	// here doesn't matter.
	if let Some(rest) = lowered.strip_prefix("on") {
		if !rest.is_empty() {
			if let Some(evt) = rest.strip_suffix("capture") {
				if !evt.is_empty() {
					// Cosmetic only (parse_event_prop lowercases the event
					// name again downstream regardless), but restoring the
					// leading capital keeps the reconstructed name looking
					// like a real prop name instead of "onclickCapture".
					let mut chars = evt.chars();
					let evt_capitalized = match chars.next() {
						Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
						None => String::new(),
					};
					return format!("on{evt_capitalized}Capture");
				}
			}
		}
	}
	lowered.to_string()
}

// ─────────────────────────── compiled skeleton ───────────────────────────

#[derive(Clone, Debug)]
enum TagSource {
	Static(String),
	/// Resolved at substitution time: may turn out to be a tag-name string,
	/// a component function, or the Fragment symbol.
	Hole(usize),
}

/// One run of text containing zero or more `${...}` holes:
/// literals.len() == holes.len() + 1, e.g. "a" ${0} "b" ${1} "c".
#[derive(Clone, Debug)]
struct TextTemplate {
	literals: Vec<String>,
	holes: Vec<usize>,
}

#[derive(Clone, Debug)]
enum AttrValueTemplate {
	Static(String),
	/// The whole attribute value is exactly one hole (`onclick=${fn}`,
	/// `disabled=${flag}`, `style=${obj}`): the *original* JS value is kept
	/// and converted with `js_val_to_prop_val`, so functions/objects/bools
	/// survive intact — unlike the old string-concatenation approach.
	Hole(usize),
	/// Literal text mixed with hole(s) (`class="a ${b} c"`): only
	/// stringifiable values make sense here, so it's rendered as text.
	Mixed(TextTemplate),
}

#[derive(Clone, Debug)]
struct AttrTemplate {
	name: String,
	value: AttrValueTemplate,
}

#[derive(Clone, Debug)]
enum ChildTemplate {
	StaticText(String),
	DynamicText(TextTemplate),
	/// A child position that is *exactly* one hole (`${x}` alone, with only
	/// whitespace around it): the value can be anything renderable — a
	/// vnode, an array/fragment, a string/number, or null — matching JSX
	/// child semantics. This is what makes `${list.map(...)}` work.
	Hole(usize),
	Element(Box<ElementTemplate>),
}

#[derive(Clone, Debug)]
struct ElementTemplate {
	tag: TagSource,
	static_attrs: Vec<(String, String)>,
	attr_holes: Vec<AttrTemplate>,
	key: Option<AttrValueTemplate>,
	ref_hole: Option<usize>,
	children: Vec<ChildTemplate>,
}

/// A whole compiled call-site. Usually one root, but a template literal can
/// have multiple top-level siblings (behaves like an implicit Fragment).
#[derive(Debug)]
struct CompiledTemplate {
	roots: Vec<ChildTemplate>,
}

// ───────────────────────── step 1: compiling ─────────────────────────

/// Builds the sentinel HTML string from just the static parts, tracking
/// which hole positions are "tag position" holes (immediately after `<` or
/// `</`) versus ordinary content/attribute holes.
fn build_sentinel_html(statics: &[String]) -> String {
	let n_holes = statics.len().saturating_sub(1);
	let mut html = String::new();
	let mut open_tag_stack: Vec<String> = Vec::new();

	for i in 0..n_holes {
		html.push_str(&statics[i]);

		let prev_trimmed = statics[i].trim_end();
		let next = statics.get(i + 1).map(String::as_str).unwrap_or("");
		// Deliberately NOT trimmed: whether the hole is immediately followed
		// by whitespace/`>`/`/` (tag position) vs. anything else (content
		// position) depends on the character right after it, and trimming
		// first would throw that signal away.
		let next_first = next.chars().next();

		let looks_like_close = prev_trimmed.ends_with("</") && next_first == Some('>');
		let looks_like_open =
			!looks_like_close && prev_trimmed.ends_with('<') && matches!(next_first, Some(c) if c.is_whitespace() || c == '>' || c == '/');

		if looks_like_close {
			// Reuse whatever synthetic name the matching opening tag used,
			// so the HTML parser doesn't treat this as a mismatched/stray
			// closing tag and mangle the tree.
			let name = open_tag_stack.pop().unwrap_or_else(|| tag_slot_name(i));
			html.push_str(&name);
		} else if looks_like_open {
			let name = tag_slot_name(i);
			open_tag_stack.push(name.clone());
			html.push_str(&name);
		} else {
			html.push_str(&hole_token(i));
		}
	}
	if let Some(last) = statics.last() {
		html.push_str(last);
	}
	html
}

/// Splits a string on hole tokens, returning the literal segments and the
/// hole indices between them (`literals.len() == holes.len() + 1`).
fn split_holes(s: &str) -> TextTemplate {
	let mut literals = Vec::new();
	let mut holes = Vec::new();
	let mut buf = String::new();
	let mut chars = s.chars().peekable();

	while let Some(c) = chars.next() {
		if c != MARK {
			buf.push(c);
			continue;
		}
		let mut tok = String::new();
		let mut closed = false;
		for c2 in chars.by_ref() {
			if c2 == MARK {
				closed = true;
				break;
			}
			tok.push(c2);
		}
		if closed {
			if let Some(idx) = tok.strip_prefix('h').and_then(|d| d.parse::<usize>().ok()) {
				literals.push(std::mem::take(&mut buf));
				holes.push(idx);
				continue;
			}
		}
		// Not a real hole token (shouldn't happen with well-formed input) —
		// keep it verbatim instead of silently eating characters.
		buf.push(MARK);
		buf.push_str(&tok);
		if closed {
			buf.push(MARK);
		}
	}
	literals.push(buf);
	TextTemplate { literals, holes }
}

fn attr_value_template(raw: &str) -> AttrValueTemplate {
	let tt = split_holes(raw);
	if tt.holes.is_empty() {
		AttrValueTemplate::Static(raw.to_string())
	} else if tt.holes.len() == 1 && tt.literals.iter().all(|l| l.is_empty()) {
		AttrValueTemplate::Hole(tt.holes[0])
	} else {
		AttrValueTemplate::Mixed(tt)
	}
}

fn compile_node(node: &Node, case_map: &HashMap<String, String>) -> Option<ChildTemplate> {
	match node.node_type() {
		Node::TEXT_NODE => {
			let text = node.text_content().unwrap_or_default();
			let tt = split_holes(&text);
			if tt.holes.is_empty() {
				if text.trim().is_empty() {
					// A whitespace-only run containing a newline is almost
					// always template-formatting indentation ("\n      "
					// between block-level tags) and should collapse to
					// nothing, matching JSX. But a whitespace-only run with
					// *no* newline is a deliberate same-line separator —
					// `<span>a</span> <span>b</span>` — and dropping it
					// would silently glue the two elements together, unlike
					// `h()` where that space is an explicit string child.
					if !text.is_empty() && !text.contains('\n') {
						Some(ChildTemplate::StaticText(" ".to_string()))
					} else {
						None
					}
				} else {
					Some(ChildTemplate::StaticText(text))
				}
			} else if tt.holes.len() == 1 && tt.literals.iter().all(|l| l.trim().is_empty()) {
				Some(ChildTemplate::Hole(tt.holes[0]))
			} else {
				Some(ChildTemplate::DynamicText(tt))
			}
		}
		Node::ELEMENT_NODE => {
			let elem: &Element = node.unchecked_ref();
			let tag_name = elem.local_name();
			let tag = match tag_slot_index(&tag_name) {
				Some(idx) => TagSource::Hole(idx),
				None => TagSource::Static(tag_name),
			};

			let mut static_attrs = Vec::new();
			let mut attr_holes = Vec::new();
			let mut key = None;
			let mut ref_hole = None;

			let attrs = elem.attributes();
			for i in 0..attrs.length() {
				let Some(a) = attrs.item(i) else { continue };
				let name = normalize_attr_name(&a.name(), case_map);
				let value_tpl = attr_value_template(&a.value());

				if name == "key" {
					key = Some(value_tpl);
				} else if name == "ref" {
					if let AttrValueTemplate::Hole(idx) = value_tpl {
						ref_hole = Some(idx);
					}
					// A static `ref="..."` string can't be a real ref; skip it.
				} else {
					match value_tpl {
						AttrValueTemplate::Static(s) => static_attrs.push((name, s)),
						other => attr_holes.push(AttrTemplate { name, value: other }),
					}
				}
			}

			let mut children = Vec::new();
			let child_nodes = node.child_nodes();
			for i in 0..child_nodes.length() {
				if let Some(c) = child_nodes.item(i) {
					if let Some(ct) = compile_node(&c, case_map) {
						children.push(ct);
					}
				}
			}

			Some(ChildTemplate::Element(Box::new(ElementTemplate { tag, static_attrs, attr_holes, key, ref_hole, children })))
		}
		_ => None,
	}
}

/// Table-context elements are only valid children of a `<table>` per the
/// HTML5 tree-construction rules; encountered anywhere else (including
/// inside our synthetic `<root>` wrapper) they're simply *ignored* by the
/// "in body" insertion mode — not an error, just silently dropped. So a
/// component whose whole template root is e.g. `html\`<tr>...\`` (not
/// nested inside a literal `<table>` in the same template) would vanish
/// with no signal. To parse correctly it needs a real `<table>` ancestor
/// providing genuine table-construction context; this returns the extra
/// markup to wrap around the compiled HTML, plus how many synthetic
/// ancestor elements to descend through afterwards to reach the node
/// whose children are the actual template roots.
fn table_context_wrapper(tag: &str) -> (&'static str, &'static str, usize) {
	match tag.to_ascii_lowercase().as_str() {
		"tr" => ("<table><tbody>", "</tbody></table>", 2),
		"td" | "th" => ("<table><tbody><tr>", "</tr></tbody></table>", 3),
		"thead" | "tbody" | "tfoot" | "caption" | "colgroup" => ("<table>", "</table>", 1),
		"col" => ("<table><colgroup>", "</colgroup></table>", 2),
		_ => ("", "", 0),
	}
}

/// Scans past leading whitespace, comments, and doctype declarations to
/// find the tag name of the first real element in the (already
/// self-close-expanded) sentinel HTML, if any. Used only to decide whether
/// a table-context wrapper is needed — a heuristic, not a full parse, but
/// sufficient since it only needs to recognize a handful of literal tag
/// names (a hole used as the root tag, `<${idx}>`, never matches one of
/// them and correctly falls through to the default wrapper).
fn first_tag_name(html: &str) -> Option<String> {
	let chars: Vec<char> = html.chars().collect();
	let n = chars.len();
	let mut i = 0;
	loop {
		while i < n && chars[i].is_whitespace() {
			i += 1;
		}
		if i >= n || chars[i] != '<' {
			return None;
		}
		if chars[i..].starts_with(&['<', '!', '-', '-']) {
			i += 4;
			while i < n && !chars[i..].starts_with(&['-', '-', '>']) {
				i += 1;
			}
			i = (i + 3).min(n);
			continue;
		}
		if i + 1 < n && chars[i + 1] == '!' {
			while i < n && chars[i] != '>' {
				i += 1;
			}
			i = (i + 1).min(n);
			continue;
		}
		let name_start = i + 1;
		let mut j = name_start;
		while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_' | ':')) {
			j += 1;
		}
		if j == name_start {
			return None;
		}
		return Some(chars[name_start..j].iter().collect());
	}
}

fn compile_template(statics: &[String]) -> Result<CompiledTemplate, JsValue> {
	let html = build_sentinel_html(statics);
	let html = expand_self_closing_tags(&html);
	let case_map = build_case_map(&html);

	let (wrap_prefix, wrap_suffix, extra_depth) = first_tag_name(&html).map(|tag| table_context_wrapper(&tag)).unwrap_or(("", "", 0));

	let parser = DomParser::new()?;
	let doc = parser.parse_from_string(&format!("<root>{wrap_prefix}{html}{wrap_suffix}</root>"), SupportedType::TextHtml)?;
	let body = doc.body().ok_or("html`: DomParser produced no body")?;
	let mut root = body.first_child().ok_or("html`: empty template")?;
	for _ in 0..extra_depth {
		root = root.first_child().ok_or("html`: DomParser produced unexpected table structure")?;
	}

	let mut roots = Vec::new();
	let child_nodes = root.child_nodes();
	for i in 0..child_nodes.length() {
		if let Some(c) = child_nodes.item(i) {
			if let Some(ct) = compile_node(&c, &case_map) {
				roots.push(ct);
			}
		}
	}
	Ok(CompiledTemplate { roots })
}

// Call-site cache: JS gives the same array identity for a tagged
// template's `strings` on every call, so a WeakMap keyed on that gives
// "compile once per call-site" caching for free.

thread_local! {
	static TEMPLATE_CACHE: WeakMap = WeakMap::new();
	static TEMPLATES: RefCell<Vec<CompiledTemplate>> = const { RefCell::new(Vec::new()) };
}

fn get_or_compile(statics: &Array, static_strs: &[String]) -> Result<usize, JsValue> {
	// `Array` extends `Object` in js-sys, but `WeakMap`'s key parameter is
	// typed as `&Object`, so we need an explicit (zero-cost) upcast.
	let key: &Object = statics.unchecked_ref::<Object>();
	if let Some(idx) = TEMPLATE_CACHE.with(|c| c.get(key).as_f64()) {
		return Ok(idx as usize);
	}
	let compiled = compile_template(static_strs)?;
	let idx = TEMPLATES.with(|t| {
		let mut t = t.borrow_mut();
		t.push(compiled);
		t.len() - 1
	});
	TEMPLATE_CACHE.with(|c| {
		c.set(key, &JsValue::from_f64(idx as f64));
	});
	Ok(idx)
}

// ───────────────────────── step 2: substituting ─────────────────────────

fn stringify_value(v: &JsValue) -> String {
	if let Some(s) = v.as_string() {
		return s;
	}
	if let Some(n) = v.as_f64() {
		return n.to_string();
	}
	if let Some(b) = v.as_bool() {
		return b.to_string();
	}
	String::new()
}

fn render_text_template(tt: &TextTemplate, values: &Array) -> String {
	let mut out = String::new();
	for (i, lit) in tt.literals.iter().enumerate() {
		out.push_str(lit);
		if let Some(&hidx) = tt.holes.get(i) {
			out.push_str(&stringify_value(&values.get(hidx as u32)));
		}
	}
	out
}

fn resolve_attr_value(av: &AttrValueTemplate, values: &Array) -> PropVal {
	match av {
		AttrValueTemplate::Static(s) => PropVal::Str(s.clone()),
		// The live value goes straight through the same conversion
		// `createElement` uses — so functions become callbacks, objects
		// stay live objects, booleans/numbers stay themselves.
		AttrValueTemplate::Hole(i) => js_val_to_prop_val(&values.get(*i as u32)),
		AttrValueTemplate::Mixed(tt) => PropVal::Str(render_text_template(tt, values)),
	}
}

fn resolve_key(av: &AttrValueTemplate, values: &Array) -> Option<String> {
	match av {
		AttrValueTemplate::Static(s) => Some(s.clone()),
		AttrValueTemplate::Hole(i) => {
			let v = values.get(*i as u32);
			if v.is_undefined() || v.is_null() {
				None
			} else if let Some(s) = v.as_string() {
				Some(s)
			} else if let Some(n) = v.as_f64() {
				Some(n.to_string())
			} else {
				v.as_bool().map(|b| b.to_string())
			}
		}
		AttrValueTemplate::Mixed(tt) => Some(render_text_template(tt, values)),
	}
}

fn render_child(ct: &ChildTemplate, values: &Array, out: &mut Vec<VNode>) {
	match ct {
		ChildTemplate::StaticText(s) => out.push(VNode::text(s.clone())),
		ChildTemplate::DynamicText(tt) => {
			let s = render_text_template(tt, values);
			if !s.is_empty() {
				out.push(VNode::text(s));
			}
		}
		ChildTemplate::Hole(i) => {
			// `js_to_vnode` already knows how to turn a live value into a
			// vnode: pass-through vnodes (incl. nested `html` calls),
			// arrays → fragment, strings/numbers → text, null/bool → null.
			let v = values.get(*i as u32);
			if let Ok(vn) = js_to_vnode(&v) {
				if !matches!(vn.inner, VNodeInner::Null) {
					out.push(vn);
				}
			}
		}
		ChildTemplate::Element(elem_tpl) => {
			if let Some(vn) = render_element(elem_tpl, values) {
				out.push(vn);
			}
		}
	}
}

fn build_static_attrs(builder: crate::vnode::ElementBuilder, tpl: &ElementTemplate, values: &Array) -> crate::vnode::ElementBuilder {
	let mut builder = builder;
	for (k, v) in &tpl.static_attrs {
		builder = builder.attr(k.clone(), v.clone());
	}
	for a in &tpl.attr_holes {
		// `dangerouslySetInnerHTML={{ __html: ... }}` — diff::set_prop looks
		// for the flattened key "dangerouslySetInnerHTML.__html" holding a
		// plain string; unwrap the live JS `{ __html }` object here rather
		// than passing it through as an opaque `PropVal::Js`, which
		// set_prop has no rule for and would just silently drop.
		if a.name == "dangerouslySetInnerHTML" {
			if let AttrValueTemplate::Hole(i) = &a.value {
				let raw = values.get(*i as u32);
				let html_val = Reflect::get(&raw, &"__html".into()).unwrap_or(JsValue::UNDEFINED);
				if let Some(s) = html_val.as_string() {
					builder = builder.attr("dangerouslySetInnerHTML.__html", s);
					continue;
				}
			}
		}
		builder = builder.attr(a.name.clone(), resolve_attr_value(&a.value, values));
	}
	builder
}

fn render_element(tpl: &ElementTemplate, values: &Array) -> Option<VNode> {
	let mut child_vnodes = Vec::new();
	for c in &tpl.children {
		render_child(c, values, &mut child_vnodes);
	}

	let key = tpl.key.as_ref().and_then(|k| resolve_key(k, values));
	let node_ref: Option<NodeRef> = tpl.ref_hole.and_then(|i| js_ref_to_node_ref(&values.get(i as u32)));

	match &tpl.tag {
		TagSource::Static(tag) => {
			let mut builder = VNode::tag(tag.clone());
			builder = build_static_attrs(builder, tpl, values);
			if let Some(k) = key {
				builder = builder.key(k);
			}
			if let Some(r) = node_ref {
				builder = builder.ref_(r);
			}
			Some(builder.children(child_vnodes).build())
		}

		TagSource::Hole(idx) => {
			let type_val = values.get(*idx as u32);

			// `<${Fragment}>...</${Fragment}>` — same Fragment symbol
			// `createElement` recognizes.
			let frag_sym = js_sys::Symbol::for_("MicroReact.Fragment");
			if type_val.is_symbol() && js_sys::Object::is(&type_val, frag_sym.as_ref()) {
				let vn = VNode::fragment(child_vnodes);
				return Some(vn.with_key(key));
			}

			// A hole that evaluates to a string acts as a dynamic tag name,
			// e.g. `` html`<${tagVar} />` `` where `tagVar = "section"`.
			if let Some(tag) = type_val.as_string() {
				let mut builder = VNode::tag(tag);
				builder = build_static_attrs(builder, tpl, values);
				if let Some(k) = key {
					builder = builder.key(k);
				}
				if let Some(r) = node_ref {
					builder = builder.ref_(r);
				}
				return Some(builder.children(child_vnodes).build());
			}

			// A hole that evaluates to a function is a component reference —
			// wire it up exactly like `createElement` does: static + dynamic
			// attrs become props, children get attached as `props.children`.
			if type_val.is_function() {
				let fn_: js_sys::Function = type_val.clone().unchecked_into();
				let fn_name = Reflect::get(&fn_, &"name".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "Anonymous".to_string());

				let mut props: Props = Vec::new();
				for (k, v) in &tpl.static_attrs {
					props.push((k.clone(), PropVal::Str(v.clone())));
				}
				for a in &tpl.attr_holes {
					props.push((a.name.clone(), resolve_attr_value(&a.value, values)));
				}

				let children_for_fn = child_vnodes.clone();
				let vn = VNode::component(
					fn_name,
					ComponentFn::new(move |comp_props| {
						let js_props = props_to_js_object(&comp_props);
						if !children_for_fn.is_empty() {
							let children_val = children_to_js(&children_for_fn);
							let _ = Reflect::set(&js_props, &"children".into(), &children_val);
						}
						match fn_.call1(&JsValue::NULL, &js_props) {
							Ok(result) => Ok(js_to_vnode(&result).unwrap_or_else(|_| VNode::null())),
							// See bindings.rs's identical component wrapper: propagate
							// as Err and let diff.rs walk up to the nearest boundary.
							Err(err) => Err(err),
						}
					}),
					props,
				);
				return Some(vn.with_key(key));
			}

			None
		}
	}
}

// ───────────────────── pure-logic unit tests ─────────────────────
//
// These exercise `expand_self_closing_tags` and `normalize_attr_name` in
// isolation — no DOM/JS needed, so they run under plain `cargo test --lib`,
// unlike the DomParser-dependent paths which need `wasm-pack test`.
#[cfg(test)]
mod pure_logic_tests {
	use super::*;

	// ── expand_self_closing_tags ──

	#[test]
	fn self_closing_non_void_element_gets_explicit_close_tag() {
		assert_eq!(expand_self_closing_tags(r#"<div class="x" />"#), r#"<div class="x"></div>"#);
	}

	#[test]
	fn self_closing_component_slot_gets_explicit_close_tag() {
		// This is the exact case that used to swallow siblings: a
		// self-closed component hole followed by more markup.
		let input = "<mr-slot-0 /><span>after</span>";
		let expected = "<mr-slot-0></mr-slot-0><span>after</span>";
		assert_eq!(expand_self_closing_tags(input), expected);
	}

	#[test]
	fn self_closing_void_element_just_drops_the_slash() {
		assert_eq!(expand_self_closing_tags("<br/>"), "<br>");
		assert_eq!(expand_self_closing_tags(r#"<img src="a.png" />"#), r#"<img src="a.png">"#);
	}

	#[test]
	fn non_self_closing_tags_pass_through_unchanged() {
		let input = "<div class=\"a\"><p>hi</p></div>";
		assert_eq!(expand_self_closing_tags(input), input);
	}

	#[test]
	fn slash_inside_quoted_attribute_value_is_not_mistaken_for_self_close() {
		let input = r#"<a href="a/b/c">text</a>"#;
		assert_eq!(expand_self_closing_tags(input), input);
	}

	#[test]
	fn slash_inside_single_quoted_attribute_value_is_not_mistaken() {
		let input = "<a href='a/b/c'>text</a>";
		assert_eq!(expand_self_closing_tags(input), input);
	}

	#[test]
	fn closing_tags_are_left_untouched() {
		let input = "<div></div>";
		assert_eq!(expand_self_closing_tags(input), input);
	}

	#[test]
	fn comments_are_copied_verbatim_even_with_slashes_inside() {
		let input = "<!-- a </div> b/c --><div>x</div>";
		assert_eq!(expand_self_closing_tags(input), input);
	}

	#[test]
	fn multiple_self_closing_siblings_each_get_closed_independently() {
		let input = "<hr/><div class=\"a\" /><br/>";
		let expected = "<hr><div class=\"a\"></div><br>";
		assert_eq!(expand_self_closing_tags(input), expected);
	}

	#[test]
	fn nested_self_closing_inside_normal_element() {
		let input = r#"<ul><li class="x" /><li>b</li></ul>"#;
		let expected = r#"<ul><li class="x"></li><li>b</li></ul>"#;
		assert_eq!(expand_self_closing_tags(input), expected);
	}

	#[test]
	fn hole_tokens_inside_attribute_values_survive_untouched() {
		let input = "<div class=\"a \u{E000}h0\u{E000} b\" />";
		let expected = "<div class=\"a \u{E000}h0\u{E000} b\"></div>";
		assert_eq!(expand_self_closing_tags(input), expected);
	}

	#[test]
	fn text_only_input_is_unchanged() {
		assert_eq!(expand_self_closing_tags("just text, no tags"), "just text, no tags");
	}

	// ── build_case_map / normalize_attr_name case restoration ──
	//
	// Regression coverage for the bug where a camelCase *component* prop
	// written in a `html\`\`` template (e.g. `setThemeIdx="${fn}"`) got
	// silently flattened to `setthemeidx` because `DomParser` lowercases
	// attribute names, and the old `normalize_attr_name` only restored
	// casing for a hardcoded table of DOM props plus `on...Capture` event
	// names. `build_case_map` recovers the author's original casing from
	// the pre-parse HTML text itself, so *any* camelCase name — not just
	// known DOM/event ones — survives.

	#[test]
	fn case_map_records_camel_case_attr_names() {
		let map = build_case_map(r#"<mr-slot-0 themeidx="x" setThemeIdx="y"></mr-slot-0>"#);
		assert_eq!(map.get("setthemeidx").map(String::as_str), Some("setThemeIdx"));
		// Already-lowercase names aren't recorded — nothing to restore, and
		// leaving them out lets the SVG/event fallback table still apply.
		assert_eq!(map.get("themeidx"), None);
	}

	#[test]
	fn case_map_ignores_gt_and_eq_inside_quoted_values() {
		// Neither the `>` nor the `=` inside these quoted values should be
		// mistaken for tag-end or a bogus name/value split.
		let map = build_case_map(r#"<div title="a > b" setThemeIdx="x=y"></div>"#);
		assert_eq!(map.get("setthemeidx").map(String::as_str), Some("setThemeIdx"));
		assert_eq!(map.len(), 1);
	}

	#[test]
	fn case_map_ignores_text_content_and_comments() {
		let map = build_case_map("<!-- setThemeIdx=\"nope\" --><p>shouldExplode=\"also nope\"</p>");
		assert!(map.is_empty());
	}

	#[test]
	fn normalize_attr_name_prefers_case_map_for_component_props() {
		let mut map = HashMap::new();
		map.insert("setthemeidx".to_string(), "setThemeIdx".to_string());
		map.insert("shouldexplode".to_string(), "shouldExplode".to_string());
		assert_eq!(normalize_attr_name("setthemeidx", &map), "setThemeIdx");
		assert_eq!(normalize_attr_name("shouldexplode", &map), "shouldExplode");
	}

	#[test]
	fn normalize_attr_name_case_map_does_not_break_svg_auto_correction() {
		// A deliberately-lowercase SVG attr like `viewbox` must still get
		// auto-corrected to `viewBox` via CASED_ATTR_NAMES — the case map
		// only ever holds names that had *some* uppercase in the source, so
		// it can't accidentally shadow this fallback.
		let map = build_case_map(r#"<svg viewbox="0 0 1 1"></svg>"#);
		assert_eq!(normalize_attr_name("viewbox", &map), "viewBox");
	}

	// ── normalize_attr_name ──

	#[test]
	fn restores_class_name() {
		assert_eq!(normalize_attr_name("classname", &HashMap::new()), "className");
	}

	#[test]
	fn restores_html_for() {
		assert_eq!(normalize_attr_name("htmlfor", &HashMap::new()), "htmlFor");
	}

	#[test]
	fn restores_dangerously_set_inner_html() {
		assert_eq!(normalize_attr_name("dangerouslysetinnerhtml", &HashMap::new()), "dangerouslySetInnerHTML");
	}

	#[test]
	fn restores_misc_camel_case_dom_props() {
		assert_eq!(normalize_attr_name("tabindex", &HashMap::new()), "tabIndex");
		assert_eq!(normalize_attr_name("readonly", &HashMap::new()), "readOnly");
		assert_eq!(normalize_attr_name("colspan", &HashMap::new()), "colSpan");
		assert_eq!(normalize_attr_name("srcset", &HashMap::new()), "srcSet");
	}

	#[test]
	fn restores_capture_suffix_on_event_props() {
		assert_eq!(normalize_attr_name("onclickcapture", &HashMap::new()), "onClickCapture");
		// Only the leading letter of the collapsed event name can be
		// cosmetically restored (mid-word boundaries like "Enter" in
		// "mouseenter" are unrecoverable from lowercase alone) — but the
		// "Capture" suffix, which is the part that's functionally load
		// bearing for parse_event_prop, is always restored exactly.
		assert_eq!(normalize_attr_name("onmouseentercapture", &HashMap::new()), "onMouseenterCapture");
	}

	#[test]
	fn plain_event_props_are_left_lowercase() {
		// No "Capture" suffix to restore; parse_event_prop lowercases the
		// event name anyway, so plain lowercase is already correct.
		assert_eq!(normalize_attr_name("onclick", &HashMap::new()), "onclick");
		assert_eq!(normalize_attr_name("onmouseenter", &HashMap::new()), "onmouseenter");
	}

	#[test]
	fn ordinary_lowercase_html_attrs_pass_through() {
		assert_eq!(normalize_attr_name("class", &HashMap::new()), "class");
		assert_eq!(normalize_attr_name("id", &HashMap::new()), "id");
		assert_eq!(normalize_attr_name("disabled", &HashMap::new()), "disabled");
		assert_eq!(normalize_attr_name("placeholder", &HashMap::new()), "placeholder");
		assert_eq!(normalize_attr_name("data-foo", &HashMap::new()), "data-foo");
	}

	#[test]
	fn bare_on_with_nothing_after_is_left_alone() {
		// Not a real event prop (no event name) — shouldn't be mangled by
		// the capture-suffix logic.
		assert_eq!(normalize_attr_name("on", &HashMap::new()), "on");
	}

	#[test]
	fn bare_oncapture_with_no_event_name_is_left_alone() {
		assert_eq!(normalize_attr_name("oncapture", &HashMap::new()), "oncapture");
	}

	// ── first_tag_name / table_context_wrapper ──

	#[test]
	fn first_tag_name_finds_simple_root_tag() {
		assert_eq!(first_tag_name("<tr><td>x</td></tr>"), Some("tr".to_string()));
	}

	#[test]
	fn first_tag_name_skips_leading_whitespace_and_comments() {
		assert_eq!(first_tag_name("  \n<!-- note --><td>x</td>"), Some("td".to_string()));
	}

	#[test]
	fn first_tag_name_none_for_hole_root() {
		// A dynamic root tag (`<${idx}>`) was already rewritten to a
		// `mr-slot-N` custom element by the time this runs, but a bare
		// hole token with no `<` at all should still return None cleanly.
		assert_eq!(first_tag_name("just text"), None);
	}

	#[test]
	fn table_context_wrapper_wraps_tr() {
		assert_eq!(table_context_wrapper("tr"), ("<table><tbody>", "</tbody></table>", 2));
	}

	#[test]
	fn table_context_wrapper_wraps_td_and_th() {
		assert_eq!(table_context_wrapper("td"), ("<table><tbody><tr>", "</tr></tbody></table>", 3));
		assert_eq!(table_context_wrapper("TH"), ("<table><tbody><tr>", "</tr></tbody></table>", 3));
	}

	#[test]
	fn table_context_wrapper_wraps_section_level_tags() {
		for tag in ["thead", "tbody", "tfoot", "caption", "colgroup"] {
			assert_eq!(table_context_wrapper(tag), ("<table>", "</table>", 1));
		}
	}

	#[test]
	fn table_context_wrapper_wraps_col() {
		assert_eq!(table_context_wrapper("col"), ("<table><colgroup>", "</colgroup></table>", 2));
	}

	#[test]
	fn table_context_wrapper_no_op_for_ordinary_tags() {
		assert_eq!(table_context_wrapper("div"), ("", "", 0));
		assert_eq!(table_context_wrapper("table"), ("", "", 0));
	}

	#[test]
	fn is_void_element_matches_known_void_tags_case_insensitively() {
		assert!(is_void_element("img"));
		assert!(is_void_element("IMG"));
		assert!(is_void_element("br"));
		assert!(!is_void_element("div"));
		assert!(!is_void_element("mr-slot-0"));
	}
}

// ───────────────────────────── entry point ─────────────────────────────

#[wasm_bindgen(js_name = htmlTemplate)]
pub fn html_template(statics: Array, values: Array) -> Result<JsValue, JsValue> {
	let static_strs: Vec<String> = statics.iter().filter_map(|v| v.as_string()).collect();
	let tpl_idx = get_or_compile(&statics, &static_strs)?;

	let vnode = TEMPLATES.with(|t| {
		let templates = t.borrow();
		let compiled = &templates[tpl_idx];
		let mut roots = Vec::new();
		for r in &compiled.roots {
			render_child(r, &values, &mut roots);
		}
		match roots.len() {
			0 => VNode::null(),
			1 => roots.into_iter().next().unwrap(),
			_ => VNode::fragment(roots),
		}
	});

	vnode_to_js(vnode)
}
