//! The lexer. Byte-oriented, no allocation beyond string literals.

use crate::diag::{Diagnostic, Result, Status};

/// One token, with the byte range it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What it is.
    pub kind: Kind,
    /// Byte range in the source, end-exclusive.
    pub span: (usize, usize),
}

/// Token kinds. The grammar is deliberately small: no functors, no lists, no
/// cut, no arithmetic on strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A relation or constant name: lowercase-initial identifier.
    Ident(String),
    /// A variable: uppercase-initial identifier.
    Var(String),
    /// A string literal, escapes already resolved.
    Str(String),
    /// An integer literal. Negation is a separate token.
    Int(i64),
    /// `_`
    Underscore,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `:`
    Colon,
    /// `:-`
    Implies,
    /// `?-`
    Ask,
    /// `!`
    Bang,
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
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// End of input. Always the last token.
    End,
}

impl Kind {
    /// How this token prints inside a diagnostic.
    pub fn describe(&self) -> String {
        match self {
            Self::Ident(s) => format!("identifier `{s}`"),
            Self::Var(s) => format!("variable `{s}`"),
            Self::Str(s) => format!("string {s:?}"),
            Self::Int(n) => format!("integer `{n}`"),
            Self::End => "end of input".to_string(),
            other => format!("`{}`", other.symbol()),
        }
    }

    fn symbol(&self) -> &'static str {
        match self {
            Self::Underscore => "_",
            Self::LParen => "(",
            Self::RParen => ")",
            Self::LBrace => "{",
            Self::RBrace => "}",
            Self::Comma => ",",
            Self::Dot => ".",
            Self::Colon => ":",
            Self::Implies => ":-",
            Self::Ask => "?-",
            Self::Bang => "!",
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            _ => "?",
        }
    }
}

/// Tokenize `src`. The final token is always [`Kind::End`].
///
/// # Errors
/// Returns `invalid-query` on an unterminated string, an unknown escape, a
/// non-ASCII or otherwise unexpected character, or an integer that does not fit
/// an `i64`.
pub fn lex(src: &str) -> Result<Vec<Token>> {
    Lexer {
        src: src.as_bytes(),
        at: 0,
        out: Vec::new(),
    }
    .run()
}

struct Lexer<'a> {
    src: &'a [u8],
    at: usize,
    out: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.at).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.src.get(self.at + ahead).copied()
    }

    fn err(&self, from: usize, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::at(Status::InvalidQuery, (from, self.at.max(from + 1)), msg)
    }

    fn push(&mut self, kind: Kind, from: usize) {
        self.out.push(Token {
            kind,
            span: (from, self.at),
        });
    }

    fn run(mut self) -> Result<Vec<Token>> {
        while let Some(b) = self.peek() {
            let from = self.at;
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => self.at += 1,
                b'%' => self.skip_comment(),
                b'"' => self.string(from)?,
                b'0'..=b'9' => self.integer(from)?,
                b'a'..=b'z' => self.word(from, Kind::Ident),
                b'A'..=b'Z' => self.word(from, Kind::Var),
                b'_' => self.underscore(from),
                _ => self.punctuation(from, b)?,
            }
        }
        let end = self.at;
        self.out.push(Token {
            kind: Kind::End,
            span: (end, end),
        });
        Ok(self.out)
    }

    fn skip_comment(&mut self) {
        while let Some(b) = self.peek() {
            self.at += 1;
            if b == b'\n' {
                break;
            }
        }
    }

    /// `_` alone is a wildcard; `_foo` is not a legal name in this dialect.
    fn underscore(&mut self, from: usize) {
        self.at += 1;
        while matches!(
            self.peek(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        ) {
            self.at += 1;
        }
        if self.at == from + 1 {
            self.push(Kind::Underscore, from);
        } else {
            // Named wildcards read as variables everywhere else; treat `_x` as a
            // variable so range restriction reports it rather than the lexer.
            let text = String::from_utf8_lossy(self.slice(from)).into_owned();
            self.push(Kind::Var(text), from);
        }
    }

    fn word(&mut self, from: usize, make: fn(String) -> Kind) {
        while matches!(
            self.peek(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        ) {
            self.at += 1;
        }
        let text = String::from_utf8_lossy(self.slice(from)).into_owned();
        self.push(make(text), from);
    }

    fn slice(&self, from: usize) -> &[u8] {
        self.src.get(from..self.at).unwrap_or_default()
    }

    fn integer(&mut self, from: usize) -> Result<()> {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        let text = String::from_utf8_lossy(self.slice(from)).into_owned();
        let n: i64 = text.parse().map_err(|e| {
            self.err(
                from,
                format!("integer `{text}` is not a valid 64-bit value: {e}"),
            )
        })?;
        self.push(Kind::Int(n), from);
        Ok(())
    }

    fn string(&mut self, from: usize) -> Result<()> {
        self.at += 1; // opening quote
        let mut text = String::new();
        loop {
            let Some(b) = self.peek() else {
                return Err(self.err(from, "unterminated string literal: no closing `\"`"));
            };
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(esc) = self.peek() else {
                        return Err(self.err(from, "unterminated string literal: no closing `\"`"));
                    };
                    self.at += 1;
                    match esc {
                        b'"' => text.push('"'),
                        b'\\' => text.push('\\'),
                        b'n' => text.push('\n'),
                        b't' => text.push('\t'),
                        other => {
                            let c = char::from(other);
                            return Err(self.err(
                                from,
                                format!(
                                    "unknown escape `\\{c}` in a string literal; \
                                     this dialect has \\\" \\\\ \\n and \\t"
                                ),
                            ));
                        }
                    }
                }
                // Multi-byte UTF-8 passes through one byte at a time; the bytes
                // are contiguous in the source so the string rebuilds intact.
                _ => text.push_str(&String::from_utf8_lossy(&[b])),
            }
        }
        // `from_utf8_lossy` per byte mangles multi-byte characters, so rebuild
        // the payload from the source range when it holds any.
        let raw = self
            .src
            .get(from + 1..self.at.saturating_sub(1))
            .unwrap_or_default();
        if !raw.is_ascii() && !raw.contains(&b'\\') {
            text = String::from_utf8_lossy(raw).into_owned();
        }
        self.push(Kind::Str(text), from);
        Ok(())
    }

    fn punctuation(&mut self, from: usize, b: u8) -> Result<()> {
        let two = self.peek_at(1);
        let (kind, len) = match (b, two) {
            (b':', Some(b'-')) => (Kind::Implies, 2),
            (b'?', Some(b'-')) => (Kind::Ask, 2),
            (b'!', Some(b'=')) => (Kind::Ne, 2),
            (b'<', Some(b'=')) => (Kind::Le, 2),
            (b'>', Some(b'=')) => (Kind::Ge, 2),
            (b'(', _) => (Kind::LParen, 1),
            (b')', _) => (Kind::RParen, 1),
            (b'{', _) => (Kind::LBrace, 1),
            (b'}', _) => (Kind::RBrace, 1),
            (b',', _) => (Kind::Comma, 1),
            (b'.', _) => (Kind::Dot, 1),
            (b':', _) => (Kind::Colon, 1),
            (b'!', _) => (Kind::Bang, 1),
            (b'=', _) => (Kind::Eq, 1),
            (b'<', _) => (Kind::Lt, 1),
            (b'>', _) => (Kind::Gt, 1),
            (b'+', _) => (Kind::Plus, 1),
            (b'-', _) => (Kind::Minus, 1),
            (b'*', _) => (Kind::Star, 1),
            (b'/', _) => (Kind::Slash, 1),
            _ => {
                self.at += 1;
                let c = String::from_utf8_lossy(self.slice(from)).into_owned();
                return Err(self.err(from, format!("unexpected character `{c}`")));
            }
        };
        self.at += len;
        self.push(kind, from);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Kind> {
        lex(src)
            .expect("lexes")
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn a_rule_lexes_to_its_pieces() {
        assert_eq!(
            kinds("calls(A, B) :- x."),
            vec![
                Kind::Ident("calls".into()),
                Kind::LParen,
                Kind::Var("A".into()),
                Kind::Comma,
                Kind::Var("B".into()),
                Kind::RParen,
                Kind::Implies,
                Kind::Ident("x".into()),
                Kind::Dot,
                Kind::End,
            ]
        );
    }

    #[test]
    fn two_character_operators_win_over_one() {
        assert_eq!(
            kinds("<= >= != ?- :- < > ! = :"),
            vec![
                Kind::Le,
                Kind::Ge,
                Kind::Ne,
                Kind::Ask,
                Kind::Implies,
                Kind::Lt,
                Kind::Gt,
                Kind::Bang,
                Kind::Eq,
                Kind::Colon,
                Kind::End,
            ]
        );
    }

    #[test]
    fn comments_run_to_end_of_line() {
        assert_eq!(kinds("% a comment\n42"), vec![Kind::Int(42), Kind::End]);
    }

    #[test]
    fn string_escapes_resolve() {
        assert_eq!(
            kinds(r#""a\"b\\c\nd\te""#),
            vec![Kind::Str("a\"b\\c\nd\te".into()), Kind::End]
        );
    }

    #[test]
    fn non_ascii_survives_a_string_literal() {
        assert_eq!(
            kinds("\"héllo\""),
            vec![Kind::Str("héllo".into()), Kind::End]
        );
    }

    #[test]
    fn an_unterminated_string_is_a_diagnostic_not_a_panic() {
        let d = lex("\"oops").expect_err("unterminated");
        assert_eq!(d.status, Status::InvalidQuery);
        assert!(d.message.contains("unterminated"), "{}", d.message);
    }

    #[test]
    fn an_unknown_escape_names_the_legal_ones() {
        let d = lex(r#""a\qb""#).expect_err("bad escape");
        assert!(d.message.contains("\\q"), "{}", d.message);
        assert!(d.message.contains("\\n"), "{}", d.message);
    }

    #[test]
    fn a_bare_underscore_is_a_wildcard_and_a_named_one_is_a_variable() {
        assert_eq!(
            kinds("_ _x"),
            vec![Kind::Underscore, Kind::Var("_x".into()), Kind::End]
        );
    }

    #[test]
    fn an_unexpected_character_names_itself() {
        let d = lex("a @ b").expect_err("bad char");
        assert!(d.message.contains('@'), "{}", d.message);
    }
}
