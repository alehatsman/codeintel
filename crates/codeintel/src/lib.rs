//! The CLI and the single-tool MCP server, over the engine and the fact store.
//!
//! Thin by construction: five verbs, one MCP tool, and a hard surface budget
//! asserted in CI. See `specs/05-surface.md`.
//!
//! This crate is also the *host*: it supplies the engine's regex builtin and
//! its dictionary, which is what keeps `crates/datalog` at zero dependencies.
//!
//! `index` and `query` land at M2; `schema` and `status` at M4. `mcp` is the
//! fifth and last verb.

pub mod census;
pub mod index;
pub mod load;
pub mod mcp;
pub mod overlay;
pub mod query;
pub mod regexes;
pub mod render;
pub mod schema;
pub mod status;

pub use index::{Plan, Report};
pub use load::load_facts;
pub use query::{Answer, Options, Warm};
pub use regexes::Regexes;
pub use render::{Rendered, Row, Sites, render, render_row};
pub use status::Status;
