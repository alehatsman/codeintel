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
    both_in(|| engine(program), query).0
}

/// [`both`] over any engine builder, for programs with installed relations.
///
/// The fast side is the real thing: semi-naive over the demand-rewritten
/// program. The slow side is naive over the program as written, so a rewrite
/// that drops a seed — which the fast side alone cannot notice — disagrees
/// here. Returns the fast answer and its result, for assertions on `stats`.
fn both_in(make: impl Fn() -> Engine, query: &str) -> (Vec<String>, QueryResult) {
    let mut fast = make();
    let quick = fast
        .query(query, &Limits::default())
        .expect("semi-naive answers");
    let mut slow = make();
    let plain = slow
        .query_naive(query, &Limits::default())
        .expect("naive answers");

    let a = render(&fast, &quick);
    let b = render(&slow, &plain);
    assert_eq!(a, b, "semi-naive and naive disagree on {query:?}");
    assert_eq!(quick.columns, plain.columns);
    (a, quick)
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

// ── demand transformation ───────────────────────────────────────────────────

/// A wide graph: 12 roots, each a three-hop chain. Seeding from one root must
/// not cost the other eleven.
const WIDE: &str = r#"
calls("a1", "a0"). calls("a2", "a1"). calls("a3", "a2").
calls("b1", "b0"). calls("b2", "b1"). calls("b3", "b2").
calls("c1", "c0"). calls("c2", "c1"). calls("c3", "c2").
calls("d1", "d0"). calls("d2", "d1"). calls("d3", "d2").
impact_of(Seed, C) :- calls(C, Seed).
impact_of(Seed, C) :- impact_of(Seed, B), calls(C, B).
at(S, L) :- calls(S, _), L = 1.
"#;

/// Answers must be identical with and without the transformation, and the
/// transformed run must derive strictly fewer tuples. Equal counts mean it
/// silently did not apply — the failure that otherwise surfaces only as an
/// unexplained timeout on a big repo.
fn demand_applies(program: &str, query: &str) -> Vec<String> {
    let mut fast = engine(program);
    let with = fast.query(query, &Limits::default()).expect("answers");
    let mut slow = engine(program);
    let without = slow
        .query_undemanded(query, &Limits::default())
        .expect("answers");

    assert_eq!(
        render(&fast, &with),
        render(&slow, &without),
        "{query:?} changed answer"
    );
    assert!(
        !with.stats.transformed.is_empty(),
        "{query:?}: nothing was adorned, so the transformation did not apply"
    );
    assert!(
        with.stats.derived < without.stats.derived,
        "{query:?}: derived {} with the transformation and {} without — equal or worse means it \
         did not apply",
        with.stats.derived,
        without.stats.derived
    );
    render(&fast, &with)
}

#[test]
fn demand_transformation_applies_to_a_constant_seed() {
    let rows = demand_applies(WIDE, r#"?- impact_of("a0", C)."#);
    assert_eq!(rows, vec!["\"a1\"", "\"a2\"", "\"a3\""]);
}

#[test]
fn demand_transformation_binds_the_seed_from_an_earlier_literal() {
    // The headline pattern: the seed comes sideways, not from a goal constant.
    // A transformation scoped to goal constants does not handle this.
    let rows = demand_applies(WIDE, r#"?- calls("a1", S), impact_of(S, C)."#);
    assert_eq!(
        rows,
        vec!["\"a0\" \"a1\"", "\"a0\" \"a2\"", "\"a0\" \"a3\""]
    );
}

#[test]
fn demand_transformation_applies_to_a_non_recursive_predicate() {
    // `symbol_at` in miniature: non-recursive, and the majority of the win.
    let program = r#"
span("f", "src/a.rs", 1, 200).
span("g", "src/a.rs", 40, 60).
span("h", "src/b.rs", 1, 90).
symbol_at(F, Line, S) :- span(S, F, A, B), between(A, B, Line).
"#;
    let rows = demand_applies(program, r#"?- symbol_at("src/a.rs", 50, S)."#);
    assert_eq!(rows, vec!["\"f\"", "\"g\""]);
}

#[test]
fn a_fully_free_goal_is_left_alone() {
    let mut e = engine(WIDE);
    let result = e
        .query("?- impact_of(S, C).", &Limits::default())
        .expect("answers");
    assert!(
        result.stats.transformed.is_empty(),
        "nothing to propagate: {:?}",
        result.stats
    );
}

#[test]
fn demand_transformation_does_not_change_a_negated_answer() {
    // A derived predicate under `!` is requested unrestricted on purpose:
    // restricting it would change the answer rather than the cost.
    let program = r#"
node("a"). node("b"). node("c").
edge("a", "b").
reaches(X, Y) :- edge(X, Y).
reaches(X, Z) :- reaches(X, Y), edge(Y, Z).
unreached(X) :- node(X), !reaches(_, X).
"#;
    assert_eq!(both(program, "?- unreached(X)."), vec!["\"a\"", "\"c\""]);
}

#[test]
fn demand_transformation_agrees_with_naive_evaluation() {
    for query in [
        r#"?- impact_of("a0", C)."#,
        r#"?- calls("a1", S), impact_of(S, C)."#,
        "?- impact_of(S, C).",
    ] {
        both(WIDE, query);
    }
}

/// Aggregates over recursion, seeded and nested. `GRAPH`'s closure, counted.
const COUNTED: &str = r#"
edge("a", "b").
edge("b", "c").
edge("c", "d").
edge("d", "e").
edge("e", "b").
node(X) :- edge(X, _).
node(Y) :- edge(_, Y).
reaches(X, Y) :- edge(X, Y).
reaches(X, Z) :- reaches(X, Y), edge(Y, Z).
fanout(X, N) :- node(X), N = count{ Y : reaches(X, Y) }.
wide(X) :- fanout(X, N), N > 3.
deep(X, N) :- node(X), N = count{ Y : edge(X, Y), M = count{ Z : reaches(Y, Z) }, M > 3 }.
"#;

/// An aggregate over a recursive relation, alone, seeded, and nested — the
/// aggregate + demand combinations that `both` had never seen.
#[test]
fn aggregates_over_recursion_agree_with_naive_evaluation() {
    // Unseeded: the whole closure, counted per node.
    assert_eq!(
        both(COUNTED, "?- fanout(X, N)."),
        vec!["\"a\" 4", "\"b\" 4", "\"c\" 4", "\"d\" 4", "\"e\" 4"]
    );
    // Seeded: the rewrite requests `reaches` plain from inside the `count{}`.
    let (rows, result) = both_in(|| engine(COUNTED), r#"?- fanout("a", N)."#);
    assert_eq!(rows, vec!["4"]);
    assert!(
        result.stats.transformed.iter().any(|n| n == "fanout@bf"),
        "{:?}",
        result.stats.transformed
    );
    // Seeded through a derived predicate that filters on the count.
    let (rows, _) = both_in(|| engine(COUNTED), r#"?- wide("a")."#);
    assert_eq!(rows, vec![""], "a ground goal that holds is one empty row");
    // Nested: `reaches` appears only inside the inner `count{}`. The rewrite
    // has to request it from there, or the rewrite is dropped and the seeded
    // query silently pays for the whole closure.
    let (rows, result) = both_in(|| engine(COUNTED), r#"?- deep("a", N)."#);
    assert_eq!(
        rows,
        vec!["1"],
        "a's one edge target, b, reaches four nodes"
    );
    assert!(
        result.stats.transformed.iter().any(|n| n == "deep@bf"),
        "the nested aggregate dropped the rewrite: {:?}",
        result.stats.transformed
    );
}

/// The rewrite adds a guard literal to every body it touches, so a program
/// exactly at `max_body_literals` as written is over it rewritten. The limits
/// bound what runs: the plain program runs instead, and the answer is the
/// same.
#[test]
fn a_rewrite_over_the_planning_limits_falls_back_to_the_plain_program() {
    // `impact_of`'s recursive body has two literals as written, three guarded.
    let limits = Limits {
        max_body_literals: 2,
        ..Limits::default()
    };
    let mut e = engine(WIDE);
    let result = e
        .query(r#"?- impact_of("a0", C)."#, &limits)
        .expect("the plain program is within the limit");
    assert!(
        result.stats.transformed.is_empty(),
        "{:?}",
        result.stats.transformed
    );
    assert_eq!(render(&e, &result), vec!["\"a1\"", "\"a2\"", "\"a3\""]);
}

// ── the standard library ────────────────────────────────────────────────────

/// The 15 base relations of `specs/01-facts.md`, empty.
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

/// Compiles every pattern to one that matches nothing.
///
/// The engine takes its regex engine from the host, and this file must not
/// take a dependency. Nothing here holds facts, so no `match` ever decides an
/// answer — this stands in for the seam, not for `regex`.
#[derive(Debug)]
struct NoMatches;

#[derive(Debug)]
struct NeverMatches;

impl crate::matcher::Matcher for NeverMatches {
    fn is_match(&self, _text: &str) -> bool {
        false
    }
}

impl crate::matcher::Regexes for NoMatches {
    fn compile(&self, _pattern: &str) -> Result<Box<dyn crate::matcher::Matcher>, String> {
        Ok(Box::new(NeverMatches))
    }
}

fn stdlib_engine() -> Engine {
    let mut e = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(NoMatches));
    for (name, arity) in BASE {
        e.insert_relation(name, crate::relation::Relation::new(arity));
    }
    e
}

/// The engine must accept its own standard library, **as written**, with no
/// transformation applied. A rule that is legal only after demand
/// transformation would leave the differential test with nothing to compare
/// against — which is the failure this one line guards.
#[test]
fn the_engine_loads_its_own_standard_library() {
    let mut e = stdlib_engine();
    e.load_rules(include_str!("../../../rules/stdlib.dl"))
        .expect("stdlib loads");
    assert!(e.rule_count() > 30, "loaded {} rules", e.rule_count());
}

#[test]
fn the_naive_evaluator_accepts_the_standard_library_too() {
    let mut e = stdlib_engine();
    e.load_rules(include_str!("../../../rules/stdlib.dl"))
        .expect("stdlib loads");
    let result = e
        .query_naive(r#"?- def(S, F, "function", N)."#, &Limits::default())
        .expect("naive evaluation runs the stdlib as written");
    assert!(result.rows.is_empty(), "no facts were loaded");
}

/// Install a base relation from text rows. A token that parses as a
/// non-negative integer becomes an integer atom, everything else a string —
/// the same split `specs/01-facts.md` § Integers describes.
fn install(e: &mut Engine, name: &str, rows: &[&[&str]]) {
    let arity = rows.first().map_or(1, |r| r.len());
    let mut rel = crate::relation::Relation::new(arity);
    for row in rows {
        let atoms: Vec<crate::atom::Atom> = row
            .iter()
            .map(|token| {
                token
                    .parse::<u32>()
                    .ok()
                    .filter(|n| crate::atom::is_int(*n))
                    .or_else(|| e.intern(token))
                    .unwrap_or(0)
            })
            .collect();
        assert!(
            rel.push(&atoms),
            "row {row:?} has the wrong width for {name}"
        );
    }
    e.insert_relation(name, rel);
}

/// One struct with two methods, one of which calls the other by name. Small
/// enough to hand-verify, shaped enough to exercise the location bridge,
/// containment, name resolution and the seeded traversals.
fn store_fixture() -> Engine {
    let mut e = stdlib_engine();
    install(&mut e, "file", &[&["src/store.rs", "rust"]]);
    install(
        &mut e,
        "def",
        &[
            &["S#", "src/store.rs", "struct", "Store"],
            &["S#get().", "src/store.rs", "method", "get"],
            &["S#put().", "src/store.rs", "method", "put"],
        ],
    );
    install(
        &mut e,
        "def_span",
        &[
            &["S#", "1", "50", "0", "1000"],
            &["S#get().", "10", "20", "100", "400"],
            &["S#put().", "30", "40", "500", "900"],
        ],
    );
    install(
        &mut e,
        "parent",
        &[
            &["S#", "src/store.rs"],
            &["S#get().", "S#"],
            &["S#put().", "S#"],
        ],
    );
    // `put` calls `get` by name, in the same file, so rule 1 of `ref` resolves
    // it without SCIP and the call graph has one edge.
    install(
        &mut e,
        "name_ref",
        &[&["get", "src/store.rs", "35", "8", "S#put()."]],
    );
    e.load_rules(include_str!("../../../rules/stdlib.dl"))
        .expect("stdlib loads");
    e
}

#[test]
fn the_standard_library_answers_over_hand_written_facts() {
    // The location bridge: a file:line from a stack trace becomes a symbol.
    let (rows, _) = both_in(store_fixture, r#"?- innermost_at("src/store.rs", 15, S)."#);
    assert_eq!(rows, vec!["\"S#get().\""], "the tightest span wins");

    // ...and the line that only the struct covers still resolves to the struct.
    let (rows, _) = both_in(store_fixture, r#"?- innermost_at("src/store.rs", 5, S)."#);
    assert_eq!(rows, vec!["\"S#\""]);

    // Containment closure. Ordering is by atom — dictionary order, which is
    // the order the facts above interned their strings in, not lexicographic
    // order.
    let (rows, _) = both_in(store_fixture, r#"?- within("S#get().", A)."#);
    assert_eq!(rows, vec!["\"src/store.rs\"", "\"S#\""]);

    // Orientation, one round trip.
    let (rows, _) = both_in(store_fixture, r#"?- about("S#get().", Rel, A, B, L)."#);
    assert!(
        rows.iter().any(|row| row.contains("\"parent\"")),
        "about reports the parent: {rows:?}"
    );

    // A span question, with arithmetic.
    let (rows, _) = both_in(store_fixture, "?- long_def(S).");
    assert!(rows.is_empty(), "no definition here is over 80 lines");
}

/// The combinations `specs/03-datalog.md` § Validation 5 promises and the
/// two-rule programs above cannot reach: a seeded ground negation, and the
/// negation-inside-recursion back-off that `impact_of` takes. Each is run
/// against naive evaluation of the library as written, so a rewrite that drops
/// a row is a disagreement, not a fast wrong answer.
#[test]
fn the_seeded_stdlib_traversals_agree_with_naive_evaluation() {
    // Seeded ground negation: `!tighter_at(F, L, S)` with every column bound.
    let (rows, result) = both_in(store_fixture, r#"?- innermost_at("src/store.rs", 15, S)."#);
    assert_eq!(rows, vec!["\"S#get().\""]);
    assert!(
        result
            .stats
            .transformed
            .iter()
            .any(|n| n == "tighter_at@bbb"),
        "{:?}",
        result.stats.transformed
    );

    // Negation + demand + recursion: seeding `!ambiguous` makes the rewrite
    // unstratifiable, the engine backs it off, and the answer must survive.
    let (rows, result) = both_in(store_fixture, r#"?- def(S, _, _, "get"), impact_of(S, C)."#);
    assert_eq!(rows, vec!["\"S#get().\" \"S#put().\""], "put calls get");
    assert!(
        result.stats.transformed.iter().any(|n| n == "impact_of@bf"),
        "{:?}",
        result.stats.transformed
    );
    assert!(
        !result
            .stats
            .transformed
            .iter()
            .any(|n| n.starts_with("ambiguous@")),
        "{:?}",
        result.stats.transformed
    );

    // Both seeds in one query, which is where the back-off has to be per
    // predicate to keep either.
    let (rows, _) = both_in(
        store_fixture,
        r#"?- innermost_at("src/store.rs", 35, S), impact_of(S, C)."#,
    );
    assert_eq!(rows, vec![] as Vec<String>, "nothing calls put");
    let (rows, _) = both_in(store_fixture, r#"?- reach_of("S#put().", C)."#);
    assert_eq!(rows, vec!["\"S#get().\""]);
}

/// Demand transformation must apply **against the shipped rule library**, not
/// only against a two-rule test program.
///
/// This is the gate `docs/plan.md` M1 asked for, widened by what M2 found: the
/// transformation is validated after rewriting and falls back to the plain
/// program when the rewrite does not stratify, so a rewrite that stops applying
/// is silent — the query still answers, it just answers by computing all-pairs
/// reachability first. On a real repository that is a timeout rather than a
/// wrong answer, which is why nothing caught it until there was a real
/// repository.
#[test]
fn demand_transformation_applies_over_the_standard_library() {
    let mut e = stdlib_engine();
    e.load_rules(include_str!("../../../rules/stdlib.dl"))
        .expect("stdlib loads");

    for (query, wanted) in [
        (
            r#"?- innermost_at("src/store.rs", 15, S)."#,
            "innermost_at@bbf",
        ),
        (
            r#"?- def(S, _, _, "get"), impact_of(S, C)."#,
            "impact_of@bf",
        ),
        (
            r#"?- innermost_at("src/store.rs", 15, S), impact_of(S, C)."#,
            "impact_of@bf",
        ),
        // Both at once. Seeding `!tighter_at` is what breaks `impact_of`'s
        // stratification, so this is the query that proves the back-off is per
        // predicate and not per program: before it was, this pair fell all the
        // way back to the plain program and computed all-pairs reachability.
        (
            r#"?- innermost_at("src/store.rs", 15, S), impact_of(S, C)."#,
            "tighter_at@bbb",
        ),
        (r#"?- reach_of("S#get().", C)."#, "reach_of@bf"),
    ] {
        let result = e.query(query, &Limits::default()).expect("answers");
        assert!(
            result.stats.transformed.iter().any(|n| n == wanted),
            "{query}\n  wanted {wanted} among {:?}",
            result.stats.transformed
        );
    }
}

/// Ground-negation seeding is what makes `innermost_at` cheap; backing it off
/// for `ambiguous` is what keeps `impact_of` stratifiable. Assert both, so
/// losing either is a test failure rather than a slow query.
#[test]
fn the_negation_backoff_keeps_what_it_can() {
    let mut e = stdlib_engine();
    e.load_rules(include_str!("../../../rules/stdlib.dl"))
        .expect("stdlib loads");

    let seeded = e
        .query(
            r#"?- innermost_at("src/store.rs", 15, S)."#,
            &Limits::default(),
        )
        .expect("answers");
    assert!(
        seeded
            .stats
            .transformed
            .iter()
            .any(|n| n == "tighter_at@bbb"),
        "the negated literal was not seeded: {:?}",
        seeded.stats.transformed
    );

    let fell_back = e
        .query(
            r#"?- def(S, _, _, "get"), impact_of(S, C)."#,
            &Limits::default(),
        )
        .expect("answers");
    assert!(
        !fell_back
            .stats
            .transformed
            .iter()
            .any(|n| n.starts_with("ambiguous@")),
        "seeding a negated literal here makes the program unstratifiable: {:?}",
        fell_back.stats.transformed
    );
}

/// `Stats::empty_at` names the body literal that stopped the join, and it must
/// keep naming an *outer* literal when the body also contains an aggregate.
///
/// `count{}` evaluates a sub-goal through the same recursion, so a shared
/// high-water mark lets the inner body's depth overwrite the outer one. The
/// number then indexes the outer plan out of range — the diagnostic silently
/// degrades — or, with a shorter inner goal, names a literal that was never
/// reached, which is worse than saying nothing.
#[test]
fn an_aggregate_does_not_steal_the_empty_literal() {
    // `r` is defined but disjoint from `p`, so the join stops at it. It has to
    // *exist*: a literal naming nothing at all is now `invalid-query`, which is
    // a different defect from a literal that simply matches no rows.
    let program = "\
        p(\"a\"). p(\"b\"). p(\"c\").\n\
        q(\"a\"). q(\"b\").\n\
        r(\"z\").\n";

    // No aggregate: the second literal is the one that matches nothing.
    let mut e = engine(program);
    let plain = e
        .query("?- p(X), r(X).", &Limits::default())
        .expect("answers");
    assert!(plain.rows.is_empty());
    assert_eq!(plain.stats.empty_at.as_deref(), Some("r(X)"));

    // With an aggregate whose sub-goal is *longer* than the outer body, the
    // shared cell used to push the mark past the end of the outer plan.
    let mut e = engine(program);
    let counted = e
        .query(
            "?- p(X), N = count{ Y : p(Y), q(Y), p(Y) }, N > 99.",
            &Limits::default(),
        )
        .expect("answers");
    assert!(counted.rows.is_empty());
    assert_eq!(counted.stats.empty_at.as_deref(), Some("N > 99"));
}
