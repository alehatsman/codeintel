//! `codeintel schema` and `codeintel status`.
//!
//! These are `docs/plan.md` M4's done-when list for the two read-only verbs.
//! What they guard is a single failure mode: a catalogue that *declares* what
//! the index holds instead of *counting* it. A static list advertising sixteen
//! kinds when the index holds five is invariant 5 broken by the onboarding
//! text itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use codeintel::schema;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
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

fn stdout(root: &Path, args: &[&str]) -> String {
    let out = run(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Characters per token. A proxy, not a tokenizer: pulling one in costs a
/// dependency for a number that only has to be right to within a rule of
/// thumb. Dense tabular text runs worse than prose, so this is deliberately
/// the optimistic end and the assertion below leaves headroom for it.
const CHARS_PER_TOKEN: usize = 4;

#[test]
fn the_schema_fits_its_token_budget_with_the_rule_list_complete() {
    // `specs/05-surface.md` § `schema`: ~1500 tokens, measured, with every
    // rule listed. If it stops fitting, the standard library is too big — cut
    // rules, not the catalogue.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);

    let tokens = text.len() / CHARS_PER_TOKEN;
    assert!(tokens <= 1_500, "{tokens} tokens, {} chars", text.len());

    for rule in schema::rules(schema::STDLIB) {
        assert!(text.contains(&rule.head), "{} is not advertised", rule.head);
    }
}

#[test]
fn the_rule_list_matches_the_rules() {
    // The `%%` signature is written out, so it can drift from the clause below
    // it. Every advertised head must name a real predicate at its real arity,
    // and every predicate in the file must be advertised — otherwise `schema`
    // is a catalogue of a stdlib that no longer exists.
    let advertised: Vec<(String, usize)> = schema::rules(schema::STDLIB)
        .iter()
        .map(|r| {
            let (name, arity) =
                schema::split_head(&r.head).unwrap_or_else(|| panic!("`{}` is not a head", r.head));
            (name.to_string(), arity)
        })
        .collect();

    let mut defined: Vec<(String, usize)> = Vec::new();
    for line in schema::STDLIB.lines() {
        let head = line.split_once(":-").map_or(line, |(h, _)| h);
        let Some((name, args)) = head.split_once('(') else {
            continue;
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        let Some((args, _)) = args.rsplit_once(')') else {
            continue;
        };
        let row = (name.to_string(), args.split(',').count());
        if !defined.contains(&row) {
            defined.push(row);
        }
    }

    for rule in &advertised {
        assert!(
            defined.contains(rule),
            "`{}/{}` is advertised but not defined at that arity",
            rule.0,
            rule.1
        );
    }
    for rule in &defined {
        assert!(
            advertised.contains(rule),
            "`{}/{}` is defined but has no `%%` line, so `schema` hides it",
            rule.0,
            rule.1
        );
    }
    assert_eq!(advertised.len(), 37, "the rule count moved: {advertised:?}");
}

#[test]
fn schema_counts_come_from_this_index() {
    // The whole point: a kind at 0 is a kind the agent must not query, and it
    // is only knowable by counting.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);

    // The fixture is Rust, so it has functions and no classes.
    assert!(text.contains("class 0"), "{text}");
    assert!(!text.contains("function 0"), "{text}");
    // Tier A only: every reference is provenance `name`.
    assert!(text.contains("exact 0"), "{text}");
    assert!(text.contains("Lang  rust"), "{text}");
}

#[test]
fn schema_with_no_index_says_so_rather_than_reading_as_an_inventory() {
    // Zero everywhere is indistinguishable from "your code has none of these"
    // unless the output says which it is.
    let dir = tempfile::tempdir().expect("tempdir");
    let text = stdout(dir.path(), &["schema"]);
    assert!(text.contains("THERE IS NO INDEX HERE"), "{text}");
    assert!(text.contains("codeintel index ."), "{text}");
    // The catalogue is still there: an agent can learn the system before
    // indexing anything.
    assert!(text.contains("innermost_at"), "{text}");
}

#[test]
fn status_reports_per_language_counts_and_the_fingerprint() {
    // "python: 1,204 files, 11 defs" is visibly absurd to a human in one
    // second; `status: ok` is not (`docs/plan.md` M4).
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: ok"), "{text}");
    assert!(text.contains("extractor: blake3:"), "{text}");
    assert!(text.contains("rust "), "{text}");
    assert!(text.contains("defs,"), "{text}");
    assert!(text.contains("changed since index: none"), "{text}");
    assert!(text.contains("scip: none"), "{text}");
}

#[test]
fn status_json_is_the_bug_report_artifact() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");

    assert_eq!(json["status"], "ok");
    assert!(
        json["extractor_fingerprint"]
            .as_str()
            .is_some_and(|f| f.starts_with("blake3:")),
        "{json}"
    );
    assert!(json["languages"]["rust"]["defs"].as_u64().unwrap_or(0) > 0);
    // Every relation, including the ones at zero — an absent key would read as
    // "not measured" rather than "empty".
    for rel in facts::RELATIONS {
        assert!(
            json["relations"][rel.name].is_number(),
            "{} is missing from status json",
            rel.name
        );
    }
    assert!(json["scip"].as_array().is_some_and(Vec::is_empty));
}

#[test]
fn status_notices_a_file_that_changed_after_the_index() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    std::fs::write(dir.path().join("src/store.rs"), "pub fn added() {}\n").expect("write");

    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: stale"), "{text}");
    assert!(text.contains("src/store.rs"), "{text}");
}

#[test]
fn status_with_no_index_is_an_answer_not_a_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["status"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("status: no-index"), "{text}");
    assert!(text.contains("run: codeintel index ."), "{text}");
}
