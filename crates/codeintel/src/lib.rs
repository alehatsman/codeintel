//! The CLI and the single-tool MCP server, over the engine and the fact store.
//!
//! Thin by construction: five verbs, one MCP tool, and a hard surface budget
//! asserted in CI. See `specs/05-surface.md`.
//!
//! At M1 this crate is the *host*: it supplies the engine's regex builtin and
//! its dictionary, which is what keeps `crates/datalog` at zero dependencies.
//! The verbs land at M2 and M4.

pub mod load;
pub mod regexes;

pub use load::{load_facts, render, render_row};
pub use regexes::Regexes;
