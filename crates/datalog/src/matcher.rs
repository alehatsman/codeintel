//! The regex seam.
//!
//! `crates/datalog` has no dependencies, and a hand-rolled backtracker would
//! buy none — `regex` is already a non-optional transitive dependency of
//! `tree-sitter` — while giving up linear-time matching. So the engine names an
//! interface and the host installs an implementation.

use core::fmt::Debug;

/// A compiled pattern.
pub trait Matcher: Debug + Send + Sync {
    /// True when the pattern matches anywhere in `text`.
    fn is_match(&self, text: &str) -> bool;
}

/// Compiles patterns for the `match` builtin.
pub trait Regexes: Debug + Send + Sync {
    /// Compile `pattern`.
    ///
    /// # Errors
    /// Returns the engine's own message for an invalid pattern; it is shown to
    /// the user verbatim, so it should be the compiler's diagnostic rather than
    /// a paraphrase.
    fn compile(&self, pattern: &str) -> Result<Box<dyn Matcher>, String>;
}
