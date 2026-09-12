//! The fixture crate. Declares the modules so that this tree builds, which is
//! what tier B needs: `rust-analyzer scip .` resolves nothing in a pile of
//! loose files.

pub mod app;
pub mod db;
pub mod kinds;
pub mod net;
pub mod store;
pub mod ui;
