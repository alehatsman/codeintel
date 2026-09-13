//! Turning a result into text a human and an agent can both read.
//!
//! A raw `SymId` is a ~68-character SCIP string — unreadable, and impossible to
//! retype correctly. Every column that holds one renders as `Name path:line`
//! instead (`specs/05-surface.md` § Symbol rendering). That is presentation
//! only: the tuple is unchanged and the raw form is what round-trips.
//!
//! **Both forms of a row are produced together, and the set is ordered once.**
//! Rendering twice and sorting each result independently gives two arrays whose
//! `i`th entries describe different tuples — so a consumer that shows the
//! display form and feeds the raw form into its next query binds the wrong
//! symbol. The two notations are one answer and are kept in one order.
//!
//! Values stay a `Vec<String>` until the moment of printing. Joining on tabs
//! and splitting back apart loses the column boundaries of any value that
//! contains a tab, which doc comments do.

use std::borrow::Cow;
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
    /// so this costs a walk and nothing else.
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

    /// One cell, expanded. A symbol becomes `Name path:line`; anything else is
    /// its own text.
    fn display(&self, engine: &Engine, atom: Atom) -> String {
        let Some(site) = self.sites.get(&atom) else {
            return raw(engine, atom);
        };
        let (name, file) = (raw(engine, site.name), raw(engine, site.file));
        let where_ = match site.line {
            // A symbol with no span still renders as its name: a bare SymId is
            // no more useful for having no location.
            None => file,
            Some(line) => format!("{file}:{}", raw(engine, line)),
        };
        if name.is_empty() {
            // A crate-root module has no name. Prefixing the location with a
            // space to hold an empty column reads as a rendering glitch, and
            // the location alone is the whole of what is known.
            return where_;
        }
        format!("{name} {where_}")
    }
}

/// One atom as itself: an integer as digits, a string as its text.
fn raw(engine: &Engine, atom: Atom) -> String {
    engine
        .resolve(atom)
        .map_or_else(|| atom.to_string(), ToString::to_string)
}

/// One result row, in both notations. The two `Vec`s are the same length as
/// `columns` and describe the same tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// One value per column, symbols expanded.
    pub display: Vec<String>,
    /// One value per column, atoms as themselves. This is what round-trips
    /// into another query's literal.
    pub raw: Vec<String>,
}

impl Row {
    /// The printed line: values separated by tabs, each value's own control
    /// characters escaped.
    ///
    /// Text output promises one row per line and `columns.len()` fields
    /// (`specs/05-surface.md` § `query`). A `def_doc` holds a whole doc comment
    /// and so holds newlines, which broke both promises *silently*: one row
    /// printed as seven lines with ragged field counts, under `status: ok`.
    #[must_use]
    pub fn line(&self, raw: bool) -> String {
        let values = if raw { &self.raw } else { &self.display };
        values
            .iter()
            .map(|v| escape(v))
            .collect::<Vec<_>>()
            .join("\t")
    }
}

/// A value's control characters, escaped so the row survives printing.
///
/// Backslash goes first and is escaped itself, so the mapping is reversible: a
/// source text containing a literal `\n` and an escaped newline would otherwise
/// print identically, and a consumer could not tell which it had.
fn escape(value: &str) -> Cow<'_, str> {
    if !value.contains(['\\', '\n', '\r', '\t']) {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// A rendered result, and whether printing it hit the byte cap.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    /// The rows, ordered lexicographically by their display text.
    pub rows: Vec<Row>,
    /// True when `max_result_bytes` dropped rows.
    pub truncated: bool,
}

/// Render every row in both notations, cap the printed bytes, then order.
///
/// `raw` selects which notation the byte cap is measured against, because that
/// is the one that will be printed. Both notations are produced either way: the
/// JSON response carries them side by side.
///
/// **Ordering is always by the display text**, whichever notation is printed.
/// The engine's own order is by atom, which is dictionary insertion order, so a
/// cold index and an incrementally-updated one would print the same rows in
/// different orders (`specs/05-surface.md` § `query`). Ordering both notations
/// by one key also means `--raw` and the default print the same answer in the
/// same sequence, rather than two shuffles of it.
///
/// **The raw row breaks ties.** Two different tuples can print identically —
/// two symbols of one name on one line — and ordered by display alone they
/// kept atom order, which is the index-dependent order this sort removes.
///
/// The cap runs **after** the sort. An earlier version capped first, on the
/// argument that a truncated answer should be the engine's stable prefix — but
/// the engine's order is by atom, which is dictionary insertion order, so that
/// made the surviving *set* depend on whether the index was built cold or
/// incrementally. Sorting first makes the set a function of the rendered text
/// alone, which is the property a consumer actually relies on.
///
/// `max_result_rows` is applied inside the engine, before any of this, so that
/// cap still selects an index-dependent set. Fixing it means the engine
/// materialising every row and the host doing all capping — a real change, not
/// made here, and named so the remaining gap is not mistaken for closed.
#[must_use]
pub fn render(
    engine: &Engine,
    result: &QueryResult,
    sites: &Sites,
    raw_wanted: bool,
    max_bytes: usize,
) -> Rendered {
    let mut rows: Vec<Row> = result
        .rows
        .iter()
        .map(|tuple| Row {
            display: tuple.iter().map(|a| sites.display(engine, *a)).collect(),
            raw: tuple.iter().map(|a| raw(engine, *a)).collect(),
        })
        .collect();
    rows.sort_by(|a, b| a.display.cmp(&b.display).then_with(|| a.raw.cmp(&b.raw)));

    let mut bytes = 0usize;
    let mut kept = 0usize;
    for row in &rows {
        // The newline the caller will print is part of what the consumer pays.
        let width = row.line(raw_wanted).len() + 1;
        if bytes + width > max_bytes {
            break;
        }
        bytes += width;
        kept += 1;
    }
    let truncated = kept < rows.len();
    rows.truncate(kept);
    Rendered { rows, truncated }
}

/// One row with no symbol expansion. The form fact files and goldens are
/// written in.
#[must_use]
pub fn render_row(engine: &Engine, row: &[Atom]) -> String {
    row.iter()
        .map(|a| raw(engine, *a))
        .collect::<Vec<_>>()
        .join("\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use datalog::{Limits, Strings};

    /// An engine holding three definitions whose display order is the reverse
    /// of their raw order: the `SymId` sorts by path, the display form by name.
    fn engine() -> (Engine, Sites) {
        let mut engine = Engine::new(Box::new(Strings::new()));
        let mut def = Relation::new(4);
        let mut span = Relation::new(5);
        let mut relations = BTreeMap::new();
        for (path, name, line) in [
            ("src/a.rs", "zulu", 10u32),
            ("src/m.rs", "mike", 20),
            ("src/z.rs", "alpha", 30),
        ] {
            let symbol = engine
                .intern(&format!("local {path} {name}()."))
                .expect("room");
            let file = engine.intern(path).expect("room");
            let name = engine.intern(name).expect("room");
            let kind = engine.intern("function").expect("room");
            assert!(def.push(&[symbol, file, kind, name]));
            assert!(span.push(&[symbol, line, line, 0, 0]));
        }
        def.settle();
        span.settle();
        relations.insert("def", def.clone());
        relations.insert("def_span", span.clone());
        let sites = Sites::of(&relations);
        engine.insert_relation("def", def);
        engine.insert_relation("def_span", span);
        (engine, sites)
    }

    fn rendered(max_bytes: usize, raw_wanted: bool) -> Rendered {
        let (mut engine, sites) = engine();
        let result = engine
            .query("?- def(S, F, _, N).", &Limits::default())
            .expect("the query runs");
        render(&engine, &result, &sites, raw_wanted, max_bytes)
    }

    #[test]
    fn both_notations_describe_the_same_tuple_at_the_same_index() {
        // Rendering each notation separately and sorting both gives two arrays
        // whose `i`th rows are different tuples — so a consumer that shows the
        // display form and feeds the raw form back binds the wrong symbol.
        let out = rendered(usize::MAX, false);
        assert_eq!(out.rows.len(), 3);
        for row in &out.rows {
            let name = row.raw.get(2).expect("the Name column");
            let symbol = row.display.first().expect("the symbol column");
            assert!(
                symbol.starts_with(name),
                "display {symbol:?} does not describe raw {:?}",
                row.raw
            );
        }
    }

    #[test]
    fn the_order_is_the_display_text_whichever_notation_is_printed() {
        // One answer in two notations, not two shuffles of it.
        let display = rendered(usize::MAX, false);
        let raw = rendered(usize::MAX, true);
        let names: Vec<&str> = display
            .rows
            .iter()
            .map(|r| r.raw[2].as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["alpha", "mike", "zulu"], "not in display order");
        assert_eq!(
            display.rows, raw.rows,
            "--raw reordered an answer it only reprints"
        );
    }

    #[test]
    fn rows_that_print_the_same_are_ordered_by_the_tuple_not_the_atom() {
        // Two symbols, one name, one line: identical display rows. Interned in
        // reverse raw order, so the engine yields them z-first; a display-only
        // sort kept that, and an index built in the other order printed a-first.
        let mut engine = Engine::new(Box::new(Strings::new()));
        let mut def = Relation::new(4);
        let mut span = Relation::new(5);
        for symbol in ["local src/a.rs z().", "local src/a.rs a()."] {
            let symbol = engine.intern(symbol).expect("room");
            let file = engine.intern("src/a.rs").expect("room");
            let kind = engine.intern("function").expect("room");
            let name = engine.intern("same").expect("room");
            assert!(def.push(&[symbol, file, kind, name]));
            assert!(span.push(&[symbol, 7, 7, 0, 0]));
        }
        def.settle();
        span.settle();
        let mut relations = BTreeMap::new();
        relations.insert("def", def.clone());
        relations.insert("def_span", span.clone());
        let sites = Sites::of(&relations);
        engine.insert_relation("def", def);
        engine.insert_relation("def_span", span);

        let result = engine
            .query("?- def(S, _, _, _).", &Limits::default())
            .expect("the query runs");
        let out = render(&engine, &result, &sites, false, usize::MAX);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[0].display, out.rows[1].display, "not a tie");
        let raw: Vec<&str> = out.rows.iter().map(|r| r.raw[0].as_str()).collect();
        assert_eq!(raw, ["local src/a.rs a().", "local src/a.rs z()."]);
    }

    #[test]
    fn the_byte_cap_counts_the_printed_form_not_the_raw_one() {
        // The raw rows here are much wider than the display rows. A cap set to
        // fit two display rows must yield two rows — measuring the raw width
        // instead would drop one that fits.
        let all = rendered(usize::MAX, false);
        let two: usize = all
            .rows
            .iter()
            .take(2)
            .map(|r| r.line(false).len() + 1)
            .sum();
        let out = rendered(two, false);
        assert_eq!(out.rows.len(), 2, "{:?}", out.rows);
        assert!(out.truncated);

        // And the raw notation is measured when it is the one being printed.
        let raw_two: usize = all
            .rows
            .iter()
            .take(2)
            .map(|r| r.line(true).len() + 1)
            .sum();
        assert!(
            raw_two > two,
            "the fixture does not exercise the difference"
        );
        assert_eq!(rendered(raw_two, true).rows.len(), 2);
    }

    #[test]
    fn a_value_holding_a_tab_stays_one_value() {
        // Values are carried structurally, so a doc comment with a tab in it
        // cannot arrive as more values than there are columns.
        let (mut engine, sites) = engine();
        let mut doc = Relation::new(2);
        let symbol = engine.intern("local src/a.rs zulu().").expect("room");
        let text = engine.intern("a doc\twith a tab").expect("room");
        assert!(doc.push(&[symbol, text]));
        doc.settle();
        engine.insert_relation("def_doc", doc);

        let result = engine
            .query("?- def_doc(S, D).", &Limits::default())
            .expect("the query runs");
        let out = render(&engine, &result, &sites, false, usize::MAX);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].raw.len(), result.columns.len());
        assert_eq!(out.rows[0].raw[1], "a doc\twith a tab");
    }
}
