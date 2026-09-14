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
    /// Every status, for the drift test and for anything enumerating them.
    pub const ALL: [Self; 12] = [
        Self::Ok,
        Self::Truncated,
        Self::NoIndex,
        Self::Stale,
        Self::NoScip,
        Self::ScipStale,
        Self::InvalidQuery,
        Self::Unstratified,
        Self::Timeout,
        Self::BudgetExceeded,
        Self::Locked,
        Self::Corrupt,
    ];
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

    /// The status a refusal from `crates/facts` maps to. An I/O failure has
    /// none (`specs/04-storage.md` § Segment format).
    #[must_use]
    pub const fn of_fault(fault: facts::Fault) -> Self {
        match fault {
            facts::Fault::Corrupt => Self::Corrupt,
            facts::Fault::Stale => Self::Stale,
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

#[cfg(test)]
mod spec_tests {
    use super::Status;

    /// `specs/05-surface.md`'s status table, as the table itself.
    const SPEC: &str = include_str!("../../../specs/05-surface.md");

    /// The binding document, which must cite the taxonomy rather than repeat it.
    const OVERVIEW: &str = include_str!("../../../specs/00-overview.md");

    /// Statuses that were specified and then removed. A retired name lingering
    /// in a spec is the same lie as an undocumented one, and it survived the
    /// first removal *because* 00-overview.md enumerated the taxonomy too.
    const RETIRED: &[&str] = &["unsupported-language"];

    /// The taxonomy is a contract, so both directions of drift are defects.
    ///
    /// A status the code emits and the table omits leaves a consumer branching
    /// on something undocumented — `corrupt` was in exactly that state. A
    /// status the table lists and the code cannot produce is the mirror image,
    /// and `unsupported-language` was in *that* one: documented, never emitted,
    /// and deciding it per query would have cost a full tree walk that belongs
    /// in `codeintel status`.
    #[test]
    fn the_status_taxonomy_matches_the_spec() {
        let documented: Vec<&str> = SPEC
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .flat_map(|row| {
                // `timeout` / `budget-exceeded` share one row.
                let head = row.split('|').next().unwrap_or_default();
                head.split('/')
                    .filter_map(|cell| cell.trim().trim_matches('`').into())
                    .collect::<Vec<_>>()
            })
            .filter(|name| !name.is_empty())
            .collect();

        for status in Status::ALL {
            assert!(
                documented.contains(&status.as_str()),
                "`{}` is emitted but has no row in specs/05-surface.md",
                status.as_str()
            );
        }
        for name in &documented {
            assert!(
                Status::ALL.iter().any(|s| s.as_str() == *name),
                "specs/05-surface.md documents `{name}`, which nothing emits"
            );
        }
    }

    /// One enumeration, in one file.
    ///
    /// `00-overview.md` is what `CLAUDE.md` sends an implementing agent to
    /// first, so a stale list there outranks a correct one in the surface spec.
    /// It may *cite* the taxonomy; it may not restate it.
    #[test]
    fn only_the_surface_spec_enumerates_the_taxonomy() {
        for name in RETIRED {
            assert!(
                !OVERVIEW.contains(name),
                "`{name}` was retired but still appears in specs/00-overview.md"
            );
        }
        let listed = Status::ALL
            .iter()
            .filter(|s| OVERVIEW.contains(&format!("`{}`", s.as_str())))
            .count();
        assert!(
            listed <= 2,
            "specs/00-overview.md names {listed} statuses; it should cite \
             05-surface.md § Status taxonomy rather than repeat it, because a \
             taxonomy written down twice drifts"
        );
    }
}
