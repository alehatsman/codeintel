//! The UI layer. This file deliberately violates the layering rule below, and
//! `docs/plan.md` M2 requires that a conformance query finds it and that
//! removing the import returns zero rows with `status: ok`.

use crate::db::conn;
use std::fmt as format_mod;

/// Draw the panel.
pub fn draw() -> bool {
    conn::open("sqlite://memory")
}
