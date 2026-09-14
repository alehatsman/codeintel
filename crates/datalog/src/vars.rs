//! Which variables a literal binds, reads, or mentions — answered once.
//!
//! Three passes ask this question: safety (`check`), the run-time planner
//! (`solve`) and the adornment walk (`transform`). `specs/03-datalog.md`
//! § Magic sets requires the walk to mirror the planner, and a mirror kept
//! as two copies of the same `match` drifts the first time one variant is
//! added. So there is one copy, here, and the other three modules call it.

use std::collections::BTreeSet;

use crate::ast::{Expr, Literal};
use crate::atom::Term;

/// True when a term needs no further binding: a constant, or a variable in
/// `bound`. A wildcard is never bound — it names nothing to bind.
pub(crate) fn is_bound(t: Term, bound: &BTreeSet<u16>) -> bool {
    match t {
        Term::Const(_) => true,
        Term::Var(v) => bound.contains(&v),
        Term::Wildcard => false,
    }
}

/// The variables among a slice of terms: a rule head, or arguments.
pub(crate) fn term_vars(terms: &[Term]) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    for t in terms {
        add(*t, &mut out);
    }
    out
}

fn add(t: Term, into: &mut BTreeSet<u16>) {
    if let Term::Var(v) = t {
        into.insert(v);
    }
}

/// Every variable a literal binds once it has run.
///
/// A relation literal binds all of its arguments, `between` binds its output,
/// an assignment binds its target. A negation, a comparison and a string test
/// bind nothing: they only filter.
pub(crate) fn binds(lit: &Literal, into: &mut BTreeSet<u16>) {
    match lit {
        Literal::Pos(p) => p.args.iter().for_each(|t| add(*t, into)),
        Literal::Between { out, .. } => add(*out, into),
        Literal::Assign { target, .. } => add(*target, into),
        Literal::Neg(_) | Literal::Compare { .. } | Literal::Str { .. } => {}
    }
}

/// Every variable a literal reads but does not bind.
///
/// An aggregate's goal is *inside* the literal and contributes nothing here;
/// [`goal_vars`] is the walk into it.
pub(crate) fn reads(lit: &Literal, into: &mut BTreeSet<u16>) {
    match lit {
        Literal::Neg(p) => p.args.iter().for_each(|t| add(*t, into)),
        Literal::Compare { lhs, rhs, .. }
        | Literal::Assign {
            expr: Expr::Arith { lhs, rhs, .. },
            ..
        } => {
            add(*lhs, into);
            add(*rhs, into);
        }
        Literal::Str { subject, .. } => add(*subject, into),
        Literal::Between { lo, hi, .. } => {
            add(*lo, into);
            add(*hi, into);
        }
        Literal::Assign {
            expr: Expr::Term(t),
            ..
        } => add(*t, into),
        Literal::Assign {
            expr: Expr::Count { .. },
            ..
        }
        | Literal::Pos(_) => {}
    }
}

/// Every variable a literal mentions: what it binds and what it reads. For an
/// aggregate assignment that is the target alone — the counted term and the
/// goal are inside, which is the whole point of safety rule 4.
pub(crate) fn vars(lit: &Literal) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    binds(lit, &mut out);
    reads(lit, &mut out);
    out
}

/// Every variable an aggregate's goal mentions, at every depth.
pub(crate) fn goal_vars(goal: &[Literal], out: &mut BTreeSet<u16>) {
    for lit in goal {
        binds(lit, out);
        reads(lit, out);
        if let Literal::Assign {
            expr: Expr::Count { goal: inner, .. },
            ..
        } = lit
        {
            goal_vars(inner, out);
        }
    }
}
