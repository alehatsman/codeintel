//! Tier A against `tests/fixtures/rust/`.
//!
//! `specs/02-extraction.md` § Validation, in order: golden facts, span
//! exactness, kind fidelity, query liveness, locality. Each one is a mechanical
//! guard on a failure that is otherwise invisible — a dead query pattern shows
//! up as `status: ok` with zero rows, and a cross-file lookup in the extractor
//! shows up as a wrong answer weeks later.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use datalog::atom::is_int;
use extract::lang::{self, KINDS};
use extract::{Anchors, Extractor, walk};
use facts::{Interner, RELATIONS, Segment};

/// Where the fixture lives, relative to this crate.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

/// The whole fixture, extracted file by file into one dictionary.
struct Extracted {
    interner: Interner,
    segments: BTreeMap<String, Segment>,
    _dir: tempfile::TempDir,
}

fn extract_fixture() -> Extracted {
    extract_tree(&fixture())
}

fn extract_tree(root: &Path) -> Extracted {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut interner = Interner::open(dir.path()).expect("dictionary");
    let mut segments = BTreeMap::new();
    let (found, skips) = walk(root);
    assert!(skips.unreadable.is_empty(), "{:?}", skips.unreadable);
    for candidate in found {
        let src = std::fs::read_to_string(&candidate.abs).expect("source is UTF-8");
        let mut extractor = Extractor::new(candidate.lang).expect("extractor");
        let out = extractor
            .file(&candidate.path, &src, &mut interner, &Anchors::default())
            .expect("extracts");
        segments.insert(candidate.path, out.segment);
    }
    Extracted {
        interner,
        segments,
        _dir: dir,
    }
}

/// Every fact as a ground Datalog clause, sorted — the same syntax a fact file
/// uses, so a golden diff reads as the thing it describes.
fn render(extracted: &Extracted) -> String {
    render_with(&extracted.interner, &extracted.segments)
}

fn render_with(interner: &Interner, segments: &BTreeMap<String, Segment>) -> String {
    let mut lines = BTreeSet::new();
    for segment in segments.values() {
        for rel in RELATIONS {
            let Some(rows) = segment.relation(rel.name) else {
                continue;
            };
            for row in rows.iter() {
                let args: Vec<String> = row
                    .iter()
                    .map(|a| {
                        if is_int(*a) {
                            a.to_string()
                        } else {
                            format!("{:?}", interner.resolve(*a).unwrap_or("<?>"))
                        }
                    })
                    .collect();
                lines.insert(format!("{}({}).", rel.name, args.join(", ")));
            }
        }
    }
    let mut out: Vec<&String> = lines.iter().collect();
    out.sort();
    out.iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Rows of one relation, as resolved strings.
fn rows(extracted: &Extracted, relation: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for segment in extracted.segments.values() {
        let Some(rel) = segment.relation(relation) else {
            continue;
        };
        for row in rel.iter() {
            out.push(
                row.iter()
                    .map(|a| {
                        if is_int(*a) {
                            a.to_string()
                        } else {
                            extracted.interner.resolve(*a).unwrap_or("<?>").to_string()
                        }
                    })
                    .collect(),
            );
        }
    }
    out.sort();
    out
}

#[test]
fn golden_facts() {
    let extracted = extract_fixture();
    let rendered = render(&extracted);
    let golden = fixture().join("expected.facts");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &rendered).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert_eq!(
        rendered, expected,
        "extraction changed. Re-read the diff, then `UPDATE_GOLDEN=1 cargo test -p extract`"
    );
}

#[test]
fn spans_are_exact() {
    let extracted = extract_fixture();
    let spans: BTreeMap<String, (usize, usize)> = rows(&extracted, "def_span")
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone(),
                (r.get(3)?.parse().ok()?, r.get(4)?.parse().ok()?),
            ))
        })
        .collect();

    for def in rows(&extracted, "def") {
        let (sym, path, _kind, name) = (
            def.first().expect("S"),
            def.get(1).expect("F"),
            def.get(2).expect("Kind"),
            def.get(3).expect("Name"),
        );
        let src = std::fs::read_to_string(fixture().join(path)).expect("source");
        let (start, end) = *spans.get(sym).expect("every def has a span");
        let text = src
            .get(start..end)
            .expect("the span is a byte range in the file");
        assert!(
            text.contains(name.as_str()),
            "{sym}: span {start}..{end} does not contain {name}"
        );
        // The name token is where the spec says it is: 1-based line, 0-based
        // UTF-8 byte column, and the identifier itself is there.
        let named = rows(&extracted, "def_name")
            .into_iter()
            .find(|r| r.first().map(String::as_str) == Some(sym.as_str()))
            .expect("every def has a def_name");
        let line: usize = named.get(1).expect("Line").parse().expect("a number");
        let col: usize = named.get(2).expect("Col").parse().expect("a number");
        let at = src.lines().nth(line - 1).expect("the line exists");
        assert!(
            at.get(col..).is_some_and(|t| t.starts_with(name.as_str())),
            "{sym}: {path}:{line}:{col} is {at:?}, which does not start with {name}"
        );
    }
}

#[test]
fn kinds_are_faithful() {
    // The test upstream's query fails: struct, enum, union, type alias, trait,
    // const and impl must not collapse into one kind.
    let extracted = extract_fixture();
    let by_name: BTreeMap<String, String> = rows(&extracted, "def")
        .into_iter()
        .filter(|r| r.get(1).map(String::as_str) == Some("src/kinds.rs"))
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
    let distinct: BTreeSet<&String> = by_name.values().collect();
    assert!(
        distinct.len() >= 7,
        "only {distinct:?} kinds in the fixture"
    );

    // Nothing is emitted outside the closed vocabulary.
    for def in rows(&extracted, "def") {
        let kind = def.get(2).expect("Kind");
        assert!(KINDS.contains(&kind.as_str()), "{kind} is not a Kind");
    }
}

#[test]
fn an_impl_block_owns_its_methods() {
    // Upstream captures `impl_item` as a reference, which parents every method
    // to the file. Ours makes it a scope, so `parent` is the type.
    let extracted = extract_fixture();
    let parents: BTreeMap<String, String> = rows(&extracted, "parent")
        .into_iter()
        .filter_map(|r| Some((r.first()?.clone(), r.get(1)?.clone())))
        .collect();
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
    // Exactly one parent row per def.
    let defs = rows(&extracted, "def").len();
    assert_eq!(rows(&extracted, "parent").len(), defs);
    // And the impl block itself defined nothing.
    assert!(
        !rows(&extracted, "def")
            .iter()
            .any(|r| r.get(3).map(String::as_str) == Some("Store")
                && r.get(2).map(String::as_str) == Some("class")),
        "an impl block is not a definition"
    );
}

#[test]
fn every_pattern_matches_at_least_once() {
    // A dead pattern is the signature of a grammar node rename, and it
    // otherwise surfaces as `status: ok` with zero rows.
    let extracted = extract_fixture();
    let kinds: BTreeSet<String> = rows(&extracted, "def")
        .into_iter()
        .filter_map(|r| r.get(2).cloned())
        .collect();
    for kind in [
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
    ] {
        assert!(kinds.contains(kind), "no {kind} matched in the fixture");
    }
    assert!(!rows(&extracted, "name_ref").is_empty(), "no call sites");
    assert!(!rows(&extracted, "import").is_empty(), "no imports");
    assert!(
        rows(&extracted, "import")
            .iter()
            .any(|r| r.get(2).is_some_and(|alias| !alias.is_empty())),
        "the aliased-import pattern never matched"
    );
}

#[test]
fn no_node_takes_two_definition_captures() {
    // The other half of query liveness. Upstream matches a method in a
    // `declaration_list` as both `@definition.method` and
    // `@definition.function`, which emits two `def` rows for one method. A
    // query cannot be trusted to stay disjoint by inspection, so it is checked.
    use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

    for lang in extract::LANGS {
        let language = (lang.language)();
        let query = Query::new(&language, lang.tags).expect("tags.scm compiles");
        let mut parser = Parser::new();
        parser.set_language(&language).expect("the grammar loads");

        let (found, _) = walk(&fixture());
        for candidate in found.iter().filter(|c| c.lang.name == lang.name) {
            let src = std::fs::read_to_string(&candidate.abs).expect("source");
            let tree = parser.parse(&src, None).expect("parses");
            let mut claimed: BTreeMap<usize, Vec<String>> = BTreeMap::new();
            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(&query, tree.root_node(), src.as_bytes());
            while let Some(m) = matches.next() {
                for capture in m.captures() {
                    let Some(label) = query.capture_names().get(capture.index as usize) else {
                        continue;
                    };
                    if label.starts_with("definition.") || label.starts_with("scope.") {
                        claimed
                            .entry(capture.node.id())
                            .or_default()
                            .push((*label).to_string());
                    }
                }
            }
            for (node, labels) in claimed {
                let mut distinct: Vec<&String> = labels.iter().collect();
                distinct.sort();
                distinct.dedup();
                assert!(
                    distinct.len() == 1,
                    "{}: node {node} is captured as {distinct:?}",
                    candidate.path
                );
            }
        }
    }
}

#[test]
fn facts_are_a_function_of_their_own_file() {
    // The mechanical guard on incremental soundness: every fact for file X is
    // reproducible by extracting X alone, with nothing else indexed.
    let whole = extract_fixture();
    for (path, segment) in &whole.segments {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut interner = Interner::open(dir.path()).expect("dictionary");
        let src = std::fs::read_to_string(fixture().join(path)).expect("source");
        let lang = lang::for_path(path).expect("a language");
        let mut extractor = Extractor::new(lang).expect("extractor");
        let alone = extractor
            .file(path, &src, &mut interner, &Anchors::default())
            .expect("extracts");

        let one = BTreeMap::from([(path.clone(), alone.segment)]);
        let same = BTreeMap::from([(path.clone(), segment.clone())]);
        // Atom ids differ between the two dictionaries; the resolved facts must
        // not.
        assert_eq!(
            render_with(&interner, &one),
            render_with(&whole.interner, &same),
            "{path} is not local"
        );
    }
}

#[test]
fn a_second_extraction_is_byte_identical() {
    let first = extract_fixture();
    let second = extract_fixture();
    assert_eq!(render(&first), render(&second));
}
