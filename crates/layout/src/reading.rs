//! Semantic reading order with explicit block precedence (01, 33).
//! The document is an explicit query input because the frozen snapshot does
//! not store semantic ranks. Pass the same revision used to create it.
use crate::{Diagnostic, LayoutSnapshot, LineRef, RelationStatus, Resolution, Subject};
use reprise_diag::{Code, Severity};
use reprise_doc::{Document, NodeId};
use std::collections::{BTreeMap, BTreeSet};

pub const READING_CYCLE: Code = Code::new("layout.reading-cycle");
pub const READING_CONFLICT: Code = Code::new("layout.reading-conflict");
pub const READING_MISSING: Code = Code::new("layout.reading-missing");
pub const READING_PARTIAL: Code = Code::new("layout.reading-partial");
pub const READING_LIMIT: Code = Code::new("layout.reading-limit");
pub const READING_REVISION: Code = Code::new("layout.reading-revision");
pub const MAX_READING_EDGES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingStep {
    pub line: LineRef,
    pub page: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingOrder {
    pub steps: Vec<ReadingStep>,
    pub diagnostics: Vec<Diagnostic>,
}

impl LayoutSnapshot {
    /// A total order of all placed lines. Blocks follow the semantic tree;
    /// lines stay in source order inside each block. Overrides constrain
    /// whole blocks. Stable Kahn sorting fills partial orders semantically;
    /// cycles break at the first semantic member of an actual cycle.
    pub fn reading_order(&self, doc: &Document) -> Vec<ReadingStep> {
        self.reading_order_report(doc).steps
    }

    pub fn reading_order_report(&self, doc: &Document) -> ReadingOrder {
        let mut diagnostics = Vec::new();
        let mut report = |severity, code, subject, message: &str| {
            diagnostics.push(Diagnostic::new(severity, code, subject, message));
        };
        let mut nodes: Vec<NodeId> = self.blocks.iter().map(|b| b.node).collect();
        if self.revision == doc.revision() {
            let mut ranks = BTreeMap::new();
            let mut stack = doc.blocks();
            stack.reverse();
            while let Some(node) = stack.pop() {
                if ranks.contains_key(&node) {
                    continue;
                }
                ranks.insert(node, ranks.len());
                stack.extend(doc.children(Some(node)).into_iter().rev());
            }
            nodes.sort_by_key(|n| (ranks.get(n).copied().unwrap_or(usize::MAX), *n));
        } else {
            report(
                Severity::Warning,
                READING_REVISION,
                Subject::Document,
                "reading query document differs from snapshot; placed flow order is used",
            );
        }
        let ranks: BTreeMap<_, _> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let mut edges = BTreeSet::new();
        let mut overrides = false;
        let mut limited = false;
        for rel in self
            .relations
            .iter()
            .filter(|r| r.schema == reprise_doc::reading::READING_ORDER)
        {
            overrides = true;
            let endpoint = |role: &str| {
                rel.targets
                    .iter()
                    .find(|t| t.role == role)
                    .filter(|t| matches!(t.status, RelationStatus::Valid | RelationStatus::Rebound))
                    .and_then(|t| match t.resolved {
                        Some(Resolution::Node(n)) => ranks.get(&n).copied(),
                        _ => None,
                    })
            };
            let (Some(a), Some(b)) = (endpoint("before"), endpoint("after")) else {
                report(
                    Severity::Warning,
                    READING_MISSING,
                    Subject::Relation(rel.id),
                    "reading precedence has a missing or unplaced endpoint; ignored",
                );
                continue;
            };
            if edges.contains(&(a, b)) {
                continue;
            }
            if edges.len() >= MAX_READING_EDGES {
                limited = true;
                continue;
            }
            if edges.contains(&(b, a)) {
                report(
                    Severity::Warning,
                    READING_CONFLICT,
                    Subject::Relation(rel.id),
                    "opposite reading constraints conflict; cycle repair determines the order",
                );
            }
            edges.insert((a, b));
        }
        if limited {
            report(
                Severity::Warning,
                READING_LIMIT,
                Subject::Document,
                "reading order exceeds 4096 edges; excess constraints are ignored",
            );
        }
        let mut successors = vec![BTreeSet::new(); nodes.len()];
        let mut indegree = vec![0usize; nodes.len()];
        for &(a, b) in &edges {
            if let Some(children) = successors.get_mut(a) {
                children.insert(b);
            }
            if let Some(n) = indegree.get_mut(b) {
                *n = n.saturating_add(1);
            }
        }
        let mut ready: BTreeSet<usize> = indegree
            .iter()
            .enumerate()
            .filter_map(|(i, &n)| (n == 0).then_some(i))
            .collect();
        let mut remaining: BTreeSet<usize> = (0..nodes.len()).collect();
        let mut ordered = Vec::new();
        let mut partial = false;
        while !remaining.is_empty() {
            partial |= ready.len() > 1;
            let next = match ready.pop_first() {
                Some(i) => i,
                None => {
                    let i = cycle_member(&successors, &remaining);
                    report(
                        Severity::Warning,
                        READING_CYCLE,
                        nodes
                            .get(i)
                            .copied()
                            .map_or(Subject::Document, Subject::Node),
                        "reading cycle broken at its first semantic member",
                    );
                    i
                }
            };
            remaining.remove(&next);
            ordered.push(next);
            for &b in successors.get(next).into_iter().flatten() {
                if let Some(n) = indegree.get_mut(b) {
                    *n = n.saturating_sub(1);
                    if *n == 0 && remaining.contains(&b) {
                        ready.insert(b);
                    }
                }
            }
        }
        if overrides && partial {
            report(
                Severity::Info,
                READING_PARTIAL,
                Subject::Document,
                "partial reading order completed in semantic order",
            );
        }
        let steps = ordered
            .into_iter()
            .filter_map(|i| nodes.get(i).and_then(|&node| self.block(node)))
            .flat_map(|b| {
                b.lines.iter().enumerate().filter_map(|(line, l)| {
                    self.frame(l.frame).map(|f| ReadingStep {
                        line: LineRef { node: b.node, line },
                        page: f.page,
                    })
                })
            })
            .collect();
        ReadingOrder { steps, diagnostics }
    }
}

// Iterative DFS: every node and edge is visited at most once. No recursion
// or arbitrary retry budget, even for long cycles. Downstream blocked nodes
// are never selected as a cycle break merely because their rank is smaller.
fn cycle_member(edges: &[BTreeSet<usize>], remaining: &BTreeSet<usize>) -> usize {
    let mut state = vec![0u8; edges.len()];
    for &root in remaining {
        if state.get(root).copied() != Some(0) {
            continue;
        }
        if let Some(s) = state.get_mut(root) {
            *s = 1;
        }
        let Some(children) = edges.get(root) else {
            continue;
        };
        let mut stack = vec![(root, children.iter())];
        while let Some((node, children)) = stack.last_mut() {
            if let Some(&child) = children.next() {
                if !remaining.contains(&child) {
                    continue;
                }
                if state.get(child).copied() == Some(1) {
                    return stack
                        .iter()
                        .skip_while(|(n, _)| *n != child)
                        .map(|(n, _)| *n)
                        .min()
                        .unwrap_or(child);
                }
                if state.get(child).copied() == Some(0) {
                    if let Some(s) = state.get_mut(child) {
                        *s = 1;
                    }
                    if let Some(children) = edges.get(child) {
                        stack.push((child, children.iter()));
                    }
                }
            } else {
                if let Some(s) = state.get_mut(*node) {
                    *s = 2;
                }
                stack.pop();
            }
        }
    }
    remaining.first().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deep_cycle_walk_uses_no_recursion_and_ignores_blocked_downstream_nodes() {
        let count = MAX_READING_EDGES;
        let mut edges = vec![BTreeSet::new(); count];
        for (i, children) in edges.iter_mut().enumerate().skip(1) {
            children.insert(if i + 1 == count { 1 } else { i + 1 });
        }
        edges[1].insert(0);
        assert_eq!(cycle_member(&edges, &(0..count).collect()), 1);
        assert_eq!(cycle_member(&[], &BTreeSet::new()), 0);
    }
}
