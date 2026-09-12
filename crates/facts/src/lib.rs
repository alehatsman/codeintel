//! The fact store: string interner, columnar segments, and the manifest.
//!
//! After interning, every fact is a fixed-width row of `u32`, so the on-disk
//! bytes are the in-memory representation and loading is `mmap` plus
//! concatenate. See `specs/04-storage.md`.
//!
//! Segments and the manifest land at M2; M1 is the dictionary.

pub mod intern;

pub use intern::{Dict, Interner};
