//! The relation table: id, name, and arity for every base relation.
//!
//! It lives here as a compile-time constant rather than in the segment file
//! (`specs/04-storage.md` § Segment format), so a segment carries relation
//! *ids* and nothing else. A schema change bumps [`SCHEMA_VERSION`] and a
//! mismatch on open is `stale` with the reindex command — never a silent read
//! of rows against the wrong column meanings.
//!
//! Fifteen relations against a ceiling of sixteen
//! (`specs/00-overview.md` § Surface budget). `has_type` is deferred and
//! deliberately absent (`specs/01-facts.md`).
//!
//! Note what is **not** here: `exported` and `implements`. Both are derived in
//! `rules/stdlib.dl` over the narrower facts below — `visibility` and
//! `scip_impl`/`name_impl` — because whether a symbol is visible outside its
//! crate, or implements a trait named in another file, is a join and not a
//! token. That is invariant 3, and it is the same move `ref` made in v1.

/// Bumped whenever a relation is added, removed, or changes arity.
pub const SCHEMA_VERSION: u32 = 3;

/// One base relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rel {
    /// Its id, as written into a segment. Stable for a given [`SCHEMA_VERSION`].
    pub id: u32,
    /// Its name, as written in Datalog.
    pub name: &'static str,
    /// How many columns it has.
    pub arity: usize,
}

/// Every base relation, in id order. The index into this array *is* the id.
///
/// Order is the order of `specs/01-facts.md` § Base relations, which is also
/// roughly the order the extractor emits them in.
pub const RELATIONS: &[Rel] = &[
    Rel {
        id: 0,
        name: "file",
        arity: 2,
    },
    Rel {
        id: 1,
        name: "def",
        arity: 4,
    },
    Rel {
        id: 2,
        name: "def_span",
        arity: 5,
    },
    Rel {
        id: 3,
        name: "def_name",
        arity: 3,
    },
    Rel {
        id: 4,
        name: "def_sig",
        arity: 2,
    },
    Rel {
        id: 5,
        name: "def_doc",
        arity: 2,
    },
    Rel {
        id: 6,
        name: "parent",
        arity: 2,
    },
    Rel {
        id: 7,
        name: "visibility",
        arity: 2,
    },
    Rel {
        id: 8,
        name: "resolved",
        arity: 1,
    },
    Rel {
        id: 9,
        name: "import",
        arity: 3,
    },
    Rel {
        id: 10,
        name: "scip_ref",
        arity: 6,
    },
    Rel {
        id: 11,
        name: "name_ref",
        arity: 5,
    },
    Rel {
        id: 12,
        name: "scip_impl",
        arity: 2,
    },
    Rel {
        id: 13,
        name: "name_impl",
        arity: 4,
    },
    Rel {
        id: 14,
        name: "extern",
        arity: 4,
    },
    Rel {
        id: 15,
        name: "name_export",
        arity: 2,
    },
];

/// The relation with this id.
#[must_use]
pub fn by_id(id: u32) -> Option<&'static Rel> {
    RELATIONS.get(usize::try_from(id).ok()?)
}

/// The relation with this name.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Rel> {
    RELATIONS.iter().find(|r| r.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_indices() {
        for (i, rel) in RELATIONS.iter().enumerate() {
            assert_eq!(u32::try_from(i), Ok(rel.id), "{} is out of order", rel.name);
        }
    }

    #[test]
    fn names_are_unique_and_resolvable() {
        for rel in RELATIONS {
            assert_eq!(by_name(rel.name), Some(rel));
            assert_eq!(by_id(rel.id), Some(rel));
        }
        let mut names: Vec<&str> = RELATIONS.iter().map(|r| r.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate relation name");
    }

    #[test]
    fn the_fact_spec_declares_the_version_this_code_writes() {
        // `specs/01-facts.md` is the ABI, and its front matter carries the
        // schema version. It sat at 1 for the whole of v2 — the § Changelog
        // table in that same file already described v2, so the document
        // disagreed with itself and with this constant, and nothing noticed.
        // A version declared in prose and asserted nowhere is one that drifts.
        let spec = include_str!("../../../specs/01-facts.md");
        let declared = spec
            .lines()
            .find_map(|l| l.strip_prefix("schema_version:"))
            .map(str::trim)
            .expect("specs/01-facts.md front matter declares schema_version");
        assert_eq!(
            declared,
            SCHEMA_VERSION.to_string(),
            "specs/01-facts.md front matter says schema_version: {declared}, \
             facts::schema::SCHEMA_VERSION is {SCHEMA_VERSION}"
        );
    }

    #[test]
    fn the_base_relation_budget_holds() {
        // `specs/00-overview.md` § Surface budget: <= 16, currently 16. This is
        // one of the four numbers CI asserts; the budget is spent (#22), so the
        // next relation deletes one, and that is a spec change.
        assert!(RELATIONS.len() <= 16, "base relations: {}", RELATIONS.len());
        assert_eq!(RELATIONS.len(), 16);
    }
}
