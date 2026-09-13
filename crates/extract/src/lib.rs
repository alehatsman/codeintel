//! Extraction: the tree-sitter tier, the SCIP tier, and the anchor join.
//!
//! Extractors emit facts and resolve nothing — every tuple traces to a byte
//! range or a SCIP field, and cross-file knowledge enters only through rules.
//! See `specs/02-extraction.md`.
//!
//! Tier A runs first, entirely, then tier B: the anchor join needs tier A's
//! `def_name` index to match against.

pub mod error;
pub mod lang;
pub mod scip;
pub mod sweep;
pub mod symbol;
pub mod tier_a;
pub mod tier_b;
pub mod walk;

pub use error::{Error, Result};
pub use lang::{Export, LANGS, Lang};
pub use scip::Ingest;
pub use tier_a::{Counts, Extracted, Extractor};
pub use tier_b::{Anchor, Anchors};
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
    // The version above has been 0.1.0 through every extractor fix, so the
    // code itself goes in. A comment edit re-extracts too; that is the cheap
    // direction to be wrong in.
    for (name, source) in SOURCES {
        hasher.update(name.as_bytes());
        hasher.update(source.as_bytes());
    }
    // The grammars are caret requirements: `cargo update` changes parses
    // without changing a byte of ours.
    for (name, version) in pinned(LOCKFILE) {
        hasher.update(name.as_bytes());
        hasher.update(version.as_bytes());
    }
    for lang in LANGS {
        // Destructured with no `..`: a field added to `Lang` does not compile
        // until it is hashed here or named as changing no fact. That is how
        // `vis_inherits_under` was missed.
        let Lang {
            name,
            extensions,
            // A grammar is covered by its locked version above.
            language: _,
            tags,
            imports,
            kind_remap,
            export,
            vis_inherits_under,
            doc_markers,
            comment_kinds,
            attribute_kinds,
            keyword_kinds,
            sig_stops,
            function_values,
            // It changes no fact.
            indexer: _,
        } = lang;
        // The authored queries and the kind mapping are the extractor, as much
        // as the code is.
        field(&mut hasher, "name", [*name]);
        field(&mut hasher, "tags", [*tags]);
        field(&mut hasher, "imports", [*imports]);
        field(
            &mut hasher,
            "kind_remap",
            kind_remap.iter().flat_map(|(a, b)| [*a, *b]),
        );
        field(&mut hasher, "export", [format!("{export:?}").as_str()]);
        field(&mut hasher, "extensions", extensions.iter().copied());
        field(
            &mut hasher,
            "vis_inherits_under",
            vis_inherits_under.iter().copied(),
        );
        field(&mut hasher, "doc_markers", doc_markers.iter().copied());
        field(&mut hasher, "comment_kinds", comment_kinds.iter().copied());
        field(
            &mut hasher,
            "attribute_kinds",
            attribute_kinds.iter().copied(),
        );
        field(&mut hasher, "keyword_kinds", keyword_kinds.iter().copied());
        field(
            &mut hasher,
            "function_values",
            function_values.iter().copied(),
        );
        let stops: Vec<String> = sig_stops.iter().map(char::to_string).collect();
        field(&mut hasher, "sig_stops", stops.iter().map(String::as_str));
    }
    // The shared tables are as much the extractor as the per-language rows.
    // `indexer` is deliberately absent from all of this: it changes no fact.
    for name in lang::KINDS.iter().chain(lang::ROLES).chain(lang::TYPE_LIKE) {
        hasher.update(name.as_bytes());
    }
    for (from, to) in scip::KIND_MAP {
        hasher.update(from.as_bytes());
        hasher.update(to.as_bytes());
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

/// Hash one named list with explicit boundaries, so moving an item from one
/// list to its neighbour changes the fingerprint.
fn field<'a>(hasher: &mut blake3::Hasher, tag: &str, items: impl IntoIterator<Item = &'a str>) {
    hasher.update(tag.as_bytes());
    hasher.update(&[0]);
    for item in items {
        hasher.update(&(item.len() as u64).to_le_bytes());
        hasher.update(item.as_bytes());
    }
    hasher.update(&[1]);
}

/// Every file in `crates/extract/src`. A test fails when this list and the
/// directory disagree.
const SOURCES: &[(&str, &str)] = &[
    ("error.rs", include_str!("error.rs")),
    ("lang.rs", include_str!("lang.rs")),
    ("lib.rs", include_str!("lib.rs")),
    ("scip.rs", include_str!("scip.rs")),
    ("sweep.rs", include_str!("sweep.rs")),
    ("symbol.rs", include_str!("symbol.rs")),
    ("tier_a.rs", include_str!("tier_a.rs")),
    ("tier_b.rs", include_str!("tier_b.rs")),
    ("walk.rs", include_str!("walk.rs")),
];

const LOCKFILE: &str = include_str!("../../../Cargo.lock");

/// `(name, version)` of every locked crate that decides a parse: the
/// tree-sitter runtime and grammars, and the SCIP decoder.
fn pinned(lockfile: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut name = None;
    for line in lockfile.lines() {
        if let Some(n) = line.strip_prefix("name = ") {
            name = Some(n.trim_matches('"'));
        } else if let (Some(n), Some(v)) = (name, line.strip_prefix("version = ")) {
            if n == "tree-sitter" || n.starts_with("tree-sitter-") || n == "scip" || n == "protobuf"
            {
                out.push((n, v.trim_matches('"')));
            }
            name = None;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_is_stable_within_a_build() {
        assert_eq!(fingerprint(), fingerprint());
        assert!(fingerprint().starts_with("blake3:"));
    }

    #[test]
    fn every_extractor_source_file_is_hashed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("read src")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| {
                std::path::Path::new(n)
                    .extension()
                    .is_some_and(|e| e == "rs")
            })
            .collect();
        on_disk.sort();
        let hashed: Vec<&str> = SOURCES.iter().map(|(n, _)| *n).collect();
        assert_eq!(on_disk, hashed, "SOURCES must list every file in src/");
    }

    #[test]
    fn the_lockfile_pins_every_grammar_and_the_scip_decoder() {
        let names: Vec<&str> = pinned(LOCKFILE).iter().map(|(n, _)| *n).collect();
        for want in [
            "tree-sitter",
            "tree-sitter-go",
            "tree-sitter-python",
            "tree-sitter-rust",
            "tree-sitter-typescript",
            "scip",
            "protobuf",
        ] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
    }

    #[test]
    fn moving_an_item_between_lists_changes_the_hash() {
        let hash = |a: &[&str], b: &[&str]| {
            let mut h = blake3::Hasher::new();
            field(&mut h, "a", a.iter().copied());
            field(&mut h, "b", b.iter().copied());
            h.finalize()
        };
        assert_ne!(hash(&["enum"], &[]), hash(&[], &["enum"]));
        assert_ne!(hash(&["ab", "c"], &[]), hash(&["a", "bc"], &[]));
    }
}
