//! Building and refreshing the index.
//!
//! One routine, two callers. `codeintel index` runs it unbounded;
//! `codeintel query`'s auto-refresh runs it with a deadline
//! (`specs/05-surface.md` § `query`). Everything it finishes is committed, so
//! a refresh that runs out of time converges next call instead of abandoning
//! its work and paying the full budget forever.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use extract::scip::Ingest;
use extract::walk::{Candidate, Skips};
use extract::{Anchors, Counts, Extractor, tier_b, walk};
use facts::{FileEntry, ScipInput, Store, segment_name};

/// The SCIP index `index` reads when `--scip` is not given.
pub const DEFAULT_SCIP: &str = "index.scip";

/// What to do.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Discard and rewrite everything, dictionary included.
    pub rebuild: bool,
    /// Restrict to these languages. Empty means every registered one.
    pub langs: Vec<String>,
    /// Stop starting new files after this instant and report the rest stale.
    pub deadline: Option<Instant>,
    /// SCIP indexes to ingest. Empty means tier A only.
    ///
    /// All-or-nothing by specification: a SCIP index carries cross-file
    /// references, so a changed one invalidates the reference graph globally
    /// and every file is re-extracted (`specs/04-storage.md`
    /// § Incremental reindex).
    pub scip: Vec<PathBuf>,
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
    /// Tier-B facts produced, by shape.
    pub scip_counts: tier_b::Counts,
    /// Tier-A definitions that adopted a SCIP identity.
    pub anchored: usize,
    /// Languages found in the tree, so `index` can name the indexer for each.
    pub langs: BTreeSet<String>,
    /// SCIP inputs ingested.
    pub scip: Vec<ScipInput>,
    /// SCIP documents dropped, with the reason.
    pub scip_skipped: Vec<(String, &'static str)>,
    /// Indexed files modified after the newest SCIP input was built. Their
    /// `name_ref` rows are fresh and their `scip_ref` rows are not.
    pub scip_stale: Vec<String>,
    /// Wall time.
    pub elapsed_ms: u128,
}

impl Report {
    /// True when the index is not a complete picture of the tree right now.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !self.stale.is_empty()
    }

    /// How many tier-A definitions adopted a SCIP identity, as a percentage.
    ///
    /// The anchor rate is the health metric for the join: a drop means it is
    /// drifting, and `index` prints it so that the drop is visible before a
    /// query built on it is wrong (`specs/02-extraction.md` § Validation).
    #[must_use]
    pub fn anchor_rate(&self) -> Option<f64> {
        (self.counts.defs > 0 && !self.scip.is_empty()).then(|| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a percentage for a human; the counts are in the millions at most"
            )]
            {
                self.anchored as f64 * 100.0 / self.counts.defs as f64
            }
        })
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

    let (ingest, inputs) = read_scip(&root, &plan.scip)?;
    // SCIP is all-or-nothing: a reference that *disappeared* from a document
    // leaves no evidence anywhere else, so there is nothing to drive its
    // removal per-document. A changed input re-extracts everything
    // (specs/04-storage.md § Incremental reindex).
    let scip_changed = store.manifest().scip != inputs;
    let invalidated = extractor_changed || scip_changed;
    report.scip.clone_from(&inputs);
    report.scip_skipped.clone_from(&ingest.skipped);
    let newest_scip = inputs.iter().map(|i| i.mtime).max();

    let mut extractors: HashMap<&'static str, Extractor> = HashMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for candidate in wanted {
        seen.insert(candidate.path.clone());
        report.langs.insert(candidate.lang.name.to_string());
        if plan.deadline.is_some_and(|d| Instant::now() >= d) {
            report.stale.push(candidate.path);
            continue;
        }
        let known = store.manifest().files.get(&candidate.path);
        if newest_scip.is_some_and(|scip| candidate.mtime > scip) {
            report.scip_stale.push(candidate.path.clone());
        }
        if !plan.rebuild
            && !invalidated
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
        if !plan.rebuild && !invalidated && known.is_some_and(|e| e.hash == hash) {
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
        // Tier A runs first, entirely, then tier B — the join needs tier A's
        // `def_name` index to anchor against (specs/02-extraction.md).
        let anchors = Anchors::of(&ingest, &candidate.path);
        let mut extracted = extractor
            .file(&candidate.path, &src, store.interner_mut(), &anchors)
            .with_context(|| format!("extracting {}", candidate.path))?;
        report.counts.defs += extracted.counts.defs;
        report.counts.refs += extracted.counts.refs;
        report.counts.imports += extracted.counts.imports;
        report.anchored += extracted.anchored;

        let scip_counts = tier_b::emit(
            &mut extracted.segment,
            &ingest,
            &candidate.path,
            Some(&src),
            &extracted.defs,
            store.interner_mut(),
        )
        .with_context(|| format!("ingesting SCIP facts for {}", candidate.path))?;
        add(&mut report.scip_counts, scip_counts);

        let mut entry = new_entry(&candidate, &hash);
        if !anchors.is_empty() || scip_counts.refs > 0 {
            entry.tiers.push("scip".to_string());
        }
        let mut segment = extracted.segment;
        store
            .put(&candidate.path, &mut segment, entry)
            .with_context(|| format!("writing the segment for {}", candidate.path))?;
        report.indexed += 1;
    }

    // Files only tier B covers: an unsupported language, or one the walk does
    // not reach. They get an ordinary per-file segment rather than the single
    // `_scip.bin` blob in specs/04-storage.md § Layout — see the note there.
    for (path, doc) in &ingest.docs {
        if seen.contains(path) || plan.deadline.is_some_and(|d| Instant::now() >= d) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(root.join(path)) else {
            // SCIP names a file that is not here. Emitting facts about it would
            // answer questions about source nobody can open.
            continue;
        };
        seen.insert(path.clone());
        let mut segment = facts::Segment::new();
        let file = store.intern(path).context("the dictionary is full")?;
        let lang = store.intern(&doc.lang).context("the dictionary is full")?;
        segment.push("file", &[file, lang]);
        let scip_counts = tier_b::emit(
            &mut segment,
            &ingest,
            path,
            std::fs::read_to_string(root.join(path)).ok().as_deref(),
            &[],
            store.interner_mut(),
        )
        .with_context(|| format!("ingesting SCIP facts for {path}"))?;
        add(&mut report.scip_counts, scip_counts);
        store
            .put(
                path,
                &mut segment,
                FileEntry {
                    seg: segment_name(path),
                    mtime: mtime_of(&meta),
                    size: meta.len(),
                    hash: String::new(),
                    lang: doc.lang.clone(),
                    tiers: vec!["scip".to_string()],
                },
            )
            .with_context(|| format!("writing the segment for {path}"))?;
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
    store.manifest_mut().scip = inputs;
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

/// Read every SCIP input and merge them.
///
/// Multiple indexes are ingested independently — symbol strings are globally
/// unique by construction, so there is no merge logic beyond concatenation
/// (`specs/02-extraction.md` § Acquisition). A missing input is not an error:
/// `./index.scip` is a default, and its absence just means tier A only.
fn read_scip(root: &Path, paths: &[PathBuf]) -> Result<(Ingest, Vec<ScipInput>)> {
    let mut merged = Ingest::default();
    let mut inputs = Vec::new();
    for path in paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        let Ok(meta) = std::fs::metadata(&absolute) else {
            continue;
        };
        let one = Ingest::read(&absolute, root)
            .with_context(|| format!("reading {}", absolute.display()))?;
        inputs.push(ScipInput {
            path: path.display().to_string().replace('\\', "/"),
            tool: one.tool.clone(),
            mtime: mtime_of(&meta),
            documents: one.docs.len() as u64,
        });
        merged.tool = one.tool;
        merged.docs.extend(one.docs);
        merged.symbols.extend(one.symbols);
        merged.externs.extend(one.externs);
        merged.skipped.extend(one.skipped);
    }
    inputs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((merged, inputs))
}

fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn add(into: &mut tier_b::Counts, counts: tier_b::Counts) {
    into.refs += counts.refs;
    into.resolved += counts.resolved;
    into.only += counts.only;
}

fn new_entry(candidate: &Candidate, hash: &str) -> FileEntry {
    FileEntry {
        seg: segment_name(&candidate.path),
        mtime: candidate.mtime,
        size: candidate.size,
        hash: hash.to_string(),
        lang: candidate.lang.name.to_string(),
        tiers: vec!["ts".to_string()],
    }
}
