//! Building and refreshing the index.
//!
//! One routine, two callers. `codeintel index` runs it unbounded;
//! `codeintel query`'s auto-refresh runs it with a deadline
//! (`specs/05-surface.md` § `query`). Everything it finishes is committed, so
//! a refresh that runs out of time converges next call instead of abandoning
//! its work and paying the full budget forever.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use extract::walk::{Candidate, Skips};
use extract::{Counts, Extractor, walk};
use facts::{FileEntry, Store, segment_name};

/// What to do.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Discard and rewrite everything, dictionary included.
    pub rebuild: bool,
    /// Restrict to these languages. Empty means every registered one.
    pub langs: Vec<String>,
    /// Stop starting new files after this instant and report the rest stale.
    pub deadline: Option<Instant>,
}

/// What happened.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Files extracted this run.
    pub indexed: usize,
    /// Files the manifest already had, unchanged.
    pub unchanged: usize,
    /// Files that vanished and were dropped.
    pub removed: usize,
    /// Files that are not UTF-8.
    pub binary: usize,
    /// What the walk dropped, by reason.
    pub skips: Skips,
    /// Files a deadline stopped us from refreshing.
    pub stale: Vec<String>,
    /// Facts produced, by shape.
    pub counts: Counts,
    /// Wall time.
    pub elapsed_ms: u128,
}

impl Report {
    /// True when the index is not a complete picture of the tree right now.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !self.stale.is_empty()
    }
}

/// Walk, extract what changed, drop what vanished, and commit.
///
/// # Errors
/// I/O failure, or a file that will not extract. A file that is merely absent
/// or unreadable is counted, not fatal.
pub fn refresh(store: &mut Store, plan: &Plan) -> Result<Report> {
    let started = Instant::now();
    let root = store.root().to_path_buf();
    let mut report = Report::default();

    store.sweep_tmp().context("sweeping .tmp files")?;
    let (found, skips) = walk(&root);
    report.skips = skips;

    let wanted: Vec<Candidate> = found
        .into_iter()
        .filter(|c| plan.langs.is_empty() || plan.langs.iter().any(|l| l == c.lang.name))
        .collect();

    // A changed extractor invalidates every fact, not just the files a user
    // happens to touch afterwards (specs/04-storage.md § Manifest).
    let fingerprint = extract::fingerprint();
    let extractor_changed = store.manifest().extractor_fingerprint != fingerprint;

    let mut extractors: HashMap<&'static str, Extractor> = HashMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for candidate in wanted {
        seen.insert(candidate.path.clone());
        if plan.deadline.is_some_and(|d| Instant::now() >= d) {
            report.stale.push(candidate.path);
            continue;
        }
        let known = store.manifest().files.get(&candidate.path);
        if !plan.rebuild
            && !extractor_changed
            && known.is_some_and(|e| e.looks_unchanged(candidate.mtime, candidate.size))
        {
            report.unchanged += 1;
            continue;
        }

        let Ok(bytes) = std::fs::read(&candidate.abs) else {
            // Vanished or unreadable between the walk and here. Report it
            // rather than dropping its facts on a transient error.
            report.skips.unreadable.push(candidate.path.clone());
            continue;
        };
        let hash = facts::content_hash(&bytes);
        if !plan.rebuild && !extractor_changed && known.is_some_and(|e| e.hash == hash) {
            // Touched but not changed: refresh the fast-path fields so the next
            // run does not hash it again.
            let mut entry = known
                .cloned()
                .unwrap_or_else(|| new_entry(&candidate, &hash));
            entry.mtime = candidate.mtime;
            entry.size = candidate.size;
            store
                .manifest_mut()
                .files
                .insert(candidate.path.clone(), entry);
            report.unchanged += 1;
            continue;
        }
        let Ok(src) = String::from_utf8(bytes) else {
            report.binary += 1;
            continue;
        };

        let extractor = match extractors.entry(candidate.lang.name) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(
                Extractor::new(candidate.lang)
                    .with_context(|| format!("preparing the {} extractor", candidate.lang.name))?,
            ),
        };
        let (mut segment, counts) = extractor
            .file(&candidate.path, &src, store.interner_mut())
            .with_context(|| format!("extracting {}", candidate.path))?;
        report.counts.defs += counts.defs;
        report.counts.refs += counts.refs;
        report.counts.imports += counts.imports;

        let entry = new_entry(&candidate, &hash);
        store
            .put(&candidate.path, &mut segment, entry)
            .with_context(|| format!("writing the segment for {}", candidate.path))?;
        report.indexed += 1;
    }

    // A refresh that only added would leave a deleted file's facts answering
    // queries (specs/05-surface.md § `query`).
    let vanished: Vec<String> = store
        .manifest()
        .files
        .keys()
        .filter(|path| !seen.contains(*path))
        .cloned()
        .collect();
    for path in vanished {
        if store.forget(&path).context("dropping a vanished file")? {
            report.removed += 1;
        }
    }

    store.manifest_mut().extractor_fingerprint = fingerprint;
    store.commit().context("committing the index")?;
    report.elapsed_ms = started.elapsed().as_millis();
    Ok(report)
}

/// Throw the index away, keeping only the dictionary generation counter.
///
/// `--rebuild` renumbers every atom, and `query --raw` hands raw atom ids to
/// the caller, so the generation is what stops a saved id from resolving to a
/// *different string* rather than to an error
/// (`specs/04-storage.md` § Manifest).
///
/// # Errors
/// I/O failure removing the store directory.
pub fn discard(root: &Path) -> Result<u64> {
    let dir = root.join(facts::store::DIR);
    let generation = facts::Manifest::open(&dir)
        .ok()
        .flatten()
        .map_or(0, |m| m.dict_generation);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    Ok(generation.saturating_add(1))
}

/// Add `.codeintel/` to an existing `.gitignore` that does not already ignore
/// it, and say so.
///
/// `specs/04-storage.md` § Layout: announce it, do not do it silently. A store
/// committed by accident is a large derived directory in someone's history, and
/// a `.gitignore` this tool edited without saying so is worse.
///
/// # Errors
/// I/O failure reading or appending to `.gitignore`.
pub fn ignore_the_store(root: &Path) -> Result<bool> {
    let path = root.join(".gitignore");
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path).context("reading .gitignore")?;
    if text
        .lines()
        .any(|line| line.trim().trim_end_matches('/').trim_start_matches('/') == ".codeintel")
    {
        return Ok(false);
    }
    let separator = if text.ends_with('\n') || text.is_empty() {
        ""
    } else {
        "\n"
    };
    std::fs::write(&path, format!("{text}{separator}.codeintel/\n"))
        .context("appending to .gitignore")?;
    Ok(true)
}

fn new_entry(candidate: &Candidate, hash: &str) -> FileEntry {
    FileEntry {
        seg: segment_name(&candidate.path),
        mtime: candidate.mtime,
        size: candidate.size,
        hash: hash.to_string(),
        lang: candidate.lang.name.to_string(),
        // Tier B lands at M3; nothing here has a SCIP counterpart yet.
        tiers: vec!["ts".to_string()],
    }
}
