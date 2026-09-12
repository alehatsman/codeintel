//! A Datalog engine over interned tuples.
//!
//! The engine knows nothing about code: a term is a `u32`, a relation is a flat
//! sorted `Vec<u32>`, and every code-intel concept lives above this crate. That
//! seam is what keeps evaluation testable, and `tests/seam.rs` enforces it.
//!
//! The dialect is Prolog-flavoured and deliberately small — no functors, no
//! lists, no cut. See `specs/03-datalog.md` for the grammar, the safety rules
//! and the evaluation strategy.

pub mod ast;
pub mod atom;
pub mod check;
#[cfg(test)]
mod conformance;
pub mod db;
pub mod diag;
pub mod engine;
pub mod lex;
pub mod limits;
pub mod matcher;
pub mod parse;
pub mod relation;
pub mod solve;
pub mod strata;
pub mod symbols;

pub use atom::{Atom, Term};
pub use db::Db;
pub use diag::{Diagnostic, Status};
pub use engine::{Engine, QueryResult};
pub use limits::{Limits, Stats};
pub use matcher::{Matcher, Regexes};
pub use parse::parse;
pub use relation::Relation;
pub use symbols::{Strings, Symbols};
