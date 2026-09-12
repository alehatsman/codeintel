//! What extraction can fail with.

use std::fmt;

/// An extraction failure. Every variant names the file or the query at fault.
#[derive(Debug)]
pub enum Error {
    /// A `.scm` file does not compile against its grammar.
    Query {
        /// Which language.
        lang: &'static str,
        /// Which query file — `tags` or `imports`.
        which: &'static str,
        /// The compiler's own message.
        message: String,
    },
    /// The grammar could not be installed in a parser.
    Grammar {
        /// Which language.
        lang: &'static str,
        /// The binding's own message.
        message: String,
    },
    /// tree-sitter returned no tree at all.
    Parse {
        /// Which file.
        path: String,
    },
    /// A `@definition.<suffix>` capture whose suffix is not a `Kind`.
    ///
    /// Reported rather than emitted as `unknown`: `unknown` is for a construct
    /// we cannot classify, not for a typo in a query we wrote.
    Capture {
        /// Which language.
        lang: &'static str,
        /// The capture as written.
        capture: String,
    },
    /// The atom space is exhausted — 3.8 billion distinct strings.
    Atoms,
    /// A position or offset past the integer atom range.
    ///
    /// `specs/01-facts.md` § Integers: files larger than 268M lines or bytes
    /// are not supported, and this is what that limit feels like.
    TooLarge {
        /// Which file.
        path: String,
    },
    /// I/O failure.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query {
                lang,
                which,
                message,
            } => write!(f, "queries/{lang}/{which}.scm does not compile: {message}"),
            Self::Grammar { lang, message } => {
                write!(f, "the {lang} grammar did not load: {message}")
            }
            Self::Parse { path } => write!(f, "{path} did not parse"),
            Self::Capture { lang, capture } => write!(
                f,
                "queries/{lang}/tags.scm captures @{capture}, which is not a Kind in this schema"
            ),
            Self::Atoms => f.write_str("the string dictionary is full"),
            Self::TooLarge { path } => write!(
                f,
                "{path} is larger than the 268M-line / 268M-byte limit on integer atoms"
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Extraction's result type.
pub type Result<T> = std::result::Result<T, Error>;
