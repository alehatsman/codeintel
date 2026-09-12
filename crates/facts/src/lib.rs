//! The fact store: string interner, columnar segments, and the manifest.
//!
//! After interning, every fact is a fixed-width row of `u32`, so the on-disk
//! bytes are the in-memory representation and loading is `mmap` plus
//! concatenate. See `specs/04-storage.md`.
//!
//! Empty at M0. See `docs/plan.md` M1 and M2.
