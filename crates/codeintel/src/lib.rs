//! The CLI and the single-tool MCP server, over the engine and the fact store.
//!
//! Thin by construction: five verbs, one MCP tool, and a hard surface budget
//! asserted in CI. See `specs/05-surface.md`.
//!
//! This crate is also the *host*: it supplies the engine's regex builtin and
//! its dictionary, which is what keeps `crates/datalog` at zero dependencies.
//!
//! `index` and `query` land here at M2. `schema`, `status` and `mcp` are M4.

pub mod index;
pub mod load;
pub mod query;
pub mod regexes;
pub mod status;

pub use index::{Plan, Report};
pub use load::{load_facts, render, render_row};
pub use query::{Answer, Options};
pub use regexes::Regexes;
pub use status::Status;
