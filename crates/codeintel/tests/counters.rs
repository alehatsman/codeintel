//! Cost goldens: `docs/plan.md` M6 § Pulled forward, harness 1.
//!
//! Every query in `tests/fixtures/counters/queries.tsv` runs against its
//! fixture, and what the engine did — `derived`, `demand`, `transformed` — is
//! diffed as text against `expected.tsv`. Those counters are exact, so this
//! is the regression gate CI can run without flaking. Wall clock is
//! `benches/corpus.rs`, on a machine that can be trusted with it.
//!
//! Rows unchanged and `derived` moved: a cost change. Rows moved: a
//! correctness change. Either way, re-read the diff, then
//! `UPDATE_GOLDEN=1 cargo test -p codeintel --test counters`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use codeintel::query::{self, Options};

const QUERIES: &str = include_str!("../../../tests/fixtures/counters/queries.tsv");

const HEADER: &str = "id\tfixture\tstatus\trows\ttruncated\tderived\tdemand\ttransformed\n";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

#[test]
fn every_fixed_query_costs_what_the_golden_says() {
    let rust = indexed("rust");
    let go = indexed("go");

    let mut got = String::from(HEADER);
    for line in QUERIES
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let mut fields = line.splitn(3, '\t');
        let shape = "queries.tsv rows are id<TAB>fixture<TAB>query";
        let (id, fixture, goal) = (
            fields.next().expect(shape),
            fields.next().expect(shape),
            fields.next().expect(shape),
        );
        let root = [("rust", &rust), ("go", &go)]
            .into_iter()
            .find(|(name, _)| *name == fixture)
            .map(|(_, dir)| dir.path())
            .expect("queries.tsv names a fixture this test indexes");
        // A counter that varies between two runs of the same query is not a
        // golden, and comparing it would flake rather than say so.
        let first = record(root, goal);
        let second = record(root, goal);
        assert_eq!(
            first, second,
            "query {id} reported different counters on two runs of the same index"
        );
        writeln!(got, "{id}\t{fixture}\t{first}").expect("writes to a String");
    }

    let golden = fixtures().join("counters/expected.tsv");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &got).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert!(
        expected == got,
        "query cost changed. Rows unchanged means a cost change, rows moved means a \
         correctness change. Re-read the diff, then \
         `UPDATE_GOLDEN=1 cargo test -p codeintel --test counters`\n{}",
        diff(&expected, &got)
    );
}

/// One query's counters, tab-separated. `elapsed_ms` is left out on purpose:
/// it is the one field that is not exact.
fn record(root: &Path, goal: &str) -> String {
    let options = Options {
        no_refresh: true,
        ..Options::default()
    };
    let answer = query::run(root, goal, &options).expect("the query runs");
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        answer.status.as_str(),
        answer.rows.len(),
        answer.truncated,
        answer.derived,
        answer.demand,
        answer.transformed.join(","),
    )
}

/// The lines that differ, as `-expected` / `+got`, keyed by position. The
/// query set is ordered by id, so a position is a query.
fn diff(expected: &str, got: &str) -> String {
    let (old, new): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), got.lines().collect());
    let mut out = String::new();
    for i in 0..old.len().max(new.len()) {
        let (a, b) = (old.get(i).copied(), new.get(i).copied());
        if a != b {
            if let Some(a) = a {
                writeln!(out, "-{a}").expect("writes to a String");
            }
            if let Some(b) = b {
                writeln!(out, "+{b}").expect("writes to a String");
            }
        }
    }
    out
}

/// A copy of a fixture, indexed with both tiers.
///
/// The clock is pinned the way `tests/scip.rs` pins it: every source at a fixed
/// past instant and `index.scip` a minute later. A copy that crosses a second
/// boundary would otherwise leave some sources newer than the SCIP index,
/// they would index tier A only, and every counter reading them would flake.
fn indexed(fixture: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixtures().join(fixture), dir.path());
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
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
