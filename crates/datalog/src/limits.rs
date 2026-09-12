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
    /// Strata accepted at planning. Default 32.
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
            max_strata: 32,
            max_body_literals: 32,
        }
    }
}

impl Limits {
    /// The names a `cap` field can carry, for a consumer that wants to switch
    /// on them rather than parse prose.
    pub const CAPS: [&'static str; 4] = [
        "max_result_rows",
        "max_result_bytes",
        "max_derived_tuples",
        "max_time_ms",
    ];
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
    pub plan: Vec<String>,
    /// Predicates the demand transformation rewrote, so an unexpectedly slow
    /// query can be diagnosed rather than guessed at.
    pub transformed: Vec<String>,
    /// Stdlib rules a query-local rule shadowed. Never silent.
    pub shadowed: Vec<String>,
}
