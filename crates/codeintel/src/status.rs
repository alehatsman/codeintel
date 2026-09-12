//! The status taxonomy, from `specs/05-surface.md`.
//!
//! Every response carries one. A consumer branches on it instead of catching
//! failures or guessing what an empty result means, and **`ok` with zero rows
//! means "this is not true of your code"** — which must be distinguishable
//! from every failure below without reading prose. That is invariant 6, and
//! it is the most common way a tool like this lies to an agent.

use std::fmt;

/// What happened to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Ran, results complete.
    Ok,
    /// Ran, a cap dropped rows.
    Truncated,
    /// No `.codeintel/` here.
    NoIndex,
    /// Sources changed and the refresh was skipped, ran out of time, or the
    /// schema does not match.
    Stale,
    /// The goal's dependency closure reaches a relation only SCIP populates,
    /// and no SCIP index was ingested.
    NoScip,
    /// Sources changed after the SCIP index was built, so `name_ref` is fresh
    /// and `scip_ref` is not.
    ScipStale,
    /// Parse or safety failure.
    InvalidQuery,
    /// Negation cycle.
    Unstratified,
    /// A time limit aborted evaluation.
    Timeout,
    /// A size limit aborted evaluation.
    BudgetExceeded,
    /// Another writer holds the lock.
    Locked,
    /// A segment or the dictionary did not survive validation.
    Corrupt,
}

impl Status {
    /// Its wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Truncated => "truncated",
            Self::NoIndex => "no-index",
            Self::Stale => "stale",
            Self::NoScip => "no-scip",
            Self::ScipStale => "scip-stale",
            Self::InvalidQuery => "invalid-query",
            Self::Unstratified => "unstratified",
            Self::Timeout => "timeout",
            Self::BudgetExceeded => "budget-exceeded",
            Self::Locked => "locked",
            Self::Corrupt => "corrupt",
        }
    }

    /// True when the query actually ran, whatever else happened.
    ///
    /// `stale`, `truncated`, `no-scip` and `scip-stale` are answers about a
    /// known-imperfect index, not failures: the caller gets rows and is told
    /// what is wrong with them. `no-scip` in particular is the difference
    /// between "your code does not do this" and "this index cannot see it",
    /// which is invariant 6 in its sharpest form.
    #[must_use]
    pub const fn answered(self) -> bool {
        matches!(
            self,
            Self::Ok | Self::Truncated | Self::Stale | Self::NoScip | Self::ScipStale
        )
    }

    /// The status a `datalog` diagnostic maps to.
    #[must_use]
    pub const fn of(diagnostic: datalog::Status) -> Self {
        match diagnostic {
            datalog::Status::Unstratified => Self::Unstratified,
            datalog::Status::Timeout => Self::Timeout,
            datalog::Status::BudgetExceeded => Self::BudgetExceeded,
            // `InvalidQuery` and the catch-all share an arm on purpose.
            // `datalog::Status` is `#[non_exhaustive]`, and a status this build
            // does not know is still a rejected query — reporting it with the
            // engine's own message attached beats a panic.
            datalog::Status::InvalidQuery | _ => Self::InvalidQuery,
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_a_kebab_name_and_they_are_distinct() {
        let all = [
            Status::Ok,
            Status::Truncated,
            Status::NoIndex,
            Status::Stale,
            Status::NoScip,
            Status::ScipStale,
            Status::InvalidQuery,
            Status::Unstratified,
            Status::Timeout,
            Status::BudgetExceeded,
            Status::Locked,
            Status::Corrupt,
        ];
        let mut names: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        assert!(names.iter().all(|n| !n.contains('_')), "{names:?}");
    }

    #[test]
    fn only_the_statuses_that_carry_rows_count_as_answered() {
        assert!(Status::Ok.answered());
        assert!(Status::Truncated.answered());
        assert!(Status::Stale.answered());
        assert!(Status::NoScip.answered());
        assert!(Status::ScipStale.answered());
        assert!(!Status::NoIndex.answered());
        assert!(!Status::Locked.answered());
        assert!(!Status::Corrupt.answered());
    }
}
