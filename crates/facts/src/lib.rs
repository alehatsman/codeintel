//! The fact store: string interner, columnar segments, and the manifest.
//!
//! After interning, every fact is a fixed-width row of `u32`, so the on-disk
//! bytes are the in-memory representation and loading is `mmap` plus
//! concatenate. See `specs/04-storage.md`.

pub mod intern;
pub mod lock;
pub mod manifest;
pub mod schema;
pub mod segment;
pub mod store;

pub use intern::{Dict, Interner};
pub use lock::{Contended, Lock};
pub use manifest::{FileEntry, Manifest, ScipInput};
pub use schema::{RELATIONS, Rel, SCHEMA_VERSION};
pub use segment::Segment;
pub use store::Store;

use std::io::{Result, Write};
use std::path::Path;

/// Write `bytes` to `path` via a `.tmp` file that is `fsync`ed and renamed.
///
/// The `fsync` is not optional (`specs/04-storage.md` § Concurrency): without
/// it a power loss can land the rename while the data is still in page cache,
/// leaving a manifest that names a **zero-filled** segment. Atom `0` is the
/// integer zero, so those bytes decode as well-formed facts saying every
/// symbol lives at line 0 — no error, no status, a silently wrong answer.
///
/// # Errors
/// I/O failure creating, writing, syncing, or renaming.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.tmp"),
        None => "tmp".to_string(),
    });
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// `blake3:<hex>` over some bytes — a file's content hash, or a fingerprint.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// The segment file name for a repo-relative path.
///
/// Hashing the path rather than escaping it keeps every segment name a fixed
/// length and flat, with no directory tree to create and no path-length limit
/// to hit. The manifest is what maps it back.
#[must_use]
pub fn segment_name(path: &str) -> String {
    format!("{}.bin", blake3::hash(path.as_bytes()).to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atomic_write_leaves_no_tmp_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("seg").join("a.bin");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        atomic_write(&path, b"hello").expect("writes");
        assert_eq!(std::fs::read(&path).expect("reads"), b"hello");
        let left: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
            .expect("lists")
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

    #[test]
    fn a_segment_name_is_a_function_of_the_path() {
        assert_eq!(segment_name("src/a.rs"), segment_name("src/a.rs"));
        assert_ne!(segment_name("src/a.rs"), segment_name("src/b.rs"));
        assert!(
            Path::new(&segment_name("src/a.rs"))
                .extension()
                .is_some_and(|ext| ext == "bin")
        );
    }
}
