//! Finding the files to index.
//!
//! `.gitignore`-aware via the `ignore` crate, and everything it drops is
//! summarized **by reason** (`specs/05-surface.md` § `index`): a silently
//! unindexed subtree is the single most confusing failure this tool can have.
//!
//! Skipped files produce **zero** facts — not even a `file` row — so
//! `!file(F, _)` means "not indexed", which is different from "indexed and
//! empty" (`specs/02-extraction.md` § Budget).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::lang::{self, Lang};

/// Largest file tier A will parse.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// A file worth extracting.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Repo-relative, `/`-separated. This is `F` in every relation.
    pub path: String,
    /// Where it actually is.
    pub abs: PathBuf,
    /// Its language.
    pub lang: &'static Lang,
    /// Its length in bytes.
    pub size: u64,
    /// Its mtime in seconds since the epoch.
    pub mtime: u64,
}

/// What the walk dropped, by reason.
///
/// There is no `ignored` count. The `ignore` crate does not report the entries
/// its matchers drop, and recovering the number means a second traversal with
/// the matchers off — which on a repo with a populated `target/` is the most
/// expensive thing in the walk, spent on a cosmetic figure.
/// `specs/05-surface.md` § `index` records the deviation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skips {
    /// Over [`MAX_BYTES`].
    pub too_large: usize,
    /// Extension with no registered grammar, counted per extension so the
    /// summary can say `88 unsupported (.scala)`.
    pub unsupported: BTreeMap<String, usize>,
    /// Paths the walker itself could not read.
    pub unreadable: Vec<String>,
}

impl Skips {
    /// How many files were skipped in total.
    #[must_use]
    pub fn total(&self) -> usize {
        self.too_large + self.unsupported.values().sum::<usize>() + self.unreadable.len()
    }
}

/// Walk `root` and return the indexable files, sorted by path.
///
/// Sorted because `docs/plan.md` M2 requires that shuffling the file order
/// changes nothing, and the cheapest way to hold that is to not have an order
/// that depends on the filesystem in the first place.
#[must_use]
pub fn walk(root: &Path) -> (Vec<Candidate>, Skips) {
    let mut out = Vec::new();
    let mut skips = Skips::default();
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        // Honour `.gitignore` in a tree that is not a git checkout — a worktree,
        // an export, a vendored copy. The default refuses to, and the effect is
        // that `target/` silently becomes 40,000 indexed files.
        .require_git(false);
    // The store is derived and lives under the root; indexing it would be
    // indexing ourselves.
    walker.filter_entry(|e| e.file_name() != std::ffi::OsStr::new(".codeintel"));

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                skips.unreadable.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(path) = relative(root, entry.path()) else {
            continue;
        };
        let Some(lang) = lang::for_path(&path) else {
            let ext = path
                .rsplit_once('.')
                .map_or_else(|| "(none)".to_string(), |(_, ext)| format!(".{ext}"));
            *skips.unsupported.entry(ext).or_default() += 1;
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            skips.unreadable.push(path);
            continue;
        };
        if meta.len() > MAX_BYTES {
            skips.too_large += 1;
            continue;
        }
        out.push(Candidate {
            path,
            abs: entry.path().to_path_buf(),
            lang,
            size: meta.len(),
            mtime: mtime_of(&meta),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, skips)
}

/// A path under `root`, `/`-separated.
#[must_use]
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for part in rel.components() {
        let part = part.as_os_str().to_str()?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    (!out.is_empty()).then_some(out)
}

/// An mtime in whole seconds since the epoch, or zero if the platform has none.
#[must_use]
pub fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::create_dir_all(root.join("target")).expect("mkdir");
        std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").expect("write");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("README.md"), "# hi\n").expect("write");
        std::fs::write(root.join("notes.scala"), "object A\n").expect("write");
        std::fs::write(root.join("target/big.rs"), "fn ignored() {}\n").expect("write");
        std::fs::write(root.join(".gitignore"), "target/\n").expect("write");
        dir
    }

    #[test]
    fn the_walk_finds_source_and_counts_what_it_dropped() {
        let dir = fixture();
        let (found, skips) = walk(dir.path());
        let paths: Vec<&str> = found.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["src/lib.rs", "src/main.rs"]);
        assert_eq!(skips.unsupported.get(".md"), Some(&1));
        assert_eq!(skips.unsupported.get(".scala"), Some(&1));
        assert_eq!(skips.unsupported.get(".rs"), None, "target/ is gitignored");
    }

    #[test]
    fn a_file_over_the_cap_is_reported_not_parsed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let big = vec![b'\n'; usize::try_from(MAX_BYTES).unwrap_or(0) + 1];
        std::fs::write(dir.path().join("big.rs"), big).expect("write");
        let (found, skips) = walk(dir.path());
        assert!(found.is_empty());
        assert_eq!(skips.too_large, 1);
        assert_eq!(skips.total(), 1);
    }

    #[test]
    fn the_store_directory_is_not_indexed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".codeintel/seg")).expect("mkdir");
        std::fs::write(dir.path().join(".codeintel/oops.rs"), "fn a() {}").expect("write");
        std::fs::write(dir.path().join("real.rs"), "fn b() {}").expect("write");
        let (found, _) = walk(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found.first().map(|c| c.path.as_str()), Some("real.rs"));
    }

    #[test]
    fn paths_are_slash_separated_and_relative() {
        let root = Path::new("/a/b");
        assert_eq!(
            relative(root, Path::new("/a/b/c/d.rs")).as_deref(),
            Some("c/d.rs")
        );
        assert_eq!(relative(root, Path::new("/a/b")), None);
        assert_eq!(relative(root, Path::new("/x/y.rs")), None);
    }
}
