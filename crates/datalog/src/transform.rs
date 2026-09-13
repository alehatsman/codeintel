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
//! A derived predicate inside an aggregate goal is always requested
//! **unrestricted**: counting reads the whole relation, and restricting it
//! would change the answer rather than the cost.
//!
//! A *negated* literal is the interesting case, and it is why this function
//! takes an `excluded` set. When every argument is bound the literal is a
//! ground membership test, and a demanded relation answers it exactly — that is
//! `!tighter_at(F, Line, S)` in `innermost_at`, and without it the headline
//! location query materializes `tighter_at` over every line of every file.
//!
//! But seeding a negated predicate adds a magic rule whose body is the *call
//! site*, and magic sets always put a predicate, its magic relation and its
//! callers in one strongly connected component. When the edge into it is
//! negative, that component is unstratifiable — which is what happens to
//! `!ambiguous(N)` inside `ref` as soon as the query also reaches `impact_of`,
//! because `impact_of`'s own recursion closes the loop.
//!
//! So seeding is decided per predicate, not per program: the caller rewrites,
//! asks [`crate::strata::negative_cycle`] what broke, adds those predicates to
//! `excluded`, and rewrites again. Every attempt is a program that passed the
//! same safety and stratification checks as the original, and the fixpoint of
//! that loop is the most demand this program can carry.

use std::collections::{BTreeSet, VecDeque};

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
    /// Predicates whose negated occurrences were seeded. The caller excludes
    /// these one component at a time when the rewrite does not stratify.
    pub seeded_negations: BTreeSet<String>,
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
pub fn transform(
    program: &Program,
    derived: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> Option<Transformed> {
    let query = program.query.as_ref()?;
    let mut x = Xform {
        source: &program.rules,
        derived,
        excluded,
        seeded_negations: BTreeSet::new(),
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
        seeded_negations: x.seeded_negations,
    })
}

struct Xform<'a> {
    source: &'a [Rule],
    derived: &'a BTreeSet<String>,
    excluded: &'a BTreeSet<String>,
    seeded_negations: BTreeSet<String>,
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
        for i in order(&query.body, &bound) {
            let Some(lit) = query.body.get(i) else {
                continue;
            };
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
            // A negated literal whose arguments are *all* bound is a ground
            // membership test, and a demanded relation answers it exactly: its
            // seed is that one tuple, so it holds the tuple iff the full
            // relation does. This is the `!tighter_at(F, Line, S)` in
            // `innermost_at`, and without it the headline location query
            // materializes tighter_at over the whole repository — measured at
            // M2 as a timeout on 1,070 definitions.
            //
            // With anything free — `!calls(_, S)` — the relation really is read
            // whole, and restricting it would change the answer rather than the
            // cost.
            Literal::Neg(pred) if self.derived.contains(&pred.name) => {
                let adornment = adorn(&pred.args, bound);
                if self.excluded.contains(&pred.name) || !adornment.iter().all(|b| *b) {
                    self.request_plain(&pred.name);
                    return lit.clone();
                }
                self.seeded_negations.insert(pred.name.clone());
                self.out.push(Rule {
                    head: Pred {
                        name: magic_name(&pred.name, &adornment),
                        args: keep(&pred.args, &adornment),
                        span: pred.span,
                    },
                    body: preceding.to_vec(),
                    vars: vars.to_vec(),
                    span,
                });
                let name = adorned_name(&pred.name, &adornment);
                self.request(&pred.name, adornment);
                Literal::Neg(Pred {
                    name,
                    ..pred.clone()
                })
            }
            Literal::Assign {
                expr: Expr::Count { goal, .. },
                ..
            } => {
                self.request_inside(goal);
                lit.clone()
            }
            other => other.clone(),
        }
    }

    /// Request, unadorned, every derived predicate an aggregate's goal names,
    /// however deeply its `count{}`s nest.
    ///
    /// Walking one level left a predicate seen only inside a nested aggregate
    /// unrequested, so its rules were missing from the rewrite, `check`
    /// rejected the rewrite, and the plain program ran with `transformed`
    /// empty: rows right, cost unbounded, nothing reported.
    fn request_inside(&mut self, goal: &[Literal]) {
        for inner in goal {
            match inner {
                Literal::Pos(p) | Literal::Neg(p) if self.derived.contains(&p.name) => {
                    self.request_plain(&p.name);
                }
                Literal::Assign {
                    expr: Expr::Count { goal, .. },
                    ..
                } => self.request_inside(goal),
                _ => {}
            }
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
            for i in order(&rule.body, &bound) {
                let Some(lit) = rule.body.get(i) else {
                    continue;
                };
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

/// The order adornment walks a body: most-bound literal first, filters as
/// soon as their inputs exist, written order as the tie-break.
///
/// The written order is not it. `callers(C, S), def(S, _, _, "evaluate")`
/// walked as written adorns `callers` free-free — nothing is bound yet — so
/// no seed is made and the query computes every caller of everything, while
/// the reverse spelling seeds `callers@fb` and derives 36 tuples instead of
/// 198,025. The planner reorders most-bound-first at run time, but by then
/// the adornment is fixed. So the walk here mirrors the planner's choice,
/// without relation sizes: a literal sharing no bound variable goes after
/// every literal that shares one, then a constant or a bound variable in an
/// argument counts as bound, and the literal with the most bound arguments
/// goes first. Safety is still checked on the program as written; this is a cost
/// decision, and the rewrite is validated again afterwards.
fn order(body: &[Literal], initially: &BTreeSet<u16>) -> Vec<usize> {
    let mut bound = initially.clone();
    let mut taken = vec![false; body.len()];
    let mut out = Vec::with_capacity(body.len());
    // Safety rule 4, as the walk must respect it: an aggregate whose goal
    // binds a variable the rest of the body also mentions has to follow the
    // literal that binds it outside, or the rewrite fails `check` and is
    // dropped. `runnable` alone does not know this — it trusts the written
    // order the checker already approved.
    let mentioned: Vec<BTreeSet<u16>> = body.iter().map(crate::check::literal_vars).collect();
    let waits_on: Vec<BTreeSet<u16>> = body
        .iter()
        .enumerate()
        .map(|(i, lit)| {
            let Literal::Assign {
                expr: Expr::Count { goal, .. },
                ..
            } = lit
            else {
                return BTreeSet::new();
            };
            let mut inner = BTreeSet::new();
            crate::solve::goal_vars(goal, &mut inner);
            let elsewhere: BTreeSet<u16> = mentioned
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .flat_map(|(_, vars)| vars.iter().copied())
                .collect();
            inner.intersection(&elsewhere).copied().collect()
        })
        .collect();
    while out.len() < body.len() {
        let mut best: Option<((bool, usize), usize)> = None;
        for (i, lit) in body.iter().enumerate() {
            if taken.get(i).copied().unwrap_or(true)
                || !crate::solve::runnable(lit, &bound)
                || waits_on.get(i).is_some_and(|w| !w.is_subset(&bound))
            {
                continue;
            }
            // A filter is free and runs as soon as it can; a relation literal
            // ranks connected before disconnected, as the planner does, then
            // by how many of its arguments are already bound. An aggregate is
            // neither: it is a sub-evaluation, and everything that precedes a
            // literal here becomes the body of that literal's magic rule, so
            // an aggregate placed early puts an aggregate edge into every seed
            // it precedes — and into a recursive cycle, if the seeded
            // predicate is one. It ranks as an unbound relation literal:
            // after anything already narrowed, before nothing else.
            let rank = match lit {
                Literal::Pos(p) => (
                    !crate::solve::disconnected(&p.args, &bound),
                    p.args
                        .iter()
                        .filter(|t| match t {
                            Term::Const(_) => true,
                            Term::Var(v) => bound.contains(v),
                            Term::Wildcard => false,
                        })
                        .count(),
                ),
                Literal::Assign {
                    expr: Expr::Count { .. },
                    ..
                } => (bound.is_empty(), 0),
                _ => (true, usize::MAX),
            };
            if best.is_none_or(|(r, _)| rank > r) {
                best = Some((rank, i));
            }
        }
        let Some((_, pick)) = best else { break };
        if let Some(slot) = taken.get_mut(pick) {
            *slot = true;
        }
        if let Some(lit) = body.get(pick) {
            bind(lit, &mut bound);
        }
        out.push(pick);
    }
    // A safe body always drains; if it did not, keep the rest as written
    // rather than drop a literal.
    for (i, done) in taken.iter().enumerate() {
        if !done {
            out.push(i);
        }
    }
    out
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
