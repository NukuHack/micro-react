use micro_react::jsx::{JsxError, transpile_jsx_str};

#[test]
fn basic_div() {
	let out = transpile_jsx_str("const el = <div>hi</div>;").expect("should transpile");
	assert_eq!(out, "const el = html`<div>hi</div>`;");
}

#[test]
fn fragment_shorthand_becomes_fragment_hole() {
	let out = transpile_jsx_str("const el = <>hi</>;").expect("should transpile");
	assert_eq!(out, "const el = html`<${Fragment}>hi</${Fragment}>`;");
}

#[test]
fn nested_elements() {
	let out = transpile_jsx_str("<ul><li>a</li><li>b</li></ul>").expect("should transpile");
	assert_eq!(out, "html`<ul><li>a</li><li>b</li></ul>`");
}

#[test]
fn self_closing_tag_is_left_self_closing() {
	// Expansion to `<foo></foo>` is html_template's job at render time, not
	// the transpiler's — self-closing syntax passes through untouched.
	let out = transpile_jsx_str("<Foo />").expect("should transpile");
	assert_eq!(out, "html`<${Foo} />`");
}

#[test]
fn component_tags_become_holes() {
	let out = transpile_jsx_str("<Foo><Bar>x</Bar></Foo>").expect("should transpile");
	assert_eq!(out, "html`<${Foo}><${Bar}>x</${Bar}></${Foo}>`");
}

#[test]
fn lowercase_tags_stay_static() {
	let out = transpile_jsx_str("<div><span>x</span></div>").expect("should transpile");
	assert_eq!(out, "html`<div><span>x</span></div>`");
}

#[test]
fn hole_with_simple_identifier() {
	let out = transpile_jsx_str("<div>{count}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${count}</div>`");
}

#[test]
fn hole_with_member_access() {
	let out = transpile_jsx_str("<div>{user.name}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${user.name}</div>`");
}

#[test]
fn hole_with_ternary() {
	let out = transpile_jsx_str("<div>{flag ? 'a' : 'b'}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${flag ? 'a' : 'b'}</div>`");
}

#[test]
fn hole_with_arrow_function_braces() {
	// The trickiest brace-balancing case: an arrow function body with its
	// own `{}` nested inside the outer JSX hole's `{}`.
	let out = transpile_jsx_str("<button onclick={() => { doThing(); }}>go</button>").expect("should transpile");
	assert_eq!(out, "html`<button onclick=${() => { doThing(); }}>go</button>`");
}

#[test]
fn attribute_hole_is_converted() {
	let out = transpile_jsx_str(r#"<input value={draft} disabled={isLoading} />"#).expect("should transpile");
	assert_eq!(out, "html`<input value=${draft} disabled=${isLoading} />`");
}

#[test]
fn jsx_inside_a_string_is_not_treated_as_jsx() {
	let src = r#"const s = "a (< b) and <div>fake</div> too";"#;
	let out = transpile_jsx_str(src).expect("should transpile");
	assert_eq!(out, src);
}

#[test]
fn jsx_inside_a_comment_is_not_treated_as_jsx() {
	let src = "// <div>not real</div>\nconst x = 1;";
	let out = transpile_jsx_str(src).expect("should transpile");
	assert_eq!(out, src);
}

#[test]
fn comparison_operator_is_not_mistaken_for_jsx() {
	let src = "const ok = a < b() && c < d.e;";
	let out = transpile_jsx_str(src).expect("should transpile");
	assert_eq!(out, src);
}

#[test]
fn multiple_jsx_blocks_in_one_file() {
	let src = "function A() { return <div>a</div>; }\nfunction B() { return <span>b</span>; }";
	let out = transpile_jsx_str(src).expect("should transpile");
	assert_eq!(out, "function A() { return html`<div>a</div>`; }\nfunction B() { return html`<span>b</span>`; }");
}

#[test]
fn malformed_jsx_missing_closing_tag_is_a_real_error() {
	let err = transpile_jsx_str("<div>oops").unwrap_err();
	assert_eq!(err, JsxError::UnterminatedTag(5));
}

#[test]
fn malformed_jsx_mismatched_closing_tag_is_a_real_error() {
	let err = transpile_jsx_str("<div>x</span>").unwrap_err();
	assert!(matches!(err, JsxError::MismatchedClosingTag { ref expected, ref found, .. } if expected == "div" && found == "span"));
}

#[test]
fn unbalanced_hole_is_a_real_error() {
	let err = transpile_jsx_str("<div>{oops</div>").unwrap_err();
	assert!(matches!(err, JsxError::UnbalancedHole(_)));
}

// ─── Regression tests: nested JSX inside a `{}` hole ───
// Previously `{expr}` holes were copied verbatim, so any JSX *inside* the
// expression (e.g. the classic `{items.map(x => <li>...</li>)}` pattern)
// was left untranspiled and shipped to the JS engine as raw `<li>` text
// sitting where an expression was expected -- "expected expression, got '<'".

#[test]
fn nested_jsx_inside_child_hole_is_transpiled() {
	let out = transpile_jsx_str("<ul>{items.map(x => <li>{x}</li>)}</ul>").expect("should transpile");
	assert_eq!(out, "html`<ul>${items.map(x => html`<li>${x}</li>`)}</ul>`");
}

#[test]
fn nested_jsx_inside_attribute_hole_is_transpiled() {
	let out = transpile_jsx_str(r#"<div content={<span>hi</span>}></div>"#).expect("should transpile");
	assert_eq!(out, "html`<div content=${html`<span>hi</span>`}></div>`");
}

#[test]
fn nested_jsx_with_own_attribute_holes_is_transpiled() {
	// Mirrors the real-world `about.jsx` / `home.jsx` pattern: a `.map()`
	// callback returning JSX whose own attributes contain further holes.
	let src = r#"<div class="grid">{features.map(f => (<div key={f.title} class="item"><h4>{f.title}</h4><p>{f.desc}</p></div>))}</div>"#;
	let out = transpile_jsx_str(src).expect("should transpile");
	let expected =
		"html`<div class=\"grid\">${features.map(f => (html`<div key=${f.title} class=\"item\"><h4>${f.title}</h4><p>${f.desc}</p></div>`))}</div>`";
	assert_eq!(out, expected);
}

#[test]
fn nested_jsx_inside_hole_with_component_tag() {
	let out = transpile_jsx_str("<div>{cond ? <Foo/> : <Bar/>}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${cond ? html`<${Foo}/>` : html`<${Bar}/>`}</div>`");
}

#[test]
fn deeply_nested_map_inside_map_is_transpiled() {
	let src = "<div>{rows.map(r => <ul>{r.cells.map(c => <li>{c}</li>)}</ul>)}</div>";
	let out = transpile_jsx_str(src).expect("should transpile");
	assert_eq!(out, "html`<div>${rows.map(r => html`<ul>${r.cells.map(c => html`<li>${c}</li>`)}</ul>`)}</div>`");
}

#[test]
fn hole_without_nested_jsx_is_unaffected_by_the_fix() {
	// Guards against a regression where the recursive call might mangle
	// ordinary expressions that merely contain a `<` comparison.
	let out = transpile_jsx_str("<div>{a < b ? x : y}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${a < b ? x : y}</div>`");
}
