//! `manifest.json`: index metadata and the per-file segment table.
//!
//! The manifest is the only thing that names segments, so a reader sees either
//! the old index or the new one and never a mix (`specs/04-storage.md`
//! § Concurrency). It is written last, after every segment is `fsync`ed.

use std::collections::BTreeMap;
use std::fmt;
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
    /// The second the last committing refresh that observed every file
    /// started. A file whose mtime is at or past it may have changed after
    /// that refresh read it (`specs/04-storage.md` § Manifest). Zero in a
    /// manifest written before the field existed, which clears nothing.
    #[serde(default)]
    pub indexed_at: u64,
    /// SCIP inputs ingested, empty until M3.
    #[serde(default)]
    pub scip: Vec<ScipInput>,
    /// Symbols defined in more than one document across the merge of every
    /// SCIP input, which the anchor join refused. Not per input: a collision
    /// can span two, so no input has a count of its own
    /// (`specs/04-storage.md` § Manifest). Zero until the inputs are parsed.
    #[serde(default)]
    pub scip_collisions: u64,
    /// Indexed files, keyed by repo-relative path with `/` separators.
    pub files: BTreeMap<String, FileEntry>,
}

/// One ingested SCIP index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipInput {
    /// Where it was read from.
    pub path: String,
    /// `Metadata.tool_info`, so `status` can name what produced it. Empty
    /// until the index is actually parsed, which a refresh that changes
    /// nothing never does.
    pub tool: String,
    /// Its mtime in seconds since the epoch.
    pub mtime: u64,
    /// Its length in bytes. With `mtime`, this is what decides whether the
    /// reference graph has to be rebuilt.
    pub size: u64,
    /// How many documents it carried. Zero until it is parsed.
    pub documents: u64,
    /// Occurrences it placed nowhere: the document declares no position
    /// encoding and the line is not ASCII before the column, where UTF-8,
    /// UTF-16 and UTF-32 disagree (`specs/02-extraction.md` § Position
    /// normalization). Zero until it is parsed.
    #[serde(default)]
    pub ambiguous: u64,
}

impl ScipInput {
    /// True when this is the same file, byte for byte as far as a `stat` can
    /// tell.
    ///
    /// `tool` and `documents` are deliberately not compared: they are only
    /// known after the index is parsed, and a refresh that changes nothing
    /// never parses it. Comparing them would make every refresh look like a
    /// changed SCIP input and re-extract the whole tree.
    #[must_use]
    pub fn same_bytes(&self, other: &Self) -> bool {
        self.path == other.path && self.mtime == other.mtime && self.size == other.size
    }
}

/// One indexed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Segment file name inside `seg/`.
    pub seg: SegName,
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
    /// The `hash` the SCIP inputs last saw: this file's, when it was not
    /// newer than them at ingest, else the one recorded before; `""` when
    /// they never saw any version. `None` in an index written before this
    /// field existed, which falls back to mtime until the next ingest
    /// (`specs/04-storage.md` § Manifest).
    #[serde(default)]
    pub scip_hash: Option<String>,
}

impl FileEntry {
    /// True when `mtime` and `size` both match — the fast path that lets a
    /// reindex skip hashing (`specs/04-storage.md` § Manifest).
    #[must_use]
    pub fn looks_unchanged(&self, mtime: u64, size: u64) -> bool {
        self.mtime == mtime && self.size == size
    }

    /// True when the file can be trusted unchanged without hashing it: `mtime`
    /// and `size` match, and `mtime` is before `indexed_at`.
    ///
    /// mtime has one-second resolution, so an edit in the second a refresh
    /// read the file, at the same length, leaves both halves as recorded. A
    /// file touched at or past `indexed_at` is never cleared here — git's
    /// racy-clean rule (`specs/04-storage.md` § Manifest).
    #[must_use]
    pub fn is_clean(&self, mtime: u64, size: u64, indexed_at: u64) -> bool {
        self.looks_unchanged(mtime, size) && mtime < indexed_at
    }

    /// True when the SCIP inputs, the newest of which was written at
    /// `newest_scip`, did not see this file's current bytes.
    ///
    /// By content when the manifest recorded what they saw, so a touched or
    /// restored file clears itself; by mtime only for an index written before
    /// `scip_hash` existed.
    #[must_use]
    pub fn scip_stale(&self, newest_scip: u64) -> bool {
        match &self.scip_hash {
            Some(seen) => *seen != self.hash,
            None => self.mtime > newest_scip,
        }
    }
}

/// A segment file name: 64 lowercase hex digits and `.bin`, the shape
/// [`crate::segment_name`] produces and the only one that parses.
///
/// It is joined onto `seg/` to load a segment and to unlink a retired one, and
/// the manifest it comes from sits in a directory a cloned repository can ship.
/// A bare `String` there let `"/home/x/.ssh/id_ed25519"` or `"../../main.rs"`
/// through `Path::join` and into `remove_file` (`specs/04-storage.md`
/// § Manifest).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SegName(String);

impl SegName {
    /// Hex digits in a blake3 digest.
    const HEX: usize = 64;

    /// The name for a segment's encoded bytes.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(format!("{}.bin", blake3::hash(bytes).to_hex()))
    }

    /// The name as written in the manifest. Empty for the [`Default`]
    /// placeholder, which [`crate::Store::put`] replaces.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SegName {
    type Error = String;

    fn try_from(name: String) -> std::result::Result<Self, String> {
        let hex = name.strip_suffix(".bin").unwrap_or_default();
        if hex.len() == Self::HEX && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(Self(name))
        } else {
            Err(format!(
                "segment name {name:?} is not {} lowercase hex digits and `.bin`",
                Self::HEX
            ))
        }
    }
}

impl From<SegName> for String {
    fn from(name: SegName) -> Self {
        name.0
    }
}

impl AsRef<Path> for SegName {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl fmt::Display for SegName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
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
            indexed_at: 0,
            scip: Vec::new(),
            scip_collisions: 0,
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
        let manifest = serde_json::from_str(&text)
            .map_err(|e| crate::corrupt(&format!("{MANIFEST} is not readable: {e}")))?;
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

    fn seg_name() -> SegName {
        SegName::try_from(format!("{}.bin", "a3f1".repeat(16))).expect("a valid name")
    }

    #[test]
    fn a_manifest_round_trips() {
        let mut manifest = Manifest::new(Path::new("/tmp/repo"), "blake3:abc".to_string());
        manifest.files.insert(
            "src/store.rs".to_string(),
            FileEntry {
                seg: seg_name(),
                mtime: 1_757_000_000,
                size: 4_021,
                hash: "blake3:9c2e".to_string(),
                lang: "rust".to_string(),
                tiers: vec!["ts".to_string()],
                scip_hash: Some("blake3:9c2e".to_string()),
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
            seg: SegName::default(),
            mtime: 10,
            size: 20,
            hash: String::new(),
            lang: "rust".to_string(),
            scip_hash: None,
            tiers: Vec::new(),
        };
        assert!(entry.looks_unchanged(10, 20));
        assert!(!entry.looks_unchanged(11, 20));
        assert!(!entry.looks_unchanged(10, 21));
    }

    #[test]
    fn a_file_touched_in_or_after_the_indexing_second_is_never_clean() {
        let entry = FileEntry {
            seg: SegName::default(),
            mtime: 10,
            size: 20,
            hash: String::new(),
            lang: "rust".to_string(),
            scip_hash: None,
            tiers: Vec::new(),
        };
        assert!(entry.is_clean(10, 20, 11));
        assert!(!entry.is_clean(10, 20, 10), "the indexing second itself");
        assert!(
            !entry.is_clean(10, 20, 0),
            "a manifest from before indexed_at"
        );
        assert!(!entry.is_clean(10, 21, 50));
    }

    #[test]
    fn scip_staleness_is_by_content_when_recorded_and_by_mtime_otherwise() {
        let mut entry = FileEntry {
            seg: SegName::default(),
            mtime: 10,
            size: 20,
            hash: "blake3:aa".to_string(),
            lang: "rust".to_string(),
            tiers: vec!["ts".to_string()],
            scip_hash: Some("blake3:aa".to_string()),
        };
        assert!(!entry.scip_stale(5), "newer than SCIP but the same bytes");
        entry.hash = "blake3:bb".to_string();
        assert!(entry.scip_stale(50), "older than SCIP but different bytes");
        entry.scip_hash = Some(String::new());
        assert!(entry.scip_stale(50), "never seen");
        entry.scip_hash = None;
        assert!(entry.scip_stale(5), "no record: by mtime");
        assert!(!entry.scip_stale(50));
    }

    #[test]
    fn a_manifest_without_scip_hash_still_parses() {
        // An index written before the field existed.
        let text = r#"{"schema_version":1,"created_at":"","roots":[],"writer_version":"",
            "extractor_fingerprint":"","dict_bin_len":0,"dict_idx_len":0,"dict_generation":0,
            "files":{"a.rs":{"seg":"SEG","mtime":1,"size":2,"hash":"h","lang":"rust","tiers":["ts"]}}}"#
            .replace("SEG", seg_name().as_str());
        let manifest: Manifest = serde_json::from_str(&text).expect("parses");
        assert_eq!(manifest.files["a.rs"].scip_hash, None);
    }

    #[test]
    fn a_segment_name_of_any_other_shape_is_refused() {
        let hex = "0123456789abcdef".repeat(4);
        SegName::try_from(format!("{hex}.bin")).expect("a content hash parses");
        for bad in [
            String::new(),
            "/etc/passwd".to_string(),
            format!("/{hex}.bin"),
            format!("../{hex}.bin"),
            format!("seg/../{hex}.bin"),
            format!("{}.bin", hex.get(1..).unwrap_or_default()),
            format!("{hex}0.bin"),
            format!("{}.bin", hex.to_uppercase()),
            hex.clone(),
            format!("{hex}.bin.tmp"),
        ] {
            assert!(SegName::try_from(bad.clone()).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_manifest_naming_a_path_outside_seg_does_not_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut manifest = Manifest::new(dir.path(), "blake3:abc".to_string());
        manifest.files.insert(
            "gone.rs".to_string(),
            FileEntry {
                seg: seg_name(),
                mtime: 1,
                size: 2,
                hash: "blake3:00".to_string(),
                lang: "rust".to_string(),
                tiers: vec!["ts".to_string()],
                scip_hash: None,
            },
        );
        manifest.save(dir.path()).expect("saves");
        let path = dir.path().join(MANIFEST);
        let text = std::fs::read_to_string(&path).expect("reads");
        for hostile in ["/home/x/.ssh/id_ed25519", "../../src/main.rs"] {
            std::fs::write(&path, text.replace(seg_name().as_str(), hostile)).expect("writes");
            let error = Manifest::open(dir.path()).expect_err("a hostile name is refused");
            assert!(error.to_string().contains("rm -rf .codeintel"), "{error}");
        }
    }
}
