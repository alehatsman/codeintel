//! The host seam: this crate supplies the dictionary and the regex engine, and
//! the standard library runs on top of both.

use codeintel::Regexes;
use datalog::atom::Atom;
use datalog::relation::Relation;
use datalog::{Engine, Limits};
use facts::Interner;
use tempfile::TempDir;

const BASE: [(&str, usize); 15] = [
    ("file", 2),
    ("def", 4),
    ("def_span", 5),
    ("def_name", 3),
    ("def_sig", 2),
    ("def_doc", 2),
    ("parent", 2),
    ("visibility", 2),
    ("resolved", 1),
    ("import", 3),
    ("scip_ref", 6),
    ("name_ref", 5),
    ("scip_impl", 2),
    ("name_impl", 4),
    ("extern", 4),
];

/// An engine over the real dictionary and the real regex engine, with the
/// shipped standard library loaded.
fn engine(dir: &TempDir) -> Engine {
    let interner = Interner::open(dir.path()).expect("a dictionary");
    let mut engine = Engine::new(Box::new(interner)).with_regexes(Box::new(Regexes::new()));
    for (name, arity) in BASE {
        engine.insert_relation(name, Relation::new(arity));
    }
    engine
        .load_rules(include_str!("../../../rules/stdlib.dl"))
        .expect("the shipped standard library loads");
    engine
}

fn install(engine: &mut Engine, name: &str, rows: &[&[&str]]) {
    let arity = rows.first().map_or(1, |r| r.len());
    let mut rel = Relation::new(arity);
    for row in rows {
        let atoms: Vec<Atom> = row
            .iter()
            .map(|t| {
                t.parse::<u32>()
                    .ok()
                    .or_else(|| engine.intern(t))
                    .unwrap_or(0)
            })
            .collect();
        assert!(
            rel.push(&atoms),
            "row {row:?} has the wrong width for {name}"
        );
    }
    engine.insert_relation(name, rel);
}

fn rows(engine: &Engine, result: &datalog::QueryResult) -> Vec<String> {
    result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|a| {
                    engine
                        .resolve(*a)
                        .map_or_else(|| a.to_string(), ToString::to_string)
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn the_match_builtin_runs_the_shipped_is_test_rules() {
    let dir = TempDir::new().expect("a temp dir");
    let mut engine = engine(&dir);
    install(
        &mut engine,
        "file",
        &[
            &["src/store.rs", "rust"],
            &["tests/store.rs", "rust"],
            &["src/store_test.go", "go"],
            &["web/app.test.ts", "typescript"],
            &["src/tests_helper.rs", "rust"],
        ],
    );

    let result = engine
        .query("?- is_test(F).", &Limits::default())
        .expect("answers");
    let mut found = rows(&engine, &result);
    found.sort();
    assert_eq!(
        found,
        vec![
            "src/store_test.go".to_string(),
            "tests/store.rs".to_string(),
            "web/app.test.ts".to_string(),
        ]
    );
}

#[test]
fn an_invalid_pattern_reports_the_regex_compilers_own_diagnostic() {
    let dir = TempDir::new().expect("a temp dir");
    let mut engine = engine(&dir);
    install(&mut engine, "file", &[&["src/a.rs", "rust"]]);

    let diagnostic = engine
        .query(
            r#"?- file(F, _), match(F, "(unclosed")."#,
            &Limits::default(),
        )
        .expect_err("rejected");
    assert!(diagnostic.message.contains("invalid regex"), "{diagnostic}");
    assert!(
        diagnostic.message.contains("unclosed group"),
        "{diagnostic}"
    );
}

#[test]
fn a_query_does_not_write_to_the_dictionary() {
    let dir = TempDir::new().expect("a temp dir");
    let mut engine = engine(&dir);
    install(&mut engine, "file", &[&["src/a.rs", "rust"]]);
    engine
        .query(r#"?- file("a-path-no-corpus-has", L)."#, &Limits::default())
        .expect("answers");
    assert!(
        !dir.path().join("dict.bin").exists(),
        "the query path is read-only on disk"
    );
}

#[test]
fn the_location_bridge_answers_over_the_real_host() {
    let dir = TempDir::new().expect("a temp dir");
    let mut engine = engine(&dir);
    install(&mut engine, "file", &[&["src/store.rs", "rust"]]);
    install(
        &mut engine,
        "def",
        &[
            &["Store#", "src/store.rs", "struct", "Store"],
            &["Store#get().", "src/store.rs", "method", "get"],
        ],
    );
    install(
        &mut engine,
        "def_span",
        &[
            &["Store#", "1", "50", "0", "1000"],
            &["Store#get().", "10", "20", "100", "400"],
        ],
    );

    let result = engine
        .query(
            r#"?- innermost_at("src/store.rs", 15, S)."#,
            &Limits::default(),
        )
        .expect("answers");
    assert_eq!(rows(&engine, &result), vec!["Store#get().".to_string()]);
}
