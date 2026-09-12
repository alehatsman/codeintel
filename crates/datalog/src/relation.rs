//! A relation: a set of fixed-arity tuples, kept sorted and deduplicated.
//!
//! Flat `Vec<Atom>` rather than `Vec<[Atom; N]>` — arity is runtime-determined,
//! and a flat buffer keeps one code path instead of a macro-generated family.
//! Sorted and deduplicated is the invariant every operation restores: it gives
//! binary-search lookup on a bound prefix, and set semantics for free.
//!
//! The sort order serves a bound prefix only. A lookup that binds a later
//! column with the prefix free — `def(S, _, _, N)` with `N` bound — goes
//! through a per-column secondary index: the row ids ordered by that column,
//! built the first time a lookup asks for it and dropped by any write.

use core::ops::Range;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::atom::Atom;

/// A set of tuples of one arity.
#[derive(Debug, Clone)]
pub struct Relation {
    arity: usize,
    data: Vec<Atom>,
    sorted: bool,
    /// Row ids ordered by one column, ties in row order; keyed by column.
    /// A cache over `data`, so equality ignores it and every write clears it.
    index: RefCell<BTreeMap<usize, Arc<[usize]>>>,
}

impl PartialEq for Relation {
    fn eq(&self, other: &Self) -> bool {
        self.arity == other.arity && self.sorted == other.sorted && self.data == other.data
    }
}

impl Eq for Relation {}

/// The rows one lookup visits, as row ids in row order.
#[derive(Debug, Clone)]
pub enum Candidates {
    /// A contiguous block: a bound prefix, or the whole relation.
    Rows(Range<usize>),
    /// A block of one column's index.
    Column {
        /// The index: row ids ordered by the column.
        ids: Arc<[usize]>,
        /// The block of `ids` whose column equals the key.
        at: Range<usize>,
    },
}

impl Iterator for Candidates {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        match self {
            Self::Rows(range) => range.next(),
            Self::Column { ids, at } => at.next().and_then(|i| ids.get(i).copied()),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = match self {
            Self::Rows(range) => range.len(),
            Self::Column { at, .. } => at.len(),
        };
        (n, Some(n))
    }
}

impl ExactSizeIterator for Candidates {}

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
            index: RefCell::new(BTreeMap::new()),
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
        self.index.get_mut().clear();
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

    /// The rows a lookup must visit when the columns given as `Some` are
    /// bound: a prefix block when the leading column is bound, the narrowest
    /// bound column's index block when it is not, every row when nothing is.
    ///
    /// Each path yields rows in row order, so the same rows come out in the
    /// same order whichever served them. A pattern of the wrong width, or an
    /// unsettled relation, visits nothing.
    #[must_use]
    pub fn select(&self, pattern: &[Option<Atom>]) -> Candidates {
        if !self.sorted || pattern.len() != self.arity {
            return Candidates::Rows(0..0);
        }
        let prefix: Vec<Atom> = pattern.iter().map_while(|p| *p).collect();
        if !prefix.is_empty() {
            return Candidates::Rows(self.prefix(&prefix));
        }
        let mut best: Option<Candidates> = None;
        for (col, key) in pattern.iter().enumerate().skip(1) {
            let Some(key) = key else { continue };
            let Some((ids, at)) = self.column(col, *key) else {
                continue;
            };
            if best.as_ref().is_none_or(|b| at.len() < b.len()) {
                best = Some(Candidates::Column { ids, at });
            }
        }
        best.unwrap_or(Candidates::Rows(0..self.len()))
    }

    /// The block of column `col`'s index whose value is `key`.
    fn column(&self, col: usize, key: Atom) -> Option<(Arc<[usize]>, Range<usize>)> {
        let ids = self.order_by(col)?;
        let lo = ids.partition_point(|&r| self.cell(r, col).is_some_and(|a| a < key));
        let hi = ids.partition_point(|&r| self.cell(r, col).is_some_and(|a| a <= key));
        Some((ids, lo..hi))
    }

    /// Row ids ordered by column `col`, ties in row order. Built on first use
    /// and kept until the next write.
    fn order_by(&self, col: usize) -> Option<Arc<[usize]>> {
        if !self.sorted || col >= self.arity {
            return None;
        }
        if let Some(ids) = self.index.borrow().get(&col) {
            return Some(Arc::clone(ids));
        }
        let mut ids: Vec<usize> = (0..self.len()).collect();
        // Stable, so equal keys keep row order.
        ids.sort_by_key(|&r| self.cell(r, col));
        let ids: Arc<[usize]> = ids.into();
        self.index.borrow_mut().insert(col, Arc::clone(&ids));
        Some(ids)
    }

    fn cell(&self, row: usize, col: usize) -> Option<Atom> {
        self.data.get(row * self.arity + col).copied()
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
        self.index.get_mut().clear();
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

    /// What `select` must agree with: every row whose bound columns match.
    fn scan(r: &Relation, pattern: &[Option<Atom>]) -> Vec<usize> {
        (0..r.len())
            .filter(|&i| {
                r.row(i).is_some_and(|row| {
                    pattern
                        .iter()
                        .zip(row)
                        .all(|(want, got)| want.is_none_or(|a| a == *got))
                })
            })
            .collect()
    }

    #[test]
    fn a_bound_non_leading_column_is_served_by_its_index_in_row_order() {
        let r = rel(
            3,
            &[&[1, 7, 1], &[2, 5, 1], &[3, 7, 2], &[4, 7, 1], &[5, 5, 2]],
        );
        let by_second = r.select(&[None, Some(7), None]);
        assert!(matches!(by_second, Candidates::Column { .. }));
        assert_eq!(by_second.collect::<Vec<_>>(), vec![0, 2, 3]);
        assert_eq!(
            r.select(&[None, Some(5), None]).collect::<Vec<_>>(),
            vec![1, 4]
        );
        assert_eq!(r.select(&[None, Some(6), None]).collect::<Vec<_>>(), vec![]);
        assert_eq!(
            r.select(&[None, None, Some(2)]).collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    #[test]
    fn a_bound_prefix_still_wins_over_a_bound_column() {
        let r = rel(2, &[&[1, 1], &[1, 2], &[2, 1]]);
        let c = r.select(&[Some(1), Some(1)]);
        assert!(matches!(c, Candidates::Rows(_)));
        assert_eq!(c.collect::<Vec<_>>(), vec![0]);
        let all = r.select(&[None, None]);
        assert!(matches!(all, Candidates::Rows(_)));
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn the_narrowest_bound_column_is_the_one_visited() {
        // Column 1 is 9 everywhere; column 2 is unique. Binding both must
        // visit one row, not four.
        let r = rel(3, &[&[1, 9, 1], &[2, 9, 2], &[3, 9, 3], &[4, 9, 4]]);
        let c = r.select(&[None, Some(9), Some(3)]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn a_write_drops_the_index() {
        let mut r = rel(2, &[&[1, 5], &[2, 6]]);
        assert_eq!(r.select(&[None, Some(6)]).collect::<Vec<_>>(), vec![1]);
        assert!(r.push(&[0, 6]));
        assert_eq!(
            r.select(&[None, Some(6)]).len(),
            0,
            "unsettled: visits nothing"
        );
        r.settle();
        assert_eq!(r.select(&[None, Some(6)]).collect::<Vec<_>>(), vec![0, 2]);
        let more = rel(2, &[&[3, 6]]);
        assert_eq!(r.absorb(&more), 1);
        assert_eq!(
            r.select(&[None, Some(6)]).collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
    }

    #[test]
    fn select_agrees_with_a_scan_on_every_binding_shape() {
        // A small LCG: deterministic, no dependency, enough collisions to
        // matter. Values are drawn from a narrow range so every column has
        // duplicate keys and every pattern has hits and misses.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = |bound: u32| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            u32::try_from(state >> 33).unwrap_or(0) % bound
        };
        for arity in 1..=4 {
            let mut r = Relation::new(arity);
            for _ in 0..200 {
                let row: Vec<Atom> = (0..arity).map(|_| next(6)).collect();
                assert!(r.push(&row));
            }
            r.settle();
            for _ in 0..300 {
                let pattern: Vec<Option<Atom>> = (0..arity)
                    .map(|_| if next(2) == 0 { None } else { Some(next(7)) })
                    .collect();
                let got: Vec<usize> = r.select(&pattern).collect();
                let want = scan(&r, &pattern);
                // Every path may visit rows the other bound columns reject;
                // none may skip a match or reorder one.
                let matched: Vec<usize> =
                    got.iter().copied().filter(|&i| want.contains(&i)).collect();
                assert_eq!(matched, want, "arity {arity} pattern {pattern:?}");
                // With one bound column the index block is exactly the matches.
                let bound = pattern.iter().flatten().count();
                if bound == 1 && pattern.first().is_some_and(Option::is_none) {
                    assert!(matches!(r.select(&pattern), Candidates::Column { .. }));
                    assert_eq!(got, want, "arity {arity} pattern {pattern:?}");
                }
            }
        }
    }

    #[test]
    fn a_pattern_of_the_wrong_width_visits_nothing() {
        let r = rel(2, &[&[1, 2]]);
        assert_eq!(r.select(&[Some(1)]).len(), 0);
        assert_eq!(r.select(&[None, Some(2), None]).len(), 0);
    }

    #[test]
    fn an_empty_relation_answers_every_lookup_negatively() {
        let r = Relation::new(3);
        assert!(!r.contains(&[1, 2, 3]));
        assert_eq!(r.prefix(&[1]), 0..0);
        assert_eq!(r.select(&[None, Some(2), None]).len(), 0);
        assert_eq!(r.len(), 0);
    }
}
