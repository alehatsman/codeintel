//! Diagnostics. Every rejection names the rule, the variable, and the fix.

use core::fmt;

/// Why a program or query was rejected, or how an evaluation ended badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// The program does not typecheck: a safety rule, a parse error, an arity
    /// clash, or a builtin used wrongly.
    InvalidQuery,
    /// Negation or aggregation inside a cycle.
    Unstratified,
    /// `max_time_ms` fired.
    Timeout,
    /// `max_derived_tuples` or `max_regex_steps` fired.
    BudgetExceeded,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidQuery => "invalid-query",
            Self::Unstratified => "unstratified",
            Self::Timeout => "timeout",
            Self::BudgetExceeded => "budget-exceeded",
        })
    }
}

/// A rejection an agent must be able to act on without reading the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Which kind of failure this is.
    pub status: Status,
    /// Actionable prose naming the offending rule and variable.
    pub message: String,
    /// Byte range in the submitted source, when the failure has a location.
    pub span: Option<(usize, usize)>,
}

impl Diagnostic {
    /// A diagnostic with a source location.
    pub fn at(status: Status, span: (usize, usize), message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            span: Some(span),
        }
    }

    /// A diagnostic with no single location — a stratification cycle, a budget.
    pub fn whole(status: Status, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            span: None,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}

impl core::error::Error for Diagnostic {}

/// The engine's result type.
pub type Result<T> = core::result::Result<T, Diagnostic>;
