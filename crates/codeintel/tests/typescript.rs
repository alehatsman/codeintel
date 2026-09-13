//! `codeintel` end to end over a TypeScript tree, TSX included.
//!
//! `docs/plan.md` M5's done-when for a language, in the part that needs the
//! whole binary rather than the extractor: its `imports.scm` produces usable
//! `import/3` rows, a conformance rule over them passes and fails correctly,
//! and the anchor rate against its SCIP index clears 95%.
//!
//! The fixture carries a real `index.scip` produced by `scip-typescript` 0.4.0,
//! committed so that these run without the indexer installed. Regenerate it
//! with `npx @sourcegraph/scip-typescript@0.4.0 index` inside
//! `tests/fixtures/typescript/`. The `tier_a` tests below delete it first:
//! conformance is the capability that works on a first run with no SCIP index.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/typescript")
}

/// A throwaway copy of the TypeScript fixture, `index.scip` included.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    // The golden file is not source; the fixture is a tree to index.
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    dir
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
        if name == ".codeintel" || name == "node_modules" {
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

/// Column `n` of every row, as a sorted set.
fn column(rows: &[String], n: usize) -> Vec<&str> {
    rows.iter()
        .filter_map(|r| r.split('\t').nth(n))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn a_typescript_tree_indexes_and_names_its_indexer() {
    let dir = tier_a_tree();
    let summary = index(dir.path());
    // Seven `.ts` (a `.d.ts` among them) and two `.tsx`.
    assert!(summary.contains("9 indexed"), "{summary}");
    // Not "no SCIP index found" — the literal command
    // (`specs/02-extraction.md` § Acquisition).
    assert!(summary.contains("scip-typescript"), "{summary}");

    let (rows, stderr) = query(dir.path(), "?- file(F, L).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&rows, 1), ["typescript", "typescriptreact"]);
}

#[test]
fn an_import_is_the_specifier_without_its_quotes() {
    // Never resolved to a path (`specs/01-facts.md` § `import`). `import * as
    // net` binds a local name for the module; `import { Row }` binds a member
    // and has no alias.
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- import("ui/panel.tsx", M, A)."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(rows, ["../db/conn\t", "../net/conn\tnet", "./row\t"]);
}

#[test]
fn a_conformance_rule_fires_and_then_stops_firing() {
    // The headline capability, on a tree with no SCIP index at all: no module
    // under ui/ may import from db/. It has to find the violation, and it has
    // to return zero rows with `status: ok` once the violation is gone.
    let dir = tier_a_tree();
    index(dir.path());

    let rules = dir.path().join("conformance.dl");
    std::fs::write(
        &rules,
        "%% ui_imports_db(F, M)  no module under ui/ may import from db/\n\
         ui_imports_db(F, M) :- import(F, M, _), prefix(F, \"ui/\"), contains(M, \"db/\").\n",
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
        String::from_utf8_lossy(&violating.stdout).contains("ui/panel.tsx"),
        "the violation names the file"
    );

    // Remove the offending import and the same check passes. The `net` import
    // stays, so this is the rule getting narrower rather than the file getting
    // emptier.
    let panel = dir.path().join("ui/panel.tsx");
    let source = std::fs::read_to_string(&panel).expect("read");
    let cleaned: Vec<&str> = source
        .lines()
        .filter(|l| !l.contains(r#"from "../db/conn""#))
        .collect();
    std::fs::write(&panel, cleaned.join("\n")).expect("write");

    let clean = run(dir.path(), &args);
    let stderr = String::from_utf8_lossy(&clean.stderr);
    assert_eq!(clean.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

#[test]
fn a_method_is_reachable_through_its_class() {
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(T, _, "class", "Store"), within(S, T), def(S, _, "method", N)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&rows, 2), ["#evict", "get", "handle", "put"]);
}

#[test]
fn every_definition_anchors_against_the_scip_index() {
    // `docs/plan.md` M5: anchor rate >= 95%. The one definition that does not
    // anchor here is `AFTER_WIDE`, whose SCIP column is ambiguous (#21).
    let dir = tree();
    let summary = index(dir.path());
    assert!(summary.contains("scip-typescript"), "{summary}");
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
    // `open` is exported by both `db/conn` and `net/conn`, so tier A's
    // `!ambiguous(N)` guard refuses the edge. The type checker gives each
    // caller its one target, JSX tags included.
    //
    // Six rows have a FILE as the caller. `scip-typescript` 0.4.0 records
    // the binding an import makes as an ordinary reference at module scope,
    // with an empty role bitset, as `scip-python` does; a decorator is
    // referenced at module scope, which is also where it runs; and so is the
    // name in `export default outer`. The rows are what the indexer stated
    // (docs/research.md § Language coverage).
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), "?- calls(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        rows,
        [
            "Draw ui/panel.tsx:18\tPanel ui/panel.tsx:10",
            "Panel ui/panel.tsx:10\tRow ui/row.tsx:1",
            "Panel ui/panel.tsx:10\topen db/conn.ts:1",
            "Panel ui/panel.tsx:10\topen net/conn.ts:1",
            "app/app.ts\topen db/conn.ts:1",
            "describe kinds.ts:12\tdescribe kinds.ts:55",
            "handle store/store.ts:33\tget store/store.ts:20",
            "hidden kinds.ts:96\touter kinds.ts:86",
            "kinds.ts\touter kinds.ts:86",
            "kinds.ts\tsealed kinds.ts:15",
            "outer kinds.ts:86\tinner kinds.ts:88",
            "put store/store.ts:29\tconstructor store/store.ts:7",
            "start app/app.ts:3\topen db/conn.ts:1",
            "store/store.test.ts\twarm store/store.ts:42",
            "testWarm store/store.test.ts:3\twarm store/store.ts:42",
            "ui/panel.tsx\tRow ui/row.tsx:1",
            "ui/panel.tsx\topen db/conn.ts:1",
            "warm store/store.ts:42\tget store/store.ts:20",
        ]
    );
    let (exact, _) = query(dir.path(), "?- calls_exact(A, B).");
    assert_eq!(
        exact.len(),
        rows.len(),
        "every TypeScript edge here is exact"
    );
}

#[test]
fn implements_has_rows_because_the_indexer_states_them() {
    // `implements/3` is empty on Rust, whose indexer emits no relationships
    // (docs/plan.md M4). `scip-typescript` writes `is_implementation` for a
    // class that `implements` an interface, and for each method that satisfies
    // one, across files.
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- implements(S, T, "exact")."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        rows,
        [
            "Config kinds.ts:33\tHandler kinds.ts:19",
            "Store store/store.ts:16\tHandler kinds.ts:19",
            "handle kinds.ts:51\thandle kinds.ts:21",
            "handle store/store.ts:33\thandle kinds.ts:21",
        ]
    );
}

#[test]
fn tier_b_owns_a_method_by_its_class() {
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "store/store.ts", "method", N), parent(S, P), def(P, _, PK, PN)."#,
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
            ("#evict", "Store"),
            ("describe", "Entry"),
            ("get", "Store"),
            ("handle", "Store"),
            ("put", "Store"),
        ]
    );
}

#[test]
fn an_overload_set_is_one_symbol_and_an_accessor_pair_is_two() {
    // `specs/01-facts.md` § `def`. The compiler names two overload signatures
    // and their implementation one symbol, and tier A agrees: one `def`, a
    // span per declaration. It names a getter and its setter apart
    // (`<get>size`, `<set>size`), and once anchored they are two definitions.
    let dir = tree();
    index(dir.path());
    let (parse, stderr) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, "parse"), def_span(S, A, B, _, _)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&parse, 0), ["parse kinds.ts:76"]);
    assert_eq!(column(&parse, 1), ["76", "77", "78"]);

    let (size, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, "size"), def_sig(S, G)."#,
    );
    assert_eq!(
        size,
        [
            "size kinds.ts:59\tget size(): number",
            "size kinds.ts:63\tset size(value: number)",
        ]
    );
}

#[test]
fn an_undeclared_column_after_a_non_ascii_character_is_skipped_not_misplaced() {
    // `scip-typescript` 0.4.0 declares no position encoding and counts UTF-16
    // (#21). On `export const WIDE = "é"; export const AFTER_WIDE = legacy;`
    // the definition of `AFTER_WIDE` and the reference to `legacy` sit after
    // the `é`, so both are skipped and counted. `WIDE` still anchors.
    let dir = tree();
    let summary = index(dir.path());
    assert!(
        summary.contains("scip ambiguous: 2 occurrence(s) skipped"),
        "{summary}"
    );
    let (rows, _) = query(dir.path(), r#"?- def(S, "kinds.ts", _, "AFTER_WIDE")."#);
    assert_eq!(rows.len(), 1, "one definition, not two: {rows:?}");
    let (resolved, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, N), resolved(S), match(N, "WIDE")."#,
    );
    assert_eq!(column(&resolved, 1), ["WIDE"]);
}

#[test]
fn an_export_clause_exports_a_declaration_that_stays_restricted() {
    // `export { retries, sealed as seal }` and `export default outer` in
    // kinds.ts (#22). `visibility` is what the declaration states, so all
    // three stay `restricted`; `exported` reaches them through `name_export`
    // (`specs/01-facts.md` § `name_export`).
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, N), visibility(S, "restricted"), exported(S)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&rows, 1), ["outer", "retries", "sealed"]);
    // A restricted declaration no clause names is still not exported.
    let (rows, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, "hidden"), exported(S)."#,
    );
    assert!(rows.is_empty(), "{rows:?}");
}
