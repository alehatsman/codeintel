//! Containment by span nesting: the one non-trivial algorithm in tier A.
//!
//! Written once and shared. `specs/02-extraction.md` § Emission uses it for
//! `parent`, § Deriving `From` uses it to attribute a reference to the
//! definition it sits inside, and tier B calls the same code at M3 — both
//! tiers, one algorithm, one test suite.
//!
//! Sort by `(start, -end)` and sweep with a stack: everything still on the
//! stack when an item is reached is an ancestor of it, and the top of the
//! stack is the innermost one.

use std::cmp::Reverse;

/// A half-open byte range.
pub type Span = (usize, usize);

/// For each span, the index of the innermost span that strictly encloses it.
///
/// Ties — two items with the identical span — resolve by input order: the
/// earlier one is the outer. That keeps the result total and deterministic
/// rather than letting two items cancel each other out, which is the same
/// failure `tighter_at` in `rules/stdlib.dl` compares widths to avoid.
#[must_use]
pub fn containment(spans: &[Span]) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|i| {
        spans
            .get(*i)
            .map_or((0, Reverse(0), *i), |(s, e)| (*s, Reverse(*e), *i))
    });

    let mut parent = vec![None; spans.len()];
    let mut stack: Vec<usize> = Vec::new();
    for i in order {
        let Some((start, _)) = spans.get(i).copied() else {
            continue;
        };
        while stack
            .last()
            .and_then(|top| spans.get(*top))
            .is_some_and(|(_, end)| *end <= start)
        {
            stack.pop();
        }
        if let Some(slot) = parent.get_mut(i) {
            *slot = stack.last().copied();
        }
        stack.push(i);
    }
    parent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_finds_the_innermost_enclosing_span() {
        //  0 ..................... 100   outer
        //     10 ....... 50            inner
        //        20 .. 30            deepest
        //                    60 .. 70  sibling
        let spans = [(0, 100), (10, 50), (20, 30), (60, 70)];
        assert_eq!(
            containment(&spans),
            vec![None, Some(0), Some(1), Some(0)],
            "input order must not matter to the answer"
        );
    }

    #[test]
    fn input_order_does_not_change_the_answer() {
        let spans = [(20, 30), (60, 70), (0, 100), (10, 50)];
        assert_eq!(containment(&spans), vec![Some(3), Some(2), None, Some(2)]);
    }

    #[test]
    fn a_zero_width_position_lands_inside_its_definition() {
        // How a reference is attributed: give it its own position as a span
        // and it falls out of the same sweep.
        let spans = [(0, 100), (10, 50), (30, 30)];
        assert_eq!(containment(&spans), vec![None, Some(0), Some(1)]);
    }

    #[test]
    fn adjacent_spans_do_not_contain_each_other() {
        let spans = [(0, 10), (10, 20)];
        assert_eq!(containment(&spans), vec![None, None]);
    }

    #[test]
    fn identical_spans_resolve_by_input_order() {
        let spans = [(0, 10), (0, 10)];
        assert_eq!(containment(&spans), vec![None, Some(0)]);
    }

    #[test]
    fn nothing_is_not_a_problem() {
        assert!(containment(&[]).is_empty());
    }
}
