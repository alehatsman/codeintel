//! The agent eval, re-derived: `docs/plan.md` M6 § Pulled forward, harness 3.
//!
//! `docs/agent-eval.md` found three defects in its own reference answers, and
//! found none of them by assertion: "a reference answer nobody recomputes is an
//! assertion nobody checks". This recomputes them. Set A runs against the rust
//! fixture with its committed `index.scip`, so it can run on every commit. Set
//! B's tree is this repository at `8471824`, which a shallow CI checkout does
//! not have, so it stays with `score.py`.
//!
//! The scoring rule is `tests/fixtures/eval-m4/score.py`'s: an answer is
//! correct when the set of values in the question's answer variables matches
//! the reference. `score.py` stays the tool for scoring a live agent run.
//!
//! **A failure here is a finding, not a golden to refresh.** A moved reference
//! answer means the facts or the rules changed what a question's correct
//! answer is. Say why in the commit, then regenerate with
//! `score.py expected A <indexed fixture>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use codeintel::Status;
use codeintel::query::{self, Options};

fn eval() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/eval-m4")
}

/// Every set-A reference query, re-run, reproduces `expectedA.tsv` exactly.
#[test]
fn set_a_reference_answers_still_hold() {
    let root = indexed();
    let expected = std::fs::read_to_string(eval().join("expectedA.tsv")).expect("expectedA.tsv");
    let expected: BTreeMap<String, String> = expected
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (id, rest) = l.split_once('\t').expect("id<TAB>count<TAB>values");
            (id.to_string(), rest.to_string())
        })
        .collect();

    let mut moved = Vec::new();
    for q in questions() {
        let got = answer_values(root.path(), &q.reference, &q.vars).unwrap_or_default();
        let line = format!(
            "{}\t{}",
            got.len(),
            got.into_iter().collect::<Vec<_>>().join("|")
        );
        if expected.get(&q.id) != Some(&line) {
            moved.push(format!("Q{}: {}", q.id, q.reference));
        }
    }
    assert!(
        moved.is_empty(),
        "set A reference answers moved — the facts or rules changed what these questions' \
         correct answers are. A finding, not a golden: explain it, then regenerate with \
         `score.py expected A`.\n{}",
        moved.join("\n")
    );
}

/// The recorded runs score as `docs/agent-eval.md` § Set A, re-run says,
/// question by question: round 3's Q26 is the one failure in ninety.
#[test]
fn recorded_set_a_runs_fail_exactly_the_recorded_questions() {
    let root = indexed();
    let expected = expected_values();
    let questions = questions();

    for (run, want) in [("A1", vec![]), ("A2", vec![]), ("A3", vec!["26"])] {
        let answers =
            std::fs::read_to_string(eval().join("runs").join(format!("{run}.tsv"))).expect("run");
        let answers: BTreeMap<&str, &str> = answers
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| l.split_once('\t'))
            .collect();
        let mut failed = Vec::new();
        for q in &questions {
            let got = answers
                .get(q.id.as_str())
                .and_then(|query| answer_values(root.path(), query, &q.vars));
            if got.as_ref() != expected.get(&q.id) {
                failed.push(q.id.as_str());
            }
        }
        assert_eq!(
            failed, want,
            "{run} no longer fails the questions it is recorded failing"
        );
    }
}

struct Question {
    id: String,
    vars: Vec<String>,
    reference: String,
}

fn questions() -> Vec<Question> {
    std::fs::read_to_string(eval().join("setA.tsv"))
        .expect("setA.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let fields: Vec<&str> = l.split('\t').collect();
            let field = |i: usize| fields.get(i).expect("id<TAB>vars<TAB>text<TAB>query");
            Question {
                id: (*field(0)).to_string(),
                vars: field(1).split(',').map(str::to_string).collect(),
                reference: (*field(3)).to_string(),
            }
        })
        .collect()
}

/// `expectedA.tsv` as value sets, the way `score.py score` reads it.
fn expected_values() -> BTreeMap<String, BTreeSet<String>> {
    std::fs::read_to_string(eval().join("expectedA.tsv"))
        .expect("expectedA.tsv")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut fields = l.splitn(3, '\t');
            let id = fields.next().expect("id").to_string();
            let payload = fields.nth(1).unwrap_or_default();
            let values = if payload.is_empty() {
                BTreeSet::new()
            } else {
                payload.split('|').map(str::to_string).collect()
            };
            (id, values)
        })
        .collect()
}

/// The set of tuples over `wanted`, each `\x1f`-joined — or `None` for a query
/// that did not run, or that does not bind every wanted variable.
///
/// `score.py` treats these four statuses as "no answer" and every other one as
/// rows, so a `no-scip` answer is still scored on what it returned.
fn answer_values(root: &Path, program: &str, wanted: &[String]) -> Option<BTreeSet<String>> {
    let options = Options {
        limit: 5000,
        no_refresh: true,
        raw: true,
        ..Options::default()
    };
    let answer = query::run(root, program, &options).expect("the query runs");
    if matches!(
        answer.status,
        Status::InvalidQuery | Status::Unstratified | Status::Timeout | Status::BudgetExceeded
    ) {
        return None;
    }
    let index: Vec<usize> = wanted
        .iter()
        .map(|name| answer.columns.iter().position(|c| c == name))
        .collect::<Option<_>>()?;
    answer
        .rows
        .iter()
        .map(|row| {
            index
                .iter()
                .map(|&i| row.raw.get(i).cloned())
                .collect::<Option<Vec<_>>>()
                .map(|v| v.join("\x1f"))
        })
        .collect()
}

/// The rust fixture, indexed with both tiers, clock pinned as in `tests/scip.rs`.
fn indexed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust");
    copy(&from, dir.path());
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
