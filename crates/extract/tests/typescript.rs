//! Tier A against `tests/fixtures/typescript/`.
//!
//! The shared guards of `specs/02-extraction.md` § Validation are in
//! [`common`]. What is here is what only TypeScript can claim: that an arrow
//! binding is a function, that a bare class member is public while a bare
//! declaration is not, that JSX tags are uses, and that one symbol can have
//! several declarations in one file.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::{Extracted, extract_tree, fixture};

/// Every kind `queries/typescript/tags.scm` claims to emit.
///
/// No `struct`, `trait` or `macro`: TypeScript has none. `module` is a
/// `namespace` or a `declare module`, never the file.
const KINDS: &[&str] = &[
    "class",
    "interface",
    "enum",
    "typealias",
    "module",
    "function",
    "method",
    "constructor",
    "field",
    "constant",
    "variable",
];

fn root() -> PathBuf {
    fixture("typescript")
}

fn extracted() -> Extracted {
    extract_tree(&root())
}

#[test]
fn golden_facts() {
    common::assert_golden(&extracted());
}

#[test]
fn spans_are_exact() {
    common::assert_spans_exact(&extracted());
}

#[test]
fn kinds_stay_inside_the_vocabulary() {
    common::assert_kinds_closed(&extracted());
}

#[test]
fn every_parent_names_something_that_exists() {
    common::assert_parents_exist(&extracted());
}

#[test]
fn every_kind_is_emitted_at_least_once() {
    common::assert_kinds_present(&extracted(), KINDS);
}

#[test]
fn no_node_takes_two_definition_captures() {
    common::assert_captures_disjoint(&root());
}

#[test]
fn facts_are_a_function_of_their_own_file() {
    common::assert_facts_are_local(&extracted());
}

#[test]
fn a_second_extraction_is_byte_identical() {
    common::assert_deterministic(&root());
}

#[test]
fn kinds_are_faithful() {
    // The test upstream's query fails outright: it captures only ambient
    // forms, so every one of these would be missing, not merely mislabelled.
    let extracted = extracted();
    let found: BTreeSet<(String, String)> = extracted
        .rows_in("def", "kinds.ts")
        .into_iter()
        .filter_map(|r| Some((r.get(3)?.clone(), r.get(2)?.clone())))
        .collect();
    for (name, kind) in [
        ("BANNER", "constant"),
        ("retries", "variable"),
        ("legacy", "variable"),
        // An arrow binding: `const` says constant, the value node says
        // function, and `calls` counts only callable kinds.
        ("describe", "function"),
        ("hidden", "function"),
        ("sealed", "function"),
        ("parse", "function"),
        ("ids", "function"),
        ("outer", "function"),
        ("inner", "function"),
        ("Handler", "interface"),
        ("name", "field"),
        ("Mode", "enum"),
        ("Fast", "constant"),
        ("Slow", "constant"),
        ("Key", "typealias"),
        ("Config", "class"),
        ("Base", "class"),
        ("limit", "field"),
        ("#secret", "field"),
        ("constructor", "constructor"),
        ("handle", "method"),
        ("describe", "method"),
        ("size", "method"),
        ("#check", "method"),
        ("run", "method"),
        ("Limits", "module"),
        ("MAX", "constant"),
    ] {
        assert!(
            found.contains(&(name.to_string(), kind.to_string())),
            "{name} should be a {kind}; kinds.ts has {found:?}"
        );
    }
    // A `const` in a function body is a local, not a definition.
    assert!(!found.iter().any(|(n, _)| n == "local"), "{found:?}");
}

/// TypeScript states visibility in two places, and silence means opposite
/// things in them (`specs/01-facts.md` § `visibility`).
#[test]
fn a_bare_declaration_is_restricted_and_a_bare_member_is_public() {
    let vis = extracted().pairs("visibility");
    for sym in [
        "local kinds.ts BANNER.",
        "local kinds.ts Config#",
        "local kinds.ts Limits/",
        "local kinds.ts Limits/MAX.",
        // A class member with no modifier, and one with `static readonly`.
        "local kinds.ts Config#limit.",
        "local kinds.ts Config#DEFAULT.",
        "local kinds.ts Config#handle().",
        // `export` inside a `declare module` block.
        "local ambient.d.ts `\"legacy-logger\"`/log().",
    ] {
        assert_eq!(vis.get(sym).map(String::as_str), Some("public"), "{sym}");
    }
    for sym in [
        "local kinds.ts retries.",
        "local kinds.ts sealed().",
        "local kinds.ts Limits/hidden().",
        "local kinds.ts Config#quiet.",
        "local kinds.ts Config#attempts.",
        "local kinds.ts Config#`#secret`.",
        "local kinds.ts Config#`#check`().",
    ] {
        assert_eq!(
            vis.get(sym).map(String::as_str),
            Some("restricted"),
            "{sym}"
        );
    }
    // An interface member and an enum member have nowhere to write one.
    for sym in [
        "local kinds.ts Handler#handle().",
        "local kinds.ts Handler#name.",
        "local kinds.ts Mode#Fast.",
    ] {
        assert_eq!(vis.get(sym).map(String::as_str), Some("inherited"), "{sym}");
    }
}

/// `specs/01-facts.md` § `def`: one symbol, several declarations, one file.
#[test]
fn one_symbol_with_several_declarations_has_one_def_and_a_span_each() {
    let extracted = extracted();
    // Rows as the store holds them. A segment is deduplicated when it is
    // written, so the `def` row three declarations share is one row, and the
    // in-memory segment here has not been written yet.
    let of = |relation: &str, sym: &str| -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = extracted
            .rows(relation)
            .into_iter()
            .filter(|r| r.first().map(String::as_str) == Some(sym))
            .collect();
        rows.dedup();
        rows
    };
    // Two overload signatures and the implementation.
    let parse = "local kinds.ts parse().";
    assert_eq!(of("def", parse).len(), 1);
    assert_eq!(of("parent", parse).len(), 1);
    assert_eq!(of("def_span", parse).len(), 3);
    assert_eq!(of("def_name", parse).len(), 3);
    assert_eq!(of("def_sig", parse).len(), 3);
    // A getter and its setter share a tier-A symbol; SCIP separates them.
    let size = "local kinds.ts Config#size().";
    assert_eq!(of("def", size).len(), 1);
    let sigs: Vec<String> = of("def_sig", size)
        .into_iter()
        .filter_map(|r| r.get(1).cloned())
        .collect();
    assert_eq!(sigs, ["get size(): number", "set size(value: number)"]);
}

#[test]
fn a_capitalized_jsx_tag_is_a_use_and_an_intrinsic_one_is_not() {
    let extracted = extracted();
    let names = |path: &str| -> BTreeSet<String> {
        extracted
            .rows_in("name_ref", path)
            .into_iter()
            .filter_map(|r| r.first().cloned())
            .collect()
    };
    let panel = names("ui/panel.tsx");
    assert!(panel.contains("Row"), "{panel:?}");
    assert!(panel.contains("Panel"), "{panel:?}");
    assert!(panel.contains("open"), "{panel:?}");
    for intrinsic in ["div", "span"] {
        assert!(!panel.contains(intrinsic), "{panel:?}");
        assert!(!names("ui/row.tsx").contains(intrinsic));
    }
}

#[test]
fn jsdoc_is_the_documentation_and_a_decorator_is_in_the_span() {
    let extracted = extracted();
    let docs = extracted.pairs("def_doc");
    assert_eq!(
        docs.get("local kinds.ts Config#").map(String::as_str),
        Some(
            "A class, with a documented field and a documented method.\n\nTwo lines of documentation, to prove newlines survive."
        ),
    );
    assert_eq!(
        docs.get("local kinds.ts Config#limit.").map(String::as_str),
        Some("The limit."),
    );
    assert_eq!(docs.get("local kinds.ts Config#quiet."), None);

    // `@sealed` sits inside the `export` statement, before the keyword: in the
    // span, out of the signature.
    let src = std::fs::read_to_string(root().join("kinds.ts")).expect("source");
    let span = extracted
        .rows("def_span")
        .into_iter()
        .find(|r| r.first().map(String::as_str) == Some("local kinds.ts Config#"))
        .expect("Config has a span");
    let (start, end): (usize, usize) = (
        span[3].parse().expect("start"),
        span[4].parse().expect("end"),
    );
    let text = src.get(start..end).expect("a byte range in the file");
    assert!(text.starts_with("/**\n * A class"), "{text:?}");
    assert!(text.contains("@sealed\nexport class Config"), "{text:?}");
    let sigs = extracted.pairs("def_sig");
    assert_eq!(
        sigs.get("local kinds.ts Config#").map(String::as_str),
        Some("export class Config implements Handler"),
    );
    // Not cut at the `=` of `=>`.
    assert_eq!(
        sigs.get("local kinds.ts describe().").map(String::as_str),
        Some("export const describe = (config: Config): string => config.describe()"),
    );
}

#[test]
fn an_import_is_the_specifier_without_its_quotes() {
    let imports = extracted().rows("import");
    for row in [
        ["ui/panel.tsx", "../db/conn", ""],
        // `import * as net` binds a local name for the module itself.
        ["ui/panel.tsx", "../net/conn", "net"],
        // `import type` is an import.
        ["store/store.ts", "../kinds", ""],
    ] {
        let row: Vec<String> = row.iter().map(|s| (*s).to_string()).collect();
        assert!(imports.contains(&row), "{row:?} not in {imports:?}");
    }
    assert_eq!(imports.len(), 6, "one row per statement: {imports:?}");
}

/// `specs/01-facts.md` § `name_export`: an `export { }` clause or an `export
/// default` is a fact about the file, by local name, and it never flips
/// `visibility`. Regression for #22.
#[test]
fn an_export_clause_is_a_name_export_and_leaves_visibility_alone() {
    let ex = extracted();
    let exports = ex.rows("name_export");
    let expected: Vec<Vec<String>> = [
        ["kinds.ts", "outer"],
        ["kinds.ts", "retries"],
        ["kinds.ts", "sealed"],
    ]
    .iter()
    .map(|r| r.iter().map(|s| (*s).to_string()).collect())
    .collect();
    // The local name, never the alias: `sealed as seal` records `sealed`, and
    // `export { x } from "m"` elsewhere in the fixture is an import, not this.
    assert_eq!(exports, expected);
    let vis = ex.pairs("visibility");
    for sym in [
        "local kinds.ts retries.",
        "local kinds.ts sealed().",
        "local kinds.ts outer().",
    ] {
        assert_eq!(
            vis.get(sym).map(String::as_str),
            Some("restricted"),
            "{sym}"
        );
    }
}

/// The two shapes the fixture cannot hold: `export = f` is a module-level
/// error next to other exports, and a re-export tree needs no definitions.
#[test]
fn an_export_assignment_is_a_name_export_and_a_reexport_is_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("cjs.ts"),
        "function main(): void {}\nexport = main;\n",
    )
    .expect("writes");
    std::fs::write(
        dir.path().join("re.ts"),
        "export { open } from \"./db\";\nexport * from \"./net\";\n",
    )
    .expect("writes");
    let ex = extract_tree(dir.path());
    assert_eq!(
        ex.rows("name_export"),
        [vec!["cjs.ts".to_string(), "main".to_string()]]
    );
    assert_eq!(ex.rows("import").len(), 2, "{:?}", ex.rows("import"));
}
