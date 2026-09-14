//! Answering one query.
//!
//! Auto-refresh, then load, then evaluate. An agent's loop is edit → ask, and
//! answering that from a stale index produces a confidently wrong answer — the
//! worst failure this tool has, because it looks like a right one
//! (`specs/05-surface.md` § `query`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use datalog::{Engine, Limits, Relation};
use facts::{Lock, Store};

use crate::index::{self, Plan};
use crate::overlay::{Literals, Overlay};
use crate::render::{Row, Sites};
use crate::status::Status;
use crate::wire::Wire;
use crate::{Regexes, render, schema};

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
/// `implements` is deliberately absent. Since schema 2 it is a rule with a
/// tier-A `name` branch over `name_impl`, so it answers without SCIP, and
/// `no-scip` would disclaim rows the index really has. `scip_impl`, the base
/// relation beneath it, stays: a query naming that one directly is asking for
/// the compiler's answer specifically.
pub const SCIP_BACKED: &[&str] = &["scip_ref", "resolved", "scip_impl", "extern"];

/// How much larger the engine's raw-atom byte budget is than the printed one.
///
/// The engine caps the bytes *it* materialises, measured over raw atoms; this
/// host prints a strictly shorter form. The multiplier keeps the engine's cap
/// from being the one that fires — which would truncate rows that fit the
/// printed budget — while still bounding materialisation, since a caller may
/// raise `--limit` without bound.
const RAW_BYTE_HEADROOM: usize = 8;

/// What the caller asked for.
#[derive(Debug, Clone)]
pub struct Options {
    /// Rows before truncation.
    pub limit: usize,
    /// Skip auto-refresh, for benchmarking or a deliberately pinned index.
    pub no_refresh: bool,
    /// Print the underlying atoms instead of `Name path:line`.
    ///
    /// What round-trips: the output of one query pasted into the next query's
    /// literal. The rendered form is for reading, not for feeding back.
    pub raw: bool,
    /// Extra rule files, loaded after `stdlib.dl` and before the goal.
    ///
    /// This is where a repository keeps its own conformance rules — layering,
    /// banned dependencies, allowed directions — so they live in the repository
    /// under review rather than in this binary.
    ///
    /// **Clauses here are additive, not replacements.** A predicate is the
    /// union of its clauses, so a file defining `is_test` *widens* it rather
    /// than redefining it. Only a rule written in the query program itself
    /// shadows a loaded one, and that shadowing is reported in
    /// [`Answer::shadowed`]. A repository that means to replace a stdlib rule
    /// puts it in the program, not in a rule file.
    pub rules: Vec<PathBuf>,
    /// Where the answer is sent. `max_result_bytes` is measured on it
    /// (`specs/05-surface.md` § Symbol rendering).
    pub wire: Wire,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limit: Limits::default().max_result_rows,
            no_refresh: false,
            raw: false,
            rules: Vec::new(),
            wire: Wire::Text,
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
    /// The rows, in both notations, ordered once by the display text. The two
    /// notations of a row describe the same tuple at the same index.
    pub rows: Vec<Row>,
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
    /// Why `transformed` holds what it holds (`datalog::Stats::demand`).
    pub demand: String,
    /// Base relations the goal's dependency closure reaches.
    pub depends: Vec<String>,
    /// Stdlib rules a loaded rule file or the query itself shadowed. Never
    /// silent: a repository rule quietly replacing `is_test` would change
    /// every answer that reads it.
    pub shadowed: Vec<String>,
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
            demand: String::new(),
            depends: Vec::new(),
            shadowed: Vec::new(),
        }
    }
}

/// Evaluate `program` against the index under `root`.
///
/// # Errors
/// I/O failure that is not expressible as a status. A query that is wrong, an
/// index that is missing, and a lock someone else holds are all answers.
pub fn run(root: &Path, program: &str, options: &Options) -> Result<Answer> {
    Warm::default().run(root, program, options)
}

/// Answers queries against one repository, keeping the loaded index between
/// calls for as long as its manifest does not move.
///
/// `codeintel mcp` holds one for the life of the process. The CLI builds one
/// per invocation through [`run`], so both answer through the same code
/// (`specs/05-surface.md` § MCP).
#[derive(Default)]
pub struct Warm {
    loaded: Option<Loaded>,
}

impl std::fmt::Debug for Warm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Warm")
            .field("loaded", &self.loaded.is_some())
            .finish()
    }
}

impl Warm {
    /// Refresh, then evaluate `program` — against the loaded index if the
    /// manifest has not moved since it was loaded, against a fresh load if it
    /// has.
    ///
    /// # Errors
    /// As [`run`].
    pub fn run(&mut self, root: &Path, program: &str, options: &Options) -> Result<Answer> {
        let mut answer = self.answer(root, program, options)?;
        // Every answer, a diagnostic and `no-index` included: the budget bounds
        // what is sent, not only its rows (`specs/05-surface.md` § Symbol
        // rendering).
        fit_hint(&mut answer, Limits::default().max_result_bytes, |a| {
            sent(a, options)
        });
        Ok(answer)
    }

    fn answer(&mut self, root: &Path, program: &str, options: &Options) -> Result<Answer> {
        let mut store = match open(root)? {
            Ok(store) => store,
            Err(answer) => {
                self.loaded = None;
                return Ok(answer);
            }
        };
        if !store.has_index() {
            self.loaded = None;
            return Ok(Answer::of(
                Status::NoIndex,
                format!("run: codeintel index {}", root.display()),
            ));
        }
        if let Some(hint) = stale_schema(store.manifest(), root) {
            self.loaded = None;
            return Ok(Answer::of(Status::Stale, hint));
        }

        // Every call refreshes. Warmth skips the load, never the refresh that
        // keeps the answer current.
        let (status, hint, refreshed) = refresh(&mut store, root, options)?;
        // A manifest that turned corrupt between the two reads: there is
        // nothing to evaluate against.
        if !status.answered() {
            self.loaded = None;
            return Ok(Answer::of(status, hint.unwrap_or_default()));
        }
        let current = self
            .loaded
            .take()
            .filter(|l| l.manifest == *store.manifest() && l.rules == options.rules);
        let mut loaded = if let Some(loaded) = current {
            loaded
        } else {
            let (store, relations) = match load(store, root)? {
                Ok(loaded) => loaded,
                Err(answer) => return Ok(answer),
            };
            match Loaded::of(store, relations, &options.rules)? {
                Ok(loaded) => loaded,
                Err(answer) => return Ok(answer),
            }
        };
        let answer = evaluate(&mut loaded, program, options, status, hint, refreshed);
        // The literals this call interned go with it, so the next call is given
        // the ids a freshly loaded engine would give it.
        loaded.literals.discard();
        self.loaded = Some(loaded);
        Ok(answer)
    }
}

/// An index loaded into an engine, and what answering needs beside it.
struct Loaded {
    /// The manifest this was built from. A refresh that moves it retires this.
    manifest: facts::Manifest,
    /// The rule files loaded after the standard library.
    rules: Vec<PathBuf>,
    engine: Engine,
    literals: Literals,
    relations: BTreeMap<&'static str, Relation>,
    sites: Sites,
    scip: ScipState,
    langs: Vec<&'static str>,
}

impl Loaded {
    /// Load `store` into a fresh engine with the standard library and `rules`.
    ///
    /// The inner `Err` is an answer — a rule file that does not load — and
    /// keeps nothing warm.
    fn of(store: Store, relations: Relations, rules: &[PathBuf]) -> Result<Result<Self, Answer>> {
        let scip = ScipState::of(store.manifest());
        let langs = indexers(store.manifest());

        let sites = Sites::of(&relations);
        let (interner, manifest) = store.into_parts();
        let (overlay, literals) = Overlay::new(interner);
        let mut engine = Engine::new(Box::new(overlay)).with_regexes(Box::new(Regexes::new()));
        // Declare every base relation, present or not. A relation with no rows is
        // "this is not true of your code"; an undeclared one would be a query
        // error, and the two are not the same answer.
        for rel in facts::RELATIONS {
            engine.insert_relation(
                rel.name,
                relations
                    .get(rel.name)
                    .cloned()
                    .unwrap_or_else(|| Relation::new(rel.arity)),
            );
        }
        engine
            .load_rules(STDLIB)
            .map_err(|d| anyhow::anyhow!("rules/stdlib.dl does not load: {}", d.message))?;
        // A repository's own conformance rules. Additive: a predicate is the
        // union of its clauses, so a file defining `is_test` widens it. Only a
        // rule in the query program shadows a loaded one.
        for path in rules {
            let src = match std::fs::read_to_string(path) {
                Ok(src) => src,
                Err(e) => {
                    return Ok(Err(Answer::of(
                        Status::InvalidQuery,
                        format!("{}: {e}", path.display()),
                    )));
                }
            };
            if let Err(diagnostic) = engine.load_rules(&src) {
                return Ok(Err(Answer::of(
                    Status::of(diagnostic.status),
                    format!(
                        "{}: {}{}",
                        path.display(),
                        diagnostic.message,
                        host_hint(&diagnostic)
                    ),
                )));
            }
        }
        // The rules' constants belong to the engine, not to any one call.
        literals.seal();

        Ok(Ok(Self {
            manifest,
            rules: rules.to_vec(),
            engine,
            literals,
            relations,
            sites,
            scip,
            langs,
        }))
    }
}

/// The hint for an index written by a build that speaks another schema, or
/// `None`. `status` asks the same question (`specs/05-surface.md` § `status`).
#[must_use]
pub fn stale_schema(manifest: &facts::Manifest, root: &Path) -> Option<String> {
    manifest.is_stale_schema().then(|| {
        format!(
            "this index was written for schema_version {}, this build speaks {}. run: codeintel \
             index {} --rebuild",
            manifest.schema_version,
            facts::SCHEMA_VERSION,
            root.display()
        )
    })
}

/// Base relations by name, as a store loads them.
type Relations = BTreeMap<&'static str, Relation>;

/// Open the store under `root`, or the answer a refused one deserves.
fn open(root: &Path) -> Result<Result<Store, Answer>> {
    match Store::open(root, &extract::fingerprint()) {
        Ok(store) => Ok(Ok(store)),
        Err(error) => refused(error, "opening the index").map(Err),
    }
}

/// What `crates/facts` refused, as the answer it deserves; an I/O failure, as
/// the error it is. Only a refusal is `corrupt` or `stale`: a permission error
/// answered `corrupt` would have an agent delete a healthy index
/// (`specs/04-storage.md` § Segment format).
fn refused(error: std::io::Error, doing: &str) -> Result<Answer> {
    match facts::fault(&error) {
        Some(fault) => Ok(Answer::of(Status::of_fault(fault), error.to_string())),
        None => Err(anyhow::Error::new(error).context(doing.to_string())),
    }
}

/// Read the segments `store`'s manifest names.
///
/// The writer lock is released by now, so a refresh in another process may
/// have committed and unlinked a segment this manifest still names. A missing
/// segment re-reads the manifest: moved, load the new one, once; moved again,
/// `stale`; never moved, `corrupt`, because nothing replaced it
/// (`specs/04-storage.md` § Concurrency).
fn load(mut store: Store, root: &Path) -> Result<Result<(Store, Relations), Answer>> {
    let mut retried = false;
    loop {
        let error = match store.load() {
            Ok(relations) => return Ok(Ok((store, relations))),
            Err(error) => error,
        };
        if error.kind() != std::io::ErrorKind::NotFound {
            return refused(error, "loading the index").map(Err);
        }
        let reopened = match open(root)? {
            Ok(reopened) => reopened,
            Err(answer) => return Ok(Err(answer)),
        };
        if reopened.manifest() == store.manifest() {
            return Ok(Err(Answer::of(Status::Corrupt, error.to_string())));
        }
        if retried {
            return Ok(Err(Answer::of(
                Status::Stale,
                format!(
                    "the index moved twice while this answer was loading ({error}). ask again, \
                     or run: codeintel index {}",
                    root.display()
                ),
            )));
        }
        retried = true;
        store = reopened;
    }
}

/// Evaluate `program` against a loaded index and shape the answer.
fn evaluate(
    loaded: &mut Loaded,
    program: &str,
    options: &Options,
    mut status: Status,
    mut hint: Option<String>,
    refreshed: usize,
) -> Answer {
    let budget = Limits::default().max_result_bytes;
    let mut limits = Limits::default();
    limits.max_result_rows = options.limit;
    // The engine measures its cap over **raw** atoms, and this host prints a
    // shorter form: a ~68-character `SymId` renders as `name path:line`. If the
    // engine held the printed budget it would always be the cap that fires, and
    // it would fire on rows that would have fit — the user would silently get a
    // fraction of the answer the budget allows. So the engine's cap becomes a
    // memory guard with headroom, and the printed budget is enforced where the
    // printed bytes exist (`specs/05-surface.md` § Symbol rendering).
    limits.max_result_bytes = budget.saturating_mul(RAW_BYTE_HEADROOM);
    let result = match loaded.engine.query(program, &limits) {
        Ok(result) => result,
        Err(diagnostic) => {
            return Answer::of(
                Status::of(diagnostic.status),
                format!("{}{}", diagnostic.message, host_hint(&diagnostic)),
            );
        }
    };

    // One render, one order. Rendering each notation separately and sorting
    // both would give two arrays whose `i`th rows are different tuples. The
    // render cuts at `budget` bytes of printed text, which every wire costs at
    // least; the wire is measured below.
    let printed = render(&loaded.engine, &result, &loaded.sites, options.raw, budget);

    // The index's condition first. A refresh that did not finish outranks the
    // SCIP verdict, which is why `verdict` is only asked on an `ok` refresh.
    if status == Status::Ok
        && let Some((scip_status, scip_hint)) =
            loaded.scip.verdict(&result.stats.depends, &loaded.langs)
    {
        status = scip_status;
        hint = Some(scip_hint);
    }

    // The answer for one set of kept rows. `byte_cut` is whether
    // `max_result_bytes` dropped any.
    let shape = |rows: Vec<Row>, byte_cut: bool| -> Answer {
        let truncated = result.truncated || byte_cut;
        // When both caps fire, the byte budget decided which rows are missing,
        // so it is the one reported. Naming the engine's row cap there advised
        // `--limit`, which returns the same rows every time (#27).
        let cap = if byte_cut {
            Some("max_result_bytes")
        } else {
            result.cap
        };
        let mut status = status;
        let mut hint = hint.clone();
        if truncated {
            // `--limit` moves `max_result_rows` and nothing else, so offering
            // it against a byte cap is advice that changes nothing.
            let cut = match cap {
                Some("max_result_rows") => format!(
                    "max_result_rows fired at {}; raise it with --limit or narrow the query",
                    options.limit
                ),
                // The engine's cap and the host's are different numbers, and
                // quoting the wrong one is invariant 5 with extra steps.
                Some(fired) => {
                    let at = if byte_cut {
                        budget
                    } else {
                        limits.max_result_bytes
                    };
                    format!("{fired} fired at {at} bytes; narrow the query")
                }
                None => "a cap fired; narrow the query".to_string(),
            };
            // `truncated` and `cap` carry the cut whatever the status says, so
            // a `stale` or `no-scip` already on the answer keeps the status: it
            // is the one fact nowhere else in the response
            // (`specs/05-surface.md` § Status taxonomy).
            hint = Some(match hint {
                Some(condition) if status != Status::Ok => {
                    format!("{condition}. also: {cut}")
                }
                _ => {
                    status = Status::Truncated;
                    cut
                }
            });
        }
        // `hint` is non-null whenever the status is not `ok` **or** the result
        // is empty. Zero rows from a valid query is indistinguishable from a
        // typo, a wrong constant and a path that was never indexed, and that is
        // the one case the status taxonomy cannot separate on its own
        // (`specs/05-surface.md` § Response contract).
        if hint.is_none() && rows.is_empty() {
            hint = Some(empty_hint(&result, &loaded.relations, &loaded.engine));
        }
        Answer {
            status,
            columns: result.columns.clone(),
            rows,
            truncated,
            cap,
            hint,
            derived: result.stats.derived,
            elapsed_ms: result.stats.elapsed_ms,
            refreshed,
            transformed: result.stats.transformed.clone(),
            demand: result.stats.demand.clone(),
            depends: result.stats.depends.clone(),
            shadowed: result.stats.shadowed.clone(),
        }
    };

    let mut answer = shape(printed.rows, printed.truncated);
    if sent(&mut answer, options) > budget && !answer.rows.is_empty() {
        // Not every row fits, and a cut only lengthens the rest of the answer
        // (a status, a hint), so the answer is some shorter prefix. The size
        // grows with the prefix: bisect for the longest one that fits
        // (`specs/05-surface.md` § Symbol rendering).
        let mut rows = std::mem::take(&mut answer.rows);
        let (mut lo, mut hi) = (0, rows.len().saturating_sub(1));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if sent(
                &mut shape(rows.iter().take(mid).cloned().collect(), true),
                options,
            ) <= budget
            {
                lo = mid;
            } else {
                hi = mid.saturating_sub(1);
            }
        }
        rows.truncate(lo);
        answer = shape(rows, true);
    }
    answer
}

/// The bytes `answer` costs on the wire `options` names.
///
/// Measured with the widest `elapsed_ms` there is, so where a cut lands does
/// not depend on how fast this run happened to be (invariant 8).
fn sent(answer: &mut Answer, options: &Options) -> usize {
    let elapsed = std::mem::replace(&mut answer.elapsed_ms, u64::MAX);
    let bytes = options.wire.bytes(answer, options.raw);
    answer.elapsed_ms = elapsed;
    bytes
}

/// What a hint cut to fit `max_result_bytes` ends with.
const HINT_CUT: &str = " ... [hint cut to fit max_result_bytes]";

/// Cut the hint to the longest prefix, on a character boundary, with which
/// `answer` fits `budget` as `measure` counts it.
///
/// Rows are cut first, in [`evaluate`]; this is for an answer whose envelope
/// is over budget on its own. Only the hint is cut: `status`, `truncated` and
/// `cap` are what a consumer branches on, `columns` echoes the caller's own
/// goal, and `stats` is bounded by the rule set (`specs/05-surface.md`
/// § Symbol rendering).
fn fit_hint(answer: &mut Answer, budget: usize, mut measure: impl FnMut(&mut Answer) -> usize) {
    if measure(answer) <= budget {
        return;
    }
    let Some(full) = answer.hint.take() else {
        return;
    };
    let cut = |n: usize| {
        let mut n = n.min(full.len());
        while !full.is_char_boundary(n) {
            n = n.saturating_sub(1);
        }
        format!("{}{HINT_CUT}", full.get(..n).unwrap_or_default())
    };
    // The size grows with the prefix: bisect for the longest one that fits.
    let (mut lo, mut hi) = (0, full.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        answer.hint = Some(cut(mid));
        if measure(answer) <= budget {
            lo = mid;
        } else {
            hi = mid.saturating_sub(1);
        }
    }
    answer.hint = Some(cut(lo));
}

/// What this host adds to an engine diagnostic: the verb or the rule that
/// fixes it here. The engine names the failure in its own terms and knows no
/// verb and no rule, which is invariant 6; the sentence that says `codeintel
/// schema` or `impact_of` belongs to the crate that has them.
fn host_hint(diagnostic: &datalog::Diagnostic) -> &'static str {
    match diagnostic.status {
        datalog::Status::InvalidQuery if diagnostic.message.starts_with("no relation or rule") => {
            ". check the spelling against `codeintel schema`"
        }
        datalog::Status::BudgetExceeded => {
            ". a seeded form such as `impact_of(Seed, C)` explores the reachable subgraph"
        }
        _ => "",
    }
}

/// Why an empty result is empty, in one actionable line.
///
/// The engine reports the body literal its join never got past
/// ([`datalog::Stats::empty_at`]); this adds what the index holds for that
/// literal's relation, which is what separates "wrong constant" from "that
/// relation is empty — you need a SCIP index" from "this is not true of your
/// code".
fn empty_hint(
    result: &datalog::QueryResult,
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> String {
    let Some(literal) = &result.stats.empty_at else {
        return "the goal derived no rows: this is not true of your code as indexed".to_string();
    };
    let name = literal
        .trim_start_matches('!')
        .split(['(', ' '])
        .next()
        .unwrap_or_default();
    let held = relations.get(name).map_or(0, Relation::len);
    if facts::schema::by_name(name).is_some() {
        // A constant in a typed column is the agent's most likely error, and
        // the relation total says nothing about it
        // (`specs/05-surface.md` § Response contract).
        if let Some(column) = column_hint(literal, name, relations, engine) {
            return format!("`{literal}` matched 0 rows; {column}");
        }
        return format!(
            "`{literal}` matched 0 rows; the index holds {held} `{name}` row(s). \
             everything before it in the join matched{}",
            integer_hint(literal)
        );
    }
    format!(
        "`{literal}` matched 0 rows; everything before it in the join matched{}",
        integer_hint(literal)
    )
}

/// The int-vs-string trap, for a literal whose columns carry no type.
///
/// `at(S, F, "42")` is a derived rule, so `column_hint` has no schema to say
/// its third column is a line number. A quoted constant that parses as an
/// integer is a different atom from the integer and can never match it; that
/// is the one case where naming the replacement is a statement about the atom
/// space, not a guess about the code.
fn integer_hint(literal: &str) -> String {
    for arg in arguments(literal) {
        let bare = arg.trim_matches('"');
        if arg.starts_with('"') && bare.parse::<i64>().is_ok() {
            return format!(
                ". did you mean {bare}? {arg} is a string, and a string never matches an integer"
            );
        }
    }
    String::new()
}

/// Diagnose the first constant sitting in a typed column of `relation`.
///
/// Returns `None` when every constant is in a `Free` column, which is the
/// common case and leaves the relation-total hint alone. Nothing here suggests
/// a *replacement* value: listing what the column holds is a fact about the
/// index, proposing what the agent meant is a guess (invariant 1).
fn column_hint(
    literal: &str,
    relation: &str,
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> Option<String> {
    let columns = schema::columns(relation);
    for (i, arg) in arguments(literal).iter().enumerate() {
        match columns.get(i) {
            Some(schema::Column::Int) if arg.starts_with('"') => {
                let bare = arg.trim_matches('"');
                return Some(format!(
                    "that column holds integers and {arg} is a string — they are \
                     different atoms and can never match. write {bare} unquoted"
                ));
            }
            Some(schema::Column::Path) if arg.starts_with('"') => {
                if let Some(said) = path_hint(arg.trim_matches('"'), relations, engine) {
                    return Some(said);
                }
            }
            Some(schema::Column::Vocab(values)) if arg.starts_with('"') => {
                let counts = vocabulary(relation, i, values, relations, engine);
                let listed = counts
                    .iter()
                    .map(|(value, n)| format!("{value} {n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                // The count for *this* value decides what is true. Saying "0
                // rows" while the list beside it shows a non-zero count would
                // be the hint contradicting its own evidence, and a literal
                // constrains more than one column: a value with rows of its own
                // means the emptiness came from somewhere else in the literal.
                let bare = arg.trim_matches('"');
                let lead = match counts.iter().find(|(value, _)| *value == bare) {
                    None => format!("{arg} is not one of this column's values"),
                    Some((_, 0)) => format!("{arg} has 0 rows in this index"),
                    Some((_, n)) => format!(
                        "{arg} has {n} row(s) here, so another constant in this \
                         literal is what matched nothing"
                    ),
                };
                return Some(format!("{lead}. the column holds: {listed}"));
            }
            _ => {}
        }
    }
    None
}

/// Why a path constant matched nothing, when the answer is knowable.
///
/// Paths are the constant an agent is most likely to get wrong, because they
/// arrive from `ripgrep`, `git diff` and stack traces — all of which emit
/// absolute or `./`-prefixed forms, while the index keys on repo-relative ones.
/// Every branch below reports a **fact about the index**: that a normalised
/// form is indexed, or that a file with this basename is indexed elsewhere.
/// Neither proposes what the agent meant (invariant 1); they state what exists
/// and let the agent re-aim.
fn path_hint(
    wanted: &str,
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> Option<String> {
    let indexed: BTreeSet<&str> = relations
        .get("file")
        .into_iter()
        .flat_map(Relation::iter)
        .filter_map(|row| row.first().copied().and_then(|a| engine.resolve(a)))
        .collect();
    if indexed.contains(wanted) {
        return None; // Indexed: the emptiness is elsewhere in the literal.
    }

    // The shapes the tools an agent pipes from actually emit: a leading `./`,
    // or an absolute path.
    //
    // The absolute case is matched by *suffix at a path boundary*, not by
    // stripping the root. Stripping cannot be made reliable: `root` as passed
    // is commonly `.`, so it must be canonicalised, and canonicalising resolves
    // symlinks — on macOS a path under `/var` canonicalises to `/private/var`,
    // which is no longer a prefix of what the agent wrote. A suffix match does
    // not care how the caller reached the directory. The longest match wins, so
    // an index holding both `a/b.rs` and `x/a/b.rs` reports the more specific.
    let relative = wanted.strip_prefix("./").unwrap_or_else(|| {
        indexed
            .iter()
            .filter(|p| wanted.len() > p.len() && wanted.ends_with(*p))
            .filter(|p| {
                wanted
                    .as_bytes()
                    .get(wanted.len() - p.len() - 1)
                    .is_some_and(|b| *b == b'/')
            })
            .max_by_key(|p| p.len())
            .copied()
            .unwrap_or(wanted)
    });
    if relative != wanted && indexed.contains(relative) {
        return Some(format!(
            "paths are repo-relative, and `{relative}` is indexed. \
             the leading path was not matched literally"
        ));
    }

    // Same file name somewhere else is a fact worth stating: it separates "you
    // mistyped the directory" from "this file is not indexed at all".
    let base = wanted.rsplit('/').next().unwrap_or(wanted);
    let same: Vec<&str> = indexed
        .iter()
        .copied()
        .filter(|p| p.rsplit('/').next() == Some(base))
        .take(3)
        .collect();
    if !same.is_empty() {
        return Some(format!(
            "no indexed file is `{wanted}`; `{base}` is indexed at: {}",
            same.join(" ")
        ));
    }
    Some(format!(
        "no indexed file is `{wanted}`, and none is named `{base}`. \
         the index holds {} file(s); `codeintel status` says what was skipped",
        indexed.len()
    ))
}

/// This index's count for every value of a closed vocabulary column, in the
/// vocabulary's own order so the output is stable and a zero is visible in
/// place rather than absent.
fn vocabulary(
    relation: &str,
    column: usize,
    values: &[&'static str],
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> Vec<(&'static str, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    if let Some(rows) = relations.get(relation) {
        for row in rows.iter() {
            if let Some(value) = row.get(column).copied().and_then(|a| engine.resolve(a)) {
                *counts.entry(value).or_default() += 1;
            }
        }
    }
    values
        .iter()
        .map(|v| (*v, counts.get(v).copied().unwrap_or(0)))
        .collect()
}

/// The argument text of `name(a, b, c)`, split on commas that are not inside a
/// quoted string — a symbol id contains commas often enough that a naive
/// `split(',')` misreads the column positions and diagnoses the wrong one.
fn arguments(literal: &str) -> Vec<String> {
    let inner = literal
        .split_once('(')
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .unwrap_or_default();
    if inner.is_empty() {
        return Vec::new();
    }
    // Accumulated character by character rather than sliced at byte offsets: a
    // quoted name can hold any UTF-8, and an offset into the middle of a
    // character panics.
    let mut args = vec![String::new()];
    let mut quoted = false;
    for c in inner.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                push_char(&mut args, c);
            }
            ',' if !quoted => args.push(String::new()),
            _ => push_char(&mut args, c),
        }
    }
    args.iter().map(|a| a.trim().to_string()).collect()
}

/// Append to the argument being accumulated. Split out because `args` is never
/// empty by construction and the `else` branch is unreachable, which is clearer
/// stated once than as an `expect` at three call sites.
fn push_char(args: &mut [String], c: char) {
    if let Some(last) = args.last_mut() {
        last.push(c);
    }
}

/// What the index knows about its SCIP inputs, and what that means for an
/// answer.
#[derive(Debug, Default)]
pub struct ScipState {
    /// True when no SCIP index was ingested.
    pub absent: bool,
    /// Indexed files whose bytes the SCIP inputs did not see
    /// ([`facts::FileEntry::scip_stale`]).
    pub stale: Vec<String>,
}

impl ScipState {
    /// Read the SCIP situation out of a manifest.
    ///
    /// Public so `status` reports the *same* staleness `query` acts on. They
    /// disagreed once — `query` answered `scip-stale` naming two files while
    /// `status`, the artifact you are told to paste into a bug report, printed
    /// `ok` and said nothing.
    #[must_use]
    pub fn of(manifest: &facts::Manifest) -> Self {
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
                .filter(|(_, entry)| entry.scip_stale(newest))
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
    // A read-only checkout cannot create the lock file. That is a reader that
    // cannot take the lock, and it gets the same answer
    // (`specs/04-storage.md` § Concurrency).
    let mut lock = match Lock::open(store.dir()) {
        Ok(lock) => lock,
        Err(e) => {
            return Ok((
                Status::Stale,
                Some(format!(
                    "could not open the writer lock ({e}); this answer is from the index as it \
                     stands. run: codeintel index {}",
                    root.display()
                )),
                0,
            ));
        }
    };
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

    // The manifest was read before the lock. A writer that committed in
    // between named segments this copy does not, and a refresh built on this
    // copy would commit a manifest that orphans them. Re-read now, and reopen
    // only if it moved: a reopen rebuilds the dictionary map, and the JSON
    // compare is cheap.
    let current = match facts::Manifest::open(store.dir()) {
        Ok(current) => current,
        Err(e) => return refused(e, "re-reading the manifest").map(|a| (a.status, a.hint, 0)),
    };
    if current.as_ref() != Some(store.manifest()) {
        *store = match open(root)? {
            Ok(reopened) => reopened,
            Err(answer) => return Ok((answer.status, answer.hint, 0)),
        };
    }

    // The SCIP inputs the index was built with, carried forward. Without them
    // an auto-refresh looks like "the SCIP index disappeared", re-extracts the
    // tree tier-A-only, and silently deletes every tier-B fact — the index
    // would degrade a little more with each query.
    //
    // Auto-refresh is still tier-A-only in effect: a file edited since the SCIP
    // index was built is re-extracted against the *old* anchors, so it keeps
    // whatever resolved identity still matches and reports `scip-stale`
    // (`docs/plan.md` M3). A full `codeintel index` is what reconciles it.
    let plan = Plan {
        deadline: Some(Instant::now() + Duration::from_millis(MAX_REFRESH_MS)),
        scip: store
            .manifest()
            .scip
            .iter()
            .map(|input| PathBuf::from(&input.path))
            .collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hint_over_the_budget_is_cut_on_a_char_boundary_and_the_status_kept() {
        // No rows to drop: the hint is the only thing that can shrink. Every
        // character is two bytes, so a cut that ignored boundaries would panic
        // or split one.
        let hint = "é".repeat(2_000);
        let mut answer = Answer::of(Status::NoIndex, hint.clone());
        let budget = 1_000;
        let measure = |a: &mut Answer| Wire::McpJson.bytes(a, false);
        assert!(measure(&mut answer) > budget);

        fit_hint(&mut answer, budget, measure);

        let bytes = measure(&mut answer);
        assert!(bytes <= budget, "{bytes} bytes against {budget}");
        assert_eq!(answer.status, Status::NoIndex);
        assert!(!answer.truncated, "a hint cut drops no rows");
        let cut = answer.hint.as_deref().expect("a hint");
        let kept = cut.strip_suffix(HINT_CUT).expect("the marker");
        assert!(!kept.is_empty() && hint.starts_with(kept), "{cut}");
    }

    #[test]
    fn a_hint_that_fits_is_left_alone() {
        let mut answer = Answer::of(Status::NoIndex, "run: codeintel index .");
        fit_hint(&mut answer, 262_144, |a| Wire::McpJson.bytes(a, false));
        assert_eq!(answer.hint.as_deref(), Some("run: codeintel index ."));
    }

    fn opened(root: &Path) -> Store {
        Store::open(root, &extract::fingerprint()).expect("opens")
    }

    fn indexed(source: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), source).expect("write");
        index::refresh(&mut opened(dir.path()), &Plan::default()).expect("indexes");
        dir
    }

    #[test]
    fn a_segment_a_concurrent_commit_unlinked_is_loaded_from_the_new_manifest() {
        let dir = indexed("pub fn before() {}\n");
        // This reader has read the manifest and released the lock.
        let reader = opened(dir.path());
        // Another process's refresh commits and unlinks the segment it names.
        std::fs::write(dir.path().join("a.rs"), "pub fn after_the_edit() {}\n").expect("write");
        index::refresh(&mut opened(dir.path()), &Plan::default()).expect("refreshes");

        let (store, relations) = load(reader, dir.path())
            .expect("no I/O error")
            .expect("loaded after one retry");
        assert!(relations.contains_key("def"));
        assert_eq!(store.manifest(), opened(dir.path()).manifest());
    }

    #[test]
    fn a_segment_missing_under_an_unmoved_manifest_is_corrupt() {
        let dir = indexed("pub fn only() {}\n");
        let reader = opened(dir.path());
        let seg = reader.manifest().files["a.rs"].seg.to_string();
        std::fs::remove_file(dir.path().join(".codeintel/seg").join(seg)).expect("removes");

        let answer = load(reader, dir.path())
            .expect("no I/O error")
            .map(|_| ())
            .expect_err("an answer, not a load");
        assert_eq!(answer.status, Status::Corrupt, "{:?}", answer.hint);
    }
}
