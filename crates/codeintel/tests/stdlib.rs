//! Every rule in `rules/stdlib.dl`, against the fixture, with hand-verified
//! answers.
//!
//! `docs/plan.md` M4: *every rule in `stdlib.dl` has a fixture test with a
//! hand-verified answer.* Hand-verified means the expectations below were read
//! back against `tests/fixtures/rust/`'s source, not pasted from whatever the
//! binary printed — twice here that disagreed with what the rule was supposed
//! to mean, and both are recorded in the comments rather than smoothed over.
//!
//! The fixture is indexed **once** with its SCIP index and shared, because
//! every one of these is a read.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

/// The fixture, indexed with both tiers, built once per test binary.
fn indexed() -> &'static Path {
    static TREE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TREE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust");
        copy(&from, dir.path());
        // The golden file is not source.
        drop(std::fs::remove_file(dir.path().join("expected.facts")));
        let out = Command::new(binary())
            .args(["index", ".", "--scip", "index.scip"])
            .current_dir(dir.path())
            .output()
            .expect("the binary runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir
    })
    .path()
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
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

/// Rows of one goal, rendered. `--no-refresh` because the tree does not move
/// and a refresh per query would dominate the runtime.
fn rows(goal: &str) -> Vec<String> {
    let out = Command::new(binary())
        .args(["query", goal, "--no-refresh"])
        .current_dir(indexed())
        .output()
        .expect("the binary runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("status=ok") || stderr.contains("status=no-scip"),
        "{goal}: {stderr}"
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The same, with symbols as their `SymId` — for the rules whose *identity* is
/// the point rather than their presentation.
fn raw_rows(goal: &str) -> Vec<String> {
    let out = Command::new(binary())
        .args(["query", goal, "--no-refresh", "--raw"])
        .current_dir(indexed())
        .output()
        .expect("the binary runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The same fixture with **no** SCIP index, so `ref` has only tier A's name
/// matching to work with.
///
/// Needed because name matching now defers to tier B wherever tier B resolved
/// the occurrence (`specs/01-facts.md` § Tier precedence), so over-reporting is
/// no longer observable on a SCIP-covered tree — which is the improvement. The
/// tests that exist to *measure* over-reporting therefore have to ask for the
/// tier that over-reports.
fn named_only() -> &'static Path {
    static TREE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TREE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust");
        copy(&from, dir.path());
        drop(std::fs::remove_file(dir.path().join("expected.facts")));
        // Removed, not merely un-passed: `index` auto-detects `index.scip`.
        drop(std::fs::remove_file(dir.path().join("index.scip")));
        let out = Command::new(binary())
            .args(["index", "."])
            .current_dir(dir.path())
            .output()
            .expect("the binary runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir
    })
    .path()
}

/// `rows`, against a chosen tree.
fn rows_in(root: &Path, goal: &str) -> Vec<String> {
    let out = Command::new(binary())
        .args(["query", goal, "--no-refresh"])
        .current_dir(root)
        .output()
        .expect("the binary runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("status=ok") || stderr.contains("status=no-scip"),
        "{goal}: {stderr}"
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// `raw_rows`, against a chosen tree.
fn raw_rows_in(root: &Path, goal: &str) -> Vec<String> {
    let out = Command::new(binary())
        .args(["query", goal, "--no-refresh", "--raw"])
        .current_dir(root)
        .output()
        .expect("the binary runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------- resolution

#[test]
fn ref_carries_both_provenances() {
    // `src/app.rs:9` is `conn::open("sqlite://memory")`. Tier B resolves it to
    // db::conn::open exactly; tier A sees the identifier `open`, which is
    // exported twice repo-wide, and refuses to guess.
    let exact = rows(r#"?- ref(S, "src/app.rs", 9, _, _, _, "exact"), def(S, _, _, "open")."#);
    assert_eq!(exact, vec!["open src/db/conn.rs:3"]);

    let named = rows(r#"?- ref(S, "src/app.rs", 9, _, _, _, "name"), def(S, _, _, "open")."#);
    assert!(named.is_empty(), "tier A must not guess: {named:?}");
}

#[test]
fn local_def_is_definition_within_one_file() {
    assert_eq!(rows(r#"?- local_def("src/db/conn.rs", "open")."#).len(), 1);
    // `open` is also defined in net/conn.rs, and `local_def` is per file.
    assert!(rows(r#"?- local_def("src/app.rs", "open")."#).is_empty());
}

#[test]
fn ambiguous_names_the_collisions() {
    // `open` is exported by both db::conn and net::conn. That is the fixture's
    // reason for existing: tier A cannot pick and must not.
    // A ground goal: `true` when it holds, nothing when it does not.
    assert_eq!(rows(r#"?- ambiguous("open")."#), vec!["true"]);
    assert!(rows(r#"?- ambiguous("warm")."#).is_empty());
}

// ------------------------------------------------------------ the location bridge

#[test]
fn symbol_at_and_innermost_at_agree_on_this_fixture() {
    // `src/store.rs:23` is inside `get`, which spans 19-25 (the doc comment is
    // part of the definition). Note there is exactly one enclosing definition:
    // an `impl` block is not emitted as a `def`, so the usual
    // innermost-vs-outer distinction has nothing to bite on here.
    assert_eq!(
        rows(r#"?- symbol_at("src/store.rs", 23, S)."#),
        vec!["get src/store.rs:19"]
    );
    assert_eq!(
        rows(r#"?- innermost_at("src/store.rs", 23, S)."#),
        vec!["get src/store.rs:19"]
    );

    // Where nesting *is* real: `helper` is inside module `detail` (43-48).
    let at_45 = rows(r#"?- symbol_at("src/store.rs", 45, S)."#);
    assert_eq!(
        at_45.len(),
        2,
        "module and function both enclose: {at_45:?}"
    );
    assert_eq!(
        rows(r#"?- innermost_at("src/store.rs", 45, S)."#),
        vec!["helper src/store.rs:44"]
    );
}

#[test]
fn tighter_at_is_what_innermost_at_subtracts() {
    // `detail` encloses line 45 less tightly than `helper` does.
    assert_eq!(
        rows(r#"?- tighter_at("src/store.rs", 45, S)."#),
        vec!["detail src/store.rs:43"]
    );
}

// ------------------------------------------------------------------ callability

#[test]
fn callable_is_the_four_kinds() {
    let mut kinds = rows("?- callable(K).");
    kinds.sort();
    assert_eq!(kinds, ["constructor", "function", "macro", "method"]);
}

#[test]
fn calls_over_reports_and_calls_exact_does_not() {
    // THE measurement this project exists to make. Tier A invents one edge:
    // `Store::get` calls `self.entries.get(key)`, and the identifier `get`
    // matches the only `get` tier A knows — itself.
    //
    // Asked of the **tier-A-only** tree, because that is the tier that
    // over-reports. With a SCIP index the invented edge no longer exists at
    // all: name matching defers wherever tier B resolved the occurrence
    // (`specs/01-facts.md` § Tier precedence), and the assertion below on
    // `indexed()` is what pins that.
    // Two edges, and they are the whole of what name matching can say here.
    // `draw -> open` and `start -> open` are absent because `open` is exported
    // by both `db::conn` and `net::conn`, so `!ambiguous` stops tier A from
    // picking — they exist only in the exact tier. Of the two it does emit, one
    // is right by luck (`warm` calls `store.get`, and the only `get` in the file
    // is the right one) and one is the invention.
    let all = rows_in(named_only(), "?- calls(C, S).");
    assert_eq!(
        all,
        [
            "get src/store.rs:19\tget src/store.rs:19",
            "warm src/store.rs:38\tget src/store.rs:19",
        ]
    );

    // The same goal against the SCIP-covered tree: the invented self-edge is
    // gone, because tier A never got to guess at that occurrence.
    let covered = rows("?- calls(C, S).");
    assert!(
        !covered
            .iter()
            .any(|r| r == "get src/store.rs:19\tget src/store.rs:19"),
        "the invented self-edge survived a SCIP index: {covered:?}"
    );

    let exact = rows("?- calls_exact(C, S).");
    assert_eq!(
        exact,
        [
            "draw src/ui/panel.rs:8\topen src/db/conn.rs:3",
            "start src/app.rs:7\topen src/db/conn.rs:3",
            "warm src/store.rs:38\tget src/store.rs:19",
        ]
    );
    // The invention, stated directly. Subtracting the two row counts would now
    // compare sets drawn from different trees — and it underflowed, which is
    // how this was noticed.
    let invented = "get src/store.rs:19\tget src/store.rs:19";
    assert!(all.contains(&invented.to_string()), "{all:?}");
    assert!(!exact.contains(&invented.to_string()), "{exact:?}");
}

#[test]
fn calls_at_carries_the_site_and_the_provenance() {
    let sites = rows(r#"?- calls_at(From, S, "src/app.rs", L, P)."#);
    assert_eq!(sites.len(), 1, "{sites:?}");
    assert!(sites[0].ends_with("\t9\texact"), "{sites:?}");
}

#[test]
fn callers_and_callees_read_calls_both_ways() {
    let callers = rows(r#"?- def(S, _, _, "get"), callers(C, S), def(C, _, _, "warm")."#);
    assert_eq!(callers.len(), 1, "{callers:?}");
    let callees = rows(r#"?- def(S, _, _, "warm"), callees(S, C), def(C, _, _, "get")."#);
    assert_eq!(callees.len(), 1, "{callees:?}");
}

#[test]
fn the_by_name_rules_take_a_name_instead_of_a_symid() {
    // Both files that call `open` — and no SymId to type.
    let mut callers = rows(r#"?- callers_by_name(C, "open")."#);
    callers.sort();
    assert_eq!(callers, ["draw src/ui/panel.rs:8", "start src/app.rs:7"]);

    let mut impacted = rows(r#"?- impact_by_name(C, "open")."#);
    impacted.sort();
    assert_eq!(impacted, ["draw src/ui/panel.rs:8", "start src/app.rs:7"]);
}

// ------------------------------------------------------------ seeded traversal

#[test]
fn impact_of_and_reach_of_point_in_opposite_directions() {
    // `open` is called by `draw` and `start`; neither calls anything further,
    // so the transitive set is the direct set here.
    let mut up = rows(r#"?- def(S, _, _, "open"), impact_of(S, C)."#);
    up.sort();
    up.dedup();
    assert!(up.iter().any(|r| r.contains("draw")), "{up:?}");
    assert!(up.iter().any(|r| r.contains("start")), "{up:?}");

    // Downward from `start`: it reaches `open` and nothing beyond.
    let down = rows(r#"?- def(S, _, _, "start"), reach_of(S, C)."#);
    assert_eq!(down.len(), 1, "{down:?}");
    assert!(down[0].contains("open src/db/conn.rs"), "{down:?}");
}

#[test]
fn impact_of_exact_drops_what_name_matching_invented() {
    // Seeded on `get`: `warm` calls it for real. Tier A also has `get`
    // calling itself, so the tolerant rule reports `get` as its own impactor
    // and the exact one does not.
    //
    // The tolerant form is asked of the tier-A-only tree, because that is where
    // the invention happens. On a SCIP-covered tree the two agree, which is the
    // point of tier precedence rather than a gap in this test.
    let tolerant = raw_rows_in(
        named_only(),
        r#"?- def(S, "src/store.rs", _, "get"), impact_of(S, C)."#,
    );
    let exact = raw_rows(r#"?- def(S, "src/store.rs", _, "get"), impact_of_exact(S, C)."#);
    assert!(
        tolerant
            .iter()
            .any(|r| r.contains("get().\tlocal") || r.matches("get()").count() >= 2),
        "the invented self-edge should appear in the tolerant form: {tolerant:?}"
    );
    assert!(
        exact.iter().all(|r| !r.ends_with("get().")),
        "the exact form must not carry the self-edge: {exact:?}"
    );
}

// ---------------------------------------------------------- unseeded traversal

#[test]
fn reaches_and_recursive_are_the_unseeded_pair() {
    // `start` reaches `open` transitively.
    let reached = rows(r#"?- def(A, _, _, "start"), def(B, _, _, "open"), reaches(A, B)."#);
    assert!(!reached.is_empty(), "{reached:?}");

    // And here is what the invented self-call costs, on the tier that invents
    // it: `get` is reported recursive, and `get` is not recursive. The rule is
    // right; its input is contaminated.
    assert_eq!(
        rows_in(named_only(), "?- recursive(S)."),
        vec!["get src/store.rs:19"]
    );
    // With a SCIP index the contamination is gone and nothing in the fixture is
    // recursive — the rule unchanged, the input clean.
    assert!(rows("?- recursive(S).").is_empty());
}

// --------------------------------------------------------------- location helpers

#[test]
fn file_of_defines_and_at_are_three_views_of_def() {
    let file_of = rows(r#"?- def(S, _, _, "warm"), file_of(S, F)."#);
    assert_eq!(file_of.len(), 1, "{file_of:?}");
    assert!(file_of[0].ends_with("src/store.rs"), "{file_of:?}");

    assert_eq!(
        rows(r#"?- defines("src/db/conn.rs", S), def(S, _, _, "open")."#).len(),
        1
    );

    let at = rows(r#"?- def(S, _, _, "warm"), at(S, F, L)."#);
    assert_eq!(at.len(), 1, "{at:?}");
    assert!(at[0].ends_with("\tsrc/store.rs\t38"), "{at:?}");
}

// ------------------------------------------------------------------ containment

#[test]
fn within_is_transitive_and_the_child_comes_first() {
    // `helper` is in module `detail`, which is in file `src/store.rs`. The
    // argument order is the one an M1 eval sample got backwards, which is why
    // the `%%` line says "Child is FIRST".
    let ancestors = raw_rows(r#"?- def(C, "src/store.rs", _, "helper"), within(C, A)."#);
    assert!(
        ancestors.iter().any(|a| a.contains("detail")),
        "the direct parent is missing: {ancestors:?}"
    );
    assert!(
        ancestors.len() >= 2,
        "within is transitive, so more than the direct parent: {ancestors:?}"
    );
    // The reverse direction binds nothing: `helper` is nobody's ancestor.
    assert!(raw_rows(r#"?- def(A, "src/store.rs", _, "helper"), within(C, A)."#).is_empty());
}

// ------------------------------------------------------------------------ tests

#[test]
fn is_test_matches_by_path_convention() {
    assert_eq!(rows("?- is_test(F)."), vec!["tests/store_test.rs"]);
    assert!(rows(r#"?- is_test("src/store.rs")."#).is_empty());
}

// ------------------------------------------------------------------ orientation

#[test]
fn about_answers_ten_questions_through_one_relation() {
    let mut kinds: Vec<String> = rows(r#"?- def(S, _, _, "warm"), about(S, Rel, A, B, L)."#)
        .iter()
        .filter_map(|r| r.split('\t').nth(1).map(str::to_string))
        .collect();
    kinds.sort();
    kinds.dedup();
    // `warm` has a doc, a signature, a definition site and one callee. It has
    // no caller, implements nothing and is in no test — so those discriminator
    // values are absent rather than empty-valued.
    assert_eq!(kinds, ["callee", "defined", "doc", "sig"]);

    let sig = rows(r#"?- def(S, _, _, "warm"), about(S, "sig", A, _, _)."#);
    assert_eq!(sig.len(), 1);
    assert!(
        sig[0].contains("pub fn warm(store: &Store) -> Option<Entry>"),
        "{sig:?}"
    );
}

// -------------------------------------------------------------------- dependency

#[test]
fn depends_is_file_granularity_over_ref() {
    // db/mod.rs declares `pub mod conn;` and so depends on db/conn.rs.
    let edges = rows(r#"?- depends("src/db/mod.rs", G)."#);
    assert_eq!(edges, vec!["src/db/conn.rs"]);

    // A file does not depend on itself: the rule carries `F != G`.
    assert!(rows(r#"?- depends("src/db/mod.rs", "src/db/mod.rs")."#).is_empty());
}

#[test]
fn depends_exact_is_depends_restricted_to_scip() {
    let tolerant = rows(r#"?- depends("src/app.rs", G)."#);
    let exact = rows(r#"?- depends_exact("src/app.rs", G)."#);
    assert!(!exact.is_empty(), "{exact:?}");
    assert!(
        exact.iter().all(|e| tolerant.contains(e)),
        "exact is a subset by construction: {exact:?} vs {tolerant:?}"
    );
}

// -------------------------------------------------------------- the dex "34"

#[test]
fn dead_export_and_its_exact_form() {
    // `Store` is exported and referenced only inside src/store.rs (by `warm`
    // and its own impl), so it is dead by this rule's definition — which is
    // "unreferenced *outside its own file*", not "unused".
    let dead = rows("?- dead_export(S).");
    assert!(dead.iter().any(|d| d.starts_with("Store ")), "{dead:?}");
    // `open` in db/conn.rs is called from two other files, so it is not dead.
    assert!(
        !dead.iter().any(|d| d.starts_with("open src/db/conn.rs")),
        "{dead:?}"
    );

    let exact = rows("?- dead_export_exact(S).");
    assert!(
        exact.iter().all(|e| dead.contains(e)),
        "the exact form gates on `resolved` as well, so it is a subset"
    );
}

#[test]
fn ref_outside_is_what_dead_export_negates() {
    // `open` in db/conn.rs is referenced from app.rs and panel.rs.
    let outside = rows(r#"?- def(S, "src/db/conn.rs", _, "open"), ref_outside(S, F)."#);
    assert_eq!(
        outside.len(),
        1,
        "one row per (S, defining file): {outside:?}"
    );
    assert!(outside[0].ends_with("src/db/conn.rs"), "{outside:?}");

    let exact = rows(r#"?- def(S, "src/db/conn.rs", _, "open"), ref_outside_exact(S, F)."#);
    assert_eq!(exact.len(), 1, "{exact:?}");
}

#[test]
fn entrypoint_is_a_callable_nothing_calls() {
    let entries = rows("?- entrypoint(S).");
    // `draw` and `start` are called by nothing in this tree.
    assert!(
        entries.iter().any(|e| e.starts_with("draw ")),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|e| e.starts_with("start ")),
        "{entries:?}"
    );
    // `warm` calls `get`, but nothing calls `warm` — so it IS an entrypoint.
    assert!(
        entries.iter().any(|e| e.starts_with("warm ")),
        "{entries:?}"
    );
    // `open` in db/conn.rs has two callers, so it is not one.
    assert!(
        !entries.iter().any(|e| e.starts_with("open src/db/conn.rs")),
        "{entries:?}"
    );

    // The exact form additionally requires `resolved(S)`, so a definition the
    // anchor join never reached drops out rather than being asserted about.
    let exact = rows("?- entrypoint_exact(S).");
    assert!(
        exact.iter().all(|e| entries.contains(e)),
        "{exact:?} is not a subset of {entries:?}"
    );
}

#[test]
fn def_lines_measures_and_long_def_judges() {
    // Nothing in this fixture is over 80 lines, and the rule says so rather
    // than ranking what is longest. `ok` with zero rows is the answer.
    // `def_lines` states the length and leaves the comparison to the caller.
    // `long_def` is the one opinionated form, and its 80 is fixed — three of
    // the four M4 agent-eval failures were `long_def(A, N), N > 60`, a no-op
    // filter over an already-`> 80` set (`docs/agent-eval.md`).
    assert!(
        rows("?- long_def(S).").is_empty(),
        "nothing here spans 80 lines"
    );

    let widest = rows("?- def_lines(S, N), N > 8.");
    assert!(!widest.is_empty(), "the fixture has spans wider than 8");

    // Every definition has a length, threshold or not.
    assert_eq!(
        rows("?- def_lines(S, N).").len(),
        rows("?- def_span(S, A, B, C, D).").len()
    );
}

#[test]
fn undocumented_export_finds_the_undocumented() {
    let undocumented = rows("?- undocumented_export(S).");
    // `value` on `Entry` has no doc comment; `limit` on `Config` has one.
    assert!(
        undocumented.iter().any(|u| u.starts_with("value ")),
        "{undocumented:?}"
    );
    assert!(
        !undocumented.iter().any(|u| u.starts_with("limit ")),
        "{undocumented:?}"
    );
    // And `warm`, which is documented, is absent.
    assert!(!undocumented.iter().any(|u| u.starts_with("warm ")));
}

#[test]
fn extern_and_uses_package_see_the_standard_library() {
    // The fixture declares no dependency, but it uses `HashMap` and `String`,
    // and SCIP names the package on the symbol itself. `extern` was empty here
    // until the package was read off *referenced* symbols rather than only off
    // the `SymbolInformation` an indexer chooses to emit — and rust-analyzer
    // emits none for `std`, `core` or `alloc`.
    let externs = rows("?- extern(S, M, P, V).");
    assert!(!externs.is_empty(), "no external packages found");
    assert!(
        externs.iter().all(|e| e.contains("\tcargo\t")),
        "the manager should be cargo: {externs:?}"
    );

    let mut packages: Vec<String> = rows("?- uses_package(F, P).")
        .iter()
        .filter_map(|r| r.split('\t').nth(1).map(str::to_string))
        .collect();
    packages.sort();
    packages.dedup();
    assert_eq!(packages, ["alloc", "core", "std"], "{packages:?}");

    // `src/store.rs` uses `HashMap`, which is `std`.
    let store = rows(r#"?- uses_package("src/store.rs", P)."#);
    assert!(store.iter().any(|p| p == "std"), "{store:?}");

    // The `_exact` form is the same set here: all of it came from SCIP.
    assert_eq!(
        rows("?- uses_package(F, P)."),
        rows("?- uses_package_exact(F, P).")
    );

    // And `about` reports them under the `extern` discriminator.
    assert!(!rows(r#"?- about(S, "extern", A, B, L)."#).is_empty());
}

/// The rules this fixture cannot exercise positively, pinned as empty so the
/// gap is visible rather than assumed covered.
///
/// **`implements/3` is empty for an upstream reason, and it was measured.**
/// `rust-analyzer scip .` emits **zero** `relationships` for this crate — not
/// merely none marked `is_implementation` — so there is no implementation edge
/// to read for `impl Handler for Config`. Our side of it is exercised by
/// `tier_b.rs`'s unit tests, which build the ingest directly. Closing this
/// needs an indexer that emits relationships, not a bigger fixture.
#[test]
fn the_rules_with_no_positive_coverage_are_named() {
    for goal in [
        "?- implements(S, T, P).",
        r#"?- about(S, "implements", A, B, L)."#,
        r#"?- about(S, "implementor", A, B, L)."#,
    ] {
        assert!(
            rows(goal).is_empty(),
            "{goal} unexpectedly has rows now — give it a real assertion and \
             remove it from this list"
        );
    }
}

/// Every rule `schema` advertises is exercised by some test in this file, or is
/// named above as uncovered. A rule nobody queries is a rule nobody has checked.
#[test]
fn every_advertised_rule_appears_in_this_file() {
    let source = include_str!("stdlib.rs");
    let mut unexercised = Vec::new();
    for rule in codeintel::schema::rules(codeintel::schema::STDLIB) {
        let (name, _) = codeintel::schema::split_head(&rule.head).expect("a head");
        if !source.contains(&format!("{name}(")) {
            unexercised.push(name.to_string());
        }
    }
    assert!(
        unexercised.is_empty(),
        "these rules have no test in this file: {unexercised:?}"
    );
}

/// Unused in the assertions above, but kept so the helper signature does not
/// drift: `PathBuf` is what `indexed()` hands out under the hood.
const _: fn() -> PathBuf = || indexed().to_path_buf();
