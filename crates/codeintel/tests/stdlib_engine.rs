//! The shipped rule library through the engine's own gates: it loads as
//! written, the naive twin accepts it, hand-written facts get hand-verified
//! answers, the seeded traversals agree with naive evaluation, and demand
//! transformation applies to them.
//!
//! These lived in `crates/datalog`'s conformance tests and read
//! `rules/stdlib.dl` from there, so any stdlib edit broke the code-agnostic
//! crate's unit tests. They belong to the crate that owns the library
//! (`specs/00-overview.md` invariant 6). The engine's differential twins are
//! public and doc-hidden for exactly this file.

use codeintel::Regexes;
use datalog::{Engine, Limits, QueryResult, Strings};

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

/// Run a query both ways and assert they agree; return the fast answer and
/// its result. The fast side is semi-naive over the demand-rewritten program,
/// the slow side naive over the program as written
/// (`specs/03-datalog.md` § Validation 5).
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

fn stdlib_engine() -> Engine {
    let mut e = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(Regexes::new()));
    for (name, arity) in BASE {
        e.insert_relation(name, datalog::Relation::new(arity));
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
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");
    assert!(e.rule_count() > 30, "loaded {} rules", e.rule_count());
}

#[test]
fn the_naive_evaluator_accepts_the_standard_library_too() {
    let mut e = stdlib_engine();
    e.load_rules(codeintel::query::STDLIB)
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
    let mut rel = datalog::Relation::new(arity);
    for row in rows {
        let atoms: Vec<datalog::Atom> = row
            .iter()
            .map(|token| {
                token
                    .parse::<u32>()
                    .ok()
                    .filter(|n| datalog::atom::is_int(*n))
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
    e.load_rules(codeintel::query::STDLIB)
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
    e.load_rules(codeintel::query::STDLIB)
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
    e.load_rules(codeintel::query::STDLIB)
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

/// A repository's conformance rules do not spend the stratum budget.
///
/// `--rules` is the documented home for them, and each independent rule used to
/// cost one stratum on top of the standard library's own. The library spent 39
/// of `max_strata`'s 64, so **26 conformance rules made every query fail at
/// planning** — including queries touching none of them. The limit had already
/// been raised 32 -> 64 once for the same reason.
#[test]
fn a_hundred_conformance_rules_do_not_exhaust_max_strata() {
    let mut engine = store_fixture();
    let mut extra = String::new();
    for i in 0..100 {
        use std::fmt::Write as _;
        writeln!(
            extra,
            "%% conf{i}(S) -- an independent conformance rule.\n\
             conf{i}(S) :- def(S, F, \"function\", N)."
        )
        .expect("writing to a String cannot fail");
    }
    engine.load_rules(&extra).expect("the rules load");
    engine
        .query("?- conf0(S).", &Limits::default())
        .expect("a query touching one of them still plans");
}
