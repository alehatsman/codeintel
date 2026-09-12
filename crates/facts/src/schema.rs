//! The relation table: id, name, and arity for every base relation.
//!
//! It lives here as a compile-time constant rather than in the segment file
//! (`specs/04-storage.md` § Segment format), so a segment carries relation
//! *ids* and nothing else. A schema change bumps [`SCHEMA_VERSION`] and a
//! mismatch on open is `stale` with the reindex command — never a silent read
//! of rows against the wrong column meanings.
//!
//! Fourteen relations against a ceiling of sixteen
//! (`specs/00-overview.md` § Surface budget). `has_type` is deferred and
//! deliberately absent (`specs/01-facts.md`).

/// Bumped whenever a relation is added, removed, or changes arity.
pub const SCHEMA_VERSION: u32 = 1;

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
        name: "exported",
        arity: 1,
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
        name: "implements",
        arity: 3,
    },
    Rel {
        id: 13,
        name: "extern",
        arity: 4,
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
    fn the_base_relation_budget_holds() {
        // `specs/00-overview.md` § Surface budget: <= 16, currently 14. This is
        // one of the four numbers CI asserts; growing it is a spec change.
        assert!(RELATIONS.len() <= 16, "base relations: {}", RELATIONS.len());
        assert_eq!(RELATIONS.len(), 14);
    }
}
