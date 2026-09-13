//! The warm engine, the refresh that writes nothing (#18), and what
//! `stats.refreshed` reports (#19).
//!
//! `specs/05-surface.md` § MCP and `specs/04-storage.md` § Incremental reindex.
//! A long-lived `codeintel mcp` keeps its loaded index between calls; these pin
//! the three ways that could go wrong — a stale answer after an edit, an answer
//! that depends on what was asked earlier, and a read that writes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use codeintel::query::{self, Options, Warm};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

#[test]
fn a_query_against_an_unchanged_tree_writes_nothing() {
    let dir = indexed();
    let manifest = dir.path().join(".codeintel/manifest.json");
    let before = snapshot(&manifest);

    for _ in 0..2 {
        cli_query(dir.path(), "?- file(F, _).");
    }
    assert!(
        snapshot(&manifest) == before,
        "an auto-refresh that changed nothing rewrote the manifest"
    );

    // One that did change something still commits it.
    append(
        &dir.path().join("src/store.rs"),
        "\npub fn added_by_the_test() {}\n",
    );
    cli_query(dir.path(), "?- file(F, _).");
    assert!(
        snapshot(&manifest).0 != before.0,
        "an auto-refresh that re-extracted a file did not commit it"
    );
}

/// `stats.refreshed` is a count of the files re-extracted before the answer,
/// never a list (`specs/05-surface.md` § `query`, #19).
#[test]
fn stats_refreshed_counts_the_files_a_refresh_re_extracted() {
    let dir = indexed();
    assert_eq!(
        refreshed(dir.path()),
        0,
        "an unchanged tree re-extracted something"
    );

    append(
        &dir.path().join("src/store.rs"),
        "\npub fn added_by_the_test() {}\n",
    );
    assert_eq!(
        refreshed(dir.path()),
        1,
        "one edited file is one re-extracted file"
    );
    assert_eq!(
        refreshed(dir.path()),
        0,
        "the edit was re-extracted again, so the first refresh never committed it"
    );
}

/// `stats.refreshed` from `codeintel query --format json`, with auto-refresh on.
fn refreshed(root: &Path) -> u64 {
    let out = Command::new(binary())
        .args(["query", "?- file(F, _).", "--format", "json"])
        .current_dir(root)
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON on stdout");
    json.pointer("/stats/refreshed")
        .and_then(serde_json::Value::as_u64)
        .expect("stats.refreshed is an integer")
}

#[test]
fn indexing_an_empty_tree_still_leaves_an_index() {
    let dir = tempfile::tempdir().expect("tempdir");
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
    assert!(
        dir.path().join(".codeintel/manifest.json").exists(),
        "a refresh over nothing skipped the commit that creates the index"
    );
}

#[test]
fn a_warm_engine_sees_an_edit_made_between_calls() {
    let dir = indexed();
    let goal = r#"?- def(S, "src/store.rs", _, "added_by_the_test")."#;
    let mut warm = Warm::default();

    let first = warm
        .run(dir.path(), goal, &Options::default())
        .expect("runs");
    assert!(first.rows.is_empty(), "{first:?}");

    append(
        &dir.path().join("src/store.rs"),
        "\npub fn added_by_the_test() {}\n",
    );
    let second = warm
        .run(dir.path(), goal, &Options::default())
        .expect("runs");
    assert_eq!(
        second.rows.len(),
        1,
        "the warm engine answered from the index as it was before the edit: {second:?}"
    );
}

#[test]
fn a_warm_engine_keeps_the_row_a_cold_one_keeps_under_a_cap() {
    let dir = indexed();
    // Two strings the corpus does not contain, interned in the order written.
    // The engine orders by atom, so under a limit of one the surviving row is
    // whichever string was interned first.
    let capped = r#"p("zz_second"). p("zz_first"). ?- p(X)."#;
    let options = Options {
        limit: 1,
        ..Options::default()
    };
    let cold = query::run(dir.path(), capped, &options).expect("runs");
    assert!(cold.truncated, "{cold:?}");

    let mut warm = Warm::default();
    // Interns "zz_first" first. Kept past this call, it would take the lower
    // atom and change which row survives the cap below.
    warm.run(
        dir.path(),
        r#"q("zz_first"). ?- q(X)."#,
        &Options::default(),
    )
    .expect("runs");
    let again = warm.run(dir.path(), capped, &options).expect("runs");

    assert_eq!(
        again.rows, cold.rows,
        "a literal from an earlier call changed which row survived the cap"
    );
    assert_eq!(again.status, cold.status);
}

/// Run `codeintel query` with auto-refresh on, as an agent would.
fn cli_query(root: &Path, goal: &str) {
    let out = Command::new(binary())
        .args(["query", goal])
        .current_dir(root)
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn snapshot(path: &Path) -> (Vec<u8>, SystemTime) {
    let bytes = std::fs::read(path).expect("readable");
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .expect("an mtime");
    (bytes, modified)
}

fn append(path: &Path, text: &str) {
    let mut src = std::fs::read_to_string(path).expect("readable");
    src.push_str(text);
    std::fs::write(path, src).expect("writable");
}

/// The rust fixture, indexed with both tiers, clock pinned as in `tests/scip.rs`.
fn indexed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let from: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust");
    copy(&from, dir.path());
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
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
}

const SOURCE_MTIME: u64 = 1_700_000_000;

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
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens for set_times");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("sets mtime");
}
