//! The dictionary a loaded engine interns through (`specs/05-surface.md` § MCP).
//!
//! A query interns the constants it names. Interned into the store's own
//! dictionary, a constant the corpus does not contain would stay for as long as
//! the engine does. In a long-lived `codeintel mcp` that makes atom ids — and so
//! which rows a truncated answer's stable prefix holds — depend on what was
//! asked before.
//!
//! The overlay answers a corpus string with its stored atom and any other
//! string with an atom above the dictionary. What loading interns — the
//! standard library's constants, a rule file's — is **sealed** and stays; what a
//! call interns after that is **discarded** when the call ends, so the next call
//! is assigned exactly the ids a freshly loaded engine would assign.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use datalog::Symbols;
use datalog::atom::{Atom, STR_MIN};
use facts::Interner;

/// The engine's dictionary: the corpus, then the strings it does not hold.
#[derive(Debug)]
pub struct Overlay {
    corpus: Interner,
    /// Strings past the corpus, in interning order. Only the first
    /// `bounds.alive` of them resolve.
    extra: Vec<String>,
    by_text: HashMap<String, Atom>,
    bounds: Rc<Bounds>,
}

/// The host's handle on an [`Overlay`]'s extra range, kept after the overlay
/// itself is boxed into the engine.
#[derive(Debug, Clone)]
pub struct Literals(Rc<Bounds>);

#[derive(Debug, Default)]
struct Bounds {
    /// Extra strings a discard keeps: what loading interned.
    sealed: Cell<usize>,
    /// Extra strings that currently resolve.
    alive: Cell<usize>,
}

impl Overlay {
    /// An overlay over `corpus`, and the handle that seals and discards it.
    #[must_use]
    pub fn new(corpus: Interner) -> (Self, Literals) {
        let bounds = Rc::new(Bounds::default());
        let overlay = Self {
            corpus,
            extra: Vec::new(),
            by_text: HashMap::new(),
            bounds: Rc::clone(&bounds),
        };
        (overlay, Literals(bounds))
    }
}

impl Literals {
    /// Keep everything interned so far. Called once, after the rules load.
    pub fn seal(&self) {
        self.0.sealed.set(self.0.alive.get());
    }

    /// Forget everything interned since the seal. A discarded string stops
    /// resolving at once, and its id is handed out again.
    pub fn discard(&self) {
        self.0.alive.set(self.0.sealed.get());
    }
}

impl Symbols for Overlay {
    fn intern(&mut self, s: &str) -> Option<Atom> {
        if let Some(atom) = self.corpus.lookup(s) {
            return Some(atom);
        }
        // Apply a pending discard before looking, or a discarded string would
        // come back with its old id.
        let alive = self.bounds.alive.get().min(self.extra.len());
        for gone in self.extra.drain(alive..) {
            self.by_text.remove(&gone);
        }
        if let Some(&atom) = self.by_text.get(s) {
            return Some(atom);
        }
        let index = self.corpus.len().checked_add(self.extra.len())?;
        let atom = u32::try_from(index).ok()?.checked_add(STR_MIN)?;
        self.extra.push(s.to_string());
        self.by_text.insert(s.to_string(), atom);
        self.bounds.alive.set(self.extra.len());
        Some(atom)
    }

    fn resolve(&self, atom: Atom) -> Option<&str> {
        if let Some(s) = self.corpus.resolve(atom) {
            return Some(s);
        }
        let index = usize::try_from(atom.checked_sub(STR_MIN)?)
            .ok()?
            .checked_sub(self.corpus.len())?;
        if index >= self.bounds.alive.get() {
            return None;
        }
        self.extra.get(index).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(dir: &std::path::Path) -> Interner {
        let mut interner = Interner::open(dir).expect("an empty dictionary");
        interner.intern("get").expect("interns");
        interner
    }

    #[test]
    fn a_corpus_string_keeps_its_stored_atom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut stored = corpus(dir.path());
        let get = stored.intern("get").expect("interns");
        let (mut overlay, _literals) = Overlay::new(stored);
        assert_eq!(overlay.intern("get"), Some(get));
        assert_eq!(overlay.resolve(get), Some("get"));
    }

    #[test]
    fn a_discarded_literal_stops_resolving_and_its_id_is_reused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut overlay, literals) = Overlay::new(corpus(dir.path()));
        let first = overlay.intern("not_in_corpus").expect("interns");
        assert_eq!(overlay.intern("not_in_corpus"), Some(first));
        assert_eq!(overlay.resolve(first), Some("not_in_corpus"));

        literals.discard();
        assert_eq!(overlay.resolve(first), None);
        // A fresh engine would give the next call's first literal this id.
        assert_eq!(overlay.intern("something_else"), Some(first));
        assert_eq!(overlay.resolve(first), Some("something_else"));
    }

    #[test]
    fn what_loading_interned_survives_a_discard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut overlay, literals) = Overlay::new(corpus(dir.path()));
        let rule_constant = overlay.intern("constructor").expect("interns");
        literals.seal();

        let literal = overlay.intern("from_a_query").expect("interns");
        assert_ne!(literal, rule_constant);
        literals.discard();

        assert_eq!(overlay.resolve(rule_constant), Some("constructor"));
        assert_eq!(overlay.intern("constructor"), Some(rule_constant));
        assert_eq!(overlay.resolve(literal), None);
        assert_eq!(overlay.intern("from_another_query"), Some(literal));
    }
}
