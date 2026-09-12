//! A Datalog engine over interned tuples.
//!
//! The engine knows nothing about code: a term is a `u32`, a relation is a flat
//! sorted `Vec<u32>`, and every code-intel concept lives above this crate. That
//! seam is what keeps evaluation testable, and `tests/seam.rs` enforces it.
//!
//! Empty at M0 — the skeleton milestone wires the quality gate and the crate
//! seams only. See `specs/03-datalog.md` and `docs/plan.md` M1.
