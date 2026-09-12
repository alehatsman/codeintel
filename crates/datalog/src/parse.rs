//! The parser. Tokens in, [`Program`] out, a [`Diagnostic`] on anything else.
//!
//! String literals are interned as they are parsed, so the AST carries atoms
//! and evaluation never touches text except through the string builtins.

use crate::ast::{Arith, Cmp, Expr, Literal, Pred, Program, Query, Rule, StrTest};
use crate::atom::{Term, int_atom};
use crate::diag::{Diagnostic, Result, Status};
use crate::lex::{Kind, Token, lex};
use crate::symbols::Symbols;

/// Builtin names a rule may not define.
pub const RESERVED: [&str; 6] = ["between", "match", "prefix", "suffix", "contains", "count"];

/// Parse `src`, interning its string literals into `syms`.
///
/// # Errors
/// Returns `invalid-query` with a span for any lexical or syntactic defect.
pub fn parse(src: &str, syms: &mut dyn Symbols) -> Result<Program> {
    let toks = lex(src)?;
    Parser {
        toks,
        at: 0,
        syms,
        vars: Vec::new(),
    }
    .program()
}

struct Parser<'a> {
    toks: Vec<Token>,
    at: usize,
    syms: &'a mut dyn Symbols,
    vars: Vec<String>,
}

impl Parser<'_> {
    // ── token plumbing ──────────────────────────────────────────────────────

    fn peek(&self) -> &Kind {
        self.toks.get(self.at).map_or(&Kind::End, |t| &t.kind)
    }

    fn span(&self) -> (usize, usize) {
        self.toks.get(self.at).map_or((0, 0), |t| t.span)
    }

    fn bump(&mut self) -> Kind {
        let kind = self.toks.get(self.at).map_or(Kind::End, |t| t.kind.clone());
        if self.at < self.toks.len() {
            self.at += 1;
        }
        kind
    }

    fn eat(&mut self, want: &Kind) -> bool {
        if self.peek() == want {
            self.at += 1;
            return true;
        }
        false
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(Diagnostic::at(Status::InvalidQuery, self.span(), msg))
    }

    fn expect(&mut self, want: &Kind, context: &str) -> Result<()> {
        if self.eat(want) {
            return Ok(());
        }
        let found = self.peek().describe();
        self.err(format!(
            "expected `{}` {context}, found {found}",
            describe_wanted(want)
        ))
    }

    fn var_index(&mut self, name: &str) -> Result<u16> {
        if let Some(i) = self.vars.iter().position(|v| v == name) {
            return u16::try_from(i).map_or_else(|_| self.err(TOO_MANY_VARS), Ok);
        }
        let Ok(next) = u16::try_from(self.vars.len()) else {
            return self.err(TOO_MANY_VARS);
        };
        self.vars.push(name.to_string());
        Ok(next)
    }

    // ── program ─────────────────────────────────────────────────────────────

    fn program(mut self) -> Result<Program> {
        let mut out = Program::default();
        while *self.peek() != Kind::End {
            if *self.peek() == Kind::Ask {
                let query = self.query()?;
                if out.query.is_some() {
                    return self.err("a program may hold at most one query (`?-`)");
                }
                out.query = Some(query);
            } else {
                let rule = self.rule()?;
                out.rules.push(rule);
            }
        }
        Ok(out)
    }

    fn query(&mut self) -> Result<Query> {
        let from = self.span().0;
        self.vars.clear();
        self.expect(&Kind::Ask, "to start a query")?;
        let body = self.body()?;
        let to = self.span().1;
        self.expect(&Kind::Dot, "to end a query")?;
        let columns = output_columns(&body);
        Ok(Query {
            body,
            vars: core::mem::take(&mut self.vars),
            columns,
            span: (from, to),
        })
    }

    fn rule(&mut self) -> Result<Rule> {
        let from = self.span().0;
        self.vars.clear();
        let head = self.pred()?;
        if RESERVED.contains(&head.name.as_str()) {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                head.span,
                format!(
                    "`{}` is a builtin and cannot be defined by a rule; rename the head",
                    head.name
                ),
            ));
        }
        let body = if self.eat(&Kind::Implies) {
            self.body()?
        } else {
            Vec::new()
        };
        let to = self.span().1;
        self.expect(&Kind::Dot, "to end a rule")?;
        Ok(Rule {
            head,
            body,
            vars: core::mem::take(&mut self.vars),
            span: (from, to),
        })
    }

    fn body(&mut self) -> Result<Vec<Literal>> {
        let mut out = vec![self.literal()?];
        while self.eat(&Kind::Comma) {
            out.push(self.literal()?);
        }
        Ok(out)
    }

    // ── literals ────────────────────────────────────────────────────────────

    fn literal(&mut self) -> Result<Literal> {
        if self.eat(&Kind::Bang) {
            return Ok(Literal::Neg(self.pred()?));
        }
        if let Kind::Ident(name) = self.peek().clone() {
            return self.named_literal(&name);
        }
        self.operator_literal()
    }

    fn named_literal(&mut self, name: &str) -> Result<Literal> {
        match name {
            "between" => self.between(),
            "match" => self.str_test(StrTest::Match),
            "prefix" => self.str_test(StrTest::Prefix),
            "suffix" => self.str_test(StrTest::Suffix),
            "contains" => self.str_test(StrTest::Contains),
            "count" => self.err("`count` may only appear as `X = count{ T : goal }`"),
            _ => Ok(Literal::Pos(self.pred()?)),
        }
    }

    /// A literal that starts with a term: a comparison or an assignment.
    fn operator_literal(&mut self) -> Result<Literal> {
        let from = self.span().0;
        let lhs = self.term()?;
        let op = match self.peek() {
            Kind::Eq => Cmp::Eq,
            Kind::Ne => Cmp::Ne,
            Kind::Lt => Cmp::Lt,
            Kind::Le => Cmp::Le,
            Kind::Gt => Cmp::Gt,
            Kind::Ge => Cmp::Ge,
            other => {
                let found = other.describe();
                return self.err(format!(
                    "expected a comparison or `=` after a term, found {found}; \
                     a relation literal must be written `name(Arg, ...)`"
                ));
            }
        };
        self.at += 1;
        if op == Cmp::Eq && matches!(lhs, Term::Var(_)) {
            let expr = self.expr()?;
            let to = self.prev_end();
            return Ok(Literal::Assign {
                target: lhs,
                expr,
                span: (from, to),
            });
        }
        let rhs = self.term()?;
        let to = self.prev_end();
        Ok(Literal::Compare {
            op,
            lhs,
            rhs,
            span: (from, to),
        })
    }

    fn prev_end(&self) -> usize {
        self.toks
            .get(self.at.saturating_sub(1))
            .map_or(0, |t| t.span.1)
    }

    fn between(&mut self) -> Result<Literal> {
        let from = self.span().0;
        self.at += 1; // `between`
        self.expect(&Kind::LParen, "after `between`")?;
        let lo = self.term()?;
        self.expect(&Kind::Comma, "between `between`'s arguments")?;
        let hi = self.term()?;
        self.expect(&Kind::Comma, "between `between`'s arguments")?;
        let out = self.term()?;
        self.expect(&Kind::RParen, "to close `between`")?;
        Ok(Literal::Between {
            lo,
            hi,
            out,
            span: (from, self.prev_end()),
        })
    }

    fn str_test(&mut self, test: StrTest) -> Result<Literal> {
        let from = self.span().0;
        self.at += 1; // the builtin's name
        self.expect(&Kind::LParen, &format!("after `{}`", test.name()))?;
        let subject = self.term()?;
        self.expect(
            &Kind::Comma,
            &format!("between `{}`'s arguments", test.name()),
        )?;
        let Kind::Str(pattern) = self.bump() else {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                self.span(),
                format!(
                    "`{}`'s second argument must be a literal string, not a variable",
                    test.name()
                ),
            ));
        };
        self.expect(&Kind::RParen, &format!("to close `{}`", test.name()))?;
        Ok(Literal::Str {
            test,
            subject,
            pattern,
            span: (from, self.prev_end()),
        })
    }

    fn expr(&mut self) -> Result<Expr> {
        if *self.peek() == Kind::Ident("count".to_string()) {
            return self.count();
        }
        let lhs = self.term()?;
        let op = match self.peek() {
            Kind::Plus => Arith::Add,
            Kind::Minus => Arith::Sub,
            Kind::Star => Arith::Mul,
            Kind::Slash => Arith::Div,
            _ => return Ok(Expr::Term(lhs)),
        };
        self.at += 1;
        let rhs = self.term()?;
        Ok(Expr::Arith { op, lhs, rhs })
    }

    fn count(&mut self) -> Result<Expr> {
        self.at += 1; // `count`
        self.expect(&Kind::LBrace, "after `count`")?;
        let over = self.term()?;
        self.expect(&Kind::Colon, "between `count`'s term and its goal")?;
        let goal = self.body()?;
        self.expect(&Kind::RBrace, "to close `count`")?;
        Ok(Expr::Count { over, goal })
    }

    fn pred(&mut self) -> Result<Pred> {
        let from = self.span().0;
        let Kind::Ident(name) = self.bump() else {
            return Err(Diagnostic::at(
                Status::InvalidQuery,
                (from, from + 1),
                "expected a relation name (a lowercase identifier)",
            ));
        };
        self.expect(&Kind::LParen, &format!("after `{name}`"))?;
        let mut args = vec![self.term()?];
        while self.eat(&Kind::Comma) {
            args.push(self.term()?);
        }
        self.expect(&Kind::RParen, &format!("to close `{name}`"))?;
        Ok(Pred {
            name,
            args,
            span: (from, self.prev_end()),
        })
    }

    fn term(&mut self) -> Result<Term> {
        let span = self.span();
        match self.bump() {
            Kind::Underscore => Ok(Term::Wildcard),
            Kind::Var(name) => Ok(Term::Var(self.var_index(&name)?)),
            Kind::Int(n) => int_term(n, span),
            Kind::Minus => match self.bump() {
                Kind::Int(n) => int_term(-n, span),
                other => Err(Diagnostic::at(
                    Status::InvalidQuery,
                    span,
                    format!("expected an integer after `-`, found {}", other.describe()),
                )),
            },
            Kind::Str(text) => {
                let atom = self.syms.intern(&text).ok_or_else(|| {
                    Diagnostic::at(Status::InvalidQuery, span, "the string dictionary is full")
                })?;
                Ok(Term::Const(atom))
            }
            other => Err(Diagnostic::at(
                Status::InvalidQuery,
                span,
                format!(
                    "expected a term — a variable, a string, an integer or `_` — found {}",
                    other.describe()
                ),
            )),
        }
    }
}

/// One rule may name at most this many distinct variables.
const TOO_MANY_VARS: &str = "a rule may name at most 65,536 distinct variables";

fn int_term(n: i64, span: (usize, usize)) -> Result<Term> {
    int_atom(n).map(Term::Const).ok_or_else(|| {
        Diagnostic::at(
            Status::InvalidQuery,
            span,
            format!(
                "integer `{n}` is outside the supported range 0..=268435455 \
                 (atoms are non-negative)"
            ),
        )
    })
}

/// Output columns of a query: variable indices in order of first appearance,
/// counting only what the goal's own literals mention.
fn output_columns(body: &[Literal]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut push = |t: &Term| {
        if let Term::Var(i) = t
            && !out.contains(i)
        {
            out.push(*i);
        }
    };
    for lit in body {
        match lit {
            Literal::Pos(p) | Literal::Neg(p) => p.args.iter().for_each(&mut push),
            Literal::Compare { lhs, rhs, .. } => {
                push(lhs);
                push(rhs);
            }
            Literal::Assign { target, .. } => push(target),
            Literal::Between { lo, hi, out: o, .. } => {
                push(lo);
                push(hi);
                push(o);
            }
            Literal::Str { subject, .. } => push(subject),
        }
    }
    out
}

fn describe_wanted(kind: &Kind) -> &'static str {
    match kind {
        Kind::LParen => "(",
        Kind::RParen => ")",
        Kind::LBrace => "{",
        Kind::RBrace => "}",
        Kind::Comma => ",",
        Kind::Dot => ".",
        Kind::Colon => ":",
        Kind::Implies => ":-",
        Kind::Ask => "?-",
        _ => "token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::Strings;

    fn p(src: &str) -> Result<Program> {
        parse(src, &mut Strings::new())
    }

    fn ok(src: &str) -> Program {
        p(src).expect("this source parses")
    }

    fn message(src: &str) -> String {
        p(src).expect_err("expected a rejection").message
    }

    #[test]
    fn a_fact_is_a_rule_with_no_body() {
        let prog = ok(r#"callable("function")."#);
        assert_eq!(prog.rules.len(), 1);
        let rule = prog.rules.first().expect("one rule");
        assert_eq!(rule.head.name, "callable");
        assert!(rule.body.is_empty());
    }

    #[test]
    fn a_rule_keeps_its_body_in_written_order() {
        let prog = ok("calls(A, B) :- edge(A, B), edge(B, A).");
        let rule = prog.rules.first().expect("one rule");
        assert_eq!(rule.body.len(), 2);
        assert_eq!(rule.vars, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn variables_are_numbered_by_first_appearance() {
        let prog = ok("r(X, Y) :- e(Y, X).");
        let rule = prog.rules.first().expect("one rule");
        assert_eq!(rule.head.args, vec![Term::Var(0), Term::Var(1)]);
        assert_eq!(rule.body.len(), 1);
    }

    #[test]
    fn a_query_names_its_columns_in_order_of_first_appearance() {
        let prog = ok("?- def(S, F, _, N), at(S, F, L).");
        let query = prog.query.expect("a query");
        let names: Vec<&str> = query
            .columns
            .iter()
            .filter_map(|i| query.vars.get(usize::from(*i)))
            .map(String::as_str)
            .collect();
        assert_eq!(names, vec!["S", "F", "N", "L"]);
    }

    #[test]
    fn an_equality_onto_a_variable_is_an_assignment() {
        let prog = ok("long(S, N) :- span(S, A, B), N = B - A.");
        let rule = prog.rules.first().expect("one rule");
        let last = rule.body.last().expect("two literals");
        assert!(
            matches!(
                last,
                Literal::Assign {
                    expr: Expr::Arith { op: Arith::Sub, .. },
                    ..
                }
            ),
            "{last:?}"
        );
    }

    #[test]
    fn an_equality_onto_a_constant_is_a_comparison() {
        let prog = ok(r#"r(X) :- e(X), "a" = X."#);
        let rule = prog.rules.first().expect("one rule");
        let last = rule.body.last().expect("two literals");
        assert!(
            matches!(last, Literal::Compare { op: Cmp::Eq, .. }),
            "{last:?}"
        );
    }

    #[test]
    fn count_parses_as_an_assignment_with_a_goal() {
        let prog = ok("hot(S, N) :- def(S), N = count{ C : calls(C, S) }, N > 10.");
        let rule = prog.rules.first().expect("one rule");
        let agg = rule.body.get(1).expect("three literals");
        let Literal::Assign {
            expr: Expr::Count { goal, .. },
            ..
        } = agg
        else {
            panic!("expected an aggregate, got {agg:?}");
        };
        assert_eq!(goal.len(), 1);
    }

    #[test]
    fn between_is_a_literal_of_its_own() {
        let prog = ok("symbol_at(F, L, S) :- span(S, F, A, B), between(A, B, L).");
        let rule = prog.rules.first().expect("one rule");
        assert!(matches!(rule.body.last(), Some(Literal::Between { .. })));
    }

    #[test]
    fn the_string_tests_keep_their_pattern_as_text() {
        let prog = ok(r#"is_test(F) :- file(F), match(F, "_test\\.go$")."#);
        let rule = prog.rules.first().expect("one rule");
        let Some(Literal::Str { test, pattern, .. }) = rule.body.last() else {
            panic!("expected a string test");
        };
        assert_eq!(*test, StrTest::Match);
        assert_eq!(pattern, "_test\\.go$");
    }

    #[test]
    fn a_string_test_rejects_a_variable_pattern() {
        let msg = message("r(F) :- file(F), match(F, P).");
        assert!(msg.contains("literal string"), "{msg}");
    }

    #[test]
    fn a_builtin_name_cannot_be_a_rule_head() {
        for name in RESERVED {
            let src = format!("{name}(X) :- e(X).");
            let msg = message(&src);
            assert!(msg.contains("builtin"), "{name}: {msg}");
        }
    }

    #[test]
    fn an_integer_outside_the_atom_range_is_rejected_by_name() {
        let msg = message("r(268435456).");
        assert!(msg.contains("0..=268435455"), "{msg}");
        let msg = message("r(-1).");
        assert!(msg.contains("non-negative"), "{msg}");
    }

    #[test]
    fn a_missing_dot_says_what_it_wanted() {
        let msg = message("r(X) :- e(X)");
        assert!(msg.contains("expected `.`"), "{msg}");
    }

    #[test]
    fn a_bare_identifier_is_not_a_literal() {
        let msg = message("r(X) :- e.");
        assert!(msg.contains("expected `(`"), "{msg}");
    }

    #[test]
    fn two_queries_in_one_program_are_rejected() {
        let msg = message("?- e(X). ?- f(Y).");
        assert!(msg.contains("at most one query"), "{msg}");
    }

    #[test]
    fn a_term_followed_by_nothing_useful_explains_the_literal_syntax() {
        let msg = message("r(X) :- X.");
        assert!(msg.contains("name(Arg"), "{msg}");
    }

    #[test]
    fn count_outside_an_assignment_is_rejected() {
        let msg = message("r(X) :- count{ Y : e(Y) }.");
        assert!(msg.contains("count{"), "{msg}");
    }

    #[test]
    fn spans_point_into_the_source() {
        let src = "r(X) :- e(X), 268435456 = X.";
        let d = p(src).expect_err("out of range");
        let (from, to) = d.span.expect("a span");
        assert_eq!(src.get(from..to), Some("268435456"));
    }
}
