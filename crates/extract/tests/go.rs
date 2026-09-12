//! Tier A against `tests/fixtures/go/`.
//!
//! The six shared guards of `specs/02-extraction.md` § Validation are in
//! [`common`]. What is here is what only Go can claim: that the four type
//! forms upstream folds into one `@definition.type` stay four, and that a
//! method finds its owner through a receiver that does not enclose it —
//! the case `docs/plan.md` M5 names by hand.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::{Extracted, extract_tree, fixture};

/// Every kind `queries/go/tags.scm` claims to emit.
///
/// No `module`: Go's `package_clause` names a package that spans files, so a
/// per-file `def` for it would be one definition claimed N times, and it
/// encloses nothing. Package identity comes from tier B.
const KINDS: &[&str] = &[
    "struct",
    "interface",
    "type",
    "typealias",
    "function",
    "method",
    "constant",
    "variable",
    "field",
];

fn root() -> PathBuf {
    fixture("go")
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
fn every_pattern_matches_at_least_once() {
    let extracted = extracted();
    common::assert_kinds_present(&extracted, KINDS);
    // The optional `@alias` child, in both the named and the blank form. One
    // pattern covers plain, aliased and effect-only imports; two patterns would
    // emit two rows for every aliased one.
    let aliases: Vec<String> = extracted
        .rows("import")
        .into_iter()
        .filter_map(|r| r.get(2).cloned())
        .filter(|a| !a.is_empty())
        .collect();
    assert!(aliases.contains(&"str".to_string()), "{aliases:?}");
    assert!(aliases.contains(&"_".to_string()), "{aliases:?}");
    assert_eq!(
        extracted.rows("import").len(),
        7,
        "an aliased import must produce one row, not two"
    );
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
    // The test upstream's query fails. `tree-sitter-go/queries/tags.scm` emits
    // `@definition.type` for a struct, an interface, a defined type and an
    // alias alike — and `type` is not even in this schema's vocabulary, so all
    // four would arrive as one guess.
    //
    // Asserted as (name, kind) pairs rather than a map keyed by name, because
    // `Limit` is deliberately both a field of `Config` and a package-level
    // constant — two definitions, two kinds, two symbols.
    let extracted = extracted();
    let found: BTreeSet<(String, String)> = extracted
        .rows_in("def", "kinds.go")
        .into_iter()
        .filter_map(|r| Some((r.get(3)?.clone(), r.get(2)?.clone())))
        .collect();

    for (name, kind) in [
        ("Config", "struct"),
        ("Handler", "interface"),
        ("Mode", "type"),
        ("Key", "typealias"),
        ("Limit", "field"),
        ("Limit", "constant"),
        ("Banner", "variable"),
        ("Handle", "method"),
        ("describe", "method"),
    ] {
        assert!(
            found.contains(&(name.to_string(), kind.to_string())),
            "{name} should be a {kind}; kinds.go has {found:?}"
        );
    }
}

#[test]
fn a_method_is_owned_by_its_receiver_and_not_by_its_file() {
    // The case `docs/plan.md` M5 names by hand, and the reason `@owner` exists
    // (`specs/02-extraction.md` § `@owner`). Nothing encloses a Go method, so
    // span nesting alone parents all four of these to the file — and worse,
    // synthesizes one symbol for `Get` that every type in the file would claim.
    let extracted = extracted();
    let parents = extracted.pairs("parent");
    for (method, owner) in [
        (
            "local store/store.go Store#Get().",
            "local store/store.go Store#",
        ),
        (
            "local store/store.go Store#Put().",
            "local store/store.go Store#",
        ),
        (
            "local store/store.go Store#evict().",
            "local store/store.go Store#",
        ),
        (
            "local store/store.go Entry#Describe().",
            "local store/store.go Entry#",
        ),
    ] {
        assert_eq!(
            parents.get(method).map(String::as_str),
            Some(owner),
            "{method} should be owned by {owner}",
        );
    }
    // A pointer receiver and a value receiver resolve to the same owner: `Get`
    // takes `*Store` and `Put` takes `Store`.
    assert_eq!(
        parents.get("local store/store.go Store#Get()."),
        parents.get("local store/store.go Store#Put()."),
    );
    // And a free function in the same file is still owned by the file.
    assert_eq!(
        parents
            .get("local store/store.go Warm().")
            .map(String::as_str),
        Some("store/store.go"),
    );
}

#[test]
fn two_methods_of_one_name_are_two_symbols() {
    // The collision `@owner` prevents. `kinds.go` declares `Handle` twice: once
    // as an interface method, once on `Config`. Span nesting owns the first and
    // the receiver owns the second; without the second, both would be
    // `local kinds.go Handle().` and one `def` would overwrite the other.
    let extracted = extracted();
    let handles: Vec<String> = extracted
        .rows_in("def", "kinds.go")
        .into_iter()
        .filter(|r| r.get(3).map(String::as_str) == Some("Handle"))
        .filter_map(|r| r.first().cloned())
        .collect();
    assert_eq!(
        handles,
        vec![
            "local kinds.go Config#Handle().".to_string(),
            "local kinds.go Handler#Handle().".to_string(),
        ],
    );
}

#[test]
fn a_declaration_keyword_is_in_the_span_and_in_the_signature() {
    // A Go definition is the *spec* node, so a grouped `const (...)` yields one
    // span per constant rather than one span claimed by each. That leaves the
    // keyword a preceding sibling, and without `keyword_kinds` the signature
    // for `const Limit = 64` reads `Limit` — a name with no statement of what
    // it is — and the doc comment above sits one sibling further than the
    // preamble walk would reach.
    let extracted = extracted();
    let sigs = extracted.pairs("def_sig");
    assert_eq!(
        sigs.get("local kinds.go Limit.").map(String::as_str),
        Some("const Limit"),
    );
    assert_eq!(
        sigs.get("local kinds.go Banner.").map(String::as_str),
        Some("var Banner"),
    );
    assert_eq!(
        sigs.get("local kinds.go Config#").map(String::as_str),
        Some("type Config struct"),
    );
    let docs = extracted.pairs("def_doc");
    assert_eq!(
        docs.get("local kinds.go Config#").map(String::as_str),
        Some("Config is a struct, with a documented field."),
    );
    assert_eq!(
        docs.get("local kinds.go Limit.").map(String::as_str),
        Some("Limit is a constant."),
    );
}

#[test]
fn export_is_capitalization() {
    let extracted = extracted();
    let exported: Vec<String> = extracted
        .rows("exported")
        .into_iter()
        .filter_map(|r| r.first().cloned())
        .collect();
    for sym in [
        "local kinds.go Config#",
        "local kinds.go Config#Handle().",
        "local store/store.go Store#Get().",
    ] {
        assert!(exported.contains(&sym.to_string()), "{sym} is exported");
    }
    for sym in [
        "local kinds.go Config#quiet.",
        "local kinds.go Config#describe().",
        "local store/store.go Store#evict().",
    ] {
        assert!(
            !exported.contains(&sym.to_string()),
            "{sym} is not exported"
        );
    }
}
