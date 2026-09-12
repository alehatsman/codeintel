//! `manifest.json`: index metadata and the per-file segment table.
//!
//! The manifest is the only thing that names segments, so a reader sees either
//! the old index or the new one and never a mix (`specs/04-storage.md`
//! § Concurrency). It is written last, after every segment is `fsync`ed.

use std::collections::BTreeMap;
use std::io::{Error, ErrorKind, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::schema::SCHEMA_VERSION;

/// The manifest file name, inside `.codeintel/`.
pub const MANIFEST: &str = "manifest.json";

/// Index metadata and the file table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The schema this index was written against. A mismatch is `stale`.
    pub schema_version: u32,
    /// When the index was first created, RFC 3339 UTC.
    pub created_at: String,
    /// Indexed roots. An array from day one even though v1 writes one element.
    pub roots: Vec<String>,
    /// The binary that wrote it.
    pub writer_version: String,
    /// blake3 over the extractor's own inputs — `extract::fingerprint`.
    pub extractor_fingerprint: String,
    /// Bytes of `dict.bin` a reader may trust.
    pub dict_bin_len: u64,
    /// Bytes of `dict.idx` a reader may trust.
    pub dict_idx_len: u64,
    /// Incremented by `--rebuild`, which renumbers every atom.
    pub dict_generation: u64,
    /// SCIP inputs ingested, empty until M3.
    #[serde(default)]
    pub scip: Vec<ScipInput>,
    /// Indexed files, keyed by repo-relative path with `/` separators.
    pub files: BTreeMap<String, FileEntry>,
}

/// One ingested SCIP index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipInput {
    /// Where it was read from.
    pub path: String,
    /// `Metadata.tool_info`, so `status` can name what produced it.
    pub tool: String,
    /// Its mtime in seconds since the epoch.
    pub mtime: u64,
    /// How many documents it carried.
    pub documents: u64,
}

/// One indexed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Segment file name inside `seg/`.
    pub seg: String,
    /// Source mtime in seconds since the epoch.
    pub mtime: u64,
    /// Source length in bytes.
    pub size: u64,
    /// `blake3:<hex>` over the source bytes.
    pub hash: String,
    /// Its language, as in `file(F, Lang)`.
    pub lang: String,
    /// Which tiers contributed: `ts`, `scip`.
    pub tiers: Vec<String>,
}

impl FileEntry {
    /// True when `mtime` and `size` both match — the fast path that lets a
    /// reindex skip hashing (`specs/04-storage.md` § Manifest).
    #[must_use]
    pub fn looks_unchanged(&self, mtime: u64, size: u64) -> bool {
        self.mtime == mtime && self.size == size
    }
}

impl Manifest {
    /// A fresh manifest for `root`, stamped now.
    #[must_use]
    pub fn new(root: &Path, fingerprint: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            created_at: rfc3339(now()),
            roots: vec![root.display().to_string()],
            writer_version: concat!("codeintel ", env!("CARGO_PKG_VERSION")).to_string(),
            extractor_fingerprint: fingerprint,
            dict_bin_len: 0,
            dict_idx_len: 0,
            dict_generation: 0,
            scip: Vec::new(),
            files: BTreeMap::new(),
        }
    }

    /// Read `dir/manifest.json`, or `None` when there is no index here.
    ///
    /// # Errors
    /// I/O failure, or JSON that is not a manifest. A manifest written by a
    /// different `schema_version` parses — the caller reports it as `stale`
    /// rather than reading its segments.
    pub fn open(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(MANIFEST);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        let manifest = serde_json::from_str(&text).map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("{MANIFEST} is not readable: {e}. Repair with `rm -rf .codeintel`"),
            )
        })?;
        Ok(Some(manifest))
    }

    /// Write it to `dir/manifest.json`, `fsync`ed and renamed into place.
    ///
    /// # Errors
    /// I/O failure while writing, syncing, or renaming.
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("{MANIFEST} is not JSON: {e}"),
            )
        })?;
        crate::atomic_write(&dir.join(MANIFEST), text.as_bytes())
    }

    /// True when this index was written by a build that speaks another schema.
    #[must_use]
    pub fn is_stale_schema(&self) -> bool {
        self.schema_version != SCHEMA_VERSION
    }
}

/// Seconds since the Unix epoch, or zero for a clock before it.
#[must_use]
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Seconds since the Unix epoch, as RFC 3339 in UTC.
///
/// Written out rather than taken from a date crate: the manifest needs one
/// timestamp for a human reading a bug report, and `specs/00-overview.md`
/// keeps the dependency budget tight enough that a calendar is not worth a
/// crate. The civil-date conversion below is Howard Hinnant's `civil_from_days`.
#[must_use]
pub fn rfc3339(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    let (hour, minute, second) = (rest / 3_600, (rest % 3_600) / 60, rest % 60);

    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_a_known_date_format() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_757_000_000), "2025-09-04T15:33:20Z");
        // A leap day, which is the case the civil-date arithmetic is for.
        assert_eq!(rfc3339(1_709_208_000), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn a_manifest_round_trips() {
        let mut manifest = Manifest::new(Path::new("/tmp/repo"), "blake3:abc".to_string());
        manifest.files.insert(
            "src/store.rs".to_string(),
            FileEntry {
                seg: "a3f1.bin".to_string(),
                mtime: 1_757_000_000,
                size: 4_021,
                hash: "blake3:9c2e".to_string(),
                lang: "rust".to_string(),
                tiers: vec!["ts".to_string()],
            },
        );
        let dir = tempfile::tempdir().expect("tempdir");
        manifest.save(dir.path()).expect("saves");
        let back = Manifest::open(dir.path()).expect("reads").expect("present");
        assert_eq!(back, manifest);
    }

    #[test]
    fn no_manifest_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(Manifest::open(dir.path()).expect("reads"), None);
    }

    #[test]
    fn the_fast_path_needs_both_halves() {
        let entry = FileEntry {
            seg: "a.bin".to_string(),
            mtime: 10,
            size: 20,
            hash: String::new(),
            lang: "rust".to_string(),
            tiers: Vec::new(),
        };
        assert!(entry.looks_unchanged(10, 20));
        assert!(!entry.looks_unchanged(11, 20));
        assert!(!entry.looks_unchanged(10, 21));
    }
}
