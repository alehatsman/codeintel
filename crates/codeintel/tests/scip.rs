//! Tier B end to end: `docs/plan.md` M3's done-when list, one test each.
//!
//! The fixture carries a real `index.scip` produced by `rust-analyzer scip .`,
//! committed so that these run without the indexer installed. Regenerate it
//! with `cargo build && rust-analyzer scip .` inside `tests/fixtures/rust/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

/// Always `--raw`: this file asserts what a symbol *is* — the SCIP string the
/// anchor join settled on — not how it prints. The rendered `Name path:line`
/// form is `tests/cli.rs`'s subject.
fn query(root: &Path, program: &str) -> (Vec<String>, String) {
    let out = run(root, &["query", program, "--raw"]);
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

/// rust-analyzer keys symbols by package, not cargo target, so `crate/` is
/// defined in `src/lib.rs` and again in `tests/store_test.rs`. Adopting it
/// gave one `SymId` two `def` rows in two files. The join now refuses a symbol
/// defined in more than one document, and `status` says how many.
#[test]
fn a_symbol_defined_in_two_documents_is_not_adopted() {
    let dir = tree();
    let summary = index(dir.path());
    assert!(
        summary.contains("scip collisions: 1 symbol(s)"),
        "{summary}"
    );

    let twice = rows(dir.path(), "?- def(S, F, _, _), def(S, G, _, _), F != G.");
    assert!(twice.is_empty(), "one SymId, two files: {twice:?}");
    assert!(
        rows(dir.path(), r#"?- def(S, _, _, _), suffix(S, " crate/")."#).is_empty(),
        "the colliding symbol was emitted anyway"
    );
    // Its references are facts and stay; they just join to no definition.
    assert!(
        !rows(
            dir.path(),
            r#"?- scip_ref(S, _, _, _, _, _), suffix(S, " crate/")."#
        )
        .is_empty()
    );

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip collisions: 1"), "{report}");
}

#[test]
fn tier_a_references_are_bounded_by_what_tier_b_confirms() {
    let dir = tree();
    index(dir.path());

    // Tier A's false-positive rate against a SCIP-covered tree is now **zero by
    // construction**, not zero by allowlist: name matching carries
    // `!scip_ref(...)`, so it cannot resolve a name at a position tier B
    // already answered (`specs/01-facts.md` § Tier precedence).
    //
    // This used to hold a `KNOWN_IMPRECISE` allowlist with one entry —
    // `src/store.rs:24:26`, `self.entries.get(key)` inside `Store::get`, where
    // the same-file rule binds `get` to the enclosing method and SCIP knows it
    // is `HashMap::get`. That entry documented the defect as accepted
    // behaviour. The guard removes the row, so the allowlist is gone rather
    // than merely unused: an exception list nothing populates reads as evidence
    // of rigour while asserting nothing.
    let named = rows(dir.path(), r#"?- ref(_, F, L, C, _, _, "name")."#);
    assert!(
        named.is_empty(),
        "SCIP covers this fixture, so no occurrence is left for tier A to \
         resolve: {named:?}"
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
    // *tier-A-only* `calls`, which is how this test reads it.
    //
    // Against a full index the two are now **equal**, which is the point of
    // tier precedence (`specs/01-facts.md` § Tier precedence): name matching
    // fires only where tier B never resolved the occurrence, so on a fixture
    // SCIP covers completely there is nothing left for it to guess at. This
    // assertion used to read the other way — that `calls` carried exactly one
    // tier-A self-edge for `get` that no compiler agrees with — which pinned
    // the defect as though it were the design.
    let both: BTreeSet<String> = rows(dir.path(), "?- calls(A, B).").into_iter().collect();
    assert!(exact.is_subset(&both));
    let over_reported: Vec<&String> = both.difference(&exact).collect();
    assert!(
        over_reported.is_empty(),
        "SCIP covers this fixture, so name matching must add no edge: {over_reported:?}"
    );
}

/// Tier precedence as a property rather than a count
/// (`specs/01-facts.md` § Tier precedence): where tier B resolved an
/// occurrence, tier A does not also guess at it.
///
/// Without this the tiers made independent claims about the same byte position
/// and `ref` was their union, so a guess the compiler directly contradicts
/// survived into `calls`, `depends`, `impact_of`, `dead_export` and
/// `entrypoint`. Measured on this project's own source: 1,448 of 1,629
/// name-provenance rows sat at a position `scip_ref` covered and named a
/// different symbol.
#[test]
fn name_matching_does_not_guess_where_the_compiler_answered() {
    let dir = tree();
    index(dir.path());

    let overlapping = rows(
        dir.path(),
        r#"?- ref(S, F, L, C, From, Role, "name"), scip_ref(_, F, L, C, _, _)."#,
    );
    assert!(
        overlapping.is_empty(),
        "a name guess survived where tier B had resolved the occurrence: {overlapping:?}"
    );
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
fn removing_the_scip_index_drops_the_files_only_it_covered() {
    // A file the walk never reaches — gitignored here, an unsupported language
    // in the field — has a segment made of SCIP facts and nothing else. When
    // `index.scip` goes, so must it: the alternative is `no-scip` beside rows
    // still marked `exact`.
    let dir = tree();
    let ignore = dir.path().join(".gitignore");
    let mut rules = std::fs::read_to_string(&ignore).unwrap_or_default();
    rules.push_str("\nsrc/app.rs\n");
    std::fs::write(&ignore, rules).expect("writes");
    index(dir.path());
    assert!(
        !rows(
            dir.path(),
            r#"?- ref(S, "src/app.rs", L, C, X, R, "exact")."#
        )
        .is_empty(),
        "the ignored file is covered by SCIP alone"
    );
    assert!(rows(dir.path(), r#"?- name_ref(N, "src/app.rs", L, C, X)."#).is_empty());

    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());

    assert!(rows(dir.path(), r#"?- file("src/app.rs", L)."#).is_empty());
    assert!(rows(dir.path(), r#"?- ref(S, F, L, C, X, R, "exact")."#).is_empty());
    assert!(rows(dir.path(), "?- resolved(S).").is_empty());
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

/// Staleness is by content, not by mtime: a file edited and put back holds
/// the bytes the indexer saw, and says so without the indexer rerunning.
#[test]
fn a_restored_file_is_not_scip_stale() {
    let dir = tree();
    index(dir.path());
    let path = dir.path().join("src/store.rs");
    let original = std::fs::read(&path).expect("source");

    let mut edited = original.clone();
    edited.extend_from_slice(b"\npub fn added() -> u32 {\n    7\n}\n");
    std::fs::write(&path, &edited).expect("writes");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=scip-stale"), "{stderr}");

    std::fs::write(&path, &original).expect("restores");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(
        stderr.contains("status=ok"),
        "the second look agrees: {stderr}"
    );

    // A touch alone — same bytes, new mtime — is not an edit either.
    std::fs::write(&path, &original).expect("touches");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

/// An edited file is extracted with tier A alone until the indexer reruns:
/// one `def` per definition, all of them `local`, no `scip_ref` in that file,
/// and every other file untouched. Before this, tier B emitted a second `def`
/// for each symbol at the old positions.
#[test]
fn an_edited_file_is_tier_a_only_until_the_indexer_reruns() {
    let dir = tree();
    index(dir.path());
    let get = r#"?- def(S, "src/store.rs", "method", "get")."#;
    assert_eq!(rows(dir.path(), get).len(), 1);
    assert!(
        !rows(
            dir.path(),
            r#"?- def(S, "src/store.rs", "method", "get"), resolved(S)."#
        )
        .is_empty()
    );

    // Shift every position in the file.
    let path = dir.path().join("src/store.rs");
    let mut src = std::fs::read_to_string(&path).expect("source");
    src.insert_str(0, "// a line that moves everything below it\n\n");
    std::fs::write(&path, src).expect("writes");
    index(dir.path());

    let defs = rows(dir.path(), get);
    assert_eq!(defs.len(), 1, "one definition, one row: {defs:?}");
    assert!(defs[0].starts_with("local "), "tier A alone: {defs:?}");
    assert!(rows(dir.path(), r#"?- scip_ref(S, "src/store.rs", L, C, X, R)."#).is_empty());
    assert!(!rows(dir.path(), r#"?- scip_ref(S, "src/app.rs", L, C, X, R)."#).is_empty());

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip stale:"), "{report}");
    assert!(report.contains("src/store.rs"), "{report}");
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
    // Rule 1 of parent precedence: the descriptor prefix. A field truncates to
    // the type that declares it, and that type is indexed, so the existence
    // check passes and the field is owned by the type, not by the file.
    let owners = rows(
        dir.path(),
        r#"?- def(S, _, "field", "entries"), parent(S, P)."#,
    );
    assert_eq!(owners.len(), 1, "{owners:?}");
    let owner = owners[0].split('\t').nth(1).unwrap_or_default();
    assert!(owner.ends_with("Store#"), "{owners:?}");

    // This test used to use `Store::get` and assert only that the owner
    // *string* contained `Store#`. It passed against
    // `local src/store.rs Store#` — tier A's symbol for the `impl` block, which
    // after anchoring names nothing at all. Asserting the shape of an
    // identifier without asserting that the identifier resolves is how a
    // dangling edge stays green for a milestone;
    // `every_parent_names_something_that_exists` is the assertion that catches
    // it.
}

/// Precedence rule 3's existence check, as a property over the whole index:
/// every `parent` target is a definition or a file. Nothing else is a thing
/// that can own a definition.
///
/// This guards a *silent* defect. `within/2` recurses through `parent`, so a
/// target that is neither ends the recursion early and the query returns a
/// short answer with `status=ok` — no truncation, no degradation, nothing
/// invariant 5 can report. Before the fix this found four rows: every method of
/// a Rust `impl` block in the fixture.
#[test]
fn every_parent_names_something_that_exists() {
    let dir = tree();
    index(dir.path());
    let dangling = rows(
        dir.path(),
        "?- parent(C, P), !def(P, _, _, _), !file(P, _).",
    );
    assert!(dangling.is_empty(), "parent names nothing: {dangling:?}");
}

/// The consequence, as the query an agent actually asks: a method inside an
/// `impl` block is transitively within the file that holds it.
///
/// Rust methods are the case that broke, because `rust-analyzer` names them
/// `store/impl#[Store]get().` and the descriptor prefix `store/impl#[Store]` is
/// not a definition — so rules 1 and 2 both decline and rule 3 decides.
#[test]
fn an_impl_block_method_is_within_its_file() {
    let dir = tree();
    index(dir.path());
    for method in ["get", "put", "evict"] {
        let found = rows(
            dir.path(),
            &format!(r#"?- def(S, _, "method", "{method}"), within(S, "src/store.rs")."#),
        );
        assert_eq!(found.len(), 1, "`{method}` is not within its own file");
    }
}

/// `status` reports the same SCIP staleness `query` acts on.
///
/// These disagreed: a `query` touching `scip_ref` answered `scip-stale` and
/// named the files, while `status` — the artifact with no telemetry behind it,
/// the one a bug report is supposed to carry — printed its verdict and
/// mentioned nothing. A user pasting `status` into an issue would have omitted
/// the single fact that explains the wrong answer. Both now read the same
/// `ScipState`, so they cannot drift apart again.
#[test]
fn status_reports_the_scip_staleness_that_query_acts_on() {
    let dir = tree();
    index(dir.path());

    // Edit a source file so it is newer than the committed `index.scip`, then
    // re-index so the manifest carries the new mtime.
    let touched = dir.path().join("src/app.rs");
    let mut text = std::fs::read_to_string(&touched).expect("read");
    text.push_str("\npub fn added_after_scip() {}\n");
    std::fs::write(&touched, text).expect("write");
    index(dir.path());

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip stale:"), "{report}");
    assert!(report.contains("src/app.rs"), "{report}");
    assert!(report.contains("rust-analyzer scip ."), "{report}");

    // And the query verb agrees, naming the same file.
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=scip-stale"), "{stderr}");
    assert!(stderr.contains("src/app.rs"), "{stderr}");
}
