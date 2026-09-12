//! Answering one query.
//!
//! Auto-refresh, then load, then evaluate. An agent's loop is edit → ask, and
//! answering that from a stale index produces a confidently wrong answer — the
//! worst failure this tool has, because it looks like a right one
//! (`specs/05-surface.md` § `query`).

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use datalog::{Engine, Limits};
use facts::{Lock, Store};

use crate::index::{self, Plan};
use crate::status::Status;
use crate::{Regexes, render};

/// The shipped rule library, compiled in.
pub const STDLIB: &str = include_str!("../../../rules/stdlib.dl");

/// How long auto-refresh may take before the answer is `stale`.
pub const MAX_REFRESH_MS: u64 = 2_000;

/// Base relations only tier B can populate.
///
/// A goal whose dependency **closure** reaches one of these, against an index
/// with no SCIP, gets `no-scip` rather than `ok` with zero rows. The closure,
/// not the literal syntax: `?- impact_of(S, C).` mentions none of them, and the
/// README's own headline query would otherwise return `ok` and nothing on a
/// fresh install — the exact failure invariant 6 exists to prevent
/// (`docs/plan.md` M3).
pub const SCIP_BACKED: &[&str] = &["scip_ref", "resolved", "implements", "extern"];

/// What the caller asked for.
#[derive(Debug, Clone)]
pub struct Options {
    /// Rows before truncation.
    pub limit: usize,
    /// Skip auto-refresh, for benchmarking or a deliberately pinned index.
    pub no_refresh: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limit: Limits::default().max_result_rows,
            no_refresh: false,
        }
    }
}

/// One response. Mirrors the MCP contract in `specs/05-surface.md`.
#[derive(Debug, Clone)]
pub struct Answer {
    /// What happened.
    pub status: Status,
    /// Variable names from the goal, in order of first appearance.
    pub columns: Vec<String>,
    /// Rendered rows, sorted lexicographically by their printed text.
    pub rows: Vec<String>,
    /// True when a cap dropped rows.
    pub truncated: bool,
    /// Which cap fired.
    pub cap: Option<&'static str>,
    /// Non-null whenever `status != ok`, and always actionable.
    pub hint: Option<String>,
    /// Tuples derived.
    pub derived: u64,
    /// Milliseconds spent evaluating.
    pub elapsed_ms: u64,
    /// Files re-extracted before the query ran.
    pub refreshed: usize,
    /// Predicates demand transformation rewrote. Empty means it did not apply,
    /// which on a seeded recursive rule is the difference between a seeded
    /// traversal and all-pairs reachability.
    pub transformed: Vec<String>,
    /// Base relations the goal's dependency closure reaches.
    pub depends: Vec<String>,
}

impl Answer {
    fn of(status: Status, hint: impl Into<String>) -> Self {
        Self {
            status,
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            cap: None,
            hint: Some(hint.into()),
            derived: 0,
            elapsed_ms: 0,
            refreshed: 0,
            transformed: Vec::new(),
            depends: Vec::new(),
        }
    }
}

/// Evaluate `program` against the index under `root`.
///
/// # Errors
/// I/O failure that is not expressible as a status. A query that is wrong, an
/// index that is missing, and a lock someone else holds are all answers.
pub fn run(root: &Path, program: &str, options: &Options) -> Result<Answer> {
    let mut store = Store::open(root, &extract::fingerprint()).context("opening the index")?;
    if !store.has_index() {
        return Ok(Answer::of(
            Status::NoIndex,
            format!("run: codeintel index {}", root.display()),
        ));
    }
    if store.manifest().is_stale_schema() {
        return Ok(Answer::of(
            Status::Stale,
            format!(
                "this index was written for schema_version {}, this build speaks {}. run: \
                 codeintel index {} --rebuild",
                store.manifest().schema_version,
                facts::SCHEMA_VERSION,
                root.display()
            ),
        ));
    }

    let (mut status, mut hint, refreshed) = refresh(&mut store, root, options)?;
    let scip = ScipState::of(store.manifest());
    let langs = indexers(store.manifest());

    let relations = match store.load() {
        Ok(relations) => relations,
        Err(e) => return Ok(Answer::of(Status::Corrupt, e.to_string())),
    };
    let (interner, _manifest) = store.into_parts();
    let mut engine = Engine::new(Box::new(interner)).with_regexes(Box::new(Regexes::new()));
    // Declare every base relation, present or not. A relation with no rows is
    // "this is not true of your code"; an undeclared one would be a query
    // error, and the two are not the same answer.
    for rel in facts::RELATIONS {
        engine.insert_relation(
            rel.name,
            relations
                .get(rel.name)
                .cloned()
                .unwrap_or_else(|| datalog::Relation::new(rel.arity)),
        );
    }
    engine
        .load_rules(STDLIB)
        .map_err(|d| anyhow::anyhow!("rules/stdlib.dl does not load: {}", d.message))?;

    let mut limits = Limits::default();
    limits.max_result_rows = options.limit;
    let result = match engine.query(program, &limits) {
        Ok(result) => result,
        Err(diagnostic) => {
            return Ok(Answer::of(
                Status::of(diagnostic.status),
                diagnostic.message.clone(),
            ));
        }
    };

    if result.truncated {
        status = Status::Truncated;
        hint = Some(format!(
            "{} fired; raise it with --limit or narrow the query",
            result.cap.unwrap_or("a cap")
        ));
    } else if status == Status::Ok
        && let Some((scip_status, scip_hint)) = scip.verdict(&result.stats.depends, &langs)
    {
        status = scip_status;
        hint = Some(scip_hint);
    }
    Ok(Answer {
        status,
        columns: result.columns.clone(),
        rows: render(&engine, &result),
        truncated: result.truncated,
        cap: result.cap,
        hint,
        derived: result.stats.derived,
        elapsed_ms: result.stats.elapsed_ms,
        refreshed,
        transformed: result.stats.transformed.clone(),
        depends: result.stats.depends.clone(),
    })
}

/// What the index knows about its SCIP inputs, and what that means for an
/// answer.
#[derive(Debug, Default)]
struct ScipState {
    /// True when no SCIP index was ingested.
    absent: bool,
    /// Indexed files modified after the newest SCIP input was built.
    stale: Vec<String>,
}

impl ScipState {
    fn of(manifest: &facts::Manifest) -> Self {
        let Some(newest) = manifest.scip.iter().map(|i| i.mtime).max() else {
            return Self {
                absent: true,
                stale: Vec::new(),
            };
        };
        Self {
            absent: false,
            stale: manifest
                .files
                .iter()
                .filter(|(_, entry)| entry.mtime > newest)
                .map(|(path, _)| path.clone())
                .collect(),
        }
    }

    /// The status this answer deserves, or `None` when SCIP has nothing to say
    /// about it.
    fn verdict(&self, depends: &[String], langs: &[&'static str]) -> Option<(Status, String)> {
        if !depends.iter().any(|d| SCIP_BACKED.contains(&d.as_str())) {
            return None;
        }
        let commands = if langs.is_empty() {
            "build a SCIP index for this repository".to_string()
        } else {
            format!("run: {}", langs.join(" && "))
        };
        if self.absent {
            return Some((
                Status::NoScip,
                format!(
                    "this query depends on {}, which only a SCIP index populates; the answer \
                     above is what tier A alone can see. {commands}",
                    depends
                        .iter()
                        .filter(|d| SCIP_BACKED.contains(&d.as_str()))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if self.stale.is_empty() {
            return None;
        }
        let mut named: Vec<&str> = self.stale.iter().map(String::as_str).take(5).collect();
        if self.stale.len() > named.len() {
            named.push("...");
        }
        Some((
            Status::ScipStale,
            format!(
                "{} file(s) changed after the SCIP index was built, so their `name_ref` rows are \
                 fresh and their `scip_ref` rows are not: {}. {commands}",
                self.stale.len(),
                named.join(", ")
            ),
        ))
    }
}

/// The indexer command for every language the index holds files in.
fn indexers(manifest: &facts::Manifest) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = manifest
        .files
        .values()
        .filter_map(|entry| extract::lang::by_name(&entry.lang).map(|l| l.indexer))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Bring the index up to date, within the refresh budget.
///
/// A reader that cannot take the writer lock does not fail: it reads the
/// current manifest and reports `stale`. `locked` is for a second writer, and
/// telling a reader "locked" is not actionable
/// (`specs/04-storage.md` § Concurrency).
fn refresh(
    store: &mut Store,
    root: &Path,
    options: &Options,
) -> Result<(Status, Option<String>, usize)> {
    if options.no_refresh {
        return Ok((Status::Ok, None, 0));
    }
    let mut lock = Lock::open(store.dir()).context("opening the writer lock")?;
    let held = lock.try_hold().context("taking the writer lock")?;
    let Ok(held) = held else {
        return Ok((
            Status::Stale,
            Some(format!(
                "another codeintel run holds the lock; this answer is from the index as it \
                 stands. run: codeintel index {}",
                root.display()
            )),
            0,
        ));
    };

    let plan = Plan {
        deadline: Some(Instant::now() + Duration::from_millis(MAX_REFRESH_MS)),
        ..Plan::default()
    };
    let report = index::refresh(store, &plan)?;
    drop(held);

    if report.is_stale() {
        let mut named: Vec<&str> = report.stale.iter().map(String::as_str).take(5).collect();
        if report.stale.len() > named.len() {
            named.push("...");
        }
        return Ok((
            Status::Stale,
            Some(format!(
                "auto-refresh ran out of its {MAX_REFRESH_MS} ms budget with {} file(s) left: \
                 {}. run: codeintel index {}",
                report.stale.len(),
                named.join(", "),
                root.display()
            )),
            report.indexed,
        ));
    }
    Ok((Status::Ok, None, report.indexed))
}

/// The answer as JSON, in the shape `specs/05-surface.md` § Response contract
/// specifies.
#[must_use]
pub fn to_json(answer: &Answer) -> serde_json::Value {
    serde_json::json!({
        "status": answer.status.as_str(),
        "columns": answer.columns,
        "rows": answer.rows,
        "truncated": answer.truncated,
        "cap": answer.cap,
        "hint": answer.hint,
        "stats": {
            "derived": answer.derived,
            "elapsed_ms": answer.elapsed_ms,
            "refreshed": answer.refreshed,
            "transformed": answer.transformed,
            "depends": answer.depends,
        },
    })
}
