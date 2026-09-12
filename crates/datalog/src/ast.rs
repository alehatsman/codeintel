//! The abstract syntax. One program is rules, facts, and at most one query.

use crate::atom::Term;

/// A relation applied to terms: `def(S, F, _, N)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pred {
    /// The relation name as written.
    pub name: String,
    /// Its arguments, in order.
    pub args: Vec<Term>,
    /// Byte range in the source.
    pub span: (usize, usize),
}

/// Comparison operators. `=` on a variable target is an [`Literal::Assign`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl Cmp {
    /// How it prints in a diagnostic.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    /// True for the two operators that work on any atom rather than integers.
    #[must_use]
    pub const fn is_identity(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
    }
}

/// Integer arithmetic operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

impl Arith {
    /// How it prints in a diagnostic.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        }
    }
}

/// The string tests. `Match` needs a host-injected regex engine; the other
/// three are plain string operations and always available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrTest {
    /// `match(S, "re")`
    Match,
    /// `prefix(S, "p")`
    Prefix,
    /// `suffix(S, "s")`
    Suffix,
    /// `contains(S, "c")`
    Contains,
}

impl StrTest {
    /// The builtin's name as written.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Prefix => "prefix",
            Self::Suffix => "suffix",
            Self::Contains => "contains",
        }
    }
}

/// The right-hand side of an assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A bare term: `X = Y`, `X = "get"`.
    Term(Term),
    /// `X = A + B`
    Arith {
        /// Which operator.
        op: Arith,
        /// Left operand.
        lhs: Term,
        /// Right operand.
        rhs: Term,
    },
    /// `N = count{ X : goal }`
    Count {
        /// The term whose distinct bindings are counted.
        over: Term,
        /// The goal those bindings must satisfy.
        goal: Vec<Literal>,
    },
}

/// One body literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// A positive relation literal.
    Pos(Pred),
    /// A negated relation literal: `!calls(_, S)`.
    Neg(Pred),
    /// A comparison. Both sides must already be bound.
    Compare {
        /// Which operator.
        op: Cmp,
        /// Left operand.
        lhs: Term,
        /// Right operand.
        rhs: Term,
        /// Byte range in the source.
        span: (usize, usize),
    },
    /// `X = expr`. Binds `X` when it is free, filters when it is bound.
    Assign {
        /// The target variable.
        target: Term,
        /// What it is assigned.
        expr: Expr,
        /// Byte range in the source.
        span: (usize, usize),
    },
    /// `between(Lo, Hi, X)` — the one builtin that produces bindings.
    Between {
        /// Inclusive lower bound.
        lo: Term,
        /// Inclusive upper bound.
        hi: Term,
        /// The generated variable, or a bound term to range-check.
        out: Term,
        /// Byte range in the source.
        span: (usize, usize),
    },
    /// A string test against a literal pattern.
    Str {
        /// Which test.
        test: StrTest,
        /// The atom whose string is tested.
        subject: Term,
        /// The pattern, as written.
        pattern: String,
        /// Byte range in the source.
        span: (usize, usize),
    },
}

impl Literal {
    /// The byte range this literal came from.
    #[must_use]
    pub fn span(&self) -> (usize, usize) {
        match self {
            Self::Pos(p) | Self::Neg(p) => p.span,
            Self::Compare { span, .. }
            | Self::Assign { span, .. }
            | Self::Between { span, .. }
            | Self::Str { span, .. } => *span,
        }
    }
}

/// A rule, or — with an empty body — a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The head.
    pub head: Pred,
    /// The body, in written order. Empty for a fact.
    pub body: Vec<Literal>,
    /// Variable names, indexed by [`Term::Var`].
    pub vars: Vec<String>,
    /// Byte range in the source.
    pub span: (usize, usize),
}

/// A query: `?- body.`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// The body, in written order.
    pub body: Vec<Literal>,
    /// Variable names, indexed by [`Term::Var`].
    pub vars: Vec<String>,
    /// Output columns: variable indices in order of first appearance.
    pub columns: Vec<u16>,
    /// Byte range in the source.
    pub span: (usize, usize),
}

/// A parsed program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Program {
    /// Rules and facts, in written order.
    pub rules: Vec<Rule>,
    /// The query, if the source has one.
    pub query: Option<Query>,
}
