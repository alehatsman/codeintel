//! The `match` builtin, injected into the engine.
//!
//! `crates/datalog` names an interface and takes no dependency; this crate
//! satisfies it with the `regex` crate. Hand-rolling a backtracker would buy no
//! dependency reduction — `regex` is already transitive through `tree-sitter` —
//! and would give up linear-time matching, so a pathological pattern would need
//! a step budget the host cannot honestly measure.

use datalog::matcher::{Matcher, Regexes as Compile};

/// Compiles patterns with the `regex` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct Regexes;

impl Regexes {
    /// A fresh compiler.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// One compiled pattern.
#[derive(Debug)]
struct Pattern(regex::Regex);

impl Matcher for Pattern {
    fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text)
    }
}

impl Compile for Regexes {
    fn compile(&self, pattern: &str) -> Result<Box<dyn Matcher>, String> {
        // The compiler's own diagnostic, verbatim: it names the offending
        // position in the pattern, which no paraphrase of ours would.
        regex::Regex::new(pattern)
            .map(|r| Box::new(Pattern(r)) as Box<dyn Matcher>)
            .map_err(|e| e.to_string())
    }
}
