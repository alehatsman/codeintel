//! The store: a struct with an inherent impl, so that `parent` has something
//! to disagree with the file about.

use std::collections::HashMap;
use std::fmt::Debug as Show;

/// An entry in the store.
#[derive(Debug, Clone)]
pub struct Entry {
    pub value: String,
}

/// A tiny key-value store.
pub struct Store {
    entries: HashMap<String, Entry>,
}

impl Store {
    /// Read one entry.
    ///
    /// Two lines of documentation, to prove newlines survive.
    pub fn get(&self, key: &str) -> Option<Entry> {
        let found = self.entries.get(key);
        found.cloned()
    }

    /// Write one entry.
    #[inline]
    pub fn put(&mut self, key: String, value: String) {
        self.entries.insert(key, Entry { value });
    }

    fn evict(&mut self) {
        self.entries.clear();
    }
}

/// A free function that calls a method, so `calls` has an edge to find.
pub fn warm(store: &Store) -> Option<Entry> {
    store.get("warm")
}

mod detail {
    /// Nested in a module, so the descriptor chain is more than one hop.
    pub fn helper() -> u32 {
        7
    }
}

fn show<T: Show>(_value: T) {}
