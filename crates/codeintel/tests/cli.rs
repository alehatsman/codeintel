//! `codeintel index` and `codeintel query`, end to end.
//!
//! These are `docs/plan.md` M2's done-when list, one test each. They run the
//! real binary over a copy of `tests/fixtures/rust/`, because the failures they
//! guard against — a stale answer, a deleted file still answering, a silently
//! empty result — only exist once the store, the refresh and the engine are
//! wired together.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use datalog::atom::is_int;
use facts::{RELATIONS, Store};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

/// A throwaway copy of the fixture tree, **tier A only**.
///
/// `index.scip` and the fixture's own `.gitignore` are dropped: these are M2's
/// tests, and they assert tier-A symbols and an untouched working tree. Tier B
/// has its own file.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    // The golden file is not source; the fixture is a tree to index.
    for extra in ["expected.facts", "index.scip", ".gitignore"] {
        drop(std::fs::remove_file(dir.path().join(extra)));
    }
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

fn index(root: &Path) -> Output {
    let out = run(root, &["index", "."]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn query(root: &Path, program: &str) -> (Vec<String>, String) {
    let out = run(root, &["query", program]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let rows = stdout.lines().map(str::to_string).collect();
    (rows, stderr)
}

/// Every fact in the store, resolved — for comparing two indexes that will not
/// agree on atom ids.
fn facts_of(root: &Path) -> BTreeSet<String> {
    let store = Store::open(root, "").expect("opens");
    let relations = store.load().expect("loads");
    let mut out = BTreeSet::new();
    for rel in RELATIONS {
        let Some(rows) = relations.get(rel.name) else {
            continue;
        };
        for row in rows.iter() {
            let args: Vec<String> = row
                .iter()
                .map(|a| {
                    if is_int(*a) {
                        a.to_string()
                    } else {
                        format!("{:?}", store.resolve(*a).unwrap_or("<?>"))
                    }
                })
                .collect();
            out.insert(format!("{}({}).", rel.name, args.join(", ")));
        }
    }
    out
}

fn segments(root: &Path) -> Vec<(String, Vec<u8>)> {
    let dir = root.join(".codeintel/seg");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("segments exist") {
        let entry = entry.expect("entry");
        out.push((
            entry.file_name().to_string_lossy().to_string(),
            std::fs::read(entry.path()).expect("readable"),
        ));
    }
    out.sort();
    out
}

#[test]
fn index_then_query_returns_real_rows() {
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- def(S, F, "function", N)."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(rows.iter().any(|r| r.contains("warm")), "no rows: {rows:?}");
}

#[test]
fn the_location_bridge_lands_on_the_right_symbol() {
    let dir = tree();
    index(dir.path());
    // A symbol column prints as `Name path:line`: the raw SymId below is a
    // SCIP string no agent can read or retype (`specs/05-surface.md`).
    let (rows, _) = query(dir.path(), r#"?- innermost_at("src/store.rs", 23, S)."#);
    assert_eq!(rows, vec!["get src/store.rs:19"]);

    let out = run(
        dir.path(),
        &[
            "query",
            r#"?- innermost_at("src/store.rs", 23, S)."#,
            "--raw",
        ],
    );
    let raw: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(raw, vec!["local src/store.rs Store#get()."]);
}

#[test]
fn raw_output_round_trips_into_the_next_query() {
    // The reason `--raw` exists: the pretty form is for reading, the raw form
    // is what a second query can bind as a literal.
    let dir = tree();
    index(dir.path());
    let out = run(
        dir.path(),
        &[
            "query",
            r#"?- innermost_at("src/store.rs", 23, S)."#,
            "--raw",
        ],
    );
    let symbol = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!symbol.is_empty(), "no raw row to feed back");

    let (rows, stderr) = query(dir.path(), &format!(r"?- def({symbol:?}, F, K, N)."));
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].starts_with("src/store.rs\t"), "{rows:?}");
}

#[test]
fn a_rendered_row_has_exactly_one_field_per_column() {
    // A symbol's name and location are joined by a space, not a tab, so the
    // text form is never wider than `columns` says it is — a consumer that
    // splits on tabs is never handed a ragged table.
    let dir = tree();
    index(dir.path());
    let (rows, _) = query(dir.path(), r#"?- def(S, F, "function", N)."#);
    assert!(!rows.is_empty());
    for row in &rows {
        assert_eq!(row.split('\t').count(), 3, "{row}");
    }
}

#[test]
fn an_empty_result_says_which_literal_matched_nothing() {
    // `ok` with zero rows is indistinguishable from a typo, a wrong constant
    // and a path that was never indexed unless the answer says which literal
    // stopped the join (`specs/05-surface.md` § Response contract).
    let dir = tree();
    index(dir.path());

    let (rows, stderr) = query(dir.path(), r#"?- file(F, "python")."#);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(
        stderr.contains(r#"`file(F, "python")` matched 0 rows"#),
        "{stderr}"
    );
    assert!(stderr.contains("`file` row(s)"), "{stderr}");

    // The literal named is the first one that matched nothing, not the first
    // one written: `def` here matches, and the join stops after it.
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, F, "function", N), import(F, "nosuchmodule", _)."#,
    );
    assert!(rows.is_empty(), "{rows:?}");
    assert!(
        stderr.contains(r#"import(F, "nosuchmodule", _)` matched 0 rows"#),
        "{stderr}"
    );
}

#[test]
fn a_non_empty_answer_carries_no_empty_hint() {
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- def(S, F, "function", N)."#);
    assert!(!rows.is_empty());
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(!stderr.contains("matched 0 rows"), "{stderr}");
}

#[test]
fn json_carries_the_raw_atoms_and_the_display_form() {
    // A programmatic consumer must never have to parse the pretty form back
    // apart (`specs/05-surface.md` § Symbol rendering).
    let dir = tree();
    index(dir.path());
    let out = run(
        dir.path(),
        &[
            "query",
            r#"?- def(S, F, "function", N)."#,
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    let rows = json["rows"].as_array().expect("rows");
    let display = json["display"].as_array().expect("display");
    let columns = json["columns"].as_array().expect("columns").len();
    assert_eq!(rows.len(), display.len());
    assert!(!rows.is_empty());
    assert_ne!(rows[0], display[0], "display is not expanded");

    // The two arrays are one answer in two notations. Rendering each
    // separately and sorting both gives arrays whose `i`th rows are different
    // tuples — and a consumer that shows `display[i]` then feeds `rows[i]`
    // into its next query binds the wrong symbol. `?- def(S, F, "function", N)`
    // returns S, F and N, so the display of S must start with that row's own N.
    for (raw, shown) in rows.iter().zip(display) {
        let raw = raw.as_array().expect("a row is an array of values");
        let shown = shown.as_array().expect("a row is an array of values");
        assert_eq!(raw.len(), columns, "one value per column");
        assert_eq!(shown.len(), columns, "one value per column");

        let name = raw[2].as_str().expect("the Name column");
        let symbol = shown[0].as_str().expect("the symbol column");
        assert!(
            symbol.starts_with(name),
            "display {symbol:?} does not describe raw {:?}",
            raw[0]
        );
    }
}

#[test]
fn a_conformance_rule_fires_and_then_stops_firing() {
    // The headline capability, and it needs no SCIP index: no module under
    // `ui/` may import from `db/`.
    let dir = tree();
    let violation = r#"?- import(F, M, _), match(F, "^src/ui/"), match(M, "db")."#;
    index(dir.path());
    let (rows, stderr) = query(dir.path(), violation);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows.first().is_some_and(|r| r.contains("src/ui/panel.rs")));
    assert!(stderr.contains("status=ok"), "{stderr}");

    // Remove the import and the same query is zero rows with `status: ok` —
    // "this is not true of your code", not a failure.
    let panel = dir.path().join("src/ui/panel.rs");
    let text = std::fs::read_to_string(&panel).expect("readable");
    std::fs::write(
        &panel,
        text.replace("use crate::db::conn;", "")
            .replace("conn::open(\"sqlite://memory\")", "true"),
    )
    .expect("writable");
    index(dir.path());
    let (rows, stderr) = query(dir.path(), violation);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

#[test]
fn indexing_twice_is_byte_identical() {
    let dir = tree();
    index(dir.path());
    let first = segments(dir.path());
    let out = index(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(stderr.contains("0 indexed"), "{stderr}");
    assert_eq!(first, segments(dir.path()));
}

#[test]
fn file_order_does_not_change_the_facts() {
    // Two trees with the same content, written in opposite order. The walk
    // sorts, so the facts — and the atom ids behind them — must agree.
    let first = tree();
    index(first.path());

    let second = tempfile::tempdir().expect("tempdir");
    let mut files: Vec<PathBuf> = Vec::new();
    collect(first.path(), first.path(), &mut files);
    files.sort();
    files.reverse();
    for rel in files {
        let target = second.path().join(&rel);
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        std::fs::copy(first.path().join(&rel), target).expect("copy");
    }
    index(second.path());
    assert_eq!(facts_of(first.path()), facts_of(second.path()));
}

fn collect(root: &Path, at: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(at).expect("readable") {
        let entry = entry.expect("entry");
        if entry.file_name() == std::ffi::OsStr::new(".codeintel") {
            continue;
        }
        if entry.file_type().expect("type").is_dir() {
            collect(root, &entry.path(), out);
        } else if let Ok(rel) = entry.path().strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
}

#[test]
fn deleting_a_file_removes_its_facts() {
    let dir = tree();
    index(dir.path());
    std::fs::remove_file(dir.path().join("src/ui/panel.rs")).expect("removable");
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- def(S, "src/ui/panel.rs", _, N)."#);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

#[test]
fn auto_refresh_answers_about_the_file_on_disk() {
    let dir = tree();
    index(dir.path());
    let conn = dir.path().join("src/db/conn.rs");
    let text = std::fs::read_to_string(&conn).expect("readable");
    std::fs::write(
        &conn,
        format!("{text}\npub fn added_later() -> u32 {{ 1 }}\n"),
    )
    .expect("writable");

    // No reindex between the edit and the question.
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "src/db/conn.rs", _, "added_later")."#,
    );
    assert_eq!(rows.len(), 1, "{rows:?} {stderr}");

    // And a deletion is pruned by the same refresh.
    std::fs::remove_file(&conn).expect("removable");
    let (rows, _) = query(dir.path(), r#"?- def(S, "src/db/conn.rs", _, N)."#);
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn an_incremental_index_equals_a_cold_one_as_fact_sets() {
    // Not byte-identical, and 02-extraction.md says so twice by mistake: an
    // append-only interner assigns ids in encounter order, so a symbol added to
    // an early file lands after every later file's atoms incrementally and
    // inside its own block cold.
    let dir = tree();
    index(dir.path());
    let conn = dir.path().join("src/db/conn.rs");
    let text = std::fs::read_to_string(&conn).expect("readable");
    std::fs::write(&conn, format!("{text}\npub fn late() -> u32 {{ 2 }}\n")).expect("writable");
    index(dir.path());
    let incremental = facts_of(dir.path());

    std::fs::remove_dir_all(dir.path().join(".codeintel")).expect("removable");
    index(dir.path());
    assert_eq!(incremental, facts_of(dir.path()));
}

#[test]
fn the_store_is_added_to_an_existing_gitignore_and_announced() {
    let dir = tree();
    let gitignore = dir.path().join(".gitignore");
    std::fs::write(&gitignore, "/target\n").expect("writable");

    let out = index(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("added `.codeintel/` to .gitignore"),
        "{stderr}"
    );
    let text = std::fs::read_to_string(&gitignore).expect("readable");
    assert_eq!(text, "/target\n.codeintel/\n");

    // Idempotent, and silent the second time.
    let out = index(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!stderr.contains("added `.codeintel/`"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&gitignore).expect("readable"), text);
}

#[test]
fn no_gitignore_is_not_a_reason_to_create_one() {
    let dir = tree();
    let out = index(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!stderr.contains(".gitignore"), "{stderr}");
    assert!(!dir.path().join(".gitignore").exists());
}

#[test]
fn no_index_is_a_status_not_an_empty_answer() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").expect("writable");
    let out = run(dir.path(), &["query", "?- def(S, F, K, N)."]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(stderr.contains("status=no-index"), "{stderr}");
    assert!(stderr.contains("run: codeintel index"), "{stderr}");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn a_broken_query_names_what_is_wrong_with_it() {
    let dir = tree();
    index(dir.path());
    let out = run(dir.path(), &["query", "?- def(S, F)."]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(stderr.contains("status=invalid-query"), "{stderr}");
    assert!(stderr.contains("hint:"), "{stderr}");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn json_carries_the_response_contract() {
    let dir = tree();
    index(dir.path());
    let out = run(
        dir.path(),
        &["query", r#"?- def(S, F, "method", N)."#, "--format", "json"],
    );
    let body: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON object");
    assert_eq!(body.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        body.get("truncated").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    // Three columns, not four: `"method"` is a constant in the goal, and only
    // the variables are columns.
    assert!(
        body.get("columns")
            .is_some_and(|c| c.as_array().is_some_and(|a| a.len() == 3))
    );
    assert!(
        body.get("rows")
            .is_some_and(|r| r.as_array().is_some_and(|a| !a.is_empty()))
    );
    assert!(body.get("stats").is_some());
}

#[test]
fn a_reader_that_cannot_take_the_lock_says_stale_not_locked() {
    // `locked` is for a second writer; telling a reader "locked" is not
    // actionable (04-storage.md § Concurrency).
    let dir = tree();
    index(dir.path());
    let mut lock = facts::Lock::open(&dir.path().join(".codeintel")).expect("opens");
    let held = lock.try_hold().expect("no io error").expect("uncontended");

    let answer = codeintel::query::run(
        dir.path(),
        r#"?- def(S, F, "function", N)."#,
        &codeintel::Options::default(),
    )
    .expect("answers");
    assert_eq!(answer.status, codeintel::Status::Stale);
    assert!(answer.hint.is_some_and(|h| h.contains("codeintel index")));
    assert!(!answer.rows.is_empty(), "a stale answer still carries rows");
    drop(held);
}

#[test]
fn truncation_is_reported_with_the_cap_that_fired() {
    let dir = tree();
    index(dir.path());
    let out = run(
        dir.path(),
        &[
            "query",
            "?- def(S, F, K, N).",
            "--limit",
            "2",
            "--format",
            "json",
        ],
    );
    let body: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(
        body.get("status").and_then(|v| v.as_str()),
        Some("truncated")
    );
    assert_eq!(
        body.get("cap").and_then(|v| v.as_str()),
        Some("max_result_rows")
    );
    assert_eq!(
        body.get("rows").and_then(|r| r.as_array()).map(Vec::len),
        Some(2)
    );
}
