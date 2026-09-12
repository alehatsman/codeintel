//! Stratification: negation and aggregation may not sit inside a cycle.
//!
//! Build a dependency graph over derived relations, labelling an edge negative
//! when the dependency is through `!` or an aggregate goal. A cycle containing
//! a negative edge has no well-defined answer, so it is rejected by name rather
//! than evaluated to whatever the iteration order happens to produce.

use std::collections::BTreeMap;

use crate::ast::{Expr, Literal, Program};
use crate::diag::{Diagnostic, Result, Status};

/// Derived relations grouped into evaluation order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Strata {
    /// Each stratum, in the order it must be evaluated. Relations inside one
    /// stratum are mutually recursive and reach fixpoint together.
    pub order: Vec<Vec<String>>,
}

impl Strata {
    /// How many strata the program has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// True when the program derives nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The stratum index a relation is evaluated in, if it is derived.
    #[must_use]
    pub fn stratum_of(&self, name: &str) -> Option<usize> {
        self.order.iter().position(|s| s.iter().any(|n| n == name))
    }
}

/// One edge: `from` depends on `to`, negatively when `negative` is set.
struct Edge {
    from: usize,
    to: usize,
    negative: bool,
}

/// Stratify a program's derived relations.
///
/// # Errors
/// Returns `unstratified` naming every relation in the offending cycle.
pub fn stratify(program: &Program) -> Result<Strata> {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut names: Vec<&str> = Vec::new();
    for rule in &program.rules {
        let next = names.len();
        index.entry(rule.head.name.as_str()).or_insert_with(|| {
            names.push(rule.head.name.as_str());
            next
        });
    }

    let mut edges = Vec::new();
    for rule in &program.rules {
        let Some(from) = index.get(rule.head.name.as_str()).copied() else {
            continue;
        };
        let mut deps = Vec::new();
        collect(&rule.body, false, &mut deps);
        for (name, negative) in deps {
            if let Some(to) = index.get(name).copied() {
                edges.push(Edge { from, to, negative });
            }
        }
    }

    let components = scc(names.len(), &edges);
    for edge in &edges {
        let (Some(a), Some(b)) = (components.get(edge.from), components.get(edge.to)) else {
            continue;
        };
        if edge.negative && a == b {
            let cycle: Vec<&str> = names
                .iter()
                .enumerate()
                .filter(|(i, _)| components.get(*i) == Some(a))
                .map(|(_, n)| *n)
                .collect();
            return Err(Diagnostic::whole(
                Status::Unstratified,
                format!(
                    "negation or an aggregate appears inside the recursive cycle `{}`; \
                     a relation cannot depend on the absence of its own conclusions. \
                     Split the recursion from the negation into two relations",
                    cycle.join("` -> `")
                ),
            ));
        }
    }

    // `scc` numbers components in the order it completes them, which for this
    // edge direction (head -> body) is dependencies first.
    let count = components.iter().copied().max().map_or(0, |m| m + 1);
    let mut order = vec![Vec::new(); count];
    for (i, name) in names.iter().enumerate() {
        if let Some(c) = components.get(i).copied()
            && let Some(stratum) = order.get_mut(c)
        {
            stratum.push((*name).to_string());
        }
    }
    for stratum in &mut order {
        stratum.sort();
    }
    Ok(Strata { order })
}

/// Every relation a body depends on, with the sign of the dependency.
fn collect<'a>(body: &'a [Literal], inside_negation: bool, out: &mut Vec<(&'a str, bool)>) {
    for lit in body {
        match lit {
            Literal::Pos(p) => out.push((p.name.as_str(), inside_negation)),
            Literal::Neg(p) => out.push((p.name.as_str(), true)),
            Literal::Assign {
                expr: Expr::Count { goal, .. },
                ..
            } => collect(goal, true, out),
            _ => {}
        }
    }
}

/// Tarjan's strongly connected components, iterative.
///
/// Recursion would be the shorter code and a stack overflow on a program with
/// a few thousand mutually recursive rules — which is user input.
fn scc(n: usize, edges: &[Edge]) -> Vec<usize> {
    let mut adjacency = vec![Vec::new(); n];
    for e in edges {
        if let Some(list) = adjacency.get_mut(e.from) {
            list.push(e.to);
        }
    }

    let mut state = Tarjan {
        adjacency,
        index: vec![usize::MAX; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        component: vec![usize::MAX; n],
        next_index: 0,
        next_component: 0,
    };
    for v in 0..n {
        if state.index.get(v).copied() == Some(usize::MAX) {
            state.walk(v);
        }
    }
    state.component
}

struct Tarjan {
    adjacency: Vec<Vec<usize>>,
    index: Vec<usize>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    component: Vec<usize>,
    next_index: usize,
    next_component: usize,
}

impl Tarjan {
    fn walk(&mut self, root: usize) {
        let mut work = vec![(root, 0usize)];
        self.enter(root);
        while let Some((v, edge)) = work.pop() {
            if let Some(w) = self.adjacency.get(v).and_then(|a| a.get(edge)).copied() {
                work.push((v, edge + 1));
                if self.index.get(w).copied() == Some(usize::MAX) {
                    self.enter(w);
                    work.push((w, 0));
                } else if self.on_stack.get(w).copied() == Some(true) {
                    self.relax(v, self.index.get(w).copied().unwrap_or(0));
                }
                continue;
            }
            self.close(v);
            if let Some((parent, _)) = work.last().copied() {
                self.relax(parent, self.low.get(v).copied().unwrap_or(0));
            }
        }
    }

    fn enter(&mut self, v: usize) {
        if let Some(slot) = self.index.get_mut(v) {
            *slot = self.next_index;
        }
        if let Some(slot) = self.low.get_mut(v) {
            *slot = self.next_index;
        }
        self.next_index += 1;
        self.stack.push(v);
        if let Some(slot) = self.on_stack.get_mut(v) {
            *slot = true;
        }
    }

    fn relax(&mut self, v: usize, candidate: usize) {
        if let Some(slot) = self.low.get_mut(v)
            && candidate < *slot
        {
            *slot = candidate;
        }
    }

    fn close(&mut self, v: usize) {
        if self.low.get(v) != self.index.get(v) {
            return;
        }
        while let Some(w) = self.stack.pop() {
            if let Some(slot) = self.on_stack.get_mut(w) {
                *slot = false;
            }
            if let Some(slot) = self.component.get_mut(w) {
                *slot = self.next_component;
            }
            if w == v {
                break;
            }
        }
        self.next_component += 1;
    }
}
