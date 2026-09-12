//! Loading a file of ground facts into base relations.
//!
//! Used by fixtures and by the agent eval, which needs a realistic fact set
//! with no extractor in sight. The real indexer writes segments
//! (`specs/04-storage.md`); this is the text form, and it is deliberately the
//! same syntax the query language uses.

use datalog::atom::Term;
use datalog::diag::{Diagnostic, Result, Status};
use datalog::relation::Relation;
use datalog::{Engine, parse};
use std::collections::BTreeMap;

/// Parse `src` as ground facts and install them as base relations.
///
/// A fact is a rule with no body and no variables: `def("S", "f", "method",
/// "get").` Anything else is rejected by name — a fact file that quietly
/// dropped a rule would be a fact file that answers differently than it reads.
///
/// # Errors
/// `invalid-query` for a parse error, a rule with a body, a non-ground
/// argument, or two facts of the same relation with different arities.
pub fn load_facts(engine: &mut Engine, src: &str) -> Result<usize> {
    let program = {
        let mut interner = Interning(engine);
        parse(src, &mut interner)?
    };
    if let Some(query) = program.query {
        return Err(Diagnostic::at(
            Status::InvalidQuery,
            query.span,
            "a fact file holds facts, not queries (`?-`)",
        ));
    }

    let mut rows: BTreeMap<String, Vec<Vec<u32>>> = BTreeMap::new();
    for rule in &program.rules {
        if !rule.body.is_empty() {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                rule.span,
                format!(
                    "`{}` has a body, so it is a rule; a fact file holds ground facts only",
                    rule.head.name
                ),
            ));
        }
        let mut row = Vec::with_capacity(rule.head.args.len());
        for arg in &rule.head.args {
            let Term::Const(atom) = arg else {
                return Err(Diagnostic::at(
                    Status::InvalidQuery,
                    rule.head.span,
                    format!(
                        "`{}` has a variable or `_` argument; a fact is ground",
                        rule.head.name
                    ),
                ));
            };
            row.push(*atom);
        }
        rows.entry(rule.head.name.clone()).or_default().push(row);
    }

    let mut count = 0;
    for (name, rows) in rows {
        let arity = rows.first().map_or(1, Vec::len);
        let mut relation = Relation::new(arity);
        for row in &rows {
            if !relation.push(row) {
                return Err(Diagnostic::whole(
                    Status::InvalidQuery,
                    format!(
                        "`{name}` appears with {} arguments and with {arity}; a relation's arity \
                         is fixed by its first use",
                        row.len()
                    ),
                ));
            }
            count += 1;
        }
        engine.insert_relation(name, relation);
    }
    Ok(count)
}

/// Borrows the engine's dictionary for the duration of a parse.
struct Interning<'a>(&'a mut Engine);

impl datalog::Symbols for Interning<'_> {
    fn intern(&mut self, s: &str) -> Option<u32> {
        self.0.intern(s)
    }

    fn resolve(&self, a: u32) -> Option<&str> {
        self.0.resolve(a)
    }
}
