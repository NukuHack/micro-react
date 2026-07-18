use micro_react::jsx::{JsxError, transpile_jsx_str};

// ─── Tests ───

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
	let out = transpile_jsx_str("<div>{a < b ? x : y}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${a < b ? x : y}</div>`");
}

// ── additional edge cases ──

#[test]
fn void_html_element_self_closing() {
	let out = transpile_jsx_str("<br/>").expect("should transpile");
	assert_eq!(out, "html`<br/>`");
}

#[test]
fn boolean_style_attribute_with_no_value_passes_through() {
	let out = transpile_jsx_str("<input disabled />").expect("should transpile");
	assert_eq!(out, "html`<input disabled />`");
}

#[test]
fn attribute_value_containing_gt_inside_quotes_does_not_end_tag_early() {
	let out = transpile_jsx_str(r#"<div title="a > b">x</div>"#).expect("should transpile");
	assert_eq!(out, r#"html`<div title="a > b">x</div>`"#);
}

#[test]
fn attribute_value_containing_slash_gt_inside_quotes_is_not_self_closing() {
	let out = transpile_jsx_str(r#"<a href="a/>b">x</a>"#).expect("should transpile");
	assert_eq!(out, r#"html`<a href="a/>b">x</a>`"#);
}

#[test]
fn component_name_starting_with_underscore_is_treated_as_component() {
	let out = transpile_jsx_str("<_Internal />").expect("should transpile");
	assert_eq!(out, "html`<${_Internal} />`");
}

#[test]
fn dotted_component_name_is_a_single_hole() {
	let out = transpile_jsx_str("<Context.Provider>x</Context.Provider>").expect("should transpile");
	assert_eq!(out, "html`<${Context.Provider}>x</${Context.Provider}>`");
}

#[test]
fn hyphenated_custom_element_stays_static() {
	let out = transpile_jsx_str("<my-widget>x</my-widget>").expect("should transpile");
	assert_eq!(out, "html`<my-widget>x</my-widget>`");
}

#[test]
fn tag_name_with_digits_stays_static() {
	let out = transpile_jsx_str("<h1>title</h1>").expect("should transpile");
	assert_eq!(out, "html`<h1>title</h1>`");
}

#[test]
fn component_name_with_digits_becomes_a_hole() {
	let out = transpile_jsx_str("<Foo2 />").expect("should transpile");
	assert_eq!(out, "html`<${Foo2} />`");
}

#[test]
fn empty_element_with_no_children() {
	let out = transpile_jsx_str("<div></div>").expect("should transpile");
	assert_eq!(out, "html`<div></div>`");
}

#[test]
fn literal_backtick_in_text_child_is_escaped() {
	let out = transpile_jsx_str("<div>a `code` b</div>").expect("should transpile");
	assert_eq!(out, "html`<div>a \\`code\\` b</div>`");
}

#[test]
fn fragment_shorthand_nested_inside_a_hole() {
	let out = transpile_jsx_str("<div>{cond && <>a</>}</div>").expect("should transpile");
	assert_eq!(out, "html`<div>${cond && html`<${Fragment}>a</${Fragment}>`}</div>`");
}

#[test]
fn multiple_sibling_holes_each_with_own_jsx() {
	let out = transpile_jsx_str("const arr = [<div/>, <span/>];").expect("should transpile");
	assert_eq!(out, "const arr = [html`<div/>`, html`<span/>`];");
}

#[test]
fn malformed_closing_tag_missing_gt_is_a_real_error() {
	let err = transpile_jsx_str("<div>x</div oops").unwrap_err();
	assert!(matches!(err, JsxError::MalformedClosingTag(_)));
}

#[test]
fn self_closing_component_with_attribute_holes() {
	let out = transpile_jsx_str("<Foo bar={1} baz=\"x\" />").expect("should transpile");
	assert_eq!(out, "html`<${Foo} bar=${1} baz=\"x\" />`");
}

#[test]
fn whitespace_only_between_tags_is_preserved() {
	let out = transpile_jsx_str("<div>  </div>").expect("should transpile");
	assert_eq!(out, "html`<div>  </div>`");
}

#[test]
fn single_quoted_string_containing_a_lt_is_not_treated_as_jsx() {
	let src = "const s = 'a < div>fake</div>';";
	let out = transpile_jsx_str(src).expect("should transpile");
	assert_eq!(out, src);
}
