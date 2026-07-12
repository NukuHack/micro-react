//! Coverage for the `html` tagged-template API (`src/html_template.rs`):
//! compiling a call-site's static skeleton once and substituting live
//! values into it, end to end through real DOM rendering.
//!
//! Like `tests/reconciler.rs`, these need a real DOM/JS runtime, so they
//! run through `wasm-bindgen-test` rather than plain `cargo test`:
//!
//!     wasm-pack test --headless --chrome
//!     # or --firefox
//!
//! Pure-logic pieces of html_template.rs (the self-closing-tag expansion
//! and attribute-name case restoration) have their own plain `cargo test
//! --lib` coverage in `src/html_template.rs::pure_logic_tests`, since they
//! don't need a DOM at all.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::*;

use micro_react::bindings::{render as mount_root, JsRoot};
use micro_react::html_template::html_template;

wasm_bindgen_test_configure!(run_in_browser);

// ─────────────────────────── test helpers ───────────────────────────

fn make_container() -> web_sys::Element {
	let doc = web_sys::window().unwrap().document().unwrap();
	let el = doc.create_element("div").unwrap();
	doc.body().unwrap().append_child(&el).unwrap();
	el
}

/// Build the `statics` array a JS tagged-template call would pass, and run
/// it through `html_template` with the given `values`.
fn tpl(statics: &[&str], values: Vec<JsValue>) -> JsValue {
	tpl_with_array(&Array::new_with_length(0), statics, values)
}

/// Like `tpl`, but lets the caller supply (and reuse) the `statics` Array,
/// so call-site caching can be exercised by identity.
fn tpl_with_array(reuse: &Array, statics: &[&str], values: Vec<JsValue>) -> JsValue {
	let s = if reuse.length() == statics.len() as u32 {
		reuse.clone()
	} else {
		let s = Array::new();
		for part in statics {
			s.push(&JsValue::from_str(part));
		}
		s
	};
	let v = Array::new();
	for val in values {
		v.push(&val);
	}
	html_template(s, v).expect("html_template should compile and substitute without error")
}

fn mount(vnode: JsValue) -> (web_sys::Element, JsRoot) {
	let container = make_container();
	let root = mount_root(vnode, container.clone()).expect("mount should succeed");
	(container, root)
}

fn js_fn(f: impl FnMut(JsValue) + 'static) -> (Function, Closure<dyn FnMut(JsValue)>) {
	let closure = Closure::wrap(Box::new(f) as Box<dyn FnMut(JsValue)>);
	let func: Function = closure.as_ref().unchecked_ref::<Function>().clone();
	(func, closure)
}

fn click_on(el: &web_sys::Element) {
	let ev = web_sys::MouseEvent::new("click").unwrap();
	el.dispatch_event(&ev).unwrap();
}

// ───────────────────────── static & dynamic text ─────────────────────────

#[wasm_bindgen_test]
fn renders_a_fully_static_element() {
	let vn = tpl(&["<div>hello</div>"], vec![]);
	let (container, _root) = mount(vn);
	assert_eq!(container.inner_html(), "<div>hello</div>");
}

#[wasm_bindgen_test]
fn substitutes_a_lone_text_hole() {
	let vn = tpl(&["<p>Hello, ", "!</p>"], vec![JsValue::from_str("World")]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("p").unwrap().unwrap().text_content().unwrap(), "Hello, World!");
}

#[wasm_bindgen_test]
fn substitutes_multiple_text_holes_in_one_run() {
	let vn = tpl(&["<p>", " of ", "</p>"], vec![JsValue::from_f64(3.0), JsValue::from_f64(10.0)]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("p").unwrap().unwrap().text_content().unwrap(), "3 of 10");
}

#[wasm_bindgen_test]
fn numbers_render_as_text_but_booleans_render_as_nothing() {
	// Matches JSX child semantics: `{0}` shows "0", `{false}`/`{true}` show
	// nothing (so `{cond && <X/>}` doesn't print a stray "false").
	let vn = tpl(&["<div>", "</div>"], vec![JsValue::from_f64(0.0)]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("div").unwrap().unwrap().text_content().unwrap(), "0");

	let vn = tpl(&["<div>", "</div>"], vec![JsValue::from_bool(false)]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("div").unwrap().unwrap().text_content().unwrap(), "");
}

#[wasm_bindgen_test]
fn null_and_undefined_holes_render_nothing() {
	let vn = tpl(&["<div>", "</div>"], vec![JsValue::NULL]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("div").unwrap().unwrap().child_nodes().length(), 0);

	let vn = tpl(&["<div>", "</div>"], vec![JsValue::UNDEFINED]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("div").unwrap().unwrap().child_nodes().length(), 0);
}

// ───────────────────────────── attributes ─────────────────────────────

#[wasm_bindgen_test]
fn static_attribute_is_applied() {
	let vn = tpl(&["<div id=\"main\"></div>"], vec![]);
	let (container, _root) = mount(vn);
	let div = container.query_selector("div").unwrap().unwrap();
	assert_eq!(div.get_attribute("id").as_deref(), Some("main"));
}

#[wasm_bindgen_test]
fn whole_value_attribute_hole_keeps_live_value_type() {
	// `disabled=${x}` — the whole attr value is one hole, so the live JS
	// value (a bool here) survives, rather than being stringified.
	let vn = tpl(&["<button disabled=", "></button>"], vec![JsValue::from_bool(true)]);
	let (container, _root) = mount(vn);
	let btn = container.query_selector("button").unwrap().unwrap();
	assert!(btn.has_attribute("disabled"));

	let vn = tpl(&["<button disabled=", "></button>"], vec![JsValue::from_bool(false)]);
	let (container, _root) = mount(vn);
	let btn = container.query_selector("button").unwrap().unwrap();
	assert!(!btn.has_attribute("disabled"));
}

#[wasm_bindgen_test]
fn mixed_text_and_hole_attribute_is_stringified_and_concatenated() {
	let vn = tpl(&["<div class=\"item ", " selected\"></div>"], vec![JsValue::from_str("active")]);
	let (container, _root) = mount(vn);
	let div = container.query_selector("div").unwrap().unwrap();
	assert_eq!(div.get_attribute("class").as_deref(), Some("item active selected"));
}

#[wasm_bindgen_test]
fn class_name_survives_html_lowercasing() {
	// Regression test: DomParser lowercases `className` -> `classname`
	// while parsing the sentinel HTML; html_template.rs must restore the
	// camelCase form so diff::set_prop's `key == "className"` special case
	// still fires (otherwise the class is silently dropped).
	let vn = tpl(&["<div className=", "></div>"], vec![JsValue::from_str("card")]);
	let (container, _root) = mount(vn);
	let div: web_sys::HtmlElement = container.query_selector("div").unwrap().unwrap().unchecked_into();
	assert_eq!(div.class_name(), "card");
}

#[wasm_bindgen_test]
fn html_for_survives_html_lowercasing() {
	let vn = tpl(&["<label htmlFor=", "></label>"], vec![JsValue::from_str("email")]);
	let (container, _root) = mount(vn);
	let label = container.query_selector("label").unwrap().unwrap();
	assert_eq!(label.get_attribute("for").as_deref(), Some("email"));
}

#[wasm_bindgen_test]
fn style_object_hole_is_converted_to_css_text() {
	let style = Object::new();
	Reflect::set(&style, &"color".into(), &"red".into()).unwrap();
	Reflect::set(&style, &"fontSize".into(), &JsValue::from_f64(12.0)).unwrap();

	let vn = tpl(&["<div style=", "></div>"], vec![style.into()]);
	let (container, _root) = mount(vn);
	let div: web_sys::HtmlElement = container.query_selector("div").unwrap().unwrap().unchecked_into();
	let css = div.style().css_text();
	assert!(css.contains("color: red"), "expected color in {css}");
	assert!(css.contains("font-size: 12px"), "expected font-size in {css}");
}

#[wasm_bindgen_test]
fn dangerously_set_inner_html_flattens_the_html_object() {
	let obj = Object::new();
	Reflect::set(&obj, &"__html".into(), &"<em>raw</em>".into()).unwrap();

	let vn = tpl(&["<div dangerouslySetInnerHTML=", "></div>"], vec![obj.into()]);
	let (container, _root) = mount(vn);
	let div = container.query_selector("div").unwrap().unwrap();
	assert_eq!(div.inner_html(), "<em>raw</em>");
}

// ─────────────────────────────── events ───────────────────────────────

#[wasm_bindgen_test]
fn onclick_hole_fires_on_click() {
	let calls = Rc::new(RefCell::new(0));
	let calls2 = calls.clone();
	let (f, _closure) = js_fn(move |_e| *calls2.borrow_mut() += 1);

	let vn = tpl(&["<button onClick=", ">Click</button>"], vec![f.into()]);
	let (container, _root) = mount(vn);
	let btn = container.query_selector("button").unwrap().unwrap();
	click_on(&btn);
	assert_eq!(*calls.borrow(), 1);
}

#[wasm_bindgen_test]
fn capture_suffix_survives_html_lowercasing_and_fires_first() {
	// Regression test: without restoring the "Capture" suffix that
	// DomParser's lowercasing destroys, this would silently become a
	// bubble-phase listener and fire *after* the child's handler instead
	// of before it.
	let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
	let order_capture = order.clone();
	let order_bubble = order.clone();
	let (capture_fn, _c1) = js_fn(move |_e| order_capture.borrow_mut().push("capture"));
	let (bubble_fn, _c2) = js_fn(move |_e| order_bubble.borrow_mut().push("bubble"));

	let vn = tpl(&["<div onClickCapture=", "><button onClick=", ">Click</button></div>"], vec![capture_fn.into(), bubble_fn.into()]);
	let (container, _root) = mount(vn);
	let btn = container.query_selector("button").unwrap().unwrap();
	click_on(&btn);

	assert_eq!(*order.borrow(), vec!["capture", "bubble"]);
}

// ─────────────────────── self-closing tags (bug fix) ───────────────────────

#[wasm_bindgen_test]
fn self_closing_non_void_element_does_not_swallow_its_sibling() {
	let vn = tpl(&["<div class=\"a\" /><span>after</span>"], vec![]);
	let (container, _root) = mount(vn);
	// Both must be top-level siblings of the container, not nested.
	assert_eq!(container.children().length(), 2);
	assert_eq!(container.children().item(0).unwrap().tag_name(), "DIV");
	assert_eq!(container.children().item(1).unwrap().tag_name(), "SPAN");
	assert_eq!(container.children().item(1).unwrap().text_content().unwrap(), "after");
}

#[wasm_bindgen_test]
fn self_closing_component_does_not_swallow_its_sibling() {
	// This is the sharpest form of the bug: `<${Comp} />` with no children
	// is the natural way to self-close a component, and used to leave the
	// synthetic `mr-slot-N` tag open to absorb whatever followed it.
	let comp_closure = {
		let inner = Rc::new(|_props: JsValue| -> JsValue { tpl(&["<i>comp</i>"], vec![]) });
		Closure::wrap(Box::new(move |props: JsValue| inner(props)) as Box<dyn FnMut(JsValue) -> JsValue>)
	};
	let comp_fn: Function = comp_closure.as_ref().unchecked_ref::<Function>().clone();

	let vn = tpl(&["<div><", " /><span>after</span></div>"], vec![comp_fn.into()]);
	let (container, _root) = mount(vn);
	let outer = container.query_selector("div").unwrap().unwrap();
	assert_eq!(outer.children().length(), 2, "component and span should be siblings, not nested");
	assert_eq!(outer.query_selector("i").unwrap().unwrap().text_content().unwrap(), "comp");
	assert_eq!(outer.query_selector("span").unwrap().unwrap().text_content().unwrap(), "after");
}

#[wasm_bindgen_test]
fn self_closing_void_elements_still_work_and_dont_absorb_siblings() {
	let vn = tpl(&["<br/><hr/><span>after</span>"], vec![]);
	let (container, _root) = mount(vn);
	assert_eq!(container.children().length(), 3);
	assert_eq!(container.children().item(2).unwrap().tag_name(), "SPAN");
}

#[wasm_bindgen_test]
fn img_self_close_with_attribute_hole_still_parses_correctly() {
	let vn = tpl(&["<img src=", " /><p>caption</p>"], vec![JsValue::from_str("a.png")]);
	let (container, _root) = mount(vn);
	let img = container.query_selector("img").unwrap().unwrap();
	assert_eq!(img.get_attribute("src").as_deref(), Some("a.png"));
	// The <p> must be a sibling of <img>, not swallowed by it.
	assert!(container.query_selector("img + p").unwrap().is_some());
}

// ───────────────────────── dynamic tags & fragments ─────────────────────────

#[wasm_bindgen_test]
fn dynamic_tag_name_hole_renders_the_named_tag() {
	let vn = tpl(&["<", ">text</", ">"], vec![JsValue::from_str("section"), JsValue::from_str("section")]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("section").unwrap().unwrap().text_content().unwrap(), "text");
}

#[wasm_bindgen_test]
fn multiple_top_level_roots_render_as_siblings() {
	let vn = tpl(&["<div>a</div><div>b</div><div>c</div>"], vec![]);
	let (container, _root) = mount(vn);
	assert_eq!(container.children().length(), 3);
}

// ───────────────────────────── components ─────────────────────────────

#[wasm_bindgen_test]
fn component_hole_receives_props_and_children() {
	let captured_name: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
	let captured_name2 = captured_name.clone();

	let comp_closure = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
		let name = Reflect::get(&props, &"name".into()).unwrap().as_string().unwrap();
		*captured_name2.borrow_mut() = Some(name.clone());
		let children = Reflect::get(&props, &"children".into()).unwrap();
		// Wrap children in a <b> so we can assert on the rendered shape too.
		let wrapper = Object::new();
		Reflect::set(&wrapper, &"tag".into(), &"b".into()).unwrap();
		// Just pass children through as-is by re-rendering via html`` with
		// the children as a hole, keeping this a black-box round trip.
		tpl_component_wrap(children)
	}) as Box<dyn FnMut(JsValue) -> JsValue>);

	fn tpl_component_wrap(children: JsValue) -> JsValue {
		tpl(&["<b>", "</b>"], vec![children])
	}

	let comp_fn: Function = comp_closure.as_ref().unchecked_ref::<Function>().clone();

	let vn = tpl(&["<", " name=", ">hello</", ">"], vec![comp_fn.clone().into(), JsValue::from_str("Greeter"), comp_fn.into()]);
	let (container, _root) = mount(vn);

	assert_eq!(captured_name.borrow().as_deref(), Some("Greeter"));
	assert_eq!(container.query_selector("b").unwrap().unwrap().text_content().unwrap(), "hello");
}

#[wasm_bindgen_test]
fn self_closing_component_as_top_level_root_does_not_swallow_its_sibling() {
	// Same bug class as `self_closing_component_does_not_swallow_its_sibling`,
	// but with the self-closed component as one of the template's *own*
	// top-level roots (rendered via the multi-root -> Fragment path in
	// `html_template`) rather than nested inside a wrapping element.
	let comp_closure = {
		let inner = Rc::new(|_props: JsValue| -> JsValue { tpl(&["<i>comp</i>"], vec![]) });
		Closure::wrap(Box::new(move |props: JsValue| inner(props)) as Box<dyn FnMut(JsValue) -> JsValue>)
	};
	let comp_fn: Function = comp_closure.as_ref().unchecked_ref::<Function>().clone();

	let vn = tpl(&["<", " /><span>after</span>"], vec![comp_fn.into()]);
	let (container, _root) = mount(vn);
	assert_eq!(container.children().length(), 2, "component and span should be top-level siblings, not nested");
	assert_eq!(container.query_selector("i").unwrap().unwrap().text_content().unwrap(), "comp");
	assert_eq!(container.query_selector("span").unwrap().unwrap().text_content().unwrap(), "after");
}

#[wasm_bindgen_test]
fn function_component_used_as_tag_renders_like_jsx() {
	// The exact use case: exporting a plain function and using it as an
	// html tag, both self-closed with props and with explicit children.
	let card_closure = Closure::wrap(Box::new(move |props: JsValue| -> JsValue {
		let title = Reflect::get(&props, &"title".into()).unwrap().as_string().unwrap();
		tpl(&["<section class=\"card\"><h1>", "</h1></section>"], vec![JsValue::from_str(&title)])
	}) as Box<dyn FnMut(JsValue) -> JsValue>);
	let card_fn: Function = card_closure.as_ref().unchecked_ref::<Function>().clone();

	// Self-closing usage: `<${Card} title="Hi" />`
	let vn = tpl(&["<", " title=\"Hi\" />"], vec![card_fn.into()]);
	let (container, _root) = mount(vn);
	assert!(container.query_selector("section.card").unwrap().is_some());
	assert_eq!(container.query_selector("h1").unwrap().unwrap().text_content().unwrap(), "Hi");
}

// ───────────────────────────── keys & refs ─────────────────────────────

#[wasm_bindgen_test]
fn keyed_list_reorder_preserves_dom_identity_through_html_template() {
	fn render_list(root: &mut JsRoot, items: &[(&str, &str)]) {
		// Build the list body as an array hole rather than N interpolated
		// <li> siblings, mirroring `${items.map(...)}` usage.
		let arr = Array::new();
		for (key, text) in items {
			let li = tpl(&["<li key=", ">", "</li>"], vec![JsValue::from_str(key), JsValue::from_str(text)]);
			arr.push(&li);
		}
		let vn = tpl(&["<ul>", "</ul>"], vec![arr.into()]);
		root.render(vn).unwrap();
	}

	let container = make_container();
	let mut root = mount_root(tpl(&["<ul></ul>"], vec![]), container.clone()).unwrap();

	render_list(&mut root, &[("a", "Apple"), ("b", "Banana"), ("c", "Cherry")]);
	let lis = container.query_selector_all("li").unwrap();
	let first_li_before = lis.get(0).unwrap().unchecked_into::<web_sys::Element>();

	render_list(&mut root, &[("c", "Cherry"), ("a", "Apple"), ("b", "Banana")]);
	let lis_after = container.query_selector_all("li").unwrap();
	let texts: Vec<String> =
		(0..lis_after.length()).map(|i| lis_after.get(i).unwrap().unchecked_into::<web_sys::Element>().text_content().unwrap()).collect();
	assert_eq!(texts, vec!["Cherry", "Apple", "Banana"]);

	// The "Apple" node itself (not just its text) should be the same DOM
	// node that was first rendered, proving the key was honored.
	let apple_after = lis_after.get(1).unwrap().unchecked_into::<web_sys::Element>();
	assert!(first_li_before.is_same_node(Some(apple_after.as_ref())));
}

#[wasm_bindgen_test]
fn object_ref_hole_is_populated_with_the_dom_node() {
	let ref_obj = Object::new();
	Reflect::set(&ref_obj, &"current".into(), &JsValue::NULL).unwrap();

	let vn = tpl(&["<input ref=", " />"], vec![ref_obj.clone().into()]);
	let (container, _root) = mount(vn);

	let current = Reflect::get(&ref_obj, &"current".into()).unwrap();
	assert!(!current.is_null(), "ref.current should be populated after mount");
	let input = container.query_selector("input").unwrap().unwrap();
	let current_node: web_sys::Node = current.unchecked_into();
	assert!(input.is_same_node(Some(&current_node)));
}

#[wasm_bindgen_test]
fn callback_ref_hole_is_invoked_with_the_dom_node() {
	let received: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
	let received2 = received.clone();
	let (ref_fn, _closure) = js_fn(move |node: JsValue| {
		*received2.borrow_mut() = !node.is_null() && !node.is_undefined();
	});

	let vn = tpl(&["<div ref=", "></div>"], vec![ref_fn.into()]);
	let (_container, _root) = mount(vn);
	assert!(*received.borrow(), "callback ref should have fired with a non-null node");
}

// ─────────────────────────── template caching ───────────────────────────

#[wasm_bindgen_test]
fn same_call_site_recompiles_correctly_across_calls_with_different_values() {
	let statics_arr = Array::new();
	statics_arr.push(&JsValue::from_str("<p>"));
	statics_arr.push(&JsValue::from_str("</p>"));

	let vn1 = tpl_with_array(&statics_arr, &["<p>", "</p>"], vec![JsValue::from_str("first")]);
	let (container1, _r1) = mount(vn1);
	assert_eq!(container1.text_content().unwrap(), "first");

	let vn2 = tpl_with_array(&statics_arr, &["<p>", "</p>"], vec![JsValue::from_str("second")]);
	let (container2, _r2) = mount(vn2);
	assert_eq!(container2.text_content().unwrap(), "second");
}

// ───────────────────────── medium-sized page ─────────────────────────

#[wasm_bindgen_test]
fn builds_a_medium_sized_page_end_to_end() {
	// Header + nav + a keyed todo list (with a per-item class hole and a
	// click handler) + a conditional banner + a footer, all in one
	// `html`-style composition — exercising static text, dynamic text,
	// static/hole/mixed attributes, className casing, list rendering via
	// an array hole, per-item keys, event handlers, conditional (null)
	// rendering, and multiple top-level roots together.
	let clicked: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

	let make_item = |id: &str, label: &str, done: bool, clicked: Rc<RefCell<Vec<String>>>| -> JsValue {
		let id_owned = id.to_string();
		let (on_click, closure) = js_fn(move |_e| clicked.borrow_mut().push(id_owned.clone()));
		// Leak the closure for the duration of the test; the DOM keeps the
		// JS Function alive via the listener, but Rust needs the Closure
		// kept around too so it isn't dropped while still referenced.
		closure.forget();
		let cls = if done { "todo done" } else { "todo" };
		tpl(
			&["<li class=\"", "\" key=\"", "\" onClick=", ">", "</li>"],
			vec![JsValue::from_str(cls), JsValue::from_str(id), on_click.into(), JsValue::from_str(label)],
		)
	};

	let show_banner = true;
	let items = Array::new();
	items.push(&make_item("1", "Write tests", true, clicked.clone()));
	items.push(&make_item("2", "Ship feature", false, clicked.clone()));
	items.push(&make_item("3", "Celebrate", false, clicked.clone()));

	let banner = if show_banner { tpl(&["<div className=\"banner\">3 items</div>"], vec![]) } else { JsValue::NULL };

	let page = tpl(
		&[
			"<header><h1>",
			"</h1></header>\
             <nav><a href=\"/\">Home</a></nav>\
             ",
			"\
             <ul>",
			"</ul>\
             <footer>",
			"</footer>",
		],
		vec![JsValue::from_str("My Todos"), banner, items.into(), JsValue::from_str("(c) micro-react")],
	);

	let (container, _root) = mount(page);

	assert_eq!(container.query_selector("header h1").unwrap().unwrap().text_content().unwrap(), "My Todos");
	assert_eq!(container.query_selector("nav a").unwrap().unwrap().get_attribute("href").as_deref(), Some("/"));

	let banner_el: web_sys::HtmlElement = container.query_selector("div.banner").unwrap().unwrap().unchecked_into();
	assert_eq!(banner_el.class_name(), "banner");

	let lis = container.query_selector_all("li").unwrap();
	assert_eq!(lis.length(), 3);
	let first_li = lis.get(0).unwrap().unchecked_into::<web_sys::Element>();
	assert_eq!(first_li.get_attribute("class").as_deref(), Some("todo done"));

	// Click the second item and confirm its handler fired with its id.
	let second_li = lis.get(1).unwrap().unchecked_into::<web_sys::Element>();
	click_on(&second_li);
	assert_eq!(*clicked.borrow(), vec!["2".to_string()]);

	assert_eq!(container.query_selector("footer").unwrap().unwrap().text_content().unwrap(), "(c) micro-react");

	// Sanity check on the overall shape: header, nav, banner, ul, footer.
	assert_eq!(container.children().length(), 5);
}

// ───────────────────── inline whitespace between elements ─────────────────────

#[wasm_bindgen_test]
fn same_line_whitespace_between_elements_is_preserved_as_a_space() {
	// Regression test: a whitespace-only text node with no newline is a
	// deliberate same-line separator, not template-formatting indentation,
	// and must survive as a literal space (matching what `h()` gives you
	// when that space is an explicit string child).
	let vn = tpl(&["<span>a</span> <span>b</span>"], vec![]);
	let (container, _root) = mount(vn);
	assert_eq!(container.text_content().unwrap(), "a b");
	assert_eq!(container.children().length(), 2);
}

#[wasm_bindgen_test]
fn newline_indentation_between_elements_still_collapses_to_nothing() {
	let vn = tpl(&["<ul>\n  <li>a</li>\n  <li>b</li>\n</ul>"], vec![]);
	let (container, _root) = mount(vn);
	let ul = container.query_selector("ul").unwrap().unwrap();
	// Only the two <li> elements — no stray whitespace text nodes.
	assert_eq!(ul.child_nodes().length(), 2);
	assert_eq!(ul.text_content().unwrap(), "ab");
}

#[wasm_bindgen_test]
fn whitespace_hole_separator_still_works_alongside_literal_space() {
	// `${a} ${b}` — a literal same-line space between two holes in one text
	// run — should also come through, exercising the mixed-text path
	// rather than the whitespace-only-text-node path.
	let vn = tpl(&["<p>", " ", "</p>"], vec![JsValue::from_str("left"), JsValue::from_str("right")]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("p").unwrap().unwrap().text_content().unwrap(), "left right");
}

// ───────────────────── table-context root elements ─────────────────────

#[wasm_bindgen_test]
fn tr_as_the_whole_template_root_is_not_dropped() {
	// Regression test: without a real <table> ancestor, the HTML parser's
	// "in body" insertion mode silently ignores a <tr> start tag — it
	// would otherwise vanish with no error, since <tr> only has meaning
	// inside table-construction context.
	let vn = tpl(&["<tr><td>", "</td></tr>"], vec![JsValue::from_str("cell")]);
	let (container, _root) = mount(vn);
	let tr = container.query_selector("tr").unwrap();
	assert!(tr.is_some(), "expected a <tr> to be rendered, but it was dropped");
	assert_eq!(container.query_selector("td").unwrap().unwrap().text_content().unwrap(), "cell");
}

#[wasm_bindgen_test]
fn multiple_sibling_trs_as_template_roots_all_survive() {
	let vn = tpl(&["<tr><td>", "</td></tr><tr><td>", "</td></tr>"], vec![JsValue::from_str("one"), JsValue::from_str("two")]);
	let (container, _root) = mount(vn);
	let trs = container.query_selector_all("tr").unwrap();
	assert_eq!(trs.length(), 2);
}

#[wasm_bindgen_test]
fn td_as_the_whole_template_root_is_not_dropped() {
	let vn = tpl(&["<td>", "</td>"], vec![JsValue::from_str("solo cell")]);
	let (container, _root) = mount(vn);
	let td = container.query_selector("td").unwrap();
	assert!(td.is_some(), "expected a <td> to be rendered, but it was dropped");
	assert_eq!(td.unwrap().text_content().unwrap(), "solo cell");
}

#[wasm_bindgen_test]
fn tbody_as_the_whole_template_root_is_not_dropped() {
	let vn = tpl(&["<tbody><tr><td>", "</td></tr></tbody>"], vec![JsValue::from_str("x")]);
	let (container, _root) = mount(vn);
	assert!(container.query_selector("tbody").unwrap().is_some());
	assert_eq!(container.query_selector("td").unwrap().unwrap().text_content().unwrap(), "x");
}

#[wasm_bindgen_test]
fn col_as_the_whole_template_root_is_not_dropped() {
	let vn = tpl(&["<col span=\"", "\"></col>"], vec![JsValue::from_str("2")]);
	let (container, _root) = mount(vn);
	let col = container.query_selector("col").unwrap();
	assert!(col.is_some(), "expected a <col> to be rendered, but it was dropped");
	assert_eq!(col.unwrap().get_attribute("span").as_deref(), Some("2"));
}

#[wasm_bindgen_test]
fn tr_nested_normally_inside_a_table_in_the_same_template_is_unaffected() {
	// The common case (table structure fully written out in one template)
	// must keep working exactly as before — the wrapper only kicks in
	// when the *root* itself is a table-context tag.
	let vn = tpl(&["<table><tbody><tr><td>", "</td></tr></tbody></table>"], vec![JsValue::from_str("y")]);
	let (container, _root) = mount(vn);
	assert_eq!(container.query_selector("td").unwrap().unwrap().text_content().unwrap(), "y");
	assert_eq!(container.query_selector_all("table").unwrap().length(), 1);
}

// ───────────────────────────── SVG attributes ─────────────────────────────

#[wasm_bindgen_test]
fn view_box_survives_html_lowercasing() {
	let vn = tpl(&["<svg viewBox=\"", "\"></svg>"], vec![JsValue::from_str("0 0 10 10")]);
	let (container, _root) = mount(vn);
	let svg = container.query_selector("svg").unwrap().unwrap();
	assert_eq!(svg.get_attribute("viewBox").as_deref(), Some("0 0 10 10"));
}

#[wasm_bindgen_test]
fn gradient_units_survives_html_lowercasing() {
	let vn = tpl(&["<svg><linearGradient gradientUnits=\"", "\"></linearGradient></svg>"], vec![JsValue::from_str("userSpaceOnUse")]);
	let (container, _root) = mount(vn);
	let el = container.query_selector("linearGradient").unwrap().unwrap();
	assert_eq!(el.get_attribute("gradientUnits").as_deref(), Some("userSpaceOnUse"));
}

#[wasm_bindgen_test]
fn preserve_aspect_ratio_survives_html_lowercasing() {
	let vn = tpl(&["<svg preserveAspectRatio=\"", "\"></svg>"], vec![JsValue::from_str("xMidYMid meet")]);
	let (container, _root) = mount(vn);
	let svg = container.query_selector("svg").unwrap().unwrap();
	assert_eq!(svg.get_attribute("preserveAspectRatio").as_deref(), Some("xMidYMid meet"));
}
