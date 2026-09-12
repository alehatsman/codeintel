//! Turning a result into text a human and an agent can both read.
//!
//! A raw `SymId` is a ~68-character SCIP string — unreadable, and impossible to
//! retype correctly. Every column that holds one renders as `Name path:line`
//! instead (`specs/05-surface.md` § Symbol rendering). That is presentation
//! only: the tuple is unchanged and `--raw` is what round-trips.
//!
//! One printed field per column, always. A symbol's name and location are
//! joined by a space rather than a tab, so the text form stays exactly as wide
//! as `columns` says it is — a consumer that splits on tabs is never handed a
//! ragged table.

use std::collections::{BTreeMap, HashMap};

use datalog::atom::Atom;
use datalog::{Engine, QueryResult, Relation};

/// Where a symbol is defined, for display.
#[derive(Debug, Clone, Copy)]
struct Site {
    /// Its `Name` from `def`.
    name: Atom,
    /// The file it is defined in.
    file: Atom,
    /// Its first line, from `def_span`. Absent for a symbol with no span.
    line: Option<Atom>,
}

/// Which atoms are symbols, and where they live.
///
/// Symbol-ness is **extracted, not guessed**: an atom is a symbol exactly when
/// `def` has a row for it. Nothing here infers from the shape of a string.
#[derive(Debug, Default)]
pub struct Sites {
    sites: HashMap<Atom, Site>,
}

impl Sites {
    /// Build the table from the index's base relations.
    ///
    /// One pass over `def` and one over `def_span` — both are already loaded,
    /// and both are sorted by symbol, so this costs a walk and nothing else.
    #[must_use]
    pub fn of(relations: &BTreeMap<&str, Relation>) -> Self {
        let mut sites: HashMap<Atom, Site> = HashMap::new();
        // def(S, F, Kind, Name)
        if let Some(def) = relations.get("def") {
            for row in def.iter() {
                if let [symbol, file, _, name] = *row {
                    sites.entry(symbol).or_insert(Site {
                        name,
                        file,
                        line: None,
                    });
                }
            }
        }
        // def_span(S, StartLine, ...)
        if let Some(spans) = relations.get("def_span") {
            for row in spans.iter() {
                if let [symbol, line, ..] = *row
                    && let Some(site) = sites.get_mut(&symbol)
                {
                    site.line.get_or_insert(line);
                }
            }
        }
        Self { sites }
    }

    /// One cell, rendered. A symbol becomes `Name path:line`; anything else is
    /// its own text.
    fn cell(&self, engine: &Engine, atom: Atom) -> String {
        let text = |a: Atom| -> String {
            engine
                .resolve(a)
                .map_or_else(|| a.to_string(), ToString::to_string)
        };
        let Some(site) = self.sites.get(&atom) else {
            return text(atom);
        };
        match site.line {
            // A symbol with no span still renders as its name: a bare SymId is
            // no more useful for having no location.
            None => format!("{} {}", text(site.name), text(site.file)),
            Some(line) => format!("{} {}:{}", text(site.name), text(site.file), text(line)),
        }
    }
}

/// A rendered result, and whether printing it hit a cap.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    /// The rows, sorted lexicographically by their printed text.
    pub rows: Vec<String>,
    /// True when `max_result_bytes` dropped rows here rather than in the engine.
    pub truncated: bool,
}

/// Render every row, cap the total bytes, then sort.
///
/// The byte cap is applied to the **rendered** text, not to the engine's
/// estimate over raw atoms: symbol expansion is what the consumer's context
/// window actually pays for, and the engine cannot know about it without
/// learning what a symbol is (`specs/00-overview.md` invariant 6).
///
/// Capping before sorting keeps a truncated answer the engine's stable prefix
/// rather than a re-sorted sample of it (`specs/05-surface.md` § `query`).
#[must_use]
pub fn render(
    engine: &Engine,
    result: &QueryResult,
    sites: &Sites,
    raw: bool,
    max_bytes: usize,
) -> Rendered {
    let mut rows = Vec::with_capacity(result.rows.len());
    let mut bytes = 0usize;
    let mut truncated = false;
    for row in &result.rows {
        let line = row
            .iter()
            .map(|a| {
                if raw {
                    engine
                        .resolve(*a)
                        .map_or_else(|| a.to_string(), ToString::to_string)
                } else {
                    sites.cell(engine, *a)
                }
            })
            .collect::<Vec<_>>()
            .join("\t");
        if bytes + line.len() + 1 > max_bytes {
            truncated = true;
            break;
        }
        bytes += line.len() + 1;
        rows.push(line);
    }
    rows.sort();
    Rendered { rows, truncated }
}

/// One row rendered with no symbol expansion: integers as digits, strings as
/// themselves. The form fact files and goldens are written in.
#[must_use]
pub fn render_row(engine: &Engine, row: &[Atom]) -> String {
    row.iter()
        .map(|a| {
            engine
                .resolve(*a)
                .map_or_else(|| a.to_string(), ToString::to_string)
        })
        .collect::<Vec<_>>()
        .join("\t")
}
