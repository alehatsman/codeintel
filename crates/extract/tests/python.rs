//! Tier A against `tests/fixtures/python/`.
//!
//! The six shared guards of `specs/02-extraction.md` § Validation are in
//! [`common`]. What is here is what only Python can claim: that a method is
//! a method although the grammar's query cannot say so, that a docstring
//! inside the definition is its documentation, and that a dunder is public.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use common::{Extracted, extract_tree, fixture};

/// Every kind `queries/python/tags.scm` claims to emit.
///
/// No `module`: a Python module is the file, which `file/2` already states.
/// No `constant`: Python cannot state one, and an upper-case name is a
/// convention the extractor would be guessing from.
const KINDS: &[&str] = &["class", "function", "method", "variable", "field"];

fn root() -> PathBuf {
    fixture("python")
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
    let imports = extracted.rows("import");
    // `import re as _re` is the aliased form; `from db import conn` is one
    // row for the statement with no alias, because `conn` is a member of `db`
    // and not a local name for it.
    assert!(
        imports.contains(&vec![
            "store/store.py".to_string(),
            "re".to_string(),
            "_re".to_string()
        ]),
        "{imports:?}"
    );
    assert!(
        imports.contains(&vec![
            "ui/panel.py".to_string(),
            "db".to_string(),
            String::new()
        ]),
        "{imports:?}"
    );
    assert_eq!(imports.len(), 7, "one row per specifier: {imports:?}");
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
    // The test upstream's query fails: it has no `@definition.method`, so
    // every method would be a `function` and `?- def(M,_,"method",N).` would
    // return zero rows with `status: ok`. The query here does not say
    // `method` either — it cannot see its own nesting — and the extractor
    // promotes a function under a class.
    let extracted = extracted();
    let found: BTreeSet<(String, String)> = extracted
        .rows_in("def", "kinds.py")
        .into_iter()
        .filter_map(|r| Some((r.get(3)?.clone(), r.get(2)?.clone())))
        .collect();
    for (name, kind) in [
        ("Config", "class"),
        ("Handler", "class"),
        ("BANNER", "variable"),
        ("_retries", "variable"),
        ("limit", "field"),
        ("_quiet", "field"),
        ("__init__", "method"),
        ("handle", "method"),
        ("_describe", "method"),
        ("quiet", "method"),
        ("describe", "function"),
        ("outer", "function"),
        ("inner", "function"),
    ] {
        assert!(
            found.contains(&(name.to_string(), kind.to_string())),
            "{name} should be a {kind}; kinds.py has {found:?}"
        );
    }
    // And nothing is a `constant`: `BANNER` is upper-case by convention, which
    // is not a thing the grammar states.
    assert!(!found.iter().any(|(_, k)| k == "constant"), "{found:?}");
}

#[test]
fn a_method_is_owned_by_its_class_and_a_nested_function_by_its_function() {
    // Two classes define `handle`; span nesting keeps them two symbols. The
    // nested `inner` is owned by `outer` and stays a `function`: a function
    // is a method only under a type-like owner.
    let extracted = extracted();
    let parents = extracted.pairs("parent");
    for (sym, owner) in [
        ("local kinds.py Config#handle().", "local kinds.py Config#"),
        (
            "local kinds.py Handler#handle().",
            "local kinds.py Handler#",
        ),
        ("local kinds.py Config#limit.", "local kinds.py Config#"),
        ("local kinds.py outer().inner().", "local kinds.py outer()."),
        ("local kinds.py outer().", "kinds.py"),
        ("local kinds.py BANNER.", "kinds.py"),
    ] {
        assert_eq!(
            parents.get(sym).map(String::as_str),
            Some(owner),
            "{sym} should be owned by {owner}; parents are {parents:?}",
        );
    }
}

#[test]
fn a_docstring_is_the_documentation_and_a_decorator_is_in_the_span() {
    // Python documents from inside the definition. The preamble walk over
    // preceding comments cannot see a docstring, so `tags.scm` names it with
    // `@doc` (`specs/02-extraction.md` § `@doc`). Delimiters are never part
    // of the text, and the body indentation is stripped per line, as a
    // comment's marker is.
    let extracted = extracted();
    let docs = extracted.pairs("def_doc");
    assert_eq!(
        docs.get("local kinds.py Config#").map(String::as_str),
        Some("A class, with a documented field and a documented method."),
    );
    assert_eq!(
        docs.get("local kinds.py Config#handle().")
            .map(String::as_str),
        Some("Handle one key.\n\nTwo lines of documentation, to prove newlines survive."),
    );
    // A definition with no docstring has no row. `_describe` has none, and
    // the line before `outer` is blank, so nothing is attributed to it.
    assert_eq!(docs.get("local kinds.py Config#_describe()."), None);
    assert_eq!(
        docs.get("local kinds.py outer().inner().")
            .map(String::as_str),
        None,
    );

    // `@lru_cache(...)` is inside `def_span` and outside `def_sig`.
    let src = std::fs::read_to_string(root().join("kinds.py")).expect("source");
    let describe_span: Vec<String> = extracted
        .rows("def_span")
        .into_iter()
        .find(|r| r.first().map(String::as_str) == Some("local kinds.py describe()."))
        .expect("describe has a span");
    let (start, end): (usize, usize) = (
        describe_span[3].parse().expect("start"),
        describe_span[4].parse().expect("end"),
    );
    let span_text = src
        .get(start..end)
        .expect("the span is a byte range in the file");
    assert!(
        span_text.starts_with("@lru_cache(maxsize=None)\ndef describe"),
        "{span_text:?}"
    );
    let sigs = extracted.pairs("def_sig");
    assert_eq!(
        sigs.get("local kinds.py describe().").map(String::as_str),
        Some("def describe(config: Config) -> str"),
    );
    assert_eq!(
        sigs.get("local kinds.py Config#__init__().")
            .map(String::as_str),
        Some("def __init__(self, limit: int = 64) -> None"),
    );
    assert_eq!(
        sigs.get("local kinds.py Handler#").map(String::as_str),
        Some("class Handler(Config)"),
    );
    assert_eq!(
        sigs.get("local kinds.py Config#limit.").map(String::as_str),
        Some("limit"),
    );
}

/// Python states visibility in the name: a leading underscore is PEP 8's
/// "weak internal use indicator", and a dunder is a magic name, not a private
/// one. `exported` is the rule over this (`specs/01-facts.md` § Derived
/// relations); what the extractor owes is the `Vis` value.
#[test]
fn export_is_the_underscore_and_a_dunder_is_public() {
    let extracted = extracted();
    let vis: BTreeMap<String, String> = extracted
        .rows("visibility")
        .into_iter()
        .filter_map(|r| Some((r.first()?.clone(), r.get(1)?.clone())))
        .collect();
    for sym in [
        "local kinds.py Config#",
        "local kinds.py Config#handle().",
        "local kinds.py Config#__init__().",
        "local kinds.py BANNER.",
    ] {
        assert_eq!(vis.get(sym).map(String::as_str), Some("public"), "{sym}");
    }
    for sym in [
        "local kinds.py Config#_describe().",
        "local kinds.py Config#_quiet.",
        "local kinds.py _retries.",
        "local store/store.py Store#_evict().",
    ] {
        assert_eq!(
            vis.get(sym).map(String::as_str),
            Some("restricted"),
            "{sym}"
        );
    }
    // A total function of the definition: one row each, and nothing inherits.
    assert_eq!(vis.len(), extracted.rows("def").len());
    assert!(!vis.values().any(|v| v == "inherited"));
}
