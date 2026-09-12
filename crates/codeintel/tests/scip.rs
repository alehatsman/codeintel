//! Tier B end to end: `docs/plan.md` M3's done-when list, one test each.
//!
//! The fixture carries a real `index.scip` produced by `rust-analyzer scip .`,
//! committed so that these run without the indexer installed. Regenerate it
//! with `cargo build && rust-analyzer scip .` inside `tests/fixtures/rust/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `specs/02-extraction.md` § Validation's `known-imprecise.txt`: tier-A `ref`
/// rows that no `scip_ref` confirms, each with a reason.
///
/// `src/store.rs:24:26` is `self.entries.get(key)` inside `Store::get`. Tier
/// A's same-file rule binds the name `get` to the only `get` it can see, which
/// is the enclosing method; SCIP knows it is `HashMap::get`. This is the
/// documented shape of tier-A over-reporting, not a bug to fix — the fix is a
/// SCIP index, which is what `calls_exact` selects.
const KNOWN_IMPRECISE: &[&str] = &["src/store.rs\t24\t26"];

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

/// A throwaway copy of the fixture tree, `index.scip` included.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    dir
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        // Build output is not fixture data, and copying it is slow.
        if name == "target" || name == ".codeintel" {
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
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    stderr
}

fn query(root: &Path, program: &str) -> (Vec<String>, String) {
    let out = run(root, &["query", program]);
    let rows = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    (rows, String::from_utf8_lossy(&out.stderr).to_string())
}

fn rows(root: &Path, program: &str) -> Vec<String> {
    let (rows, stderr) = query(root, program);
    assert!(
        stderr.contains("status=ok")
            || stderr.contains("status=no-scip")
            || stderr.contains("status=scip-stale"),
        "{program}: {stderr}"
    );
    rows
}

#[test]
fn the_anchor_join_resolves_everything_it_structurally_can() {
    let dir = tree();
    index(dir.path());

    // A tier-A definition is one with a `def_name` — tier B emits none, because
    // SCIP's range is not our `def_span`.
    let all = rows(dir.path(), "?- def(S, _, K, _), def_name(S, _, _).");
    let unresolved = rows(
        dir.path(),
        "?- def(S, _, K, _), def_name(S, _, _), !resolved(S).",
    );

    // Modules are exempt, and the exemption is pinned so it cannot quietly
    // grow. Tier A puts module `conn` at `mod conn;` in `src/db/mod.rs`;
    // rust-analyzer puts it at line 1 of `src/db/conn.rs`. Different files, so
    // an identifier-position join structurally cannot match them — this is a
    // property of the two models, not a defect in the join.
    let modules = |rows: &[String]| rows.iter().filter(|r| r.contains("\tmodule")).count();
    assert_eq!(
        modules(&unresolved),
        unresolved.len(),
        "a non-module definition failed to anchor: {unresolved:?}"
    );

    let anchorable = all.len() - modules(&all);
    let anchored = anchorable - (unresolved.len() - modules(&unresolved));
    assert!(anchorable > 20, "the fixture got smaller: {anchorable}");
    let rate = anchored * 100 / anchorable;
    assert!(
        rate >= 95,
        "anchor rate {rate}% ({anchored}/{anchorable}); a drop means the join is drifting"
    );
}

#[test]
fn tier_a_references_are_bounded_by_what_tier_b_confirms() {
    let dir = tree();
    index(dir.path());

    // Every `"name"` reference must either match an `"exact"` one at the same
    // position, or be listed here with a reason. This bounds tier A's
    // false-positive rate with a number instead of a hope
    // (`specs/02-extraction.md` § Validation).
    let loose = rows(
        dir.path(),
        r#"?- ref(_, F, L, C, _, _, "name"), !scip_ref(_, F, L, C, _, _)."#,
    );
    let unexpected: Vec<&String> = loose
        .iter()
        .filter(|row| !KNOWN_IMPRECISE.iter().any(|known| row.contains(known)))
        .collect();
    assert!(
        unexpected.is_empty(),
        "tier A resolved names tier B did not confirm: {unexpected:?}"
    );
}

#[test]
fn exact_calls_find_edges_tier_a_alone_cannot_see() {
    let dir = tree();

    // Tier A only, first. `open` is exported twice repo-wide — `db::conn` and
    // `net::conn` — so tier A's `!ambiguous(N)` guard refuses to pick one and
    // emits no edge at all. That is the extractor declining to guess, which is
    // correct, and it is exactly the hole tier B fills.
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());
    let name_only: BTreeSet<String> = rows(dir.path(), "?- calls(A, B).").into_iter().collect();
    assert!(
        !name_only.iter().any(|e| e.contains("open()")),
        "tier A should not have resolved an ambiguous name: {name_only:?}"
    );

    // Now with SCIP.
    std::fs::copy(fixture().join("index.scip"), dir.path().join("index.scip")).expect("restores");
    index(dir.path());
    let exact: BTreeSet<String> = rows(dir.path(), "?- calls_exact(A, B).")
        .into_iter()
        .collect();

    // Hand-audited, both directions. Three calls, and the fixture has exactly
    // three: `start` and `draw` each call `db::conn::open` (never `net`), and
    // `warm` calls `Store::get`.
    let mut audited: Vec<&str> = exact.iter().map(String::as_str).collect();
    audited.sort_unstable();
    assert_eq!(audited.len(), 3, "{audited:?}");
    for (caller, callee) in [
        ("app/start().", "db/conn/open()."),
        ("store/warm().", "impl#[Store]get()."),
        ("ui/panel/draw().", "db/conn/open()."),
    ] {
        assert!(
            exact
                .iter()
                .any(|e| e.contains(caller) && e.ends_with(callee)),
            "{caller} -> {callee} missing from {audited:?}"
        );
    }
    // The other direction: nothing calls `net::conn::open`, and tier B says so
    // rather than splitting the difference between two same-named functions.
    assert!(!exact.iter().any(|e| e.contains("net/conn/open()")));

    // Two edges tier A alone could not see at all.
    let gained = exact.len() - name_only.len();
    assert_eq!(gained, 1, "{exact:?} vs {name_only:?}");
    assert!(
        exact.iter().filter(|e| e.contains("open()")).count() == 2,
        "{exact:?}"
    );

    // `calls_exact` is a subset of `calls` by construction — the same rule with
    // `Prov` pinned — so the plan's "edges `calls` misses" is only ever true of
    // *tier-A-only* `calls`, which is how this test reads it. What the exact
    // form buys against a full index is precision: `calls` carries a tier-A
    // self-edge for `get` that no compiler agrees with.
    let both: BTreeSet<String> = rows(dir.path(), "?- calls(A, B).").into_iter().collect();
    assert!(exact.is_subset(&both));
    let over_reported: Vec<&String> = both.difference(&exact).collect();
    assert_eq!(over_reported.len(), 1, "{both:?}");
    assert!(over_reported[0].contains("get()"), "{over_reported:?}");
}

#[test]
fn removing_the_scip_index_degrades_to_name_provenance() {
    let dir = tree();
    index(dir.path());
    assert!(!rows(dir.path(), "?- resolved(S).").is_empty());

    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());

    // Clean degradation: tier A still answers, and nothing claims to be exact.
    assert!(rows(dir.path(), "?- resolved(S).").is_empty());
    assert!(rows(dir.path(), r#"?- ref(S, F, L, C, X, R, "exact")."#).is_empty());
    assert!(!rows(dir.path(), "?- def(S, F, K, N).").is_empty());
}

#[test]
fn no_scip_fires_on_the_dependency_closure_not_the_syntax() {
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());

    // `impact_of` mentions no base relation at all. Its closure reaches
    // `scip_ref`, so the answer must say so — otherwise the README's own
    // headline query returns `ok` with zero rows on a fresh install, which is
    // the exact failure invariant 6 exists to prevent.
    for program in [
        "?- calls_exact(A, B).",
        "?- impact_of(S, C).",
        "?- resolved(S).",
        "?- scip_ref(S, F, L, C, X, R).",
    ] {
        let (_, stderr) = query(dir.path(), program);
        assert!(
            stderr.contains("status=no-scip"),
            "{program} did not report no-scip: {stderr}"
        );
        assert!(stderr.contains("rust-analyzer scip ."), "{stderr}");
    }

    // A query that needs nothing from tier B is plain `ok`. `no-scip` on
    // everything would be as useless as `no-scip` on nothing.
    for program in ["?- def(S, F, K, N).", "?- import(F, M, A)."] {
        let (_, stderr) = query(dir.path(), program);
        assert!(stderr.contains("status=ok"), "{program}: {stderr}");
    }
}

#[test]
fn a_present_scip_index_answers_ok() {
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(!rows.is_empty());
}

#[test]
fn index_prints_the_indexer_command_for_every_language_it_found() {
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    let stderr = index(dir.path());
    // The literal, copy-pasteable line — not a description of one.
    assert!(stderr.contains("rust-analyzer scip ."), "{stderr}");
    assert!(stderr.contains("no SCIP index"), "{stderr}");
}

#[test]
fn index_reports_what_it_ingested_and_how_well_it_joined() {
    let dir = tree();
    let stderr = index(dir.path());
    assert!(stderr.contains("scip: index.scip"), "{stderr}");
    assert!(stderr.contains("rust-analyzer"), "{stderr}");
    assert!(stderr.contains("anchored:"), "{stderr}");
}

#[test]
fn a_file_edited_after_the_scip_index_reports_scip_stale() {
    let dir = tree();
    index(dir.path());
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");

    // Touch a source file so it is newer than `index.scip`.
    let path = dir.path().join("src/store.rs");
    let mut src = std::fs::read_to_string(&path).expect("source");
    src.push_str("\npub fn added() -> u32 {\n    7\n}\n");
    std::fs::write(&path, src).expect("writes");

    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=scip-stale"), "{stderr}");
    assert!(stderr.contains("src/store.rs"), "{stderr}");

    // A query that needs nothing exact is unaffected: its answer is fresh.
    let (_, stderr) = query(dir.path(), "?- def(S, F, K, N).");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

#[test]
fn auto_refresh_does_not_delete_tier_b() {
    let dir = tree();
    index(dir.path());
    let before = rows(dir.path(), "?- resolved(S).").len();
    assert!(before > 0);

    // Three queries in a row. Auto-refresh is a writer; if it forgot the SCIP
    // inputs it would re-extract tier-A-only and the index would lose tier B a
    // little more each time, silently.
    for _ in 0..3 {
        drop(query(dir.path(), "?- def(S, F, K, N)."));
    }
    assert_eq!(rows(dir.path(), "?- resolved(S).").len(), before);
}

#[test]
fn local_symbols_are_not_cross_linked_between_files() {
    let dir = tree();
    index(dir.path());

    // SCIP's `local N` is document-scoped. Without the rewrite every file's
    // `local 0` is one symbol, and the failure is silent and repo-wide: a
    // parameter in one file would answer questions about a parameter in
    // another.
    let locals = rows(dir.path(), r#"?- def(S, F, K, N), match(S, "^local ")."#);
    assert!(locals.len() > 5, "{locals:?}");
    let files: BTreeSet<&str> = locals
        .iter()
        .filter_map(|row| row.split('\t').nth(1))
        .collect();
    assert!(files.len() > 1, "{locals:?}");
    for row in &locals {
        let symbol = row.split('\t').next().unwrap_or_default();
        let file = row.split('\t').nth(1).unwrap_or_default();
        assert!(
            symbol.starts_with(&format!("local {file} ")),
            "a local symbol does not carry its own document: {row}"
        );
    }
}

#[test]
fn a_tier_b_definition_carries_its_semantic_owner() {
    let dir = tree();
    index(dir.path());
    // Rule 1 of parent precedence: the descriptor prefix. `Store::get` is owned
    // by the impl block's type, not by the file it sits in.
    let owners = rows(
        dir.path(),
        r#"?- def(S, _, "method", "get"), parent(S, P)."#,
    );
    assert_eq!(owners.len(), 1, "{owners:?}");
    let owner = owners[0].split('\t').nth(1).unwrap_or_default();
    assert!(
        owner.contains("impl#[Store]") || owner.contains("Store#"),
        "{owners:?}"
    );
}
