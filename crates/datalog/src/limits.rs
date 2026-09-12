//! Limits, truncation and statistics.
//!
//! Every cap has a documented default and names itself when it fires. A silent
//! cap reads as a complete answer, which is worse than an error
//! (`specs/00-overview.md` invariant 7).

/// Caps on one query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Rows returned before truncation. Default 1,000.
    pub max_result_rows: usize,
    /// Bytes of rendered output before truncation, at a row boundary.
    /// Default 262,144 — rows are not uniformly sized and the consumer is a
    /// context window.
    pub max_result_bytes: usize,
    /// Tuples derived across all strata before the query aborts.
    /// Default 10,000,000.
    pub max_derived_tuples: u64,
    /// Wall-clock budget in milliseconds. Default 5,000.
    pub max_time_ms: u64,
    /// Strata accepted at planning. Default 64.
    ///
    /// The spec said 32. `rules/stdlib.dl` alone stratifies into 37, so that
    /// default rejected every query against the shipped standard library — the
    /// limit was set against an imagined rule set rather than the real one.
    pub max_strata: usize,
    /// Literals in one rule or query body, accepted at planning. Default 32.
    pub max_body_literals: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_result_rows: 1_000,
            max_result_bytes: 262_144,
            max_derived_tuples: 10_000_000,
            max_time_ms: 5_000,
            max_strata: 64,
            max_body_literals: 32,
        }
    }
}

impl Limits {
    /// The names a `cap` field can carry, for a consumer that wants to switch
    /// on them rather than parse prose.
    /// Only the two truncating limits are here. `max_derived_tuples` and
    /// `max_time_ms` abort the query with a diagnostic rather than trim an
    /// answer, so they never appear in `cap`, and listing them told a consumer
    /// to switch on a value it would never see.
    pub const CAPS: [&'static str; 2] = ["max_result_rows", "max_result_bytes"];
}

/// What one evaluation cost. Not part of the answer, so timings here do not
/// break the byte-identical-output invariant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Tuples derived across every stratum, including duplicates.
    pub derived: u64,
    /// Wall-clock milliseconds.
    pub elapsed_ms: u64,
    /// How many strata ran.
    pub strata: usize,
    /// The literal order chosen per rule, as `head/arity: lit, lit, ...`.
    ///
    /// Bounded. A plan that hit the bound ends with a `(plan truncated at N
    /// rules)` entry rather than simply stopping.
    pub plan: Vec<String>,
    /// Predicates the demand transformation rewrote, so an unexpectedly slow
    /// query can be diagnosed rather than guessed at.
    pub transformed: Vec<String>,
    /// Stdlib rules a query-local rule shadowed. Never silent.
    pub shadowed: Vec<String>,
    /// When the goal derived no rows, the body literal — quoted as the caller
    /// wrote it — that matched nothing first.
    ///
    /// A valid query returning zero rows is indistinguishable from a typo, a
    /// wrong constant, and a base relation the host never populated.
    /// Evaluation already walks the body literal by literal, so the deepest
    /// position it reached is free, and the literal at that position is the one
    /// that stopped the join (`specs/05-surface.md` § Response contract).
    pub empty_at: Option<String>,
    /// The **base** relations the goal's dependency closure reaches, sorted.
    ///
    /// The closure, not the literal syntax of the goal. A caller that must know
    /// whether an answer depends on a relation it could not populate cannot
    /// get that from the query text: a goal over a derived predicate mentions
    /// no base relation at all, and answering `ok` with zero rows would be a
    /// lie it has no way to detect.
    pub depends: Vec<String>,
}
