//! The tuple store the evaluator reads: relations by name.

use std::collections::BTreeMap;

use crate::atom::Atom;
use crate::relation::Relation;

/// Named relations, base and derived alike.
#[derive(Debug, Clone, Default)]
pub struct Db {
    rels: BTreeMap<String, Relation>,
}

impl Db {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a relation, replacing any relation of the same name.
    pub fn insert(&mut self, name: impl Into<String>, relation: Relation) {
        self.rels.insert(name.into(), relation);
    }

    /// Add one tuple to `name`, creating the relation at that arity if needed.
    ///
    /// Returns false when the row's width disagrees with the existing arity.
    pub fn push(&mut self, name: &str, row: &[Atom]) -> bool {
        let rel = self
            .rels
            .entry(name.to_string())
            .or_insert_with(|| Relation::new(row.len().max(1)));
        rel.push(row)
    }

    /// The relation, if the store has one.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Relation> {
        self.rels.get(name)
    }

    /// The relation, creating an empty one of the given arity if absent.
    pub fn get_or_create(&mut self, name: &str, arity: usize) -> &mut Relation {
        self.rels
            .entry(name.to_string())
            .or_insert_with(|| Relation::new(arity))
    }

    /// Sort and deduplicate every relation.
    pub fn settle(&mut self) {
        for rel in self.rels.values_mut() {
            rel.settle();
        }
    }

    /// Every relation name and arity, in name order.
    pub fn schema(&self) -> impl Iterator<Item = (&str, usize)> {
        self.rels.iter().map(|(n, r)| (n.as_str(), r.arity()))
    }

    /// How many relations it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rels.len()
    }

    /// True when it holds no relations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rels.is_empty()
    }
}
