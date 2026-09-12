//! The store on disk: `.codeintel/` — dictionary, segments, manifest.
//!
//! One writer, many readers. Writing a file's facts is: encode the segment,
//! `fsync` it, rename it into place. Committing is: append the dictionary,
//! then `fsync` and rename the manifest **last**, because the manifest is the
//! only thing that names segments (`specs/04-storage.md` § Concurrency).

use std::collections::BTreeMap;
use std::io::Result;
use std::path::{Path, PathBuf};

use datalog::atom::Atom;
use datalog::relation::Relation;

use crate::intern::Interner;
use crate::manifest::{FileEntry, Manifest};
use crate::schema::{self, Rel};
use crate::segment::Segment;

/// The store directory inside a repository root.
pub const DIR: &str = ".codeintel";

/// Where segments live inside [`DIR`].
pub const SEG: &str = "seg";

/// An open store. Empty and unwritten until [`Store::commit`].
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    dir: PathBuf,
    interner: Interner,
    manifest: Manifest,
    /// True once this run has an index on disk to compare against.
    existing: bool,
}

impl Store {
    /// Open the store under `root`, or set up an empty one in memory.
    ///
    /// Nothing is written here — a `query` against a repository with no index
    /// must be able to say `no-index` without creating one.
    ///
    /// # Errors
    /// I/O failure, or a malformed dictionary or manifest.
    pub fn open(root: impl Into<PathBuf>, fingerprint: &str) -> Result<Self> {
        let root = root.into();
        let dir = root.join(DIR);
        let manifest = Manifest::open(&dir)?;
        let existing = manifest.is_some();
        Ok(Self {
            interner: Interner::open(&dir)?,
            manifest: manifest.unwrap_or_else(|| Manifest::new(&root, fingerprint.to_string())),
            root,
            dir,
            existing,
        })
    }

    /// True when an index was already present when this store was opened.
    #[must_use]
    pub const fn has_index(&self) -> bool {
        self.existing
    }

    /// The repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.codeintel/` directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The manifest as loaded, or as this run is building it.
    #[must_use]
    pub const fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The manifest, mutably.
    pub const fn manifest_mut(&mut self) -> &mut Manifest {
        &mut self.manifest
    }

    /// The atom for a string, appending it to the dictionary if new.
    pub fn intern(&mut self, s: &str) -> Option<Atom> {
        self.interner.intern(s)
    }

    /// The string behind an atom.
    #[must_use]
    pub fn resolve(&self, atom: Atom) -> Option<&str> {
        self.interner.resolve(atom)
    }

    /// The dictionary, for a parser that needs to intern query literals.
    pub const fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    /// Remove `.tmp` files left by a crashed run.
    ///
    /// # Errors
    /// I/O failure reading the segment directory.
    pub fn sweep_tmp(&self) -> Result<usize> {
        let seg = self.dir.join(SEG);
        if !seg.exists() {
            return Ok(0);
        }
        let mut swept = 0;
        for entry in std::fs::read_dir(&seg)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "tmp") {
                std::fs::remove_file(&path)?;
                swept += 1;
            }
        }
        Ok(swept)
    }

    /// Write one file's facts and record it in the manifest.
    ///
    /// # Errors
    /// I/O failure, or a segment too large to encode.
    pub fn put(&mut self, path: &str, segment: &mut Segment, entry: FileEntry) -> Result<()> {
        let bytes = segment.encode()?;
        let dir = self.dir.join(SEG);
        std::fs::create_dir_all(&dir)?;
        crate::atomic_write(&dir.join(&entry.seg), &bytes)?;
        self.manifest.files.insert(path.to_string(), entry);
        Ok(())
    }

    /// Drop a vanished file: its segment and its manifest entry.
    ///
    /// A refresh that only added would leave a deleted file's facts answering
    /// queries (`specs/05-surface.md` § `query`).
    ///
    /// # Errors
    /// I/O failure other than the segment already being gone.
    pub fn forget(&mut self, path: &str) -> Result<bool> {
        let Some(entry) = self.manifest.files.remove(path) else {
            return Ok(false);
        };
        let seg = self.dir.join(SEG).join(&entry.seg);
        match std::fs::remove_file(&seg) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),
        }
    }

    /// Flush the dictionary, then write the manifest last.
    ///
    /// # Errors
    /// I/O failure appending the dictionary or writing the manifest.
    pub fn commit(&mut self) -> Result<()> {
        self.interner.flush()?;
        self.manifest.dict_bin_len = len_of(&self.dir.join("dict.bin"))?;
        self.manifest.dict_idx_len = len_of(&self.dir.join("dict.idx"))?;
        self.manifest.save(&self.dir)?;
        self.existing = true;
        Ok(())
    }

    /// Take the dictionary and the manifest out.
    ///
    /// The engine owns its dictionary (`datalog::Symbols`), and a query moves
    /// this one into it after the last commit. Interning a literal a query
    /// mentions but the corpus does not have is then an in-memory id that is
    /// never written — which is exactly what the seam promises.
    #[must_use]
    pub fn into_parts(self) -> (Interner, Manifest) {
        (self.interner, self.manifest)
    }

    /// Every base relation named by the manifest, merged and settled.
    ///
    /// Concatenate then settle once, rather than absorbing segment by segment:
    /// the merge is the only real cost of a load and it is linear in row count
    /// (`specs/04-storage.md` § Loading).
    ///
    /// # Errors
    /// I/O failure, or a corrupt segment. A segment the manifest names but
    /// that is missing is an error too — a silently short load answers with
    /// fewer facts than the index holds.
    pub fn load(&self) -> Result<BTreeMap<&'static str, Relation>> {
        let mut merged: BTreeMap<u32, Relation> = BTreeMap::new();
        for entry in self.manifest.files.values() {
            let path = self.dir.join(SEG).join(&entry.seg);
            let bytes = std::fs::read(&path).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "the manifest names {}, which could not be read: {e}. Repair with \
                         `rm -rf .codeintel && codeintel index .`",
                        path.display()
                    ),
                )
            })?;
            let segment = Segment::decode(&bytes)?;
            for (rel, rows) in segment.iter() {
                let into = merged
                    .entry(rel.id)
                    .or_insert_with(|| Relation::new(rel.arity));
                for row in rows.iter() {
                    into.push(row);
                }
            }
        }
        let mut out = BTreeMap::new();
        for (id, mut rows) in merged {
            rows.settle();
            if let Some(Rel { name, .. }) = schema::by_id(id) {
                out.insert(*name, rows);
            }
        }
        Ok(out)
    }
}

fn len_of(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment_name;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            seg: segment_name(path),
            mtime: 1,
            size: 2,
            hash: "blake3:00".to_string(),
            lang: "rust".to_string(),
            tiers: vec!["ts".to_string()],
        }
    }

    #[test]
    fn a_store_round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let atoms = {
            let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
            assert!(!store.has_index());
            let f = store.intern("src/store.rs").expect("atom");
            let lang = store.intern("rust").expect("atom");
            let mut seg = Segment::new();
            assert!(seg.push("file", &[f, lang]));
            assert!(seg.push("def_span", &[f, 1, 9, 0, 120]));
            store
                .put("src/store.rs", &mut seg, entry("src/store.rs"))
                .expect("writes");
            store.commit().expect("commits");
            (f, lang)
        };

        let store = Store::open(dir.path(), "blake3:test").expect("reopens");
        assert!(store.has_index());
        let relations = store.load().expect("loads");
        let file = relations.get("file").expect("file rows");
        assert_eq!(file.len(), 1);
        assert_eq!(file.row(0), Some([atoms.0, atoms.1].as_slice()));
        assert_eq!(store.resolve(atoms.0), Some("src/store.rs"));
    }

    #[test]
    fn forgetting_a_file_removes_its_facts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
        let f = store.intern("a.rs").expect("atom");
        let lang = store.intern("rust").expect("atom");
        let mut seg = Segment::new();
        assert!(seg.push("file", &[f, lang]));
        store.put("a.rs", &mut seg, entry("a.rs")).expect("writes");
        store.commit().expect("commits");

        assert!(store.forget("a.rs").expect("forgets"));
        store.commit().expect("commits");
        assert!(store.load().expect("loads").is_empty());
        assert!(!store.forget("a.rs").expect("no-op"));
    }

    #[test]
    fn a_missing_segment_is_an_error_not_a_short_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
        let f = store.intern("a.rs").expect("atom");
        let lang = store.intern("rust").expect("atom");
        let mut seg = Segment::new();
        assert!(seg.push("file", &[f, lang]));
        store.put("a.rs", &mut seg, entry("a.rs")).expect("writes");
        store.commit().expect("commits");

        std::fs::remove_file(dir.path().join(DIR).join(SEG).join(segment_name("a.rs")))
            .expect("removes");
        let err = store.load().expect_err("a named segment must exist");
        assert!(err.to_string().contains("codeintel index"), "{err}");
    }

    #[test]
    fn tmp_files_from_a_crashed_run_are_swept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path(), "blake3:test").expect("opens");
        let seg = dir.path().join(DIR).join(SEG);
        std::fs::create_dir_all(&seg).expect("mkdir");
        std::fs::write(seg.join("abc.bin.tmp"), b"junk").expect("writes");
        assert_eq!(store.sweep_tmp().expect("sweeps"), 1);
        assert_eq!(store.sweep_tmp().expect("sweeps"), 0);
    }
}
