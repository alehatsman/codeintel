//! One test per safety rule in `specs/03-datalog.md`, asserting the message an
//! agent has to fix its query from.

use datalog::check::check;
use datalog::diag::Status;
use datalog::strata::stratify;
use datalog::{Strings, parse};

/// Base relations a small program can lean on, as a host would supply them.
fn base() -> Vec<(String, usize)> {
    vec![
        ("def".to_string(), 4),
        ("def_span".to_string(), 5),
        ("file".to_string(), 2),
        ("edge".to_string(), 2),
    ]
}

fn reject(src: &str) -> String {
    let program = parse(src, &mut Strings::new()).expect("parses");
    let diagnostic = check(&program, base()).expect_err("expected a rejection");
    assert_eq!(diagnostic.status, Status::InvalidQuery, "{diagnostic}");
    assert!(
        diagnostic.span.is_some(),
        "a safety rejection carries a span: {diagnostic}"
    );
    diagnostic.message
}

fn accept(src: &str) {
    let program = parse(src, &mut Strings::new()).expect("parses");
    check(&program, base()).expect("this program satisfies the safety rules");
}

#[test]
fn rule_1_range_restriction() {
    let msg = reject("r(X, Y) :- edge(X, _).");
    assert!(msg.contains("head variable `Y`"), "{msg}");
    assert!(msg.contains("safety rule 1"), "{msg}");
    assert!(msg.contains("range restriction"), "{msg}");
}

#[test]
fn rule_1_accepts_a_head_variable_bound_by_the_body() {
    accept("r(X, Y) :- edge(X, Y).");
}

#[test]
fn rule_1_rejects_a_wildcard_in_a_rule_head() {
    // `has(S, _)` derived nothing at all, silently — invariant 5.
    let msg = reject("has(S, _) :- edge(_, S).");
    assert!(msg.contains("`_` in a rule head"), "{msg}");
    assert!(msg.contains("no rows"), "{msg}");
}

#[test]
fn rule_1_leaves_a_wildcard_in_the_body_alone() {
    accept("r(X) :- edge(X, _).");
}

#[test]
fn rule_2_negation_safety() {
    let msg = reject("r(X) :- edge(X, _), !edge(Y, X).");
    assert!(msg.contains("`Y`"), "{msg}");
    assert!(msg.contains("safety rule 2"), "{msg}");
    assert!(msg.contains("negated literal"), "{msg}");
}

#[test]
fn rule_2_accepts_a_negation_over_bound_variables() {
    accept("r(X) :- edge(X, Y), !edge(Y, X).");
}

#[test]
fn rule_3_comparison_safety() {
    let msg = reject("r(X) :- edge(X, _), X < N.");
    assert!(msg.contains("`N`"), "{msg}");
    assert!(msg.contains("safety rule 3"), "{msg}");
    assert!(msg.contains("operand of `<`"), "{msg}");
}

#[test]
fn rule_3_covers_the_string_tests_too() {
    let msg = reject(r#"r(X) :- edge(X, _), match(F, "^src/")."#);
    assert!(msg.contains("subject of `match`"), "{msg}");
}

#[test]
fn rule_3_rejects_a_wildcard_operand() {
    let msg = reject("r(X) :- edge(X, _), _ < 3.");
    assert!(msg.contains("`_` binds nothing"), "{msg}");
}

#[test]
fn rule_4_aggregate_safety() {
    // `S` is counted over inside and used outside, but nothing binds it first:
    // the grouping the reader sees is not the grouping the engine would compute.
    let msg = reject("hot(S, N) :- N = count{ C : edge(C, S) }, def(S, _, _, _).");
    assert!(msg.contains("safety rule 4"), "{msg}");
    assert!(msg.contains("`S`"), "{msg}");
}

#[test]
fn rule_4_accepts_the_grouping_written_explicitly() {
    accept("hot(S, N) :- def(S, _, _, _), N = count{ C : edge(C, S) }.");
}

#[test]
fn rule_5_stratification_rejects_negation_in_a_cycle() {
    let program = parse("p(X) :- edge(X, _), !p(X).", &mut Strings::new()).expect("parses");
    check(&program, base()).expect("safe as written");
    let diagnostic = stratify(&program).expect_err("unstratified");
    assert_eq!(diagnostic.status, Status::Unstratified);
    assert!(diagnostic.message.contains('p'), "{diagnostic}");
    assert!(
        diagnostic.message.contains("recursive cycle"),
        "{diagnostic}"
    );
}

#[test]
fn rule_5_rejects_an_aggregate_in_a_cycle() {
    let src = "p(X, N) :- edge(X, _), N = count{ Y : p(Y, _) }.";
    let program = parse(src, &mut Strings::new()).expect("parses");
    let diagnostic = stratify(&program).expect_err("unstratified");
    assert_eq!(diagnostic.status, Status::Unstratified);
}

#[test]
fn rule_5_accepts_recursion_without_negation() {
    let src = "reaches(A, B) :- edge(A, B).\nreaches(A, C) :- reaches(A, B), edge(B, C).";
    let program = parse(src, &mut Strings::new()).expect("parses");
    let strata = stratify(&program).expect("stratified");
    assert_eq!(strata.len(), 1);
}

#[test]
fn rule_5_orders_a_negated_dependency_before_its_user() {
    let src = "dead(S) :- def(S, _, _, _), !live(S).\nlive(S) :- edge(_, S).";
    let program = parse(src, &mut Strings::new()).expect("parses");
    let strata = stratify(&program).expect("stratified");
    let live = strata.stratum_of("live").expect("live is derived");
    let dead = strata.stratum_of("dead").expect("dead is derived");
    assert!(
        live < dead,
        "live must reach fixpoint before dead reads its absence: {strata:?}"
    );
}

#[test]
fn rule_6_arity_consistency() {
    let msg = reject("r(X) :- edge(X, _), edge(X).");
    assert!(msg.contains("safety rule 6"), "{msg}");
    assert!(msg.contains("`edge`"), "{msg}");
    assert!(msg.contains('2'), "{msg}");
}

#[test]
fn rule_6_names_where_the_arity_was_fixed() {
    let msg = reject("r(X) :- thing(X, X), thing(X).");
    assert!(msg.contains("first used at byte"), "{msg}");
}

#[test]
fn rule_7_assignment_binds_its_target() {
    accept("long(S, N) :- def_span(S, A, B, _, _), N = B - A, N > 80.");
}

#[test]
fn rule_7_requires_the_right_hand_side_to_be_bound() {
    let msg = reject("r(S, N) :- def_span(S, A, _, _, _), N = A - B.");
    assert!(msg.contains("`B`"), "{msg}");
    assert!(msg.contains("safety rule 7"), "{msg}");
}

#[test]
fn rule_7_between_generates_its_output() {
    accept("symbol_at(F, L, S) :- def(S, F, _, _), def_span(S, A, B, _, _), between(A, B, L).");
}

#[test]
fn rule_7_between_requires_bound_endpoints() {
    let msg = reject("r(L) :- edge(A, _), between(A, B, L).");
    assert!(msg.contains("`B`"), "{msg}");
    assert!(msg.contains("bound of `between`"), "{msg}");
}

#[test]
fn rule_7_between_rejects_a_wildcard_output() {
    // The grammar says the third argument is a var. `_` generated into
    // nowhere, so every row failed and the query answered ok with no rows.
    let msg = reject("w(S) :- def_span(S, A, B, _, _), between(A, B, _).");
    assert!(msg.contains("third argument must be a variable"), "{msg}");
}

#[test]
fn rule_8_base_and_derived_are_exclusive() {
    let msg = reject("def(S, F, K, N) :- edge(S, F), K = N.");
    assert!(msg.contains("safety rule 8"), "{msg}");
    assert!(msg.contains("base relation"), "{msg}");
    assert!(msg.contains("`def`"), "{msg}");
}

#[test]
fn a_query_is_checked_like_a_rule_body() {
    let program = parse("?- edge(X, _), X < N.", &mut Strings::new()).expect("parses");
    let diagnostic = check(&program, base()).expect_err("rejected");
    assert!(diagnostic.message.contains("`N`"), "{diagnostic}");
}

#[test]
fn the_schema_reports_which_relations_are_derived() {
    let program = parse("live(S) :- edge(_, S).", &mut Strings::new()).expect("parses");
    let schema = check(&program, base()).expect("safe");
    assert!(schema.is_derived("live"));
    assert!(!schema.is_derived("edge"));
    assert_eq!(schema.arity.get("live"), Some(&1));
}

#[test]
fn an_unknown_predicate_is_rejected_rather_than_silently_empty() {
    // A typo and "this is not true of your code" are different answers. An
    // undeclared predicate used to evaluate as an empty derived relation, so
    // `?- nosuchrelation(X).` came back `ok` with zero rows — indistinguishable
    // from a correct query about code that has none of the thing asked for.
    let msg = reject("?- nosuchrelation(X).");
    assert!(msg.contains("nosuchrelation"), "{msg}");
    assert!(msg.contains("no relation or rule named"), "{msg}");

    // Inside an aggregate is the same typo one level down.
    let msg = reject("r(N) :- N = count{ X : nosuchrelation(X) }.");
    assert!(msg.contains("nosuchrelation"), "{msg}");

    // A forward reference to a rule defined later in the program stays legal.
    accept("a(X) :- b(X).\nb(X) :- edge(X, _).");
}
