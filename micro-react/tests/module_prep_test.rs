//! Integration tests for the real `micro_react::module_prep` module — exercised through
//! the crate's public API, not a local reimplementation, so these tests actually catch
//! regressions in `src/module_prep.rs`.

use micro_react::module_prep::{prepare_module_str, rewrite_exports_str};

#[cfg(test)]
mod tests {
	use super::*;

	// ─── imports ───

	#[test]
	fn strips_named_import_and_records_specifier() {
		let (code, specifiers) = prepare_module_str("import { useState } from './micro-react.js';\nconst x = 1;");
		assert_eq!(code, "\nconst x = 1;");
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].named, vec![("useState".to_string(), "useState".to_string())]);
		assert_eq!(specifiers[0].default_name, None);
		assert_eq!(specifiers[0].namespace_name, None);
		assert_eq!(specifiers[0].from, "./micro-react.js");
	}

	#[test]
	fn strips_default_import_and_records_specifier() {
		let (code, specifiers) = prepare_module_str("import Foo from './foo.js';\nFoo();");
		assert_eq!(code, "\nFoo();");
		assert_eq!(specifiers[0].default_name, Some("Foo".to_string()));
		assert_eq!(specifiers[0].named, Vec::<(String, String)>::new());
		assert_eq!(specifiers[0].namespace_name, None);
	}

	#[test]
	fn strips_named_import_with_multiple_bindings_and_extra_whitespace() {
		let (code, specifiers) = prepare_module_str("import {  a ,b ,  c  } from 'lib';");
		assert_eq!(code, "");
		assert_eq!(
			specifiers[0].named,
			vec![("a".to_string(), "a".to_string()), ("b".to_string(), "b".to_string()), ("c".to_string(), "c".to_string())]
		);
	}

	#[test]
	fn strips_named_import_with_renames() {
		let (code, specifiers) = prepare_module_str("import { val as aliasVal, original } from 'module';");
		assert_eq!(code, "");
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].named, vec![("aliasVal".to_string(), "val".to_string()), ("original".to_string(), "original".to_string())]);
	}

	#[test]
	fn strips_namespace_wildcard_import() {
		let (code, specifiers) = prepare_module_str("import * as ns from 'utils';");
		assert_eq!(code, "");
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].namespace_name, Some("ns".to_string()));
		assert_eq!(specifiers[0].named, Vec::<(String, String)>::new());
		assert_eq!(specifiers[0].default_name, None);
	}

	#[test]
	fn strips_mixed_default_and_namespace_import() {
		let (code, specifiers) = prepare_module_str("import React, * as ns from 'react';");
		assert_eq!(code, "");
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].default_name, Some("React".to_string()));
		assert_eq!(specifiers[0].namespace_name, Some("ns".to_string()));
		assert_eq!(specifiers[0].named, Vec::<(String, String)>::new());
	}

	#[test]
	fn strips_mixed_default_and_named_rename_import() {
		let (code, specifiers) = prepare_module_str("import React, { useState as useMyState } from 'react';");
		assert_eq!(code, "");
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].default_name, Some("React".to_string()));
		assert_eq!(specifiers[0].namespace_name, None);
		assert_eq!(specifiers[0].named, vec![("useMyState".to_string(), "useState".to_string())]);
	}

	#[test]
	fn import_line_without_trailing_semicolon_still_matches() {
		let (code, specifiers) = prepare_module_str("import { x } from 'mod'\nrest();");
		assert_eq!(code, "\nrest();");
		assert_eq!(specifiers[0].from, "mod");
	}

	#[test]
	fn import_line_indented_with_tabs_still_matches() {
		let (code, specifiers) = prepare_module_str("\t\timport { x } from 'mod';");
		assert_eq!(code, "");
		assert_eq!(specifiers[0].from, "mod");
	}

	#[test]
	fn multiple_import_lines_each_recorded_in_order() {
		let (code, specifiers) = prepare_module_str("import { a } from 'one';\nimport b from 'two';\nuse(a, b);");
		assert_eq!(code, "\n\nuse(a, b);");
		assert_eq!(specifiers.len(), 2);
		assert_eq!(specifiers[0].from, "one");
		assert_eq!(specifiers[1].from, "two");
		assert_eq!(specifiers[1].default_name, Some("b".to_string()));
	}

	#[test]
	fn import_line_with_trailing_code_on_same_line_is_left_untouched() {
		let src = "import { a } from 'one'; doSomethingElse();";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src);
		assert!(specifiers.is_empty());
	}

	#[test]
	fn word_starting_with_import_is_not_mistaken_for_the_keyword() {
		let src = "importantValue = 5;";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src);
		assert!(specifiers.is_empty());
	}

	// ─── export default ───

	#[test]
	fn export_default_function_keeps_declaration_and_defers_assignment() {
		let out = rewrite_exports_str("export default function Hello() { return 1; }");
		assert_eq!(out, "function Hello() { return 1; }\nexports.default = Hello;");
	}

	#[test]
	fn export_default_expression_is_rewritten_inline() {
		let out = rewrite_exports_str("export default 42;");
		assert_eq!(out, "exports.default = 42;");
	}

	#[test]
	fn export_default_class_is_rewritten_inline_not_mistaken_for_function_case() {
		let out = rewrite_exports_str("export default class Foo {}");
		assert_eq!(out, "exports.default = class Foo {}");
	}

	#[test]
	fn export_default_arrow_function_is_rewritten_inline() {
		let out = rewrite_exports_str("export default () => 1;");
		assert_eq!(out, "exports.default = () => 1;");
	}

	#[test]
	fn only_first_export_default_is_touched() {
		let src = "export default function A() {}\nexport default function B() {}";
		let out = rewrite_exports_str(src);
		assert_eq!(out, "function A() {}\nexport default function B() {}\nexports.default = A;");
	}

	#[test]
	fn word_starting_with_export_is_not_mistaken_for_the_keyword() {
		let src = "exportedThing = 1;";
		assert_eq!(rewrite_exports_str(src), src);
	}

	// ─── named re-exports ───

	#[test]
	fn named_reexport_becomes_assignments() {
		let out = rewrite_exports_str("const a = 1, b = 2;\nexport { a, b as c };");
		assert_eq!(out, "const a = 1, b = 2;\n\nexports.a = a;\nexports.c = b;");
	}

	#[test]
	fn named_reexport_with_no_space_before_brace_still_matches() {
		let out = rewrite_exports_str("const a = 1;\nexport{a};");
		assert_eq!(out, "const a = 1;\n\nexports.a = a;");
	}

	#[test]
	fn named_reexport_alias_with_irreguler_whitespace_around_as() {
		let out = rewrite_exports_str("const a = 1;\nexport { a  as   c };");
		assert_eq!(out, "const a = 1;\n\nexports.c = a;");
	}

	#[test]
	fn identifier_containing_as_is_not_mistaken_for_the_as_keyword() {
		let out = rewrite_exports_str("const gas = 1;\nexport { gas };");
		assert_eq!(out, "const gas = 1;\n\nexports.gas = gas;");
	}

	#[test]
	fn named_reexport_line_with_trailing_code_is_left_untouched() {
		let src = "export { a }; doSomethingElse();";
		assert_eq!(rewrite_exports_str(src), src);
	}

	// ─── export declarations ───

	#[test]
	fn export_const_declaration_loses_export_keyword() {
		let out = rewrite_exports_str("export const lol = () => 1;");
		assert_eq!(out, "const lol = () => 1;\nexports.lol = lol;");
	}

	#[test]
	fn export_function_declaration_loses_export_keyword() {
		let out = rewrite_exports_str("export function Meh({ name }) {\n  return 1;\n}");
		assert_eq!(out, "function Meh({ name }) {\n  return 1;\n}\nexports.Meh = Meh;");
	}

	#[test]
	fn export_declaration_with_tab_after_export_still_matches() {
		let out = rewrite_exports_str("export\tconst x = 1;");
		assert_eq!(out, "const x = 1;\nexports.x = x;");
	}

	#[test]
	fn export_declaration_with_double_space_after_export_still_matches() {
		let out = rewrite_exports_str("export  let x = 1;");
		assert_eq!(out, "let x = 1;\nexports.x = x;");
	}

	#[test]
	fn indented_export_declaration_keeps_its_leading_whitespace() {
		let out = rewrite_exports_str("  export const x = 1;");
		assert_eq!(out, "  const x = 1;\nexports.x = x;");
	}

	#[test]
	fn multiple_export_declarations_all_recorded() {
		let out = rewrite_exports_str("export const a = 1;\nexport let b = 2;\nexport function c() {}");
		assert_eq!(out, "const a = 1;\nlet b = 2;\nfunction c() {}\nexports.a = a;\nexports.b = b;\nexports.c = c;");
	}

	#[test]
	fn export_class_declaration_loses_export_keyword() {
		let out = rewrite_exports_str("export class Widget {}");
		assert_eq!(out, "class Widget {}\nexports.Widget = Widget;");
	}

	// ─── non-export/import content is left alone ───

	#[test]
	fn non_export_lines_are_untouched() {
		let src = "function plain() { return 1; }";
		assert_eq!(rewrite_exports_str(src), src);
	}

	#[test]
	fn file_with_no_imports_or_exports_round_trips_unchanged() {
		let src = "function plain() { return <div>hi</div>; }";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src);
		assert!(specifiers.is_empty());
	}

	// ─── realistic combined file ───

	#[test]
	fn realistic_module_with_default_and_named_exports() {
		let src = "export default function Hello({ name }) {\n\
		  return <div>Hello, {name}</div>;\n\
		}\n\
		\n\
		export function Meh({ name }) {\n\
		  return <div>Meh, {name}</div>;\n\
		}\n\
		\n\
		export function lol({ name }) {\n\
		  return <div>lol, {name}</div>;\n\
		}\n";

		let (code, specifiers) = prepare_module_str(src);
		assert!(specifiers.is_empty());
		assert!(code.contains("function Hello({ name }) {"));
		assert!(code.contains("function Meh({ name }) {"));
		assert!(code.contains("function lol({ name }) {"));
		assert!(code.contains("exports.default = Hello;"));
		assert!(code.contains("exports.Meh = Meh;"));
		assert!(code.contains("exports.lol = lol;"));
		assert!(!code.contains("export "));
	}

	// ─── parser boundary: syntax this hand-rolled parser does and doesn't
	// support (task.md: "worth deciding explicitly what subset of import
	// syntax is officially supported and documenting/testing the boundary
	// rather than leaving it implicit") ───

	#[test]
	fn dynamic_import_is_correctly_left_alone_not_mistaken_for_a_static_import() {
		// import(...) is a runtime expression, not a static import
		// declaration — parse_import_line requires whitespace right after
		// "import", so "import(" correctly fails to match and the line is
		// left as ordinary code.
		let src = "const mod = await import('./lazy.js');";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src);
		assert!(specifiers.is_empty());
	}

	#[test]
	fn multi_line_import_is_not_supported_left_untouched_as_plain_code() {
		// Documents a known boundary: parse_import_line matches a whole
		// *line*, so an import statement split across multiple lines is
		// not recognized at all — none of its lines get extracted, and the
		// import is silently left in the output as-is (not stripped, not
		// resolved). This is the documented limitation from task.md, not
		// a crash — pinning it down here so a future change to this
		// behavior is a deliberate, visible diff instead of a surprise.
		let src = "import {\n  a,\n  b\n} from 'lib';\nuse(a, b);";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src, "multi-line imports are not rewritten (known unsupported boundary)");
		assert!(specifiers.is_empty(), "multi-line imports are not extracted as specifiers (known unsupported boundary)");
	}

	#[test]
	fn line_comment_after_import_on_the_same_line_prevents_a_match() {
		// A `//` comment after the closing quote makes the "nothing
		// trailing except an optional semicolon" check fail, so the whole
		// line is left untouched rather than partially parsed. Documents
		// current (conservative — fails closed, not silently wrong)
		// behavior.
		let src = "import { a } from 'lib'; // pulls in a\nuse(a);";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src, "a trailing same-line comment is not stripped, so the line is left as-is (known unsupported boundary)");
		assert!(specifiers.is_empty());
	}

	#[test]
	fn import_type_named_is_ignored_not_misparsed_as_a_default_import_literally_named_type() {
		// Regression test for a real bug this review surfaced: `import
		// type { Foo } from '...'` (TypeScript type-only import, no
		// runtime binding at all) used to fall through to the
		// default-import branch and get extracted as `default_name:
		// "type"` plus a bogus named import — silently wrong rather than
		// safely ignored. It's now recognized and left untouched, since
		// there's no runtime specifier to extract.
		let src = "import type { Foo } from 'lib';\nuse(Foo);";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src, "type-only imports carry no runtime binding and should be left untouched, not misparsed");
		assert!(specifiers.is_empty());
	}

	#[test]
	fn import_type_default_is_also_ignored() {
		let src = "import type Foo from 'lib';\nuse(Foo);";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src);
		assert!(specifiers.is_empty());
	}

	#[test]
	fn a_default_import_literally_named_type_is_still_parsed_correctly() {
		// `type` is not a reserved word, so `import type from '...'` is
		// legal JS/TS meaning "import the default export and bind it to
		// the local name `type`" — the *next* token being `from` (rather
		// than another binding) is what disambiguates this from the
		// type-only-import modifier case above.
		let (code, specifiers) = prepare_module_str("import type from 'lib';\nuse(type);");
		assert_eq!(code, "\nuse(type);");
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].default_name, Some("type".to_string()));
	}

	#[test]
	fn import_assertion_clause_is_not_supported_left_untouched() {
		// `import data from './data.json' with { type: 'json' }` — the
		// trailing `with { ... }` clause means there's content after the
		// closing quote besides an optional `;`, so the "nothing trailing"
		// check fails and the whole line is left unparsed. Documents the
		// known unsupported boundary rather than silently dropping the
		// assertion and treating it as a plain import.
		let src = "import data from './data.json' with { type: 'json' };\nuse(data);";
		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(code, src, "import assertions/attributes are not supported (known unsupported boundary)");
		assert!(specifiers.is_empty());
	}

	#[test]
	fn realistic_module_with_import_and_export_together() {
		let src = "import { helper } from './util.js';\nexport function useIt() { return helper(); }\n";

		let (code, specifiers) = prepare_module_str(src);
		assert_eq!(specifiers.len(), 1);
		assert_eq!(specifiers[0].from, "./util.js");
		assert!(code.contains("function useIt() { return helper(); }"));
		assert!(code.contains("exports.useIt = useIt;"));
		assert!(!code.contains("import "));
		assert!(!code.contains("export "));
	}
}
