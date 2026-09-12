//! The join: one rule body, evaluated against the store.
//!
//! Nested-loop over literals in a plan order chosen most-bound-first, with a
//! binary search into the sorted relation on whatever leading prefix is bound.
//! No optimizer beyond that, and none is warranted at this scale.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use crate::ast::{Arith, Cmp, Expr, Literal, Pred, StrTest};
use crate::atom::{Atom, INT_MAX, Term, is_int};
use crate::db::Db;
use crate::diag::{Diagnostic, Result, Status};
use crate::limits::Limits;
use crate::matcher::Matcher;
use crate::relation::Relation;
use crate::symbols::Symbols;

/// A variable binding environment, indexed by [`Term::Var`].
pub type Env = Vec<Option<Atom>>;

/// Two stores read as one: the host's base relations underneath, the query's
/// derived relations on top. Derived wins, which is what makes a query-local
/// rule able to shadow one from the standard library.
#[derive(Debug, Clone, Copy)]
pub struct Layers<'a> {
    /// Base relations, supplied by the host and never written by a query.
    pub base: &'a Db,
    /// Derived relations, rebuilt per query.
    pub derived: &'a Db,
}

impl Layers<'_> {
    /// The relation under this name, derived first.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Relation> {
        self.derived.get(name).or_else(|| self.base.get(name))
    }
}

/// Everything one body evaluation reads.
pub struct Solver<'a> {
    /// Relations at their current extent.
    pub db: Layers<'a>,
    /// Per-relation deltas for semi-naive evaluation.
    pub delta: &'a BTreeMap<String, Relation>,
    /// The host dictionary, for the string builtins.
    pub syms: &'a dyn Symbols,
    /// Patterns compiled once per query, keyed by their source text.
    pub patterns: &'a BTreeMap<String, Box<dyn Matcher>>,
    /// The caps in force.
    pub limits: &'a Limits,
    /// When the query started, for `max_time_ms`.
    pub start: Instant,
    /// Tuples derived so far, for `max_derived_tuples`.
    pub derived: &'a Cell<u64>,
    /// The deepest position in the literal order any solution path reached.
    ///
    /// Only the goal's evaluation looks at this. When the goal derives nothing,
    /// the literal at this position is the one that matched nothing — which is
    /// the difference between "not true of your code" and "your query has a
    /// typo" ([`crate::Stats::empty_at`]).
    pub deepest: &'a Cell<usize>,
}

impl core::fmt::Debug for Solver<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Solver")
            .field("derived", &self.derived.get())
            .finish_non_exhaustive()
    }
}

impl Solver<'_> {
    /// Evaluate `body`, emitting one tuple of `head` bindings per solution.
    ///
    /// `delta_at` names the body position that must read its delta rather than
    /// the full relation — that is what makes the evaluation semi-naive.
    ///
    /// # Errors
    /// `budget-exceeded` or `timeout` when a cap fires; `invalid-query` for a
    /// runtime type error the safety rules cannot catch, such as comparing a
    /// string atom with `<`.
    pub fn run(
        &self,
        body: &[Literal],
        head: &[Term],
        vars: usize,
        delta_at: Option<usize>,
        out: &mut Relation,
    ) -> Result<()> {
        let order = self.plan(body, delta_at);
        let mut env: Env = vec![None; vars];
        self.step(body, &order, 0, &mut env, delta_at, head, out)
    }

    /// The literal order: repeatedly take the cheapest literal whose inputs are
    /// already bound. A pure function of the rule and the current relation
    /// sizes, both of which are deterministic, so the plan is deterministic.
    pub fn plan(&self, body: &[Literal], delta_at: Option<usize>) -> Vec<usize> {
        let needs = required_bindings(body);
        let mut bound: BTreeSet<u16> = BTreeSet::new();
        let mut taken = vec![false; body.len()];
        let mut order = Vec::with_capacity(body.len());

        // A delta literal must run first: it is the only one whose extent is
        // the new tuples, and running it late would scan the full relation.
        if let Some(at) = delta_at
            && let Some(lit) = body.get(at)
        {
            if let Some(slot) = taken.get_mut(at) {
                *slot = true;
            }
            bind_all(lit, &mut bound);
            order.push(at);
        }

        while order.len() < body.len() {
            let mut best: Option<(u64, usize)> = None;
            for (i, lit) in body.iter().enumerate() {
                let waiting = needs.get(i).is_some_and(|n| !n.is_subset(&bound));
                if taken.get(i).copied().unwrap_or(true) || waiting || !runnable(lit, &bound) {
                    continue;
                }
                let cost = self.cost(lit, &bound);
                if best.is_none_or(|(c, _)| cost < c) {
                    best = Some((cost, i));
                }
            }
            let Some((_, pick)) = best else { break };
            if let Some(slot) = taken.get_mut(pick) {
                *slot = true;
            }
            if let Some(lit) = body.get(pick) {
                bind_all(lit, &mut bound);
            }
            order.push(pick);
        }
        // A body that is safe always drains; if it did not, keep the rest in
        // written order rather than silently dropping literals.
        for (i, done) in taken.iter().enumerate() {
            if !done {
                order.push(i);
            }
        }
        order
    }

    /// `|R| / 2^k` for a relation literal, zero for a filter — filters are free
    /// and run as soon as their inputs exist.
    fn cost(&self, lit: &Literal, bound: &BTreeSet<u16>) -> u64 {
        let Literal::Pos(pred) = lit else { return 0 };
        let rows = self.db.get(&pred.name).map_or(0, Relation::len) as u64;
        let k = pred
            .args
            .iter()
            .filter(|t| match t {
                Term::Const(_) => true,
                Term::Var(v) => bound.contains(v),
                Term::Wildcard => false,
            })
            .count()
            .min(31);
        rows >> k
    }

    fn budget(&self) -> Result<()> {
        if self.derived.get() > self.limits.max_derived_tuples {
            return Err(Diagnostic::whole(
                Status::BudgetExceeded,
                format!(
                    "derived more than {} tuples (max_derived_tuples). Bind an argument of the \
                     traversal — a seeded form such as `impact_of(Seed, C)` explores the reachable \
                     subgraph instead of every pair",
                    self.limits.max_derived_tuples
                ),
            ));
        }
        // Microseconds and `>=`, so a budget of zero means zero rather than
        // "one free millisecond".
        let elapsed = u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
        if elapsed >= self.limits.max_time_ms.saturating_mul(1_000) {
            return Err(Diagnostic::whole(
                Status::Timeout,
                format!(
                    "exceeded max_time_ms ({} ms). Add a literal that binds an argument earlier in \
                     the body, or raise the limit",
                    self.limits.max_time_ms
                ),
            ));
        }
        Ok(())
    }

    fn step(
        &self,
        body: &[Literal],
        order: &[usize],
        k: usize,
        env: &mut Env,
        delta_at: Option<usize>,
        head: &[Term],
        out: &mut Relation,
    ) -> Result<()> {
        self.budget()?;
        if k > self.deepest.get() {
            self.deepest.set(k);
        }
        let Some(&index) = order.get(k) else {
            return self.emit(head, env, out);
        };
        let Some(lit) = body.get(index) else {
            return Ok(());
        };
        let next = |env: &mut Env, out: &mut Relation| {
            self.step(body, order, k + 1, env, delta_at, head, out)
        };

        match lit {
            Literal::Pos(pred) => {
                let from_delta = delta_at == Some(index);
                self.join(pred, from_delta, env, out, &next)
            }
            Literal::Neg(pred) => {
                if self.exists(pred, env) {
                    Ok(())
                } else {
                    next(env, out)
                }
            }
            Literal::Compare { op, lhs, rhs, span } => {
                if self.compare(*op, *lhs, *rhs, env, *span)? {
                    next(env, out)
                } else {
                    Ok(())
                }
            }
            Literal::Str {
                test,
                subject,
                pattern,
                span,
            } => {
                if self.str_test(*test, *subject, pattern, env, *span)? {
                    next(env, out)
                } else {
                    Ok(())
                }
            }
            Literal::Between {
                lo,
                hi,
                out: target,
                span,
            } => self.between(*lo, *hi, *target, *span, env, out, &next),
            Literal::Assign { target, expr, span } => {
                self.assign(*target, expr, *span, env, out, &next)
            }
        }
    }

    fn emit(&self, head: &[Term], env: &Env, out: &mut Relation) -> Result<()> {
        let mut row = Vec::with_capacity(head.len());
        for term in head {
            match term {
                Term::Const(a) => row.push(*a),
                Term::Var(v) => match env.get(usize::from(*v)).copied().flatten() {
                    Some(a) => row.push(a),
                    // Range restriction guarantees this cannot happen; emitting
                    // nothing is the safe reading if it somehow does.
                    None => return Ok(()),
                },
                // `check` rejects `_` in a head; emitting nothing is the safe
                // reading if one somehow reaches here.
                Term::Wildcard => return Ok(()),
            }
        }
        out.push(&row);
        self.derived.set(self.derived.get().saturating_add(1));
        self.budget()
    }

    // ── relation literals ───────────────────────────────────────────────────

    fn relation(&self, name: &str, from_delta: bool) -> Option<&Relation> {
        if from_delta {
            self.delta.get(name)
        } else {
            self.db.get(name)
        }
    }

    fn join(
        &self,
        pred: &Pred,
        from_delta: bool,
        env: &mut Env,
        out: &mut Relation,
        next: &dyn Fn(&mut Env, &mut Relation) -> Result<()>,
    ) -> Result<()> {
        let Some(rel) = self.relation(&pred.name, from_delta) else {
            return Ok(());
        };
        if rel.arity() != pred.args.len() {
            return Ok(());
        }
        let pattern = pattern_of(&pred.args, env);
        let prefix_len = pattern.iter().take_while(|p| p.is_some()).count();
        let prefix: Vec<Atom> = pattern.iter().take(prefix_len).flatten().copied().collect();
        let range = rel.prefix(&prefix);

        let mut trail: Vec<u16> = Vec::new();
        for i in range {
            self.budget()?;
            let Some(row) = rel.row(i) else { break };
            trail.clear();
            if unify(&pred.args, row, env, &mut trail) {
                next(env, out)?;
            }
            for v in trail.drain(..) {
                if let Some(slot) = env.get_mut(usize::from(v)) {
                    *slot = None;
                }
            }
        }
        Ok(())
    }

    /// Existence check for a negated literal. Unbound positions are wildcards:
    /// `!calls(_, S)` asks whether *any* tuple has `S` in the second column.
    fn exists(&self, pred: &Pred, env: &Env) -> bool {
        let Some(rel) = self.db.get(&pred.name) else {
            return false;
        };
        if rel.arity() != pred.args.len() {
            return false;
        }
        let pattern = pattern_of(&pred.args, env);
        let prefix_len = pattern.iter().take_while(|p| p.is_some()).count();
        let prefix: Vec<Atom> = pattern.iter().take(prefix_len).flatten().copied().collect();
        rel.prefix(&prefix).any(|i| {
            rel.row(i).is_some_and(|row| {
                pattern
                    .iter()
                    .zip(row)
                    .all(|(want, got)| want.is_none_or(|a| a == *got))
            })
        })
    }

    // ── builtins ────────────────────────────────────────────────────────────

    fn compare(
        &self,
        op: Cmp,
        lhs: Term,
        rhs: Term,
        env: &Env,
        span: (usize, usize),
    ) -> Result<bool> {
        let (Some(a), Some(b)) = (value(lhs, env), value(rhs, env)) else {
            return Ok(false);
        };
        if op.is_identity() {
            return Ok(if op == Cmp::Eq { a == b } else { a != b });
        }
        if !is_int(a) || !is_int(b) {
            let which = if is_int(a) { b } else { a };
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                span,
                format!(
                    "`{}` compares integers, but {} is a string. String atom ids are allocation-\
                     ordered, so ordering them would give answers that change between runs — use \
                     `=`, `!=`, `prefix` or `match` instead",
                    op.symbol(),
                    crate::atom::show(which, |x| self.syms.resolve(x).map(ToString::to_string))
                ),
            ));
        }
        Ok(match op {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
            Cmp::Eq | Cmp::Ne => unreachable_identity(),
        })
    }

    fn str_test(
        &self,
        test: StrTest,
        subject: Term,
        pattern: &str,
        env: &Env,
        span: (usize, usize),
    ) -> Result<bool> {
        let Some(atom) = value(subject, env) else {
            return Ok(false);
        };
        // An integer atom has no string behind it, so no string test matches.
        let Some(text) = self.syms.resolve(atom) else {
            return Ok(false);
        };
        Ok(match test {
            StrTest::Prefix => text.starts_with(pattern),
            StrTest::Suffix => text.ends_with(pattern),
            StrTest::Contains => text.contains(pattern),
            StrTest::Match => {
                let Some(compiled) = self.patterns.get(pattern) else {
                    return Err(Diagnostic::at(
                        Status::InvalidQuery,
                        span,
                        "the `match` builtin needs a regex engine and this host installed none; \
                         use `prefix`, `suffix` or `contains`",
                    ));
                };
                compiled.is_match(text)
            }
        })
    }

    fn between(
        &self,
        lo: Term,
        hi: Term,
        target: Term,
        span: (usize, usize),
        env: &mut Env,
        out: &mut Relation,
        next: &dyn Fn(&mut Env, &mut Relation) -> Result<()>,
    ) -> Result<()> {
        let (Some(lo), Some(hi)) = (value(lo, env), value(hi, env)) else {
            return Ok(());
        };
        if !is_int(lo) || !is_int(hi) {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                span,
                "`between`'s bounds must be integers",
            ));
        }
        // Already bound: degrade to a range check, which is what makes the same
        // rule fast once demand transformation binds the output.
        if let Some(bound) = value(target, env) {
            return if is_int(bound) && lo <= bound && bound <= hi {
                next(env, out)
            } else {
                Ok(())
            };
        }
        // Unbound and not a variable means `_`, which `check` rejects.
        let Term::Var(v) = target else { return Ok(()) };
        for value in lo..=hi {
            self.budget()?;
            if let Some(slot) = env.get_mut(usize::from(v)) {
                *slot = Some(value);
            }
            next(env, out)?;
        }
        if let Some(slot) = env.get_mut(usize::from(v)) {
            *slot = None;
        }
        Ok(())
    }

    fn assign(
        &self,
        target: Term,
        expr: &Expr,
        span: (usize, usize),
        env: &mut Env,
        out: &mut Relation,
        next: &dyn Fn(&mut Env, &mut Relation) -> Result<()>,
    ) -> Result<()> {
        let Some(computed) = self.eval_expr(expr, env, span)? else {
            return Ok(());
        };
        if let Some(bound) = value(target, env) {
            return if bound == computed {
                next(env, out)
            } else {
                Ok(())
            };
        }
        let Term::Var(v) = target else { return Ok(()) };
        if let Some(slot) = env.get_mut(usize::from(v)) {
            *slot = Some(computed);
        }
        let result = next(env, out);
        if let Some(slot) = env.get_mut(usize::from(v)) {
            *slot = None;
        }
        result
    }

    fn eval_expr(&self, expr: &Expr, env: &Env, span: (usize, usize)) -> Result<Option<Atom>> {
        match expr {
            Expr::Term(t) => Ok(value(*t, env)),
            Expr::Arith { op, lhs, rhs } => {
                let (Some(a), Some(b)) = (value(*lhs, env), value(*rhs, env)) else {
                    return Ok(None);
                };
                Self::arith(*op, a, b, span).map(Some)
            }
            Expr::Count { over, goal } => self.count(*over, goal, env, span).map(Some),
        }
    }

    fn arith(op: Arith, a: Atom, b: Atom, span: (usize, usize)) -> Result<Atom> {
        let bad = |msg: String| Diagnostic::at(Status::InvalidQuery, span, msg);
        if !is_int(a) || !is_int(b) {
            return Err(bad(format!(
                "`{}` is integer arithmetic, but one operand is a string",
                op.symbol()
            )));
        }
        let (x, y) = (i64::from(a), i64::from(b));
        let value = match op {
            Arith::Add => x + y,
            Arith::Sub => x - y,
            Arith::Mul => x * y,
            Arith::Div => {
                if y == 0 {
                    return Err(bad("division by zero".to_string()));
                }
                x / y
            }
        };
        crate::atom::int_atom(value).ok_or_else(|| {
            bad(format!(
                "`{x} {} {y}` is {value}, outside the supported range 0..={INT_MAX}",
                op.symbol()
            ))
        })
    }

    /// `count{ T : goal }` — distinct bindings of `T` satisfying `goal`, under
    /// the bindings already in `env`.
    fn count(&self, over: Term, goal: &[Literal], env: &Env, span: (usize, usize)) -> Result<Atom> {
        let mut scratch = Relation::new(1);
        let mut inner = env.clone();
        let order = self.plan(goal, None);
        // The sub-goal has its own literal positions, and they are meaningless
        // against the outer body's plan. Without this the aggregate's depth
        // leaks into `deepest` and `empty_at` names the wrong literal — or an
        // out-of-range one, which degrades to the generic message.
        let outer = self.deepest.get();
        let counted = self.step(goal, &order, 0, &mut inner, None, &[over], &mut scratch);
        self.deepest.set(outer);
        counted?;
        scratch.settle();
        let n = i64::try_from(scratch.len()).unwrap_or(i64::MAX);
        crate::atom::int_atom(n).ok_or_else(|| {
            Diagnostic::at(
                Status::InvalidQuery,
                span,
                format!("`count` produced {n}, outside the supported range 0..={INT_MAX}"),
            )
        })
    }
}

/// Split out so the `Cmp::Eq | Cmp::Ne` arm above has no `unreachable!`.
const fn unreachable_identity() -> bool {
    false
}

/// The value of a term under `env`, or `None` when it is unbound.
fn value(term: Term, env: &Env) -> Option<Atom> {
    match term {
        Term::Const(a) => Some(a),
        Term::Var(v) => env.get(usize::from(v)).copied().flatten(),
        Term::Wildcard => None,
    }
}

fn pattern_of(args: &[Term], env: &Env) -> Vec<Option<Atom>> {
    args.iter()
        .map(|t| match t {
            Term::Const(a) => Some(*a),
            Term::Var(v) => env.get(usize::from(*v)).copied().flatten(),
            Term::Wildcard => None,
        })
        .collect()
}

/// Bind `args` against `row`, recording what was bound so the caller can undo
/// it. A repeated variable within one literal must agree with itself.
fn unify(args: &[Term], row: &[Atom], env: &mut Env, trail: &mut Vec<u16>) -> bool {
    for (term, value) in args.iter().zip(row) {
        match term {
            Term::Wildcard => {}
            Term::Const(a) => {
                if a != value {
                    return false;
                }
            }
            Term::Var(v) => match env.get(usize::from(*v)).copied().flatten() {
                Some(bound) if bound != *value => return false,
                Some(_) => {}
                None => {
                    if let Some(slot) = env.get_mut(usize::from(*v)) {
                        *slot = Some(*value);
                        trail.push(*v);
                    }
                }
            },
        }
    }
    true
}

/// Variables an aggregate must have bound before it may run: the ones its goal
/// mentions that the rest of the body also mentions.
///
/// Without this the planner is free to hoist `N = count{ C : calls(C, S) }`
/// above the literal that binds `S`, and the aggregate then counts over every
/// `S` at once. The answer is wrong, not slow, and it is wrong silently — which
/// is why the check lives in the planner and not in a comment.
fn required_bindings(body: &[Literal]) -> Vec<BTreeSet<u16>> {
    let mut out = vec![BTreeSet::new(); body.len()];

    // An adorned relation holds only the tuples its seeds demanded — `p@bf` is
    // `p` intersected with the seeded bindings — so reading one with its bound
    // positions unbound scans every seeded tuple and filters afterwards. Same
    // answer, computed wide: this wait is a cost constraint, unlike the
    // aggregate one above, and it is kept because that width is exactly what
    // the demand transformation exists to remove. See `transform::adornment_of`.
    for (i, lit) in body.iter().enumerate() {
        let Literal::Pos(pred) = lit else { continue };
        let Some(adornment) = crate::transform::adornment_of(&pred.name) else {
            continue;
        };
        let required: BTreeSet<u16> = pred
            .args
            .iter()
            .zip(adornment.chars())
            .filter(|(_, mark)| *mark == 'b')
            .filter_map(|(t, _)| if let Term::Var(v) = t { Some(*v) } else { None })
            .collect();
        if let Some(slot) = out.get_mut(i) {
            *slot = required;
        }
    }

    for (i, lit) in body.iter().enumerate() {
        let Literal::Assign {
            expr: Expr::Count { goal, .. },
            ..
        } = lit
        else {
            continue;
        };
        let mut inside = BTreeSet::new();
        goal_vars(goal, &mut inside);
        let mut outside = BTreeSet::new();
        for (j, other) in body.iter().enumerate() {
            if j != i {
                bind_all(other, &mut outside);
                filter_vars(other, &mut outside);
            }
        }
        if let Some(slot) = out.get_mut(i) {
            slot.extend(inside.intersection(&outside).copied());
        }
    }
    out
}

/// Every variable an aggregate's goal mentions.
fn goal_vars(goal: &[Literal], out: &mut BTreeSet<u16>) {
    for lit in goal {
        bind_all(lit, out);
        filter_vars(lit, out);
        if let Literal::Assign {
            expr: Expr::Count { goal: inner, .. },
            ..
        } = lit
        {
            goal_vars(inner, out);
        }
    }
}

/// Variables a literal reads but does not bind.
fn filter_vars(lit: &Literal, out: &mut BTreeSet<u16>) {
    let mut add = |t: &Term| {
        if let Term::Var(v) = t {
            out.insert(*v);
        }
    };
    match lit {
        Literal::Neg(p) => p.args.iter().for_each(add),
        Literal::Compare { lhs, rhs, .. } => {
            add(lhs);
            add(rhs);
        }
        Literal::Str { subject, .. } => add(subject),
        Literal::Between { lo, hi, .. } => {
            add(lo);
            add(hi);
        }
        Literal::Assign { expr, .. } => match expr {
            Expr::Term(t) => add(t),
            Expr::Arith { lhs, rhs, .. } => {
                add(lhs);
                add(rhs);
            }
            Expr::Count { .. } => {}
        },
        Literal::Pos(_) => {}
    }
}

/// True when every input a literal needs is already bound.
fn runnable(lit: &Literal, bound: &BTreeSet<u16>) -> bool {
    let has = |t: &Term| match t {
        Term::Const(_) => true,
        Term::Var(v) => bound.contains(v),
        Term::Wildcard => false,
    };
    match lit {
        Literal::Pos(_) => true,
        Literal::Neg(p) => p.args.iter().all(|t| matches!(t, Term::Wildcard) || has(t)),
        Literal::Compare { lhs, rhs, .. } => has(lhs) && has(rhs),
        Literal::Str { subject, .. } => has(subject),
        Literal::Between { lo, hi, .. } => has(lo) && has(hi),
        Literal::Assign { expr, .. } => match expr {
            Expr::Term(t) => has(t),
            Expr::Arith { lhs, rhs, .. } => has(lhs) && has(rhs),
            // An aggregate's own goal binds what it needs; its outer variables
            // were checked by safety rule 4.
            Expr::Count { .. } => true,
        },
    }
}

/// Every variable a literal binds once it has run.
fn bind_all(lit: &Literal, bound: &mut BTreeSet<u16>) {
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
