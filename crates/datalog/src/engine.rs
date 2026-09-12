//! The engine: load facts, load rules, answer a query.
//!
//! Evaluation is semi-naive, stratum by stratum, with the literal order chosen
//! most-bound-first. `load_rules` is separate from `query` so the standard
//! library is parsed and stratified once and a user query is checked against an
//! already-built rule set.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::Instant;

use crate::ast::{Literal, Program, Rule};
use crate::atom::{Atom, Term, is_int};
use crate::check::{Schema, check};
use crate::db::Db;
use crate::diag::{Diagnostic, Result, Status};
use crate::limits::{Limits, Stats};
use crate::matcher::{Matcher, Regexes};
use crate::parse::parse;
use crate::relation::Relation;
use crate::solve::{Layers, Solver};
use crate::strata::{Strata, stratify};
use crate::symbols::Symbols;
use crate::transform::transform;

/// One answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueryResult {
    /// Variable names from the goal, in order of first appearance.
    pub columns: Vec<String>,
    /// Result tuples, sorted. A truncated result is a stable prefix, not a
    /// sample: an agent paging through gets the same rows each time.
    pub rows: Vec<Vec<Atom>>,
    /// True when a cap dropped rows.
    pub truncated: bool,
    /// Which cap fired, by name.
    pub cap: Option<&'static str>,
    /// What the evaluation cost.
    pub stats: Stats,
}

/// How the fixpoint is reached. The naive mode exists so semi-naive can be
/// differentially tested against it — a semi-naive bug drops rows silently, and
/// that is the only practical defence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Re-derive from each relation's delta.
    SemiNaive,
    /// Re-derive everything from the full relations, every round. Test-only:
    /// it exists to be compared against, never to answer a real question.
    #[cfg(test)]
    Naive,
}

/// A Datalog engine over one set of base relations.
pub struct Engine {
    syms: Box<dyn Symbols>,
    base: Db,
    rules: Vec<Rule>,
    regexes: Option<Box<dyn Regexes>>,
}

impl core::fmt::Debug for Engine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Engine")
            .field("relations", &self.base.len())
            .field("rules", &self.rules.len())
            .field("regexes", &self.regexes.is_some())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// An engine over an empty store, using `syms` as its dictionary.
    #[must_use]
    pub fn new(syms: Box<dyn Symbols>) -> Self {
        Self {
            syms,
            base: Db::new(),
            rules: Vec::new(),
            regexes: None,
        }
    }

    /// Install the regex engine the `match` builtin needs.
    #[must_use]
    pub fn with_regexes(mut self, regexes: Box<dyn Regexes>) -> Self {
        self.regexes = Some(regexes);
        self
    }

    /// The atom for `s`, interning it if the dictionary does not have it.
    pub fn intern(&mut self, s: &str) -> Option<Atom> {
        self.syms.intern(s)
    }

    /// The string behind an atom.
    #[must_use]
    pub fn resolve(&self, a: Atom) -> Option<&str> {
        self.syms.resolve(a)
    }

    /// Install a base relation, replacing any relation of the same name.
    pub fn insert_relation(&mut self, name: impl Into<String>, mut relation: Relation) {
        relation.settle();
        self.base.insert(name, relation);
    }

    /// The base relations, by name and arity.
    pub fn relations(&self) -> impl Iterator<Item = (&str, usize)> {
        self.base.schema()
    }

    /// Parse and check rules, adding them to the engine's rule set.
    ///
    /// # Errors
    /// `invalid-query` for a parse or safety defect, `unstratified` for a
    /// negative cycle. The rule set is left untouched when this fails.
    pub fn load_rules(&mut self, src: &str) -> Result<()> {
        let program = parse(src, self.syms.as_mut())?;
        if let Some(query) = program.query {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                query.span,
                "a rule file may not hold a query (`?-`); pass it to `query` instead",
            ));
        }
        let mut combined = Program {
            rules: self.rules.clone(),
            query: None,
        };
        combined.rules.extend(program.rules.iter().cloned());
        check(&combined, self.base_arities())?;
        stratify(&combined)?;
        self.rules.extend(program.rules);
        Ok(())
    }

    /// How many rules are loaded.
    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    fn base_arities(&self) -> Vec<(String, usize)> {
        self.base
            .schema()
            .map(|(n, a)| (n.to_string(), a))
            .collect()
    }

    /// Evaluate a query against the loaded rules and facts.
    ///
    /// The source may define rules of its own; they are layered on top for this
    /// call only. A query rule whose head matches a loaded rule shadows it, and
    /// the shadowing is reported in [`Stats::shadowed`], never silently.
    ///
    /// # Errors
    /// `invalid-query`, `unstratified`, `timeout` or `budget-exceeded`, each
    /// naming what to change.
    pub fn query(&mut self, src: &str, limits: &Limits) -> Result<QueryResult> {
        self.evaluate(src, limits, Mode::SemiNaive, true)
    }

    /// The same query under naive evaluation. The differential-testing twin of
    /// [`Self::query`]; never used to answer a real question.
    #[cfg(test)]
    pub(crate) fn query_naive(&mut self, src: &str, limits: &Limits) -> Result<QueryResult> {
        self.evaluate(src, limits, Mode::Naive, true)
    }

    /// The same query with demand transformation switched off. Test-only: it
    /// exists so the transformation can be shown to apply — equal tuple counts
    /// mean it silently did not.
    #[cfg(test)]
    pub(crate) fn query_undemanded(&mut self, src: &str, limits: &Limits) -> Result<QueryResult> {
        self.evaluate(src, limits, Mode::SemiNaive, false)
    }

    fn evaluate(
        &mut self,
        src: &str,
        limits: &Limits,
        mode: Mode,
        demand: bool,
    ) -> Result<QueryResult> {
        let start = Instant::now();
        let program = parse(src, self.syms.as_mut())?;
        let Some(goal) = program.query.clone() else {
            return Err(Diagnostic::whole(
                Status::InvalidQuery,
                "no query in this source: a query is a body introduced by `?-` and ended by `.`",
            ));
        };

        let (rules, shadowed) = self.merge_rules(&program);
        let plain = Program {
            rules,
            query: Some(goal.clone()),
        };
        let schema = check(&plain, self.base_arities())?;
        let strata = stratify(&plain)?;
        check_planning_limits(&plain, &strata, limits)?;

        // Demand transformation is a performance step, never a legality one:
        // the program above is already safe and already stratified. If the
        // rewrite does not survive the same two checks, evaluate the original
        // rather than reject a query the user wrote correctly.
        let demanded = demand
            .then(|| transform(&plain, &schema.derived))
            .flatten()
            .and_then(|t| {
                let schema = check(&t.program, self.base_arities()).ok()?;
                let strata = stratify(&t.program).ok()?;
                Some((t.program, schema, strata, t.names))
            });
        let (combined, schema, strata, transformed) = match demanded {
            Some(parts) => parts,
            None => (plain, schema, strata, Vec::new()),
        };
        let goal = combined.query.clone().unwrap_or(goal);

        let patterns = self.compile_patterns(&combined)?;
        let derived = Cell::new(0u64);
        let mut store = Db::new();
        let mut stats = Stats {
            strata: strata.len(),
            shadowed,
            elapsed_ms: 0,
            derived: 0,
            plan: Vec::new(),
            transformed,
        };

        let run = Run {
            schema: &schema,
            strata: &strata,
            patterns: &patterns,
            limits,
            start,
            mode,
        };
        self.run_strata(&combined, &run, &derived, &mut store, &mut stats)?;

        // A ground goal — `?- reaches("b", "b").` — has no columns. Truth is
        // one empty row and falsehood is no rows; without the placeholder the
        // answer would be indistinguishable from "no".
        let columns: Vec<Term> = goal.columns.iter().map(|v| Term::Var(*v)).collect();
        let ground = columns.is_empty();
        let head: Vec<Term> = if ground {
            vec![Term::Const(0)]
        } else {
            columns
        };
        let mut rows = Relation::new(head.len());
        {
            let empty = BTreeMap::new();
            let solver = Solver {
                db: Layers {
                    base: &self.base,
                    derived: &store,
                },
                delta: &empty,
                syms: self.syms.as_ref(),
                patterns: &patterns,
                limits,
                start,
                derived: &derived,
            };
            stats
                .plan
                .push(format!("?-: {}", plan_of(&solver, &goal.body)));
            solver.run(&goal.body, &head, goal.vars.len(), None, &mut rows)?;
        }
        rows.settle();

        stats.derived = derived.get();
        stats.elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(self.finish(&goal.columns, &goal.vars, &rows, limits, stats, ground))
    }

    /// Loaded rules minus any a query rule shadows, plus the query's own rules.
    fn merge_rules(&self, program: &Program) -> (Vec<Rule>, Vec<String>) {
        let local: Vec<String> = program
            .rules
            .iter()
            .map(|r| format!("{}/{}", r.head.name, r.head.args.len()))
            .collect();
        let mut shadowed = Vec::new();
        let mut rules: Vec<Rule> = Vec::new();
        for rule in &self.rules {
            let key = format!("{}/{}", rule.head.name, rule.head.args.len());
            if local.contains(&key) {
                if !shadowed.contains(&key) {
                    shadowed.push(key);
                }
                continue;
            }
            rules.push(rule.clone());
        }
        rules.extend(program.rules.iter().cloned());
        (rules, shadowed)
    }

    fn compile_patterns(&self, program: &Program) -> Result<BTreeMap<String, Box<dyn Matcher>>> {
        let mut out = BTreeMap::new();
        let mut wanted = Vec::new();
        for rule in &program.rules {
            collect_patterns(&rule.body, &mut wanted);
        }
        if let Some(query) = &program.query {
            collect_patterns(&query.body, &mut wanted);
        }
        for (pattern, span) in wanted {
            if out.contains_key(&pattern) {
                continue;
            }
            let Some(regexes) = self.regexes.as_ref() else {
                return Err(Diagnostic::at(
                    Status::InvalidQuery,
                    span,
                    "the `match` builtin needs a regex engine and this host installed none; \
                     use `prefix`, `suffix` or `contains`",
                ));
            };
            let compiled = regexes.compile(&pattern).map_err(|e| {
                Diagnostic::at(
                    Status::InvalidQuery,
                    span,
                    format!("invalid regex `{pattern}`: {e}"),
                )
            })?;
            out.insert(pattern, compiled);
        }
        Ok(out)
    }

    fn run_strata(
        &self,
        program: &Program,
        run: &Run<'_>,
        derived: &Cell<u64>,
        store: &mut Db,
        stats: &mut Stats,
    ) -> Result<()> {
        let (schema, strata, patterns, limits, start) =
            (run.schema, run.strata, run.patterns, run.limits, run.start);
        for stratum in &strata.order {
            for name in stratum {
                let arity = schema.arity.get(name).copied().unwrap_or(1);
                store.get_or_create(name, arity).settle();
            }
            let rules: Vec<&Rule> = program
                .rules
                .iter()
                .filter(|r| stratum.contains(&r.head.name))
                .collect();

            let mut delta: BTreeMap<String, Relation> = BTreeMap::new();
            let mut iteration = 0usize;
            loop {
                let mut produced: BTreeMap<String, Relation> = BTreeMap::new();
                for rule in &rules {
                    let recursive: Vec<usize> = recursive_positions(&rule.body, stratum);
                    let arity = rule.head.args.len();
                    let out = produced
                        .entry(rule.head.name.clone())
                        .or_insert_with(|| Relation::new(arity));
                    let solver = Solver {
                        db: Layers {
                            base: &self.base,
                            derived: store,
                        },
                        delta: &delta,
                        syms: self.syms.as_ref(),
                        patterns,
                        limits,
                        start,
                        derived,
                    };
                    let seeded = iteration > 0 && run.mode == Mode::SemiNaive;
                    if seeded {
                        for at in recursive {
                            solver.run(
                                &rule.body,
                                &rule.head.args,
                                rule.vars.len(),
                                Some(at),
                                out,
                            )?;
                        }
                    } else {
                        if iteration == 0 && stats.plan.len() < 64 {
                            stats.plan.push(format!(
                                "{}/{arity}: {}",
                                rule.head.name,
                                plan_of(&solver, &rule.body)
                            ));
                        }
                        solver.run(&rule.body, &rule.head.args, rule.vars.len(), None, out)?;
                    }
                }

                let mut changed = false;
                let mut next: BTreeMap<String, Relation> = BTreeMap::new();
                for (name, mut rel) in produced {
                    rel.settle();
                    let arity = rel.arity();
                    let fresh = match store.get(&name) {
                        Some(existing) => rel.minus(existing),
                        None => rel,
                    };
                    if !fresh.is_empty() {
                        changed = true;
                        store.get_or_create(&name, arity).absorb(&fresh);
                    }
                    next.insert(name, fresh);
                }
                delta = next;
                iteration += 1;
                if !changed {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Apply the row and byte caps, then render the columns.
    fn finish(
        &self,
        columns: &[u16],
        vars: &[String],
        rows: &Relation,
        limits: &Limits,
        stats: Stats,
        ground: bool,
    ) -> QueryResult {
        let names: Vec<String> = columns
            .iter()
            .map(|v| {
                vars.get(usize::from(*v))
                    .cloned()
                    .unwrap_or_else(|| format!("?{v}"))
            })
            .collect();

        let mut out = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        let mut cap = None;
        for row in rows.iter() {
            if out.len() >= limits.max_result_rows {
                truncated = true;
                cap = Some("max_result_rows");
                break;
            }
            let width = if ground { 1 } else { self.row_bytes(row) };
            if bytes + width > limits.max_result_bytes {
                truncated = true;
                cap = Some("max_result_bytes");
                break;
            }
            bytes += width;
            out.push(if ground { Vec::new() } else { row.to_vec() });
        }
        QueryResult {
            columns: names,
            rows: out,
            truncated,
            cap,
            stats,
        }
    }

    /// Rendered width of a row: each column's text plus one separator.
    fn row_bytes(&self, row: &[Atom]) -> usize {
        row.iter()
            .map(|a| {
                if is_int(*a) {
                    a.to_string().len() + 1
                } else {
                    self.syms.resolve(*a).map_or(12, str::len) + 1
                }
            })
            .sum()
    }
}

/// The per-query state the fixpoint reads.
struct Run<'a> {
    schema: &'a Schema,
    strata: &'a Strata,
    patterns: &'a BTreeMap<String, Box<dyn Matcher>>,
    limits: &'a Limits,
    start: Instant,
    mode: Mode,
}

fn plan_of(solver: &Solver<'_>, body: &[Literal]) -> String {
    solver
        .plan(body, None)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn recursive_positions(body: &[Literal], stratum: &[String]) -> Vec<usize> {
    body.iter()
        .enumerate()
        .filter_map(|(i, lit)| match lit {
            Literal::Pos(p) if stratum.contains(&p.name) => Some(i),
            _ => None,
        })
        .collect()
}

fn collect_patterns(body: &[Literal], out: &mut Vec<(String, (usize, usize))>) {
    for lit in body {
        match lit {
            Literal::Str {
                test: crate::ast::StrTest::Match,
                pattern,
                span,
                ..
            } => {
                out.push((pattern.clone(), *span));
            }
            Literal::Assign {
                expr: crate::ast::Expr::Count { goal, .. },
                ..
            } => {
                collect_patterns(goal, out);
            }
            _ => {}
        }
    }
}

fn check_planning_limits(program: &Program, strata: &Strata, limits: &Limits) -> Result<()> {
    if strata.len() > limits.max_strata {
        return Err(Diagnostic::whole(
            Status::InvalidQuery,
            format!(
                "this program needs {} strata and the limit is {} (max_strata)",
                strata.len(),
                limits.max_strata
            ),
        ));
    }
    let mut bodies: Vec<(String, usize, (usize, usize))> = program
        .rules
        .iter()
        .map(|r| {
            (
                format!("{}/{}", r.head.name, r.head.args.len()),
                r.body.len(),
                r.span,
            )
        })
        .collect();
    if let Some(query) = &program.query {
        bodies.push(("?-".to_string(), query.body.len(), query.span));
    }
    for (name, len, span) in bodies {
        if len > limits.max_body_literals {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                span,
                format!(
                    "`{name}` has {len} body literals and the limit is {} (max_body_literals); \
                     split it into two rules",
                    limits.max_body_literals
                ),
            ));
        }
    }
    Ok(())
}
