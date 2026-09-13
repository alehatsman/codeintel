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
fn a_truncated_answer_from_a_stale_index_still_says_stale() {
    // Both apply. `truncated` and `cap` carry the cut structurally; the status
    // is the one place `stale` can live, so the cap must not overwrite it (#35).
    let dir = tree();
    index(dir.path());
    let mut lock = facts::Lock::open(&dir.path().join(".codeintel")).expect("opens");
    let held = lock.try_hold().expect("no io error").expect("uncontended");

    let options = codeintel::Options {
        limit: 1,
        ..codeintel::Options::default()
    };
    let answer =
        codeintel::query::run(dir.path(), "?- def(S, F, K, N).", &options).expect("answers");
    assert_eq!(answer.status, codeintel::Status::Stale);
    assert!(answer.truncated);
    assert_eq!(answer.cap, Some("max_result_rows"));
    let hint = answer.hint.unwrap_or_default();
    assert!(hint.contains("codeintel index"), "{hint}");
    assert!(hint.contains("max_result_rows"), "{hint}");
    drop(held);
}

#[test]
fn an_error_exits_2_not_the_1_that_means_a_violation() {
    // `--expect-empty` exits 1 on rows. A program that could not even be read
    // is "I could not tell you", and CI must not read it as "you violate this".
    use std::io::Write as _;
    use std::process::Stdio;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(binary())
        .args(["query", "-", "--expect-empty"])
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&[0xff, 0xfe, b'\n'])
        .expect("write");
    let out = child.wait_with_output().expect("exits");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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

/// When the engine stops at `--limit` and the printed budget then cuts further,
/// the byte cap decided the answer, so it is the one named, and its hint is not
/// "raise --limit" (#27). ~280 printed bytes a row puts the 262,144-byte budget
/// near row 940, under a limit of 1,200.
#[test]
fn when_both_caps_fire_the_one_that_cut_the_rows_is_reported() {
    use std::fmt::Write as _;
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    let long = "x".repeat(115);
    let mut source = String::new();
    for i in 0..1500 {
        writeln!(source, "pub fn f{i:04}_{long}() {{}}").expect("writes to a String");
    }
    std::fs::write(dir.path().join("src/lib.rs"), source).expect("write");
    index(dir.path());

    let out = run(
        dir.path(),
        &[
            "query",
            "?- def(S, F, K, N).",
            "--limit",
            "1200",
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
        Some("max_result_bytes")
    );
    let rows = body
        .get("rows")
        .and_then(|r| r.as_array())
        .map_or(0, Vec::len);
    assert!((1..1200).contains(&rows), "{rows} rows");
    let hint = body
        .get("hint")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(!hint.contains("--limit"), "{hint}");
}

#[test]
fn a_conformance_check_passes_clean_and_fails_dirty() {
    // The headline capability in the form CI consumes it: a rule file in the
    // repository under review, and an exit code. `--expect-empty` exits 1 on a
    // violation, distinct from the 2 that means the query never ran — "your
    // code violates this" and "I could not tell you" are not the same result.
    let dir = tree();
    index(dir.path());

    let rules = dir.path().join("conformance.dl");
    std::fs::write(
        &rules,
        "%% ui_imports_db(F, M)  no module under ui/ may import from db/\n\
         ui_imports_db(F, M) :- import(F, M, _), prefix(F, \"src/ui/\"), contains(M, \"db\").\n",
    )
    .expect("write");
    let rules = rules.to_string_lossy().to_string();

    let violating = run(
        dir.path(),
        &[
            "query",
            "?- ui_imports_db(F, M).",
            "--rules",
            &rules,
            "--expect-empty",
        ],
    );
    assert_eq!(
        violating.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&violating.stderr)
    );
    assert!(
        String::from_utf8_lossy(&violating.stderr).contains("expect-empty"),
        "the failure has to say why"
    );

    // Remove the offending import and the same check passes.
    let panel = dir.path().join("src/ui/panel.rs");
    let source = std::fs::read_to_string(&panel).expect("read");
    let kept: Vec<&str> = source.lines().filter(|l| !l.contains("db")).collect();
    let cleaned = kept.join("\n");
    std::fs::write(&panel, cleaned).expect("write");

    let clean = run(
        dir.path(),
        &[
            "query",
            "?- ui_imports_db(F, M).",
            "--rules",
            &rules,
            "--expect-empty",
        ],
    );
    assert_eq!(
        clean.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
}

#[test]
fn rule_files_add_clauses_and_the_query_shadows_them() {
    // A predicate is the union of its clauses, so a `--rules` file defining
    // `is_test` WIDENS it rather than replacing it. Only a rule written in the
    // query program itself shadows a loaded one — and that is reported, because
    // a repository rule quietly replacing `is_test` would change every answer
    // that reads it.
    let dir = tree();
    index(dir.path());
    let rules = dir.path().join("extra.dl");
    std::fs::write(
        &rules,
        "is_test(F) :- file(F, _), contains(F, \"panel\").\n",
    )
    .expect("write");
    let rules = rules.to_string_lossy().to_string();

    // `is_test` is the union: the stdlib's path conventions plus the new one.
    let widened = run(dir.path(), &["query", "?- is_test(F).", "--rules", &rules]);
    let rows: Vec<String> = String::from_utf8_lossy(&widened.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("panel")),
        "the rule file did not widen is_test: {rows:?}"
    );
    assert!(
        !String::from_utf8_lossy(&widened.stderr).contains("shadowed"),
        "an additive clause is not a shadowing"
    );

    // The same rule in the *program* replaces every loaded clause, and says so.
    let shadowing = run(
        dir.path(),
        &[
            "query",
            "is_test(F) :- file(F, _), contains(F, \"panel\"). ?- is_test(F).",
        ],
    );
    let stderr = String::from_utf8_lossy(&shadowing.stderr);
    assert!(stderr.contains("shadowed"), "{stderr}");
    assert!(stderr.contains("is_test"), "{stderr}");
    let rows: Vec<String> = String::from_utf8_lossy(&shadowing.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        rows.iter().all(|r| r.contains("panel")),
        "the stdlib clauses survived a shadowing: {rows:?}"
    );
}

#[test]
fn a_broken_rule_file_is_an_answer_not_a_crash() {
    let dir = tree();
    index(dir.path());
    let rules = dir.path().join("broken.dl");
    std::fs::write(&rules, "this is not datalog\n").expect("write");

    let out = run(
        dir.path(),
        &[
            "query",
            "?- def(S, F, K, N).",
            "--rules",
            &rules.to_string_lossy(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("status=invalid-query"), "{stderr}");
    assert!(
        stderr.contains("broken.dl"),
        "the file has to be named: {stderr}"
    );
}

#[test]
fn a_rendered_answer_does_not_depend_on_how_the_index_was_built() {
    // Invariant 8 at the surface: the same tree answers the same way whether it
    // was indexed in one pass or incrementally. The interner is append-only, so
    // a symbol first seen in a late pass gets a higher atom id than it would
    // cold — which is why rendering sorts by printed text rather than by atom.
    let cold = tree();
    index(cold.path());

    let incremental = tree();
    let held = incremental.path().join("src/kinds.rs");
    let source = std::fs::read_to_string(&held).expect("read");
    std::fs::remove_file(&held).expect("remove");
    index(incremental.path());
    // Same bytes as `cold` now, but kinds.rs's atoms were interned last.
    std::fs::write(&held, &source).expect("restore");
    index(incremental.path());

    let goal = r"?- def(S, F, K, N).";
    let (a, _) = query(cold.path(), goal);
    let (b, _) = query(incremental.path(), goal);
    assert!(!a.is_empty());
    assert_eq!(a, b, "the same tree rendered differently");

    // And the byte cap selects the same rows, because it is applied to the
    // sorted text rather than to the engine's atom order.
    let wide = r"?- def(S, F, K, N), def_span(S, L1, L2, B1, B2).";
    let (a, _) = query(cold.path(), wide);
    let (b, _) = query(incremental.path(), wide);
    assert_eq!(a, b, "the byte cap selected a different set");
}

#[test]
fn a_ground_goal_says_true_rather_than_printing_nothing_visible() {
    // A goal with no variables has no columns: truth is one empty row and
    // falsehood is none. Printed literally that is a bare newline versus
    // nothing — an answer no one can see, and `status=ok` either way.
    let dir = tree();
    index(dir.path());

    let (rows, stderr) = query(dir.path(), r#"?- file("src/store.rs", "rust")."#);
    assert_eq!(rows, vec!["true"], "{stderr}");
    assert!(stderr.contains("status=ok"), "{stderr}");

    let (rows, stderr) = query(dir.path(), r#"?- file("src/store.rs", "python")."#);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(stderr.contains("status=ok"), "{stderr}");
    // And the hint says which literal was false, so the two are never confused.
    assert!(stderr.contains("matched 0 rows"), "{stderr}");

    // JSON keeps the tuple shape: one empty row versus none.
    let out = run(
        dir.path(),
        &[
            "query",
            r#"?- file("src/store.rs", "rust")."#,
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(json["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(json["columns"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_symbol_with_no_name_renders_as_its_location() {
    // SCIP gives a crate-root module an empty `display_name`. Holding the
    // empty column open prefixes the location with a space, which reads as a
    // rendering glitch; the location alone is the whole of what is known.
    let dir = tree();
    index(dir.path());
    let (rows, _) = query(dir.path(), r#"?- def(S, F, K, "")."#);
    for row in &rows {
        let symbol = row.split('\t').next().unwrap_or_default();
        assert!(!symbol.starts_with(' '), "leading space in {row:?}");
        assert!(!symbol.is_empty(), "empty symbol column in {row:?}");
    }
}

/// `specs/05-surface.md` § Response contract: a constant in a typed column is
/// diagnosed against that column, because "the index holds 1448 `def` rows" is
/// true and tells the agent nothing about the value it got wrong.
///
/// Both cases here are recorded agent-eval failures, not hypotheticals: an
/// agent wrote `Kind="type"` for a Rust struct, and the integer-as-string trap
/// had a warning in `schema`'s NOTES block that evidently is not where an agent
/// reads it.
#[test]
fn an_empty_result_diagnoses_the_constant_not_the_relation() {
    let dir = tree();
    index(dir.path());

    // A valid vocabulary value that has rows of its own: the emptiness came
    // from elsewhere in the literal, and the hint must not claim otherwise
    // while printing a non-zero count beside it.
    let (_, stderr) = query(dir.path(), r#"?- def(S, _, "struct", "NoSuchThing")."#);
    assert!(stderr.contains("struct"), "{stderr}");
    assert!(
        stderr.contains("another constant in this literal"),
        "a value with rows must not be blamed: {stderr}"
    );

    // A value the vocabulary does not contain at all.
    let (_, stderr) = query(dir.path(), r#"?- def(S, _, "klass", _)."#);
    assert!(
        stderr.contains("not one of this column's values"),
        "{stderr}"
    );
    // The vocabulary is listed with this index's counts, so the agent can see
    // `struct` beside the `type` it guessed. Listing is a fact about the index;
    // suggesting a replacement would be a guess (invariant 1).
    assert!(stderr.contains("the column holds:"), "{stderr}");
    assert!(stderr.contains("struct "), "{stderr}");

    // An integer column with a quoted constant can never match, whatever the
    // rest of the query says.
    let (_, stderr) = query(dir.path(), r#"?- def_span(S, "10", E, A, B)."#);
    assert!(stderr.contains("holds integers"), "{stderr}");
    assert!(stderr.contains("write 10 unquoted"), "{stderr}");
}

/// Every column type that `schema::columns` classifies must actually occur in a
/// signature. A classifier arm matching a name no relation uses is dead code
/// that reads as coverage.
///
/// Derived signatures count, not only base ones. Since schema 2 no *base*
/// relation carries a `Prov` column — `implements` became a rule and
/// `scip_impl` is `exact` by construction — yet `Prov` is exactly what a user
/// reads off `ref` and `implements`, so a base-only scan would call a live
/// classifier arm dead.
#[test]
fn every_typed_column_is_reachable() {
    let stdlib = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules/stdlib.dl"),
    )
    .expect("stdlib.dl is readable");
    let heads: Vec<String> = codeintel::schema::rules(&stdlib)
        .into_iter()
        .map(|rule| rule.head)
        .collect();
    let typed: BTreeSet<&str> = RELATIONS
        .iter()
        .map(|rel| codeintel::schema::signature(rel.name).0)
        .chain(heads.iter().map(String::as_str))
        .flat_map(|sig| {
            sig.split_once('(')
                .map_or("", |(_, args)| args.trim_end_matches(')'))
                .split(',')
                .map(str::trim)
                .collect::<Vec<_>>()
        })
        .collect();
    for name in [
        "Line",
        "Col",
        "StartLine",
        "EndLine",
        "StartByte",
        "EndByte",
        "Kind",
        "Role",
        "Prov",
    ] {
        assert!(typed.contains(name), "`{name}` is classified but unused");
    }
}

/// Text output promises one row per line and `columns.len()` tab-separated
/// fields (`specs/05-surface.md` § `query`). A `def_doc` carries a whole doc
/// comment, newlines included, and printing it unescaped broke both promises
/// silently: one matched row printed as seven lines with ragged field counts,
/// under `status: ok`. JSON was always correct, which is why nothing that
/// checked the structured format noticed.
#[test]
fn a_multi_line_value_stays_one_row() {
    let dir = tree();
    index(dir.path());

    // `get` in the fixture has a two-line doc comment, written for exactly this.
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, _, "method", "get"), def_doc(S, D)."#,
    );
    assert_eq!(rows.len(), 1, "one match must print one line: {rows:?}");
    assert!(stderr.contains("status=ok"), "{stderr}");

    let row = &rows[0];
    assert_eq!(
        row.split('\t').count(),
        2,
        "a 2-column answer must have 2 fields: {row:?}"
    );
    assert!(
        row.contains("\\n"),
        "the newline must survive as \\n: {row:?}"
    );
    assert!(
        !row.contains('\n') && !row.contains('\r'),
        "no raw control characters may reach the line: {row:?}"
    );
}

/// The escape is reversible, which is why backslash is escaped too. Without it
/// a source text holding a literal backslash-n and an escaped newline print
/// identically and a consumer cannot tell them apart.
#[test]
fn the_escape_is_reversible() {
    let dir = tree();
    std::fs::write(
        dir.path().join("src/esc.rs"),
        "/// A doc with a literal \\n inside it.\npub fn escaped() {}\n",
    )
    .expect("write");
    index(dir.path());

    let (rows, _) = query(dir.path(), r#"?- def(S, _, _, "escaped"), def_doc(S, D)."#);
    assert_eq!(rows.len(), 1, "{rows:?}");
    // The source backslash arrives doubled, so it cannot be confused with the
    // `\n` an escaped newline would have produced.
    assert!(
        rows[0].contains("\\\\n"),
        "a literal backslash must be doubled: {:?}",
        rows[0]
    );
}

/// A path constant is diagnosed against the set of indexed files
/// (`specs/05-surface.md` § Response contract). Paths are what an agent gets
/// wrong most often, because `ripgrep`, `git diff` and stack traces all emit
/// absolute or `./`-prefixed forms while the index keys on repo-relative ones.
#[test]
fn a_path_constant_is_diagnosed_against_the_indexed_files() {
    let dir = tree();
    index(dir.path());

    // A leading `./` is stripped, and the stripped form is reported as indexed.
    let (_, stderr) = query(dir.path(), r#"?- def(S, "./src/store.rs", _, N)."#);
    assert!(stderr.contains("repo-relative"), "{stderr}");
    assert!(stderr.contains("`src/store.rs` is indexed"), "{stderr}");

    // An absolute path likewise, which needs the root canonicalised: the root
    // as passed is usually `.`, which no absolute path starts with.
    let absolute = dir.path().join("src/store.rs");
    let (_, stderr) = query(
        dir.path(),
        &format!(r#"?- def(S, "{}", _, N)."#, absolute.display()),
    );
    assert!(stderr.contains("repo-relative"), "{stderr}");

    // Right file name, wrong directory: say where it actually is. This is a
    // fact about the index, not a guess at intent.
    let (_, stderr) = query(dir.path(), r#"?- def(S, "lib/store.rs", _, N)."#);
    assert!(stderr.contains("`store.rs` is indexed at"), "{stderr}");
    assert!(stderr.contains("src/store.rs"), "{stderr}");

    // Nothing by that name at all: say so, and say how to find out what was
    // skipped rather than leaving the agent to guess that it was.
    let (_, stderr) = query(dir.path(), r#"?- def(S, "nope/absent.rs", _, N)."#);
    assert!(stderr.contains("none is named `absent.rs`"), "{stderr}");
    assert!(stderr.contains("codeintel status"), "{stderr}");

    // An indexed path must NOT be blamed: the emptiness is elsewhere.
    let (_, stderr) = query(dir.path(), r#"?- def(S, "src/store.rs", "enum", N)."#);
    assert!(!stderr.contains("no indexed file"), "{stderr}");
}

#[test]
fn a_quoted_integer_in_a_derived_rule_is_named_in_the_hint() {
    // `at/3` is a rule, so no schema column says its third argument is a line.
    // "42" and 42 are different atoms and never match; the hint says so and
    // names the unquoted form, since that is a fact about atoms, not a guess
    // about the code.
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- at(S, F, "42")."#);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(stderr.contains("did you mean 42?"), "{stderr}");

    // A line that exists, written as an integer, is not second-guessed.
    let (rows, stderr) = query(dir.path(), r"?- at(S, F, 1).");
    assert!(!stderr.contains("did you mean"), "{stderr}");
    drop(rows);
}
