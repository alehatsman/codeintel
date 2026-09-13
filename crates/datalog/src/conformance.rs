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
        diagnostic.message.contains("seed"),
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

/// A literal whose leading column is free and a later one bound is served
/// through that column's index, and the plan says so — `i#c` — so a slow
/// query shows which lookups went through an index and which did not
/// (`specs/03-datalog.md` § Evaluation).
#[test]
fn the_plan_marks_a_lookup_served_by_a_column_index() {
    let rows = both(GRAPH, "into(X, Y) :- edge(X, Y). ?- into(X, \"b\").");
    assert_eq!(rows, vec!["\"a\"", "\"e\""]);
    let mut e = engine(GRAPH);
    let result = e
        .query("?- edge(X, \"b\"), edge(\"b\", Y).", &Limits::default())
        .expect("answers");
    // The leading column of `edge(X, "b")` is free and its second is bound:
    // the column path, marked. `edge("b", Y)` is a prefix lookup: unmarked.
    assert_eq!(
        result.stats.plan.last().map(String::as_str),
        Some("?-: 1, 0#1")
    );
}

/// An engine whose relations are installed as base relations, not written as
/// facts — so a constant in the goal seeds nothing and the plan is the
/// planner's alone.
fn base(relations: &[(&str, &[[&str; 2]])]) -> Engine {
    let mut e = Engine::new(Box::new(Strings::new()));
    for (name, rows) in relations {
        let mut rel = crate::relation::Relation::new(2);
        for [a, b] in *rows {
            let tuple = [e.intern(a).expect("interns"), e.intern(b).expect("interns")];
            assert!(rel.push(&tuple));
        }
        e.insert_relation(*name, rel);
    }
    e
}

const VIS: &[[&str; 2]] = &[
    ["a", "public"],
    ["b", "inherited"],
    ["c", "inherited"],
    ["d", "public"],
];
const PAR: &[[&str; 2]] = &[["b", "a"], ["c", "d"], ["x", "y"], ["z", "w"]];

/// Once anything is bound, a literal sharing none of it is a cross product,
/// and it waits for one that joins. `k` counts a constant as it counts a join
/// variable, so `vis(S, "inherited")` and `par(S, P)` tie below, and written
/// order used to break the tie toward the product: `exported/1`'s body, 868 ms
/// on tokio against 1 ms joined (#24). Both orders derive the same tuples, so
/// `stats.derived` cannot pin this. The plan can.
#[test]
fn a_literal_sharing_no_bound_variable_waits_for_one_that_does() {
    let query = r#"?- vis(P, "public"), vis(S, "inherited"), par(S, P)."#;
    let (rows, result) = both_in(|| base(&[("vis", VIS), ("par", PAR)]), query);
    assert_eq!(rows, vec!["\"a\" \"b\"", "\"d\" \"c\""]);
    // Not `0#1, 1#1, 2`: `par(S, P)` joins on `P` through its second column's
    // index, and `vis(S, "inherited")` becomes a prefix lookup on `S`.
    assert_eq!(
        result.stats.plan.last().map(String::as_str),
        Some("?-: 0#1, 2#1, 1")
    );
}

/// The adornment walk mirrors the planner, so it waits too. Walked
/// constant-first, `kind` was adorned `kind@fb` — its constant alone bound —
/// though `par(S, P)` could bind `S` first and seed both columns.
#[test]
fn the_adornment_walk_also_waits_for_a_literal_that_joins() {
    let program = r#"
vis("a", "public"). vis("d", "public").
kind("b", "inherited"). kind("c", "inherited").
par("b", "a"). par("c", "d").
"#;
    let query = r#"?- vis(P, "public"), kind(S, "inherited"), par(S, P)."#;
    let (rows, result) = both_in(|| engine(program), query);
    assert_eq!(rows, vec!["\"a\" \"b\"", "\"d\" \"c\""]);
    let transformed = &result.stats.transformed;
    assert!(
        transformed.iter().any(|n| n == "kind@bb"),
        "{transformed:?}"
    );
    assert!(
        !transformed.iter().any(|n| n == "kind@fb"),
        "{transformed:?}"
    );
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
        "{:?}: {}",
        result.stats.transformed,
        result.stats.demand
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

/// Adornment must not depend on where the binder sits in the body. Written
/// binder-last, `impact_of(S, C)` was adorned free-free, no seed was made,
/// and the query paid for the all-pairs closure; the planner's later
/// reordering could not rescue it because the adornment was already fixed.
#[test]
fn body_order_does_not_change_the_adornment() {
    let binder_first = r#"?- calls(S, "a0"), impact_of(S, C)."#;
    let binder_last = r#"?- impact_of(S, C), calls(S, "a0")."#;
    let (rows_a, first) = both_in(|| engine(WIDE), binder_first);
    let (rows_b, last) = both_in(|| engine(WIDE), binder_last);
    assert_eq!(rows_a, vec!["\"a1\" \"a2\"", "\"a1\" \"a3\""]);
    assert_eq!(rows_a, rows_b);
    assert_eq!(first.stats.transformed, last.stats.transformed);
    assert!(
        last.stats.transformed.iter().any(|n| n == "impact_of@bf"),
        "{:?}",
        last.stats.transformed
    );
    assert_eq!(first.stats.derived, last.stats.derived);
    assert_eq!(last.stats.demand, "applied");

    // Inside a rule body too, not only in the goal.
    let program = format!("{WIDE}\nreach_from(Seed, C) :- impact_of(Seed, C), calls(Seed, _).\n");
    let (rows, result) = both_in(|| engine(&program), r#"?- reach_from("a1", C)."#);
    assert_eq!(rows, vec!["\"a2\"", "\"a3\""]);
    assert!(
        result.stats.transformed.iter().any(|n| n == "impact_of@bf"),
        "{:?}",
        result.stats.transformed
    );
}

/// `stats.demand` distinguishes "nothing to seed" from "tried and dropped".
#[test]
fn stats_demand_says_why_the_rewrite_did_or_did_not_apply() {
    let mut e = engine(WIDE);
    let free = e
        .query("?- impact_of(S, C).", &Limits::default())
        .expect("answers");
    assert!(free.stats.transformed.is_empty());
    assert!(
        free.stats.demand.starts_with("nothing to seed"),
        "{}",
        free.stats.demand
    );
    let seeded = e
        .query(r#"?- impact_of("a0", C)."#, &Limits::default())
        .expect("answers");
    assert_eq!(seeded.stats.demand, "applied");
    let plain = e
        .query_undemanded(r#"?- impact_of("a0", C)."#, &Limits::default())
        .expect("answers");
    assert_eq!(plain.stats.demand, "disabled");
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
    assert!(
        result.stats.demand.contains("planning limits"),
        "{}",
        result.stats.demand
    );
    assert_eq!(render(&e, &result), vec!["\"a1\"", "\"a2\"", "\"a3\""]);
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
