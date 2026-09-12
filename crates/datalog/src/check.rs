//! The safety rules of `specs/03-datalog.md`, checked before evaluation.
//!
//! Checked on the program **as written**, before any demand transformation. A
//! rule that is safe only after the transformation would be un-evaluable by the
//! naive evaluator — and the naive evaluator is what the transformation is
//! differentially tested against. Legality and performance stay separate.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Cmp, Expr, Literal, Pred, Program, Query, Rule};
use crate::atom::Term;
use crate::diag::{Diagnostic, Result, Status};

/// What the engine knows about every relation a program mentions.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    /// Arity per relation, fixed by its first use.
    pub arity: BTreeMap<String, usize>,
    /// Relations supplied by the host.
    pub base: BTreeSet<String>,
    /// Relations defined by rules.
    pub derived: BTreeSet<String>,
    /// Where each relation was first seen, for the arity diagnostic.
    first_use: BTreeMap<String, (usize, usize)>,
}

impl Schema {
    /// A schema seeded with the host's base relations.
    #[must_use]
    pub fn with_base(base: impl IntoIterator<Item = (String, usize)>) -> Self {
        let mut out = Self::default();
        for (name, arity) in base {
            out.arity.insert(name.clone(), arity);
            out.base.insert(name);
        }
        out
    }

    /// True when `name` is defined by rules rather than supplied by the host.
    #[must_use]
    pub fn is_derived(&self, name: &str) -> bool {
        self.derived.contains(name)
    }

    fn see(&mut self, pred: &Pred) -> Result<()> {
        let arity = pred.args.len();
        match self.arity.get(&pred.name) {
            Some(known) if *known != arity => {
                let first = self.first_use.get(&pred.name).copied();
                let where_first = first.map_or_else(String::new, |(from, _)| {
                    format!(" (first used at byte {from})")
                });
                Err(Diagnostic::at(
                    Status::InvalidQuery,
                    pred.span,
                    format!(
                        "relation `{}` is used with {arity} arguments here but {known}{where_first}; \
                         a relation's arity is fixed by its first use (safety rule 6)",
                        pred.name
                    ),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.arity.insert(pred.name.clone(), arity);
                self.first_use.insert(pred.name.clone(), pred.span);
                Ok(())
            }
        }
    }
}

/// Check a program against the host's base relations.
///
/// Returns the schema the planner and evaluator work against.
///
/// # Errors
/// Returns `invalid-query` naming the rule, the variable and the safety rule
/// that was violated.
pub fn check(program: &Program, base: impl IntoIterator<Item = (String, usize)>) -> Result<Schema> {
    let mut schema = Schema::with_base(base);

    // Heads first: a rule may reference a relation defined further down.
    for rule in &program.rules {
        if schema.base.contains(&rule.head.name) {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                rule.head.span,
                format!(
                    "`{}` is a base relation supplied by the fact store and cannot also be \
                     defined by a rule (safety rule 8); pick a different head name",
                    rule.head.name
                ),
            ));
        }
        schema.see(&rule.head)?;
        schema.derived.insert(rule.head.name.clone());
    }

    for rule in &program.rules {
        check_rule(rule, &mut schema)?;
    }
    if let Some(query) = &program.query {
        check_query(query, &mut schema)?;
    }

    // Last, so that a more specific complaint — an arity that disagrees with
    // itself, an unbound head variable — is reported instead of this one.
    // Every predicate a body mentions must exist: a base relation, or a head
    // defined somewhere in this program. Otherwise a typo is indistinguishable
    // from a true statement about the code — `?- nosuchrelation(X).` returned
    // `ok` with zero rows, which is the exact failure invariant 6 is about.
    // Heads are all registered first, so a forward reference is still legal.
    for rule in &program.rules {
        known_predicates(&rule.body, &schema)?;
    }
    if let Some(query) = &program.query {
        known_predicates(&query.body, &schema)?;
    }
    Ok(schema)
}

/// Reject a literal naming a predicate nothing defines.
///
/// Walks aggregate sub-goals too: `count{ X : nosuchrelation(X) }` is the same
/// typo one level down.
fn known_predicates(body: &[Literal], schema: &Schema) -> Result<()> {
    for literal in body {
        match literal {
            Literal::Pos(pred) | Literal::Neg(pred) => {
                if !schema.base.contains(&pred.name) && !schema.derived.contains(&pred.name) {
                    return Err(Diagnostic::at(
                        Status::InvalidQuery,
                        pred.span,
                        format!(
                            "no relation or rule named `{}`; it is neither a base relation nor \
                             a rule head in this program. check the spelling against \
                             `codeintel schema`",
                            pred.name
                        ),
                    ));
                }
            }
            Literal::Assign {
                expr: Expr::Count { goal, .. },
                ..
            } => known_predicates(goal, schema)?,
            _ => {}
        }
    }
    Ok(())
}

fn check_rule(rule: &Rule, schema: &mut Schema) -> Result<()> {
    let name = format!("{}/{}", rule.head.name, rule.head.args.len());
    let ctx = Ctx {
        rule: &name,
        vars: &rule.vars,
        head: term_vars(&rule.head.args),
    };
    let bound = check_body(&rule.body, schema, &ctx, &BTreeSet::new())?;

    for arg in &rule.head.args {
        let v = match arg {
            Term::Var(v) => v,
            // A head column with no value to emit derives nothing at all, and
            // does it silently — invariant 5's failure exactly.
            Term::Wildcard => {
                return Err(Diagnostic::at(
                    Status::InvalidQuery,
                    rule.head.span,
                    format!(
                        "rule `{name}`: `_` in a rule head has no value to emit, so the rule \
                         derives no rows at all. Name the column with a variable bound in the \
                         body, or drop it from the head"
                    ),
                ));
            }
            Term::Const(_) => continue,
        };
        if bound.contains(v) {
            continue;
        }
        let var = var_name(&rule.vars, *v);
        return Err(Diagnostic::at(
            Status::InvalidQuery,
            rule.head.span,
            format!(
                "unsafe rule `{name}`: head variable `{var}` is bound by nothing in the body \
                 (safety rule 1, range restriction). Add a positive literal, an assignment or a \
                 generator that binds `{var}`, or write `_` if the column is irrelevant"
            ),
        ));
    }
    Ok(())
}

fn check_query(query: &Query, schema: &mut Schema) -> Result<()> {
    let ctx = Ctx {
        rule: "?-",
        vars: &query.vars,
        head: BTreeSet::new(),
    };
    check_body(&query.body, schema, &ctx, &BTreeSet::new())?;
    Ok(())
}

/// What a body needs to know about the rule it sits in.
struct Ctx<'a> {
    rule: &'a str,
    vars: &'a [String],
    /// Variables the head mentions — they count as "used outside" an aggregate.
    head: BTreeSet<u16>,
}

fn term_vars(terms: &[Term]) -> BTreeSet<u16> {
    terms
        .iter()
        .filter_map(|t| if let Term::Var(v) = t { Some(*v) } else { None })
        .collect()
}

/// Every variable a literal mentions. For an aggregate assignment, only the
/// target — the counted term and the goal are *inside*, which is the whole
/// point of safety rule 4.
fn literal_vars(lit: &Literal) -> BTreeSet<u16> {
    match lit {
        Literal::Pos(p) | Literal::Neg(p) => term_vars(&p.args),
        Literal::Compare { lhs, rhs, .. } => term_vars(&[*lhs, *rhs]),
        Literal::Str { subject, .. } => term_vars(&[*subject]),
        Literal::Between { lo, hi, out, .. } => term_vars(&[*lo, *hi, *out]),
        Literal::Assign { target, expr, .. } => {
            let mut out = term_vars(&[*target]);
            if let Expr::Term(t) = expr {
                out.extend(term_vars(&[*t]));
            } else if let Expr::Arith { lhs, rhs, .. } = expr {
                out.extend(term_vars(&[*lhs, *rhs]));
            }
            out
        }
    }
}

/// Walk a body in written order, returning the variables it binds.
///
/// `outer` holds variables already bound by an enclosing body — the aggregate
/// case. It is empty for a rule or query body.
fn check_body(
    body: &[Literal],
    schema: &mut Schema,
    ctx: &Ctx<'_>,
    outer: &BTreeSet<u16>,
) -> Result<BTreeSet<u16>> {
    let mut bound = outer.clone();
    for (i, lit) in body.iter().enumerate() {
        let elsewhere = used_elsewhere(ctx, body, i);
        step(lit, schema, ctx, &elsewhere, &mut bound)?;
    }
    Ok(bound)
}

/// Variables used outside body literal `i`: the head, plus every other literal.
fn used_elsewhere(ctx: &Ctx<'_>, body: &[Literal], i: usize) -> BTreeSet<u16> {
    let mut out = ctx.head.clone();
    for (j, lit) in body.iter().enumerate() {
        if j != i {
            out.extend(literal_vars(lit));
        }
    }
    out.extend(literal_vars(body.get(i).unwrap_or(&Literal::Pos(Pred {
        name: String::new(),
        args: Vec::new(),
        span: (0, 0),
    }))));
    out
}

fn step(
    lit: &Literal,
    schema: &mut Schema,
    ctx: &Ctx<'_>,
    elsewhere: &BTreeSet<u16>,
    bound: &mut BTreeSet<u16>,
) -> Result<()> {
    let (rule, vars) = (ctx.rule, ctx.vars);
    match lit {
        Literal::Pos(pred) => {
            schema.see(pred)?;
            for arg in &pred.args {
                if let Term::Var(v) = arg {
                    bound.insert(*v);
                }
            }
        }
        Literal::Neg(pred) => {
            schema.see(pred)?;
            for arg in &pred.args {
                let Term::Var(v) = arg else { continue };
                if !bound.contains(v) {
                    return Err(unbound(
                        pred.span,
                        rule,
                        &var_name(vars, *v),
                        2,
                        &format!("inside the negated literal `!{}(..)`", pred.name),
                        "negation can only filter bindings that already exist",
                    ));
                }
            }
        }
        Literal::Compare { op, lhs, rhs, span } => {
            for side in [*lhs, *rhs] {
                require_bound(
                    side,
                    bound,
                    vars,
                    *span,
                    rule,
                    3,
                    &format!("as an operand of `{}`", op.symbol()),
                )?;
            }
        }
        Literal::Str {
            test,
            subject,
            span,
            ..
        } => {
            require_bound(
                *subject,
                bound,
                vars,
                *span,
                rule,
                3,
                &format!("as the subject of `{}`", test.name()),
            )?;
        }
        Literal::Between { lo, hi, out, span } => {
            for side in [*lo, *hi] {
                require_bound(side, bound, vars, *span, rule, 7, "as a bound of `between`")?;
            }
            match out {
                Term::Var(v) => {
                    bound.insert(*v);
                }
                // The grammar (03-datalog.md § Grammar) says the output is a
                // var. A `_` there generates into nowhere: every row fails.
                Term::Wildcard => {
                    return Err(Diagnostic::at(
                        Status::InvalidQuery,
                        *span,
                        "`between`'s third argument must be a variable; `between(Lo, Hi, _)` \
                         binds nothing and yields no rows. Name the output, or write a pair of \
                         comparisons if you meant a range check",
                    ));
                }
                Term::Const(_) => {}
            }
        }
        Literal::Assign { target, expr, span } => {
            check_expr(expr, schema, ctx, elsewhere, bound, *span)?;
            if let Term::Var(v) = target {
                bound.insert(*v);
            }
        }
    }
    Ok(())
}

fn check_expr(
    expr: &Expr,
    schema: &mut Schema,
    ctx: &Ctx<'_>,
    elsewhere: &BTreeSet<u16>,
    bound: &BTreeSet<u16>,
    span: (usize, usize),
) -> Result<()> {
    let (rule, vars) = (ctx.rule, ctx.vars);
    match expr {
        Expr::Term(t) => require_bound(*t, bound, vars, span, rule, 7, "on the right of `=`"),
        Expr::Arith { op, lhs, rhs } => {
            for side in [*lhs, *rhs] {
                require_bound(
                    side,
                    bound,
                    vars,
                    span,
                    rule,
                    7,
                    &format!("as an operand of `{}`", op.symbol()),
                )?;
            }
            Ok(())
        }
        Expr::Count { over, goal } => check_count(*over, goal, schema, ctx, elsewhere, bound, span),
    }
}

/// Safety rule 4. A variable free in the aggregate's goal but used outside it
/// must be bound outside it, or the aggregate would silently count over a
/// different grouping than the reader expects.
fn check_count(
    over: Term,
    goal: &[Literal],
    schema: &mut Schema,
    ctx: &Ctx<'_>,
    elsewhere: &BTreeSet<u16>,
    bound: &BTreeSet<u16>,
    span: (usize, usize),
) -> Result<()> {
    let inner = check_body(goal, schema, ctx, bound)?;
    require_bound(
        over,
        &inner,
        ctx.vars,
        span,
        ctx.rule,
        4,
        "as `count`'s counted term",
    )?;

    // A variable the goal binds that is *also* used outside the aggregate must
    // be bound outside it too. Otherwise the grouping the reader sees and the
    // grouping the engine computes are different, silently.
    for v in inner.difference(bound) {
        if !elsewhere.contains(v) {
            continue;
        }
        let var = var_name(ctx.vars, *v);
        return Err(Diagnostic::at(
            Status::InvalidQuery,
            span,
            format!(
                "unsafe rule `{}`: `{var}` is used both inside `count{{..}}` and outside it, but \
                 nothing before the aggregate binds it (safety rule 4). Move a literal that binds \
                 `{var}` before the aggregate, so the grouping is the one you wrote",
                ctx.rule
            ),
        ));
    }
    Ok(())
}

fn require_bound(
    term: Term,
    bound: &BTreeSet<u16>,
    vars: &[String],
    span: (usize, usize),
    rule: &str,
    which: u8,
    position: &str,
) -> Result<()> {
    match term {
        Term::Const(_) => Ok(()),
        Term::Wildcard => Err(unbound(
            span,
            rule,
            "_",
            which,
            position,
            "`_` binds nothing, so it can never be compared or computed with",
        )),
        Term::Var(v) if bound.contains(&v) => Ok(()),
        Term::Var(v) => Err(unbound(
            span,
            rule,
            &var_name(vars, v),
            which,
            position,
            "move a positive literal that binds it earlier in the body",
        )),
    }
}

fn unbound(
    span: (usize, usize),
    rule: &str,
    var: &str,
    which: u8,
    position: &str,
    fix: &str,
) -> Diagnostic {
    Diagnostic::at(
        Status::InvalidQuery,
        span,
        format!(
            "unsafe rule `{rule}`: `{var}` is used {position} but nothing earlier in the body \
             binds it (safety rule {which}); {fix}"
        ),
    )
}

fn var_name(vars: &[String], v: u16) -> String {
    vars.get(usize::from(v))
        .cloned()
        .unwrap_or_else(|| format!("?{v}"))
}

/// True when this comparison is one the runtime restricts to integers.
#[must_use]
pub const fn is_integer_comparison(op: Cmp) -> bool {
    !op.is_identity()
}
