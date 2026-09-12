//! A relation: a set of fixed-arity tuples, kept sorted and deduplicated.
//!
//! Flat `Vec<Atom>` rather than `Vec<[Atom; N]>` — arity is runtime-determined,
//! and a flat buffer keeps one code path instead of a macro-generated family.
//! Sorted and deduplicated is the invariant every operation restores: it gives
//! binary-search lookup on a bound prefix, and set semantics for free.

use core::ops::Range;

use crate::atom::Atom;

/// A set of tuples of one arity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    arity: usize,
    data: Vec<Atom>,
    sorted: bool,
}

impl Relation {
    /// An empty relation of the given arity.
    ///
    /// # Panics
    /// Panics on arity zero — a nullary relation has no tuples to hold and the
    /// grammar cannot express one, so reaching here is a bug in the caller.
    #[must_use]
    pub fn new(arity: usize) -> Self {
        assert!(arity > 0, "a relation needs at least one column");
        Self {
            arity,
            data: Vec::new(),
            sorted: true,
        }
    }

    /// Its arity.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.arity
    }

    /// How many tuples it holds. Meaningful once [`Self::settle`] has run.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.data.len() / self.arity
    }

    /// True when it holds no tuples.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Row `i`, or `None` past the end.
    #[must_use]
    pub fn row(&self, i: usize) -> Option<&[Atom]> {
        self.data.get(i * self.arity..(i + 1) * self.arity)
    }

    /// Every tuple, in storage order — which is sorted order once settled.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &[Atom]> {
        self.data.chunks_exact(self.arity)
    }

    /// Append a tuple. The relation is unsorted until [`Self::settle`] runs.
    ///
    /// Rows of the wrong width are refused rather than truncated: a width
    /// mismatch is an arity bug upstream and silently reshaping it would hide
    /// the defect in the answer instead of in a diagnostic.
    pub fn push(&mut self, row: &[Atom]) -> bool {
        if row.len() != self.arity {
            return false;
        }
        self.data.extend_from_slice(row);
        self.sorted = false;
        true
    }

    /// Sort and deduplicate. Idempotent, and a no-op when nothing was pushed.
    pub fn settle(&mut self) {
        if self.sorted {
            return;
        }
        let arity = self.arity;
        let mut rows: Vec<&[Atom]> = self.data.chunks_exact(arity).collect();
        rows.sort_unstable();
        rows.dedup();
        let mut out = Vec::with_capacity(rows.len() * arity);
        for row in rows {
            out.extend_from_slice(row);
        }
        self.data = out;
        self.sorted = true;
    }

    /// True when the relation is sorted and deduplicated.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        self.sorted
    }

    /// The row range whose leading columns equal `prefix`.
    ///
    /// This is the bound-prefix lookup the join uses; an empty range means no
    /// tuple matches. Requires a settled relation — an unsettled one returns
    /// the empty range rather than a wrong one.
    #[must_use]
    pub fn prefix(&self, prefix: &[Atom]) -> Range<usize> {
        if !self.sorted || prefix.len() > self.arity {
            return 0..0;
        }
        let k = prefix.len();
        let lo = self.partition(|row| row.get(..k).is_some_and(|head| head < prefix));
        let hi = self.partition(|row| row.get(..k).is_some_and(|head| head <= prefix));
        lo..hi
    }

    /// Index of the first row for which `left` is false, over a sorted relation.
    fn partition(&self, left: impl Fn(&[Atom]) -> bool) -> usize {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.row(mid) {
                Some(row) if left(row) => lo = mid + 1,
                Some(_) => hi = mid,
                None => break,
            }
        }
        lo
    }

    /// True when the exact tuple is present.
    #[must_use]
    pub fn contains(&self, row: &[Atom]) -> bool {
        row.len() == self.arity && !self.prefix(row).is_empty()
    }

    /// Add every tuple of `other` that is not already present, and report how
    /// many were genuinely new. Both relations must have the same arity.
    pub fn absorb(&mut self, other: &Self) -> usize {
        if other.arity != self.arity {
            return 0;
        }
        let before = self.len();
        self.data.extend_from_slice(&other.data);
        self.sorted = false;
        self.settle();
        self.len().saturating_sub(before)
    }

    /// The tuples of `self` that `other` does not hold.
    #[must_use]
    pub fn minus(&self, other: &Self) -> Self {
        let mut out = Self::new(self.arity);
        for row in self.iter() {
            if !other.contains(row) {
                out.push(row);
            }
        }
        out.settle();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(arity: usize, rows: &[&[Atom]]) -> Relation {
        let mut r = Relation::new(arity);
        for row in rows {
            assert!(r.push(row), "arity {arity} row {row:?}");
        }
        r.settle();
        r
    }

    #[test]
    fn settling_sorts_and_dedupes() {
        let r = rel(2, &[&[2, 1], &[1, 9], &[2, 1], &[1, 2]]);
        let rows: Vec<&[Atom]> = r.iter().collect();
        assert_eq!(
            rows,
            vec![[1, 2].as_slice(), [1, 9].as_slice(), [2, 1].as_slice()]
        );
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn a_row_of_the_wrong_width_is_refused() {
        let mut r = Relation::new(2);
        assert!(!r.push(&[1]));
        assert!(!r.push(&[1, 2, 3]));
        assert!(r.is_empty());
    }

    #[test]
    fn prefix_finds_the_block_with_that_head() {
        let r = rel(2, &[&[1, 1], &[1, 2], &[1, 3], &[2, 1], &[3, 7]]);
        assert_eq!(r.prefix(&[1]), 0..3);
        assert_eq!(r.prefix(&[2]), 3..4);
        assert_eq!(r.prefix(&[3]), 4..5);
        assert_eq!(r.prefix(&[4]), 5..5);
        assert_eq!(r.prefix(&[0]), 0..0);
        assert_eq!(r.prefix(&[1, 2]), 1..2);
        assert_eq!(r.prefix(&[]), 0..5);
    }

    #[test]
    fn prefix_on_an_unsettled_relation_is_empty_rather_than_wrong() {
        let mut r = Relation::new(1);
        assert!(r.push(&[3]));
        assert!(r.push(&[1]));
        assert_eq!(r.prefix(&[1]), 0..0);
        r.settle();
        assert_eq!(r.prefix(&[1]), 0..1);
    }

    #[test]
    fn contains_is_exact() {
        let r = rel(2, &[&[1, 2], &[3, 4]]);
        assert!(r.contains(&[1, 2]));
        assert!(!r.contains(&[1, 3]));
        assert!(!r.contains(&[1]));
    }

    #[test]
    fn absorb_counts_only_what_was_new() {
        let mut a = rel(1, &[&[1], &[2]]);
        let b = rel(1, &[&[2], &[3]]);
        assert_eq!(a.absorb(&b), 1);
        assert_eq!(a.len(), 3);
        assert_eq!(a.absorb(&b), 0);
    }

    #[test]
    fn minus_removes_what_the_other_holds() {
        let a = rel(1, &[&[1], &[2], &[3]]);
        let b = rel(1, &[&[2]]);
        let d = a.minus(&b);
        let rows: Vec<&[Atom]> = d.iter().collect();
        assert_eq!(rows, vec![[1].as_slice(), [3].as_slice()]);
    }

    #[test]
    fn an_empty_relation_answers_every_lookup_negatively() {
        let r = Relation::new(3);
        assert!(!r.contains(&[1, 2, 3]));
        assert_eq!(r.prefix(&[1]), 0..0);
        assert_eq!(r.len(), 0);
    }
}
