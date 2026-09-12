//! Demand transformation (magic sets).
//!
//! Bottom-up evaluation ignores the query's bindings: `?- impact_of("S", C).`
//! would compute `impact_of` for every seed — the all-pairs closure of the call
//! graph — and then filter to one. The transformation synthesizes a `magic@`
//! seed relation and guards the rules with it, so evaluation grows outward from
//! the seed instead.
//!
//! It applies to recursive **and** non-recursive predicates, and to bindings
//! that come from an earlier body literal as well as from a goal constant —
//! `?- innermost_at("src/store.rs", 142, S), impact_of(S, C).` is the headline
//! pattern and it binds the seed sideways.
//!
//! Two deliberate conservatisms: a derived predicate under `!` or inside an
//! aggregate goal is requested **unrestricted**, because negation and counting
//! both read the whole relation. Restricting those would change the answer
//! rather than the cost.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::ast::{Expr, Literal, Pred, Program, Query, Rule};
use crate::atom::Term;

/// Separator between a predicate and its adornment. Not a legal identifier
/// character, so a generated name can never collide with a user's.
const MARK: char = '@';

/// Prefix of a seed relation.
const MAGIC: &str = "magic@";

/// The rewritten program, and which predicates were rewritten.
#[derive(Debug, Clone)]
pub struct Transformed {
    /// The program to evaluate.
    pub program: Program,
    /// Adorned predicate names, for `stats.transformed`.
    pub names: Vec<String>,
}

/// The adornment carried by a predicate name, if it has one.
///
/// `b` marks a bound argument position, `f` a free one. The name *is* the
/// contract: the planner reads it back to require those positions be bound
/// before the literal runs, because an adorned relation holds only the tuples
/// reachable from its seeds.
#[must_use]
pub fn adornment_of(name: &str) -> Option<&str> {
    if name.starts_with(MAGIC) {
        return None;
    }
    let (_, suffix) = name.rsplit_once(MARK)?;
    (!suffix.is_empty() && suffix.bytes().all(|b| b == b'b' || b == b'f')).then_some(suffix)
}

/// Rewrite `program` so every binding available at a call site is pushed into
/// the predicate it calls.
///
/// Returns `None` when there is nothing to propagate — a fully free query over
/// base relations, for instance.
#[must_use]
pub fn transform(program: &Program, derived: &BTreeSet<String>) -> Option<Transformed> {
    let query = program.query.as_ref()?;
    let mut x = Xform {
        source: &program.rules,
        derived,
        out: Vec::new(),
        seen: BTreeSet::new(),
        queue: VecDeque::new(),
        names: BTreeSet::new(),
    };
    let goal = x.seed(query);
    if x.names.is_empty() {
        return None;
    }
    while let Some((name, adornment)) = x.queue.pop_front() {
        x.process(&name, &adornment);
    }
    Some(Transformed {
        program: Program {
            rules: x.out,
            query: Some(goal),
        },
        names: x.names.into_iter().collect(),
    })
}

struct Xform<'a> {
    source: &'a [Rule],
    derived: &'a BTreeSet<String>,
    out: Vec<Rule>,
    seen: BTreeSet<String>,
    queue: VecDeque<(String, Vec<bool>)>,
    names: BTreeSet<String>,
}

impl Xform<'_> {
    /// Rewrite the query body, emitting the seed facts and rules its literals
    /// imply. The query is the only place a seed can come from.
    fn seed(&mut self, query: &Query) -> Query {
        let mut bound: BTreeSet<u16> = BTreeSet::new();
        let mut body: Vec<Literal> = Vec::new();
        for lit in &query.body {
            let rewritten = self.rewrite(lit, &bound, &body, &query.vars, query.span);
            bind(lit, &mut bound);
            body.push(rewritten);
        }
        Query {
            body,
            ..query.clone()
        }
    }

    /// Rewrite one body literal, requesting whatever version of its predicate
    /// it needs and emitting the magic rule that seeds it.
    fn rewrite(
        &mut self,
        lit: &Literal,
        bound: &BTreeSet<u16>,
        preceding: &[Literal],
        vars: &[String],
        span: (usize, usize),
    ) -> Literal {
        match lit {
            Literal::Pos(pred) if self.derived.contains(&pred.name) => {
                let adornment = adorn(&pred.args, bound);
                if adornment.iter().any(|b| *b) {
                    let head = Pred {
                        name: magic_name(&pred.name, &adornment),
                        args: keep(&pred.args, &adornment),
                        span: pred.span,
                    };
                    self.out.push(Rule {
                        head,
                        body: preceding.to_vec(),
                        vars: vars.to_vec(),
                        span,
                    });
                }
                let name = adorned_name(&pred.name, &adornment);
                self.request(&pred.name, adornment);
                Literal::Pos(Pred {
                    name,
                    ..pred.clone()
                })
            }
            // Negation and aggregation read the whole relation; asking for a
            // restricted one would change the answer, not the cost.
            Literal::Neg(pred) if self.derived.contains(&pred.name) => {
                self.request_plain(&pred.name);
                lit.clone()
            }
            Literal::Assign {
                expr: Expr::Count { goal, .. },
                ..
            } => {
                for inner in goal {
                    if let Literal::Pos(p) | Literal::Neg(p) = inner
                        && self.derived.contains(&p.name)
                    {
                        self.request_plain(&p.name);
                    }
                }
                lit.clone()
            }
            other => other.clone(),
        }
    }

    fn request(&mut self, name: &str, adornment: Vec<bool>) {
        let key = adorned_name(name, &adornment);
        if key != name {
            self.names.insert(key.clone());
        }
        if self.seen.insert(key) {
            self.queue.push_back((name.to_string(), adornment));
        }
    }

    fn request_plain(&mut self, name: &str) {
        let arity = self
            .source
            .iter()
            .find(|r| r.head.name == name)
            .map_or(0, |r| r.head.args.len());
        self.request(name, vec![false; arity]);
    }

    /// Emit the adorned rules for one predicate, guarded by its seed relation.
    fn process(&mut self, name: &str, adornment: &[bool]) {
        let guarded = adornment.iter().any(|b| *b);
        let rules: Vec<Rule> = self
            .source
            .iter()
            .filter(|r| r.head.name == name)
            .cloned()
            .collect();

        for rule in rules {
            let mut bound: BTreeSet<u16> = BTreeSet::new();
            let mut body: Vec<Literal> = Vec::new();
            if guarded {
                let guard = Pred {
                    name: magic_name(name, adornment),
                    args: keep(&rule.head.args, adornment),
                    span: rule.head.span,
                };
                for arg in &guard.args {
                    if let Term::Var(v) = arg {
                        bound.insert(*v);
                    }
                }
                body.push(Literal::Pos(guard));
            }
            for lit in &rule.body {
                let rewritten = self.rewrite(lit, &bound, &body, &rule.vars, rule.span);
                bind(lit, &mut bound);
                body.push(rewritten);
            }
            self.out.push(Rule {
                head: Pred {
                    name: adorned_name(name, adornment),
                    ..rule.head.clone()
                },
                body,
                vars: rule.vars.clone(),
                span: rule.span,
            });
        }
    }
}

/// The bound/free pattern of a literal's arguments under `bound`.
fn adorn(args: &[Term], bound: &BTreeSet<u16>) -> Vec<bool> {
    args.iter()
        .map(|t| match t {
            Term::Const(_) => true,
            Term::Var(v) => bound.contains(v),
            Term::Wildcard => false,
        })
        .collect()
}

/// The arguments at bound positions, in order.
fn keep(args: &[Term], adornment: &[bool]) -> Vec<Term> {
    args.iter()
        .zip(adornment)
        .filter_map(|(t, keep)| keep.then_some(*t))
        .collect()
}

fn adorned_name(name: &str, adornment: &[bool]) -> String {
    if !adornment.iter().any(|b| *b) {
        return name.to_string();
    }
    let pattern: String = adornment
        .iter()
        .map(|b| if *b { 'b' } else { 'f' })
        .collect();
    format!("{name}{MARK}{pattern}")
}

fn magic_name(name: &str, adornment: &[bool]) -> String {
    format!("{MAGIC}{}", adorned_name(name, adornment))
}

/// Variables a literal binds once it has run. Mirrors the planner's rule.
fn bind(lit: &Literal, bound: &mut BTreeSet<u16>) {
    let mut add = |t: &Term| {
        if let Term::Var(v) = t {
            bound.insert(*v);
        }
    };
    match lit {
        Literal::Pos(p) => p.args.iter().for_each(add),
        Literal::Between { out, .. } => add(out),
        Literal::Assign { target, .. } => add(target),
        Literal::Neg(_) | Literal::Compare { .. } | Literal::Str { .. } => {}
    }
}

/// Arity per relation in a transformed program, for creating empty relations.
#[must_use]
pub fn arities(program: &Program) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for rule in &program.rules {
        out.entry(rule.head.name.clone())
            .or_insert(rule.head.args.len());
    }
    out
}
