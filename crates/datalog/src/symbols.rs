//! The dictionary seam: how the engine turns a string literal into an atom.
//!
//! The engine never owns a dictionary. A host with a persistent interner
//! implements this trait over it; tests and small embeddings use [`Strings`].

use std::collections::HashMap;

use crate::atom::{Atom, STR_MIN};

/// A string dictionary the engine can read and extend.
///
/// `intern` may allocate a transient id for a string the host has never seen —
/// a query is allowed to mention a name that does not occur in the corpus, and
/// a rule head may *produce* one (`about(S, "sig", ..)`). The engine never asks
/// for such an id to be persisted.
pub trait Symbols {
    /// The atom for `s`, allocating one if the dictionary does not have it.
    ///
    /// # Errors
    /// Returns `None` when the dictionary is full — ids are a `u32` space.
    fn intern(&mut self, s: &str) -> Option<Atom>;

    /// The string behind `a`, or `None` if this dictionary does not know it.
    fn resolve(&self, a: Atom) -> Option<&str>;
}

/// An in-memory dictionary. The default host for tests and small embeddings.
#[derive(Debug, Default)]
pub struct Strings {
    by_text: HashMap<String, Atom>,
    texts: Vec<String>,
}

impl Strings {
    /// An empty dictionary.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many distinct strings it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.texts.len()
    }

    /// True when it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

impl Symbols for Strings {
    fn intern(&mut self, s: &str) -> Option<Atom> {
        if let Some(a) = self.by_text.get(s) {
            return Some(*a);
        }
        let next = u32::try_from(self.texts.len()).ok()?.checked_add(STR_MIN)?;
        self.texts.push(s.to_string());
        self.by_text.insert(s.to_string(), next);
        Some(next)
    }

    fn resolve(&self, a: Atom) -> Option<&str> {
        let index = usize::try_from(a.checked_sub(STR_MIN)?).ok()?;
        self.texts.get(index).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable_and_dedupes() {
        let mut s = Strings::new();
        let a = s.intern("get").expect("room");
        let b = s.intern("get").expect("room");
        let c = s.intern("set").expect("room");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(s.resolve(a), Some("get"));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn the_first_string_starts_the_string_range() {
        let mut s = Strings::new();
        assert_eq!(s.intern(""), Some(STR_MIN));
    }

    #[test]
    fn an_unknown_atom_resolves_to_nothing() {
        let s = Strings::new();
        assert_eq!(s.resolve(STR_MIN), None);
        assert_eq!(s.resolve(7), None);
    }
}
