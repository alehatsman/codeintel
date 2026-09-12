//! Tier A against `tests/fixtures/rust/`.
//!
//! The six guards of `specs/02-extraction.md` § Validation are in
//! [`common`], shared with every other language. What is here is what only
//! Rust can claim: that upstream's four collapsed kinds stay four, and that an
//! `impl` block owns its methods without being a definition itself.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{Extracted, extract_tree, fixture};

/// Every kind `queries/rust/tags.scm` claims to emit.
const KINDS: &[&str] = &[
    "struct",
    "enum",
    "type",
    "typealias",
    "trait",
    "module",
    "macro",
    "function",
    "method",
    "constant",
    "variable",
    "field",
];

fn root() -> PathBuf {
    fixture("rust")
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
    assert!(
        extracted
            .rows("import")
            .iter()
            .any(|r| r.get(2).is_some_and(|alias| !alias.is_empty())),
        "the aliased-import pattern never matched"
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
    // The test upstream's query fails: struct, enum, union, type alias, trait,
    // const and impl must not collapse into one kind.
    let extracted = extracted();
    let by_name: BTreeMap<String, String> = extracted
        .rows_in("def", "src/kinds.rs")
        .into_iter()
        .filter_map(|r| Some((r.get(3)?.clone(), r.get(2)?.clone())))
        .collect();

    for (name, kind) in [
        ("Config", "struct"),
        ("Mode", "enum"),
        ("Word", "type"),
        ("Key", "typealias"),
        ("Handler", "trait"),
        ("LIMIT", "constant"),
        ("BANNER", "variable"),
        ("handle", "method"),
        ("limit", "field"),
        ("Fast", "constant"),
        ("shout", "macro"),
    ] {
        assert_eq!(
            by_name.get(name).map(String::as_str),
            Some(kind),
            "{name} should be a {kind}"
        );
    }
}

#[test]
fn an_impl_block_owns_its_methods() {
    // Upstream captures `impl_item` as a reference, which parents every method
    // to the file. Ours makes it a scope, so `parent` is the type.
    let extracted = extracted();
    let parents = extracted.pairs("parent");
    assert_eq!(
        parents
            .get("local src/store.rs Store#get().")
            .map(String::as_str),
        Some("local src/store.rs Store#"),
    );
    assert_eq!(
        parents
            .get("local src/store.rs detail/helper().")
            .map(String::as_str),
        Some("local src/store.rs detail/"),
    );
    assert_eq!(
        parents.get("local src/store.rs Store#").map(String::as_str),
        Some("src/store.rs"),
    );
    // And the impl block itself defined nothing.
    assert!(
        !extracted
            .rows("def")
            .iter()
            .any(|r| r.get(3).map(String::as_str) == Some("Store")
                && r.get(2).map(String::as_str) == Some("class")),
        "an impl block is not a definition"
    );
}
