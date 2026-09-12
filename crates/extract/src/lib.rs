//! Extraction: the tree-sitter tier, the SCIP tier, and the anchor join.
//!
//! Extractors emit facts and resolve nothing — every tuple traces to a byte
//! range or a SCIP field, and cross-file knowledge enters only through rules.
//! See `specs/02-extraction.md`.
//!
//! Empty at M0. See `docs/plan.md` M2 and M3.
