//! `codeintel` end to end over a Python tree.
//!
//! `docs/plan.md` M5's done-when for a language, in the part that needs the
//! whole binary rather than the extractor: its `imports.scm` produces usable
//! `import/3` rows, a conformance rule over them passes and fails correctly,
//! and the anchor rate against its SCIP index clears 95%.
//!
//! The fixture carries a real `index.scip` produced by `scip-python` 0.6.6,
//! committed so that these run without the indexer installed. Regenerate it
//! with `npx @sourcegraph/scip-python index . --project-name fixture
//! --project-version 0.1.0` inside `tests/fixtures/python/`. The `tier_a`
//! tests below delete it first: conformance is the capability that works on a
//! first run with no SCIP index, so it is the one that has to work the day a
//! language lands.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/python")
}

/// A throwaway copy of the Python fixture, `index.scip` included.
///
/// On a pinned clock, as `scip.rs`'s `tree` explains: `index` trusts the SCIP
/// inputs to have seen a file only when its mtime is not newer than theirs,
/// and a checkout's mtimes are whatever the checkout wrote. macOS copies carry
/// them over, so a fresh worktree read the fixture as `scip-stale` (#50).
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    // The golden file is not source; the fixture is a tree to index.
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    dir
}

/// Seconds since the epoch every copied source is stamped with.
const SOURCE_MTIME: u64 = 1_700_000_000;

/// Stamp every file in the tree, recursively, with `secs`.
fn backdate(dir: &Path, secs: u64) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            backdate(&path, secs);
        } else {
            set_mtime(&path, secs);
        }
    }
}

fn set_mtime(path: &Path, secs: u64) {
    let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens for set_times");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("sets mtime");
}

/// The same tree with no SCIP index, so tier A answers alone.
fn tier_a_tree() -> tempfile::TempDir {
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("the fixture has one");
    dir
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == ".codeintel" || name == "__pycache__" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().expect("file type").is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .output()
        .expect("the binary runs")
}

fn index(root: &Path) -> String {
    let out = run(root, &["index", "."]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn query(root: &Path, program: &str) -> (Vec<String>, String) {
    let out = run(root, &["query", program]);
    (
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn a_python_tree_indexes_and_names_its_indexer() {
    let dir = tier_a_tree();
    let summary = index(dir.path());
    assert!(summary.contains("7 indexed"), "{summary}");
    // Not "no SCIP index found" — the literal command, because the gap between
    // tier A and tier B is the difference between a symbol map and a call
    // graph (`specs/02-extraction.md` § Acquisition).
    assert!(summary.contains("scip-python"), "{summary}");
}

#[test]
fn an_import_is_the_specifier_as_written() {
    // Never resolved to a path (`specs/01-facts.md` § `import`). `from db
    // import conn` is one row for `db` with no alias — `conn` is a member of
    // `db`, not a local name for it — and `import net.conn as _net` carries
    // its `as` verbatim.
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- import("ui/panel.py", M, A)."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    // The bound file is a constant, so only `M` and `A` are columns.
    assert_eq!(rows, ["db\t", "net.conn\t_net"]);
}

#[test]
fn a_conformance_rule_fires_and_then_stops_firing() {
    // The headline capability, on a tree with no SCIP index at all: no module
    // under ui/ may import from db/. It has to find the violation, and it has
    // to return zero rows with `status: ok` — not an error, not a silence —
    // once the violation is gone.
    let dir = tier_a_tree();
    index(dir.path());

    let rules = dir.path().join("conformance.dl");
    std::fs::write(
        &rules,
        "%% ui_imports_db(F, M)  no module under ui/ may import from db/\n\
         ui_imports_db(F, M) :- import(F, M, _), prefix(F, \"ui/\"), prefix(M, \"db\").\n",
    )
    .expect("write");
    let rules = rules.to_string_lossy().to_string();
    let args = [
        "query",
        "?- ui_imports_db(F, M).",
        "--rules",
        &rules,
        "--expect-empty",
    ];

    let violating = run(dir.path(), &args);
    assert_eq!(
        violating.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&violating.stderr)
    );
    assert!(
        String::from_utf8_lossy(&violating.stdout).contains("ui/panel.py"),
        "the violation names the file"
    );

    // Remove the offending import and the same check passes. The `net` import
    // stays, so this is the rule getting narrower rather than the file getting
    // emptier.
    let panel = dir.path().join("ui/panel.py");
    let source = std::fs::read_to_string(&panel).expect("read");
    let cleaned: Vec<&str> = source
        .lines()
        .filter(|l| !l.contains("from db") && !l.contains("conn.open"))
        .collect();
    std::fs::write(&panel, cleaned.join("\n")).expect("write");

    let clean = run(dir.path(), &args);
    let stderr = String::from_utf8_lossy(&clean.stderr);
    assert_eq!(clean.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

#[test]
fn a_method_is_reachable_through_its_class() {
    // `within/2` recurses through `parent`. Upstream's query would have made
    // every one of these a `function` parented to its class; the promotion
    // is what lets `"method"` be asked for by name.
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(T, _, "class", "Store"), within(S, T), def(S, _, "method", N)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.split('\t').nth(2))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(names, ["__init__", "_evict", "get", "put"]);
}

#[test]
fn every_definition_anchors_against_the_scip_index() {
    // `docs/plan.md` M5: anchor rate >= 95%. The rate is what says tier A and
    // tier B agree about *where* a definition is; below it, the two tiers are
    // describing different files and `exact` provenance is decoration.
    let dir = tree();
    let summary = index(dir.path());
    assert!(summary.contains("scip-python"), "{summary}");
    let rate = summary
        .lines()
        .find_map(|l| l.trim().strip_prefix("anchored: "))
        .unwrap_or_else(|| panic!("no anchor line in {summary}"));
    let percent: f64 = rate
        .split_once('(')
        .and_then(|(_, tail)| tail.strip_suffix("%)"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unreadable anchor rate {rate:?}"));
    assert!(percent >= 95.0, "anchor rate is {percent}%");
}

#[test]
fn the_type_checker_answers_what_tier_a_would_not_guess() {
    // `open` is defined by both `db.conn` and `net.conn`, so tier A's
    // `!ambiguous(N)` guard refuses the edge. Two callers, one correct target
    // each, and the provenance says which tier answered.
    //
    // Two rows are ABSENT that a naive reading of `ref` would expect: `import
    // db.conn` and `from store.warm import warm` bind the function at module
    // scope with no `Import` role bit (`scip-python` 0.6.6), so `ref` still
    // carries the occurrence — but `calls_at` now requires `def(From, _, _,
    // _)`, and a module-scope binding is enclosed by the file, not a def
    // (#39). The file never appears as a caller.
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), "?- calls(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        rows,
        [
            "describe kinds.py:48\t_describe kinds.py:32",
            "draw ui/panel.py:10\topen db/conn.py:4",
            "handle kinds.py:44\thandle kinds.py:25",
            "outer kinds.py:54\tinner kinds.py:57",
            "start app/app.py:9\topen db/conn.py:4",
            "test_warm store/test_store.py:6\twarm store/store.py:41",
            "warm store/store.py:41\tget store/store.py:26",
        ]
    );
    let (exact, _) = query(dir.path(), r"?- calls_exact(A, B).");
    assert_eq!(exact.len(), rows.len(), "every Python edge here is exact");

    // The absence, stated directly: no file is ever `calls`'s first argument.
    let (file_callers, _) = query(dir.path(), "?- calls(C, S), file(C, _).");
    assert_eq!(
        file_callers,
        Vec::<String>::new(),
        "a file was reported as a caller: {file_callers:?}"
    );
}

#[test]
fn tier_b_owns_a_method_by_its_module_qualified_class() {
    // Tier A parented these by span nesting; the type checker knows the
    // module. Both must land on the class, never on the file
    // (`specs/02-extraction.md` § Parent precedence).
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "store/store.py", "method", N), parent(S, P), def(P, _, PK, PN)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    let owners: Vec<(&str, &str)> = rows
        .iter()
        // Columns are the free variables in order: S, N, P, PK, PN.
        .filter_map(|r| {
            let c: Vec<&str> = r.split('\t').collect();
            Some((*c.get(1)?, *c.get(4)?))
        })
        .collect();
    assert_eq!(
        owners,
        [
            ("__init__", "Entry"),
            ("__init__", "Store"),
            ("_evict", "Store"),
            ("describe", "Entry"),
            ("get", "Store"),
            ("put", "Store"),
        ]
    );
}

#[test]
fn a_symbol_the_indexer_left_nameless_is_named_from_its_descriptor() {
    // `scip-python` writes no `display_name` and no `kind` for an attribute
    // bound in `__init__`, a parameter, or a module. The name is in the
    // symbol's own descriptor, which is read, not guessed; the kind is
    // `unknown`, because a `.` descriptor does not say field from variable
    // (`specs/02-extraction.md` § Ingest).
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "store/store.py", K, N), !def_span(S, _, _, _, _), parent(S, P), def(P, _, _, "Store")."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    let named: Vec<(&str, &str)> = rows
        .iter()
        .filter_map(|r| {
            let c: Vec<&str> = r.split('\t').collect();
            Some((*c.get(1)?, *c.get(2)?))
        })
        .collect();
    assert_eq!(named, [("unknown", "entries")]);
    let (empty, _) = query(dir.path(), r#"?- def(S, _, _, "")."#);
    assert!(empty.is_empty(), "a nameless def survived: {empty:?}");
}

#[test]
fn an_undeclared_column_after_a_non_ascii_character_is_skipped_not_misplaced() {
    // `scip-python` 0.6.6 declares no position encoding and counts UTF-16
    // (#21). On `WIDE = "é"; AFTER_WIDE = 1` it puts `AFTER_WIDE` at column
    // 12, and the byte column is 13. Read as bytes, that definition failed to
    // anchor and came back as a second, tier-B-only `def`. It is now skipped
    // and counted, and `WIDE`, whose prefix is ASCII, still anchors
    // (`specs/02-extraction.md` § Position normalization).
    let dir = tree();
    let summary = index(dir.path());
    assert!(
        summary.contains("scip ambiguous: 1 occurrence(s) skipped"),
        "{summary}"
    );

    let (rows, stderr) = query(dir.path(), r#"?- def(S, "kinds.py", _, "AFTER_WIDE")."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(rows.len(), 1, "one definition, not two: {rows:?}");

    let (resolved, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.py", _, N), resolved(S), match(N, "WIDE")."#,
    );
    let names: Vec<&str> = resolved
        .iter()
        .filter_map(|r| r.split('\t').nth(1))
        .collect();
    assert_eq!(names, ["WIDE"]);

    // The count survives into the bug-report artifact, not only the summary.
    let status = run(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("status --format json is JSON");
    let ambiguous: u64 = json["scip"]
        .as_array()
        .expect("scip inputs")
        .iter()
        .filter_map(|i| i["ambiguous"].as_u64())
        .sum();
    assert_eq!(ambiguous, 1, "{json}");
}
