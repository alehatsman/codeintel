//! The store on disk: `.codeintel/` — dictionary, segments, manifest.
//!
//! One writer, many readers. Writing a file's facts is: encode the segment,
//! `fsync` it, rename it into place under a name derived from its bytes.
//! Committing is: append the dictionary, then `fsync` and rename the manifest
//! **last**, because the manifest is the only thing that names segments — and
//! only then unlink the segments it stopped naming
//! (`specs/04-storage.md` § Concurrency).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Result;
use std::path::{Path, PathBuf};

use datalog::atom::Atom;
use datalog::relation::Relation;

use crate::intern::Interner;
use crate::manifest::{Manifest, NewEntry, SegName};
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
    /// Segments the manifest stopped naming this run. Unlinked after commit,
    /// never before: until the new manifest is in place, the old one names
    /// them and a reader may be opening them.
    retired: BTreeSet<SegName>,
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
        // Only what the manifest recorded is trusted: anything past those
        // extents is a torn append from a crashed run.
        let (bin_len, idx_len) = manifest
            .as_ref()
            .map_or((0, 0), |m| (m.dict_bin_len, m.dict_idx_len));
        Ok(Self {
            interner: Interner::open_to(&dir, bin_len, idx_len)?,
            manifest: manifest.unwrap_or_else(|| Manifest::new(&root, fingerprint.to_string())),
            root,
            dir,
            existing,
            retired: BTreeSet::new(),
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

    /// Remove what a crashed run left behind: `.tmp` files, and segments the
    /// manifest does not name.
    ///
    /// A writer calls this under the lock before it writes. Readers never do:
    /// a segment this manifest does not name may be one a newer manifest
    /// does, and only the lock holder knows there is no newer manifest.
    ///
    /// # Errors
    /// I/O failure reading the store directory.
    pub fn sweep(&self) -> Result<usize> {
        let mut swept = 0;
        let manifest_tmp = self.dir.join(format!("{}.tmp", crate::manifest::MANIFEST));
        if manifest_tmp.exists() {
            std::fs::remove_file(&manifest_tmp)?;
            swept += 1;
        }
        let seg = self.dir.join(SEG);
        if !seg.exists() {
            return Ok(swept);
        }
        let named: BTreeSet<&str> = self
            .manifest
            .files
            .values()
            .map(|e| e.seg.as_str())
            .collect();
        for entry in std::fs::read_dir(&seg)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.extension().is_some_and(|e| e == "tmp") || !named.contains(name) {
                std::fs::remove_file(&path)?;
                swept += 1;
            }
        }
        Ok(swept)
    }

    /// Write one file's facts and record it in the manifest.
    ///
    /// The segment is named here, from its bytes, and `entry` recorded under
    /// that name. A previous segment for `path` is left in place until
    /// [`Self::commit`]: the manifest on disk still names it.
    ///
    /// # Errors
    /// I/O failure, or a segment too large to encode.
    pub fn put(&mut self, path: &str, segment: &mut Segment, entry: NewEntry) -> Result<()> {
        let bytes = segment.encode()?;
        let entry = entry.named(crate::segment_name(&bytes));
        let dir = self.dir.join(SEG);
        std::fs::create_dir_all(&dir)?;
        crate::atomic_write(&dir.join(&entry.seg), &bytes)?;
        if let Some(previous) = self.manifest.files.insert(path.to_string(), entry) {
            self.retired.insert(previous.seg);
        }
        Ok(())
    }

    /// Drop a vanished file from the manifest; its segment goes at
    /// [`Self::commit`].
    ///
    /// A refresh that only added would leave a deleted file's facts answering
    /// queries (`specs/05-surface.md` § `query`). Unlinking here, before the
    /// commit, would leave a crashed run's manifest naming a file that is
    /// gone, and the index unreadable until `rm -rf`.
    pub fn forget(&mut self, path: &str) -> bool {
        let Some(entry) = self.manifest.files.remove(path) else {
            return false;
        };
        self.retired.insert(entry.seg);
        true
    }

    /// Flush the dictionary, write the manifest last, then unlink the
    /// segments it stopped naming.
    ///
    /// # Errors
    /// I/O failure appending the dictionary or writing the manifest.
    pub fn commit(&mut self) -> Result<()> {
        self.interner.flush()?;
        // What the interner vouches for, not what `stat` says. A commit that
        // interned nothing leaves a crashed run's torn tail on disk, and
        // recording its size would make every later open fail validation
        // (`specs/04-storage.md` § Manifest).
        (self.manifest.dict_bin_len, self.manifest.dict_idx_len) = self.interner.extents();
        self.manifest.save(&self.dir)?;
        self.existing = true;
        // Only now. A crash anywhere above leaves the old manifest naming
        // files that all still exist. A retired name the new manifest names
        // again — the same bytes re-extracted — is not touched.
        let named: BTreeSet<&str> = self
            .manifest
            .files
            .values()
            .map(|e| e.seg.as_str())
            .collect();
        for seg in std::mem::take(&mut self.retired) {
            if named.contains(seg.as_str()) {
                continue;
            }
            match std::fs::remove_file(self.dir.join(SEG).join(&seg)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Dict;

    fn entry(_path: &str) -> NewEntry {
        NewEntry {
            mtime: 1,
            size: 2,
            hash: "blake3:00".to_string(),
            lang: "rust".to_string(),
            tiers: vec!["ts".to_string()],
            scip_hash: None,
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

        assert!(store.forget("a.rs"));
        store.commit().expect("commits");
        assert!(store.load().expect("loads").is_empty());
        assert!(!store.forget("a.rs"));
    }

    /// One file's segment, written with `rows` in its `def_span` relation.
    fn write(store: &mut Store, path: &str, rows: &[[u32; 4]]) -> String {
        let f = store.intern(path).expect("atom");
        let lang = store.intern("rust").expect("atom");
        let mut seg = Segment::new();
        assert!(seg.push("file", &[f, lang]));
        for [a, b, c, d] in rows {
            assert!(seg.push("def_span", &[f, *a, *b, *c, *d]));
        }
        store.put(path, &mut seg, entry(path)).expect("writes");
        store.manifest().files[path].seg.to_string()
    }

    fn segment_files(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir.join(DIR).join(SEG))
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    #[test]
    fn a_changed_file_keeps_its_old_segment_until_commit() {
        // The manifest on disk names the old bytes until the new manifest
        // replaces it, so the old bytes must exist until then
        // (`specs/04-storage.md` § Concurrency).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
        let old = write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
        store.commit().expect("commits");

        let new = write(&mut store, "a.rs", &[[1, 3, 0, 20]]);
        assert_ne!(old, new, "different bytes, different name");
        assert_eq!(segment_files(dir.path()), {
            let mut both = vec![old.clone(), new.clone()];
            both.sort();
            both
        });

        store.commit().expect("commits");
        assert_eq!(segment_files(dir.path()), vec![new]);
        let rows = store.load().expect("loads");
        assert_eq!(rows["def_span"].len(), 1);
        assert_eq!(rows["def_span"].row(0).map(|r| r[2]), Some(3));
    }

    #[test]
    fn the_same_bytes_again_are_the_same_segment_and_nothing_is_unlinked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
        let first = write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
        store.commit().expect("commits");
        let again = write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
        assert_eq!(first, again);
        store.commit().expect("commits");
        assert_eq!(segment_files(dir.path()), vec![first]);
        assert_eq!(store.load().expect("loads")["def_span"].len(), 1);
    }

    #[test]
    fn a_forgotten_file_keeps_its_segment_until_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
        let seg = write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
        store.commit().expect("commits");

        assert!(store.forget("a.rs"));
        assert_eq!(segment_files(dir.path()), vec![seg]);
        store.commit().expect("commits");
        assert!(segment_files(dir.path()).is_empty());
    }

    #[test]
    fn a_crashed_run_leaves_a_readable_index_and_orphans_the_next_writer_sweeps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let committed = {
            let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
            let seg = write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
            store.commit().expect("commits");
            // A run that wrote and forgot, then died before its commit.
            write(&mut store, "a.rs", &[[1, 3, 0, 20]]);
            write(&mut store, "b.rs", &[[5, 6, 0, 30]]);
            store.forget("a.rs");
            seg
        };
        std::fs::write(dir.path().join(DIR).join("manifest.json.tmp"), b"{ torn").expect("writes");

        let store = Store::open(dir.path(), "blake3:test").expect("reopens");
        let rows = store.load().expect("the old manifest still loads");
        assert_eq!(rows["def_span"].row(0).map(|r| r[2]), Some(2));
        assert_eq!(
            store.sweep().expect("sweeps"),
            3,
            "two orphans and a manifest"
        );
        assert_eq!(segment_files(dir.path()), vec![committed]);
        assert_eq!(store.sweep().expect("sweeps"), 0);
    }

    #[test]
    fn a_torn_dictionary_append_is_cut_at_the_recorded_extents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alpha = {
            let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
            let alpha = store.intern("alpha").expect("atom");
            write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
            store.commit().expect("commits");
            alpha
        };
        // A crash mid-append: three bytes of a string and its offset landed,
        // nothing else did, and no manifest records them.
        let bin = dir.path().join(DIR).join("dict.bin");
        let idx = dir.path().join(DIR).join("dict.idx");
        let mut data = std::fs::read(&bin).expect("bin");
        let end = data.len() as u64 + 3;
        data.extend_from_slice(b"bet");
        std::fs::write(&bin, &data).expect("writes");
        let mut table = std::fs::read(&idx).expect("idx");
        table.extend_from_slice(&end.to_le_bytes());
        std::fs::write(&idx, &table).expect("writes");

        let mut store = Store::open(dir.path(), "blake3:test").expect("reopens past the tear");
        assert_eq!(store.resolve(alpha), Some("alpha"));
        let beta = store.intern("beta").expect("atom");
        assert_eq!(store.resolve(beta), Some("beta"));
        store.commit().expect("commits over the tear");

        let store = Store::open(dir.path(), "blake3:test").expect("reopens");
        assert_eq!(
            store.resolve(beta),
            Some("beta"),
            "the tear was cut, not read through"
        );
        Dict::open(&dir.path().join(DIR)).expect("the whole file validates now");
    }

    #[test]
    fn a_commit_that_interns_nothing_records_the_trusted_extents_not_a_torn_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let trusted = {
            let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
            write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
            store.commit().expect("commits");
            (store.manifest().dict_bin_len, store.manifest().dict_idx_len)
        };
        // A crash mid-append: three bytes of a string landed, its offset did
        // not, and no manifest records either.
        let bin = dir.path().join(DIR).join("dict.bin");
        let mut data = std::fs::read(&bin).expect("bin");
        data.extend_from_slice(b"bet");
        std::fs::write(&bin, &data).expect("writes");

        {
            let mut store = Store::open(dir.path(), "blake3:test").expect("reopens past the tear");
            // The same strings with different rows: the manifest changes and
            // the dictionary does not.
            write(&mut store, "a.rs", &[[1, 3, 0, 20]]);
            assert_eq!(store.interner_mut().pending(), 0);
            store.commit().expect("commits");
            assert_eq!(
                (store.manifest().dict_bin_len, store.manifest().dict_idx_len),
                trusted,
                "the torn bytes are not recorded as trusted"
            );
        }
        let store = Store::open(dir.path(), "blake3:test").expect("the next open validates");
        let rows = store.load().expect("loads");
        assert_eq!(rows["def_span"].row(0).map(|r| r[2]), Some(3));
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

        let seg = store.manifest().files["a.rs"].seg.clone();
        std::fs::remove_file(dir.path().join(DIR).join(SEG).join(seg)).expect("removes");
        let err = store.load().expect_err("a named segment must exist");
        assert!(err.to_string().contains("codeintel index"), "{err}");
    }

    #[test]
    fn a_manifest_naming_a_file_outside_the_index_is_refused_and_the_file_survives() {
        // The cloned-repository case: a committed `.codeintel/` whose manifest
        // names a victim for a source file the tree does not have, so the
        // next refresh would `forget` it and unlink the name at commit.
        let dir = tempfile::tempdir().expect("tempdir");
        let victim = dir.path().join("precious.txt");
        std::fs::write(&victim, b"keep me").expect("writes");
        {
            let mut store = Store::open(dir.path(), "blake3:test").expect("opens");
            write(&mut store, "a.rs", &[[1, 2, 0, 10]]);
            store.commit().expect("commits");
        }
        let path = dir.path().join(DIR).join(crate::manifest::MANIFEST);
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("reads")).expect("json");
        let mut gone = json["files"]["a.rs"].clone();
        gone["seg"] = victim.display().to_string().into();
        json["files"]["gone.rs"] = gone;
        std::fs::write(&path, json.to_string()).expect("writes");

        let error = Store::open(dir.path(), "blake3:test").expect_err("a hostile manifest");
        assert!(error.to_string().contains("segment name"), "{error}");
        assert_eq!(
            std::fs::read(&victim).expect("the victim is still there"),
            b"keep me"
        );
    }

    #[test]
    fn tmp_files_from_a_crashed_run_are_swept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path(), "blake3:test").expect("opens");
        let seg = dir.path().join(DIR).join(SEG);
        std::fs::create_dir_all(&seg).expect("mkdir");
        std::fs::write(seg.join("abc.bin.tmp"), b"junk").expect("writes");
        assert_eq!(store.sweep().expect("sweeps"), 1);
        assert_eq!(store.sweep().expect("sweeps"), 0);
    }
}
