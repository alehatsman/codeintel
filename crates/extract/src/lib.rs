//! Extraction: the tree-sitter tier, the SCIP tier, and the anchor join.
//!
//! Extractors emit facts and resolve nothing — every tuple traces to a byte
//! range or a SCIP field, and cross-file knowledge enters only through rules.
//! See `specs/02-extraction.md`.
//!
//! Tier B and the anchor join land at M3; this is tier A.

pub mod error;
pub mod lang;
pub mod sweep;
pub mod symbol;
pub mod tier_a;
pub mod walk;

pub use error::{Error, Result};
pub use lang::{Export, LANGS, Lang};
pub use tier_a::{Counts, Extractor};
pub use walk::{Candidate, walk};

/// blake3 over everything that decides what a fact file looks like.
///
/// The manifest carries it and a mismatch forces a full re-extract
/// (`specs/04-storage.md` § Manifest). Without it, an extractor fix reaches
/// only the files a user happens to edit afterwards: half the index is built
/// by the old query and half by the new one, `status` says `ok`, and nothing
/// will ever reconcile them.
#[must_use]
pub fn fingerprint() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    for lang in LANGS {
        hasher.update(lang.name.as_bytes());
        for ext in lang.extensions {
            hasher.update(ext.as_bytes());
        }
        // The authored queries and the kind mapping are the extractor, as much
        // as the code is.
        hasher.update(lang.tags.as_bytes());
        hasher.update(lang.imports.as_bytes());
        for (from, to) in lang.kind_remap {
            hasher.update(from.as_bytes());
            hasher.update(to.as_bytes());
        }
        hasher.update(format!("{:?}", lang.export).as_bytes());
        for marker in lang.doc_markers {
            hasher.update(marker.as_bytes());
        }
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_is_stable_within_a_build() {
        assert_eq!(fingerprint(), fingerprint());
        assert!(fingerprint().starts_with("blake3:"));
    }
}
