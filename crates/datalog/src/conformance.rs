//! Engine conformance: synthetic programs with hand-computed answers, and the
//! differential test that keeps semi-naive honest.
//!
//! No code-intel facts appear anywhere in this file. That is the point — the
//! engine has to be provably correct before code-intel facts can hide a bug
//! in it.

use crate::engine::{Engine, QueryResult};
use crate::limits::Limits;
use crate::symbols::Strings;

/// An engine over `program`, with no regex engine installed.
fn engine(program: &str) -> Engine {
    let mut e = Engine::new(Box::new(Strings::new()));
    e.load_rules(program).expect("the program loads");
    e
}

/// Rows rendered as text, in result order: the comparable form.
fn render(engine: &Engine, result: &QueryResult) -> Vec<String> {
    result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|a| {
                    engine
                        .resolve(*a)
                        .map_or_else(|| a.to_string(), |s| format!("\"{s}\""))
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Run a query both ways and assert they agree; return the semi-naive answer.
///
/// This is the acceptance gate of `docs/plan.md` M1: a semi-naive bug that
/// drops rows is otherwise invisible until it corrupts a real answer.
fn both(program: &str, query: &str) -> Vec<String> {
    let mut fast = engine(program);
    let quick = fast
        .query(query, &Limits::default())
        .expect("semi-naive answers");
    let mut slow = engine(program);
    let plain = slow
        .query_naive(query, &Limits::default())
        .expect("naive answers");

    let a = render(&fast, &quick);
    let b = render(&slow, &plain);
    assert_eq!(a, b, "semi-naive and naive disagree on {query:?}");
    assert_eq!(quick.columns, plain.columns);
    a
}

// ── the programs ────────────────────────────────────────────────────────────

/// A five-node path with one back edge, so the closure is not a tree.
const GRAPH: &str = r#"
edge("a", "b").
edge("b", "c").
edge("c", "d").
edge("d", "e").
edge("e", "b").
reaches(X, Y) :- edge(X, Y).
reaches(X, Z) :- reaches(X, Y), edge(Y, Z).
"#;

const SAME_GENERATION: &str = r#"
parent("a", "b").
parent("a", "c").
parent("b", "d").
parent("c", "e").
parent("d", "f").
parent("e", "g").
sg(X, Y) :- parent(P, X), parent(P, Y), X != Y.
sg(X, Y) :- parent(P, X), parent(Q, Y), sg(P, Q).
"#;

const NEGATION: &str = r#"
node("a"). node("b"). node("c"). node("d").
edge("a", "b").
edge("b", "c").
reachable(X) :- edge(_, X).
orphan(X) :- node(X), !reachable(X).
"#;

const NUMBERS: &str = r#"
span("f", 10, 40).
span("g", 5, 9).
span("h", 100, 260).
width(S, W) :- span(S, A, B), W = B - A.
long(S, W) :- width(S, W), W > 30.
"#;

#[test]
fn transitive_closure() {
    let rows = both(GRAPH, r#"?- reaches("a", X)."#);
    assert_eq!(rows, vec!["\"b\"", "\"c\"", "\"d\"", "\"e\""]);
}

#[test]
fn transitive_closure_finds_the_cycle() {
    let rows = both(GRAPH, r#"?- reaches("b", "b")."#);
    assert_eq!(
        rows.len(),
        1,
        "b reaches itself through the back edge: {rows:?}"
    );
}

#[test]
fn transitive_closure_unseeded_is_every_pair_that_exists() {
    let rows = both(GRAPH, "?- reaches(X, Y).");
    // a reaches b,c,d,e; each of b,c,d,e reaches all four of b,c,d,e.
    assert_eq!(rows.len(), 4 + 16);
}

#[test]
fn same_generation() {
    let rows = both(SAME_GENERATION, r#"?- sg("f", X)."#);
    assert_eq!(rows, vec!["\"g\""]);
}

#[test]
fn stratified_negation() {
    let rows = both(NEGATION, "?- orphan(X).");
    assert_eq!(rows, vec!["\"a\"", "\"d\""]);
}

#[test]
fn negation_sees_the_completed_stratum_not_a_partial_one() {
    // If `reachable` were read mid-fixpoint, `c` would appear as an orphan.
    let rows = both(NEGATION, r#"?- orphan("c")."#);
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn arithmetic() {
    let rows = both(NUMBERS, "?- width(S, W).");
    assert_eq!(rows, vec!["\"f\" 30", "\"g\" 4", "\"h\" 160"]);
}

#[test]
fn comparison_filters_on_integers() {
    let rows = both(NUMBERS, "?- long(S, W).");
    assert_eq!(rows, vec!["\"h\" 160"]);
}

#[test]
fn count_counts_distinct_bindings() {
    let program = r#"
edge("a", "b").
edge("a", "c").
edge("a", "b").
edge("b", "c").
out(X, N) :- edge(X, _), N = count{ Y : edge(X, Y) }.
"#;
    let rows = both(program, "?- out(X, N).");
    assert_eq!(rows, vec!["\"a\" 2", "\"b\" 1"]);
}

#[test]
fn count_is_zero_when_the_goal_is_empty() {
    let program = r#"
node("a").
edge("b", "c").
fanout(X, N) :- node(X), N = count{ Y : edge(X, Y) }.
"#;
    let rows = both(program, "?- fanout(X, N).");
    assert_eq!(rows, vec!["\"a\" 0"]);
}

#[test]
fn between_generates_a_range() {
    let program = r#"
span("f", 3, 6).
line(S, L) :- span(S, A, B), between(A, B, L).
"#;
    let rows = both(program, "?- line(S, L).");
    assert_eq!(rows, vec!["\"f\" 3", "\"f\" 4", "\"f\" 5", "\"f\" 6"]);
}

#[test]
fn between_degrades_to_a_range_check_when_its_output_is_bound() {
    let program = r#"
span("f", 3, 6).
line(S, L) :- span(S, A, B), between(A, B, L).
"#;
    assert_eq!(both(program, "?- line(S, 4).").len(), 1);
    assert!(both(program, "?- line(S, 9).").is_empty());
}

#[test]
fn between_with_a_reversed_range_yields_nothing() {
    let program = r#"
span("f", 6, 3).
line(S, L) :- span(S, A, B), between(A, B, L).
"#;
    assert!(both(program, "?- line(S, L).").is_empty());
}

#[test]
fn the_string_tests_that_need_no_regex_engine() {
    let program = r#"
file("src/store.rs").
file("tests/store_test.rs").
file("README.md").
rust(F) :- file(F), suffix(F, ".rs").
under_src(F) :- file(F), prefix(F, "src/").
mentions_store(F) :- file(F), contains(F, "store").
"#;
    assert_eq!(both(program, "?- rust(F).").len(), 2);
    assert_eq!(both(program, "?- under_src(F).").len(), 1);
    assert_eq!(both(program, "?- mentions_store(F).").len(), 2);
}

#[test]
fn a_repeated_variable_in_one_literal_must_agree_with_itself() {
    let program = r#"
edge("a", "a").
edge("a", "b").
loop(X) :- edge(X, X).
"#;
    assert_eq!(both(program, "?- loop(X)."), vec!["\"a\""]);
}

#[test]
fn an_integer_and_the_string_that_spells_it_are_different_atoms() {
    let program = r#"
value("n", 42).
value("s", "42").
"#;
    // The constant is not a column, so the answer is the key alone.
    assert_eq!(both(program, "?- value(K, 42)."), vec!["\"n\""]);
    assert_eq!(both(program, r#"?- value(K, "42")."#), vec!["\"s\""]);
}

#[test]
fn results_are_deduplicated_and_ordered_by_term() {
    let program = r#"
edge("c", "a").
edge("a", "b").
edge("c", "a").
out(X) :- edge(X, _).
"#;
    // Ordering is by atom, which is dictionary order, not lexicographic order:
    // `"c"` was interned first because it appears first in the program. That is
    // what `specs/03-datalog.md` § Determinism specifies — total and explicit —
    // and rendering for humans is the surface layer's job.
    assert_eq!(both(program, "?- out(X)."), vec!["\"c\"", "\"a\""]);
    assert_eq!(
        both(program, "?- out(X).").len(),
        2,
        "the duplicate edge is one row"
    );
}

#[test]
fn a_query_may_define_its_own_rules() {
    let mut e = engine(GRAPH);
    let result = e
        .query(
            "two_step(X, Z) :- edge(X, Y), edge(Y, Z). ?- two_step(\"a\", Z).",
            &Limits::default(),
        )
        .expect("answers");
    assert_eq!(render(&e, &result), vec!["\"c\""]);
}

#[test]
fn a_query_rule_shadows_a_loaded_rule_and_says_so() {
    let mut e = engine(GRAPH);
    let result = e
        .query(
            "reaches(X, Y) :- edge(X, Y). ?- reaches(\"a\", Y).",
            &Limits::default(),
        )
        .expect("answers");
    assert_eq!(
        render(&e, &result),
        vec!["\"b\""],
        "the one-hop definition won"
    );
    assert_eq!(result.stats.shadowed, vec!["reaches/2".to_string()]);
}

#[test]
fn columns_are_the_goal_variables_in_order_of_first_appearance() {
    let mut e = engine(GRAPH);
    let result = e
        .query("?- edge(X, Y), edge(Y, Z).", &Limits::default())
        .expect("answers");
    assert_eq!(
        result.columns,
        vec!["X".to_string(), "Y".to_string(), "Z".to_string()]
    );
}

#[test]
fn an_empty_answer_is_an_answer_not_an_error() {
    let mut e = engine(GRAPH);
    let result = e
        .query(r#"?- edge("z", X)."#, &Limits::default())
        .expect("answers");
    assert!(result.rows.is_empty());
    assert!(!result.truncated);
    assert_eq!(result.cap, None);
}

#[test]
fn a_source_with_no_query_says_so() {
    let mut e = engine(GRAPH);
    let diagnostic = e
        .query("r(X) :- edge(X, _).", &Limits::default())
        .expect_err("rejected");
    assert!(diagnostic.message.contains("no query"), "{diagnostic}");
}

#[test]
fn every_conformance_query_is_byte_identical_across_a_hundred_runs() {
    let cases = [
        (GRAPH, "?- reaches(X, Y)."),
        (SAME_GENERATION, "?- sg(X, Y)."),
        (NEGATION, "?- orphan(X)."),
        (NUMBERS, "?- width(S, W)."),
    ];
    for (program, query) in cases {
        let mut first: Option<Vec<String>> = None;
        for run in 0..100 {
            let mut e = engine(program);
            let result = e.query(query, &Limits::default()).expect("answers");
            let rendered = render(&e, &result);
            match &first {
                None => first = Some(rendered),
                Some(expected) => assert_eq!(*expected, rendered, "run {run} of {query:?} differs"),
            }
        }
    }
}

#[test]
fn a_ground_query_answers_yes_with_one_empty_row_and_no_with_none() {
    let mut e = engine(GRAPH);
    let yes = e
        .query(r#"?- edge("a", "b")."#, &Limits::default())
        .expect("answers");
    assert_eq!(yes.columns, Vec::<String>::new());
    assert_eq!(yes.rows.len(), 1);
    assert!(yes.rows.first().is_some_and(Vec::is_empty));

    let no = e
        .query(r#"?- edge("b", "a")."#, &Limits::default())
        .expect("answers");
    assert!(no.rows.is_empty());
}

// ── limits ──────────────────────────────────────────────────────────────────

#[test]
fn max_result_rows_truncates_and_names_the_cap() {
    let limits = Limits {
        max_result_rows: 3,
        ..Limits::default()
    };
    let mut e = engine(GRAPH);
    let result = e.query("?- reaches(X, Y).", &limits).expect("answers");
    assert_eq!(result.rows.len(), 3);
    assert!(result.truncated);
    assert_eq!(result.cap, Some("max_result_rows"));
}

#[test]
fn a_truncated_result_is_a_stable_prefix() {
    let limits = Limits {
        max_result_rows: 3,
        ..Limits::default()
    };
    let mut e = engine(GRAPH);
    let first = e.query("?- reaches(X, Y).", &limits).expect("answers");
    let full = e
        .query("?- reaches(X, Y).", &Limits::default())
        .expect("answers");
    assert_eq!(first.rows, full.rows.get(..3).unwrap_or_default());
}

#[test]
fn max_result_bytes_truncates_at_a_row_boundary() {
    let limits = Limits {
        max_result_bytes: 12,
        ..Limits::default()
    };
    let mut e = engine(GRAPH);
    let result = e.query("?- reaches(X, Y).", &limits).expect("answers");
    assert!(result.truncated);
    assert_eq!(result.cap, Some("max_result_bytes"));
    assert!(
        !result.rows.is_empty(),
        "a byte cap that fits nothing is a different defect"
    );
}

#[test]
fn max_derived_tuples_aborts_with_budget_exceeded() {
    let limits = Limits {
        max_derived_tuples: 5,
        ..Limits::default()
    };
    let mut e = engine(GRAPH);
    let diagnostic = e.query("?- reaches(X, Y).", &limits).expect_err("aborts");
    assert_eq!(diagnostic.status, crate::diag::Status::BudgetExceeded);
    assert!(
        diagnostic.message.contains("max_derived_tuples"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.message.contains("seeded"),
        "the hint names the fix: {diagnostic}"
    );
}

#[test]
fn max_time_ms_aborts_with_timeout() {
    let limits = Limits {
        max_time_ms: 0,
        ..Limits::default()
    };
    let mut e = engine(GRAPH);
    let diagnostic = e.query("?- reaches(X, Y).", &limits).expect_err("aborts");
    assert_eq!(diagnostic.status, crate::diag::Status::Timeout);
    assert!(diagnostic.message.contains("max_time_ms"), "{diagnostic}");
}

#[test]
fn max_body_literals_is_rejected_at_planning() {
    let limits = Limits {
        max_body_literals: 2,
        ..Limits::default()
    };
    let mut e = engine(GRAPH);
    let diagnostic = e
        .query("?- edge(A, B), edge(B, C), edge(C, D).", &limits)
        .expect_err("rejected");
    assert_eq!(diagnostic.status, crate::diag::Status::InvalidQuery);
    assert!(
        diagnostic.message.contains("max_body_literals"),
        "{diagnostic}"
    );
}

#[test]
fn max_strata_is_rejected_at_planning() {
    let limits = Limits {
        max_strata: 1,
        ..Limits::default()
    };
    let mut e = engine(NEGATION);
    let diagnostic = e.query("?- orphan(X).", &limits).expect_err("rejected");
    assert!(diagnostic.message.contains("max_strata"), "{diagnostic}");
}

#[test]
fn stats_report_what_the_evaluation_cost() {
    let mut e = engine(GRAPH);
    let result = e
        .query("?- reaches(X, Y).", &Limits::default())
        .expect("answers");
    assert!(result.stats.derived > 0);
    assert!(result.stats.strata >= 1);
    assert!(!result.stats.plan.is_empty());
}

// ── runtime type errors the safety rules cannot catch ───────────────────────

#[test]
fn ordering_two_string_atoms_is_an_error_not_a_byte_comparison() {
    let program = r#"
name("a", "zed").
name("b", "amy").
"#;
    let mut e = engine(program);
    let diagnostic = e
        .query("?- name(K, N), N < \"m\".", &Limits::default())
        .expect_err("rejected");
    assert!(
        diagnostic.message.contains("compares integers"),
        "{diagnostic}"
    );
    assert!(diagnostic.message.contains("between runs"), "{diagnostic}");
}

#[test]
fn division_by_zero_is_an_error() {
    let program = "v(4, 0).";
    let mut e = engine(program);
    let diagnostic = e
        .query("?- v(A, B), N = A / B.", &Limits::default())
        .expect_err("rejected");
    assert!(
        diagnostic.message.contains("division by zero"),
        "{diagnostic}"
    );
}

#[test]
fn arithmetic_below_zero_is_an_error_naming_the_range() {
    let program = "v(1, 4).";
    let mut e = engine(program);
    let diagnostic = e
        .query("?- v(A, B), N = A - B.", &Limits::default())
        .expect_err("rejected");
    assert!(
        diagnostic.message.contains("outside the supported range"),
        "{diagnostic}"
    );
}

#[test]
fn match_without_a_regex_engine_says_which_builtins_still_work() {
    let program = r#"file("src/a.rs")."#;
    let mut e = engine(program);
    let diagnostic = e
        .query(r#"?- file(F), match(F, "^src/")."#, &Limits::default())
        .expect_err("rejected");
    assert!(diagnostic.message.contains("regex engine"), "{diagnostic}");
    assert!(diagnostic.message.contains("prefix"), "{diagnostic}");
}
