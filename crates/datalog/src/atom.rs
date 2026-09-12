//! The term space: a `u32` atom, split into integers and interned strings.

use core::fmt;

/// One term. Either a small non-negative integer or an interned string id.
pub type Atom = u32;

/// Largest atom that denotes an integer. Atom `n <= INT_MAX` *is* the integer `n`.
pub const INT_MAX: Atom = 0x0FFF_FFFF;

/// Smallest atom that denotes an interned string.
pub const STR_MIN: Atom = INT_MAX + 1;

/// True if `a` denotes an integer rather than a string.
#[must_use]
pub const fn is_int(a: Atom) -> bool {
    a <= INT_MAX
}

/// The atom for integer `n`, or `None` if `n` is outside the integer range.
///
/// The range is non-negative by construction: a Datalog program that computes a
/// negative value gets an error naming the operation, never a wrapped atom.
#[must_use]
pub fn int_atom(n: i64) -> Option<Atom> {
    u32::try_from(n).ok().filter(|a| is_int(*a))
}

/// Renders an atom for a diagnostic: the integer, or the string in quotes.
///
/// `resolve` is the host's dictionary; an atom it does not know prints as
/// `atom#<id>`, which is still enough to locate the defect.
pub fn show(a: Atom, resolve: impl Fn(Atom) -> Option<String>) -> String {
    if is_int(a) {
        return a.to_string();
    }
    match resolve(a) {
        Some(s) => format!("{s:?}"),
        None => format!("atom#{a}"),
    }
}

/// A term as written in a program: a variable, a constant, or `_`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Term {
    /// A named variable, as an index into the rule's variable table.
    Var(u16),
    /// A constant atom, already interned.
    Const(Atom),
    /// `_` — matches anything, binds nothing, and never unifies with another `_`.
    Wildcard,
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Var(i) => write!(f, "?{i}"),
            Self::Const(a) => write!(f, "{a}"),
            Self::Wildcard => f.write_str("_"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_partition_is_where_the_spec_says() {
        assert!(is_int(0));
        assert!(is_int(INT_MAX));
        assert!(!is_int(STR_MIN));
        assert_eq!(STR_MIN, 0x1000_0000);
    }

    #[test]
    fn integers_outside_the_range_have_no_atom() {
        assert_eq!(int_atom(42), Some(42));
        assert_eq!(int_atom(0), Some(0));
        assert_eq!(int_atom(-1), None);
        assert_eq!(int_atom(i64::from(INT_MAX) + 1), None);
    }

    #[test]
    fn show_tells_an_integer_from_a_string() {
        assert_eq!(show(42, |_| None), "42");
        assert_eq!(show(STR_MIN, |_| Some("get".to_string())), "\"get\"");
        assert_eq!(show(STR_MIN, |_| None), "atom#268435456");
    }
}
