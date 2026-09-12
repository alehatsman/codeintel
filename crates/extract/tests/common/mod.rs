//! The tier-A validation suite, once, for every language that has a fixture.
//!
//! `specs/02-extraction.md` § Validation lists six mechanical guards: golden
//! facts, span exactness, kind fidelity, query liveness, locality, and
//! determinism. Five of the six are the same assertion whatever the language,
//! so they live here and each language's test file supplies its fixture and
//! the claims only that language can make.
//!
//! Each guard catches a failure that is otherwise invisible. A dead query
//! pattern shows up as `status: ok` with zero rows; a cross-file lookup in the
//! extractor shows up as a wrong answer weeks later.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use datalog::atom::is_int;
use extract::lang::{self, KINDS};
use extract::{Anchors, Extractor, walk};
use facts::{Interner, RELATIONS, Segment};

/// Where a fixture tree lives, relative to this crate.
pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// A whole fixture, extracted file by file into one dictionary.
pub(crate) struct Extracted {
    root: PathBuf,
    interner: Interner,
    segments: BTreeMap<String, Segment>,
    _dir: tempfile::TempDir,
}

/// Extract every file under `root` that has a registered language.
pub(crate) fn extract_tree(root: &Path) -> Extracted {
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
        root: root.to_path_buf(),
        interner,
        segments,
        _dir: dir,
    }
}

impl Extracted {
    /// Every fact as a ground Datalog clause, sorted — the same syntax a fact
    /// file uses, so a golden diff reads as the thing it describes.
    pub(crate) fn render(&self) -> String {
        render_with(&self.interner, &self.segments)
    }

    /// Rows of one relation, as resolved strings, sorted.
    pub(crate) fn rows(&self, relation: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for segment in self.segments.values() {
            let Some(rel) = segment.relation(relation) else {
                continue;
            };
            for row in rel.iter() {
                out.push(row.iter().map(|a| self.show(*a)).collect());
            }
        }
        out.sort();
        out
    }

    /// Rows of one relation in one file.
    pub(crate) fn rows_in(&self, relation: &str, path: &str) -> Vec<Vec<String>> {
        self.rows(relation)
            .into_iter()
            .filter(|r| r.get(1).map(String::as_str) == Some(path))
            .collect()
    }

    /// A relation's first two columns as a map. For `parent` and the like.
    pub(crate) fn pairs(&self, relation: &str) -> BTreeMap<String, String> {
        self.rows(relation)
            .into_iter()
            .filter_map(|r| Some((r.first()?.clone(), r.get(1)?.clone())))
            .collect()
    }

    fn show(&self, atom: u32) -> String {
        if is_int(atom) {
            atom.to_string()
        } else {
            self.interner.resolve(atom).unwrap_or("<?>").to_string()
        }
    }
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

/// Golden facts: the whole extraction, diffed as text.
pub(crate) fn assert_golden(extracted: &Extracted) {
    let rendered = extracted.render();
    let golden = extracted.root.join("expected.facts");
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

/// Span exactness: every `def_span` is a byte range that really contains the
/// name, and every `def_name` position really points at the identifier.
///
/// Asserted by re-slicing the source, not by trusting the numbers.
pub(crate) fn assert_spans_exact(extracted: &Extracted) {
    let spans: BTreeMap<String, (usize, usize)> = extracted
        .rows("def_span")
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone(),
                (r.get(3)?.parse().ok()?, r.get(4)?.parse().ok()?),
            ))
        })
        .collect();
    let names: BTreeMap<String, (usize, usize)> = extracted
        .rows("def_name")
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone(),
                (r.get(1)?.parse().ok()?, r.get(2)?.parse().ok()?),
            ))
        })
        .collect();

    for def in extracted.rows("def") {
        let (sym, path, name) = (
            def.first().expect("S"),
            def.get(1).expect("F"),
            def.get(3).expect("Name"),
        );
        let src = std::fs::read_to_string(extracted.root.join(path)).expect("source");
        let (start, end) = *spans.get(sym).expect("every def has a span");
        let text = src
            .get(start..end)
            .expect("the span is a byte range in the file");
        assert!(
            text.contains(name.as_str()),
            "{sym}: span {start}..{end} does not contain {name}"
        );
        // 1-based line, 0-based UTF-8 byte column, and the identifier is there.
        let (line, col) = *names.get(sym).expect("every def has a def_name");
        let at = src.lines().nth(line - 1).expect("the line exists");
        assert!(
            at.get(col..).is_some_and(|t| t.starts_with(name.as_str())),
            "{sym}: {path}:{line}:{col} is {at:?}, which does not start with {name}"
        );
    }
}

/// Nothing is emitted outside the closed `Kind` vocabulary, and every
/// definition has exactly one `parent`.
pub(crate) fn assert_kinds_closed(extracted: &Extracted) {
    for def in extracted.rows("def") {
        let kind = def.get(2).expect("Kind");
        assert!(KINDS.contains(&kind.as_str()), "{kind} is not a Kind");
    }
    assert_eq!(
        extracted.rows("parent").len(),
        extracted.rows("def").len(),
        "exactly one parent row per def"
    );
}

/// Every `parent` names something this extraction actually defines, or a file.
///
/// A `parent` pointing at a symbol with no `def` row stops `within/2` one hop
/// short without failing the query — a short answer, which the status taxonomy
/// cannot report. `specs/02-extraction.md` § Parent precedence.
pub(crate) fn assert_parents_exist(extracted: &Extracted) {
    let defined: BTreeSet<String> = extracted
        .rows("def")
        .into_iter()
        .filter_map(|r| r.first().cloned())
        .collect();
    let files: BTreeSet<String> = extracted
        .rows("file")
        .into_iter()
        .filter_map(|r| r.first().cloned())
        .collect();
    for (child, owner) in extracted.pairs("parent") {
        assert!(
            defined.contains(&owner) || files.contains(&owner),
            "parent({child}, {owner}) names nothing that exists"
        );
    }
}

/// Query liveness, half one: every kind the language claims to emit was
/// emitted at least once, and references and imports are not empty.
///
/// A dead pattern is the signature of a grammar node rename, and it otherwise
/// surfaces as `status: ok` with zero rows.
pub(crate) fn assert_kinds_present(extracted: &Extracted, kinds: &[&str]) {
    let found: BTreeSet<String> = extracted
        .rows("def")
        .into_iter()
        .filter_map(|r| r.get(2).cloned())
        .collect();
    for kind in kinds {
        assert!(found.contains(*kind), "no {kind} matched in the fixture");
    }
    assert!(!extracted.rows("name_ref").is_empty(), "no call sites");
    assert!(!extracted.rows("import").is_empty(), "no imports");
}

/// Query liveness, half two: no node takes two `@definition.*`/`@scope.*`
/// captures.
///
/// Upstream's Rust query matches a method in a `declaration_list` as both
/// `@definition.method` and `@definition.function` and emits two `def` rows for
/// one method. A query cannot be trusted to stay disjoint by inspection.
pub(crate) fn assert_captures_disjoint(root: &Path) {
    use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

    let (found, _) = walk(root);
    assert!(!found.is_empty(), "the fixture has no source files");
    for lang in extract::LANGS {
        let language = (lang.language)();
        let query = Query::new(&language, lang.tags).expect("tags.scm compiles");
        let mut parser = Parser::new();
        parser.set_language(&language).expect("the grammar loads");

        for candidate in found.iter().filter(|c| c.lang.name == lang.name) {
            let src = std::fs::read_to_string(&candidate.abs).expect("source");
            let tree = parser.parse(&src, None).expect("parses");
            let mut claimed: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
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
                            .insert((*label).to_string());
                    }
                }
            }
            for (node, labels) in claimed {
                assert!(
                    labels.len() == 1,
                    "{}: node {node} is captured as {labels:?}",
                    candidate.path
                );
            }
        }
    }
}

/// Locality: every fact for file X is reproducible by extracting X alone, with
/// nothing else indexed.
///
/// The mechanical guard on incremental soundness. Atom ids differ between the
/// two dictionaries; the resolved facts must not.
pub(crate) fn assert_facts_are_local(extracted: &Extracted) {
    for (path, segment) in &extracted.segments {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut interner = Interner::open(dir.path()).expect("dictionary");
        let src = std::fs::read_to_string(extracted.root.join(path)).expect("source");
        let lang = lang::for_path(path).expect("a language");
        let mut extractor = Extractor::new(lang).expect("extractor");
        let alone = extractor
            .file(path, &src, &mut interner, &Anchors::default())
            .expect("extracts");

        let one = BTreeMap::from([(path.clone(), alone.segment)]);
        let same = BTreeMap::from([(path.clone(), segment.clone())]);
        assert_eq!(
            render_with(&interner, &one),
            render_with(&extracted.interner, &same),
            "{path} is not local"
        );
    }
}

/// Determinism: a second extraction of the same tree is byte-identical.
pub(crate) fn assert_deterministic(root: &Path) {
    assert_eq!(extract_tree(root).render(), extract_tree(root).render());
}
