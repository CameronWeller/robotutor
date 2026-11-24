//! A bounded traversal that can yield in the middle of one adjacency list.
//! Borrowing the immutable index and grants keeps all slices on the same snapshot.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Bound::{Excluded, Unbounded};
use tutor_lab::curriculum::Document;

#[derive(Clone)]
pub struct Frontier<'a> {
    seed: Document,
    index: &'a BTreeMap<Document, BTreeSet<Document>>,
    grants: &'a BTreeSet<Document>,
    node_limit: usize,
    depth_limit: usize,
    queue: VecDeque<(Document, usize)>,
    after: Option<Document>,
    pub discovered: BTreeSet<Document>,
    pub edges: usize,
    pub limited: bool,
}

impl<'a> Frontier<'a> {
    pub fn start(
        seed: Document,
        index: &'a BTreeMap<Document, BTreeSet<Document>>,
        grants: &'a BTreeSet<Document>,
        node_limit: usize,
        depth_limit: usize,
    ) -> Self {
        let mut frontier = Self {
            seed,
            index,
            grants,
            node_limit,
            depth_limit,
            queue: VecDeque::new(),
            after: None,
            discovered: BTreeSet::new(),
            edges: 0,
            limited: false,
        };
        if grants.contains(&seed) {
            if node_limit == 0 {
                frontier.limited = true;
            } else {
                frontier.discovered.insert(seed);
                frontier.queue.push_back((seed, 0));
            }
        }
        frontier
    }

    /// Reuse progress only when both snapshots certify the same search prefix.
    /// A source outside the discovered set could not have supplied an inspected
    /// outgoing edge. Checking the whole grant set also covers rejected citations.
    pub fn rebase<'b>(
        self,
        index: &'b BTreeMap<Document, BTreeSet<Document>>,
        grants: &'b BTreeSet<Document>,
    ) -> (Frontier<'b>, Comparison) {
        if self.edges == 0 {
            return (
                self.on_snapshot(index, grants, true),
                Comparison {
                    restarted: true,
                    ..Comparison::default()
                },
            );
        }
        let empty = BTreeSet::new();
        let mut compared = Comparison::default();
        let mut changed = !same(self.grants, grants, &mut compared.grant_keys);
        if !changed {
            for doc in &self.discovered {
                compared.adjacencies += 1;
                let before = self.index.get(doc).unwrap_or(&empty);
                let after = index.get(doc).unwrap_or(&empty);
                if !same(before, after, &mut compared.citation_keys) {
                    changed = true;
                    break;
                }
            }
        }
        compared.restarted = changed;
        (self.on_snapshot(index, grants, changed), compared)
    }

    fn on_snapshot<'b>(
        self,
        index: &'b BTreeMap<Document, BTreeSet<Document>>,
        grants: &'b BTreeSet<Document>,
        restart: bool,
    ) -> Frontier<'b> {
        if restart {
            return Frontier::start(self.seed, index, grants, self.node_limit, self.depth_limit);
        }
        Frontier {
            seed: self.seed,
            index,
            grants,
            node_limit: self.node_limit,
            depth_limit: self.depth_limit,
            queue: self.queue,
            after: self.after,
            discovered: self.discovered,
            edges: self.edges,
            limited: self.limited,
        }
    }

    /// Return true when this snapshot has no more work under its node and depth limits.
    pub fn advance(&mut self, mut edge_budget: usize) -> bool {
        while let Some(&(doc, depth)) = self.queue.front() {
            let neighbors = self.index.get(&doc);
            let lower = self.after.map_or(Unbounded, Excluded);
            if depth == self.depth_limit {
                self.limited |= neighbors.is_some_and(|n| !n.is_empty());
            } else if let Some(next) = neighbors
                .and_then(|n| n.range((lower, Unbounded)).next())
                .copied()
            {
                if edge_budget == 0 {
                    return false;
                }
                self.after = Some(next);
                self.edges += 1;
                edge_budget -= 1;
                if self.grants.contains(&next) && !self.discovered.contains(&next) {
                    if self.discovered.len() == self.node_limit {
                        self.limited = true;
                        self.queue.clear();
                        break;
                    }
                    self.discovered.insert(next);
                    self.queue.push_back((next, depth + 1));
                }
                continue;
            }
            self.queue.pop_front();
            self.after = None;
        }
        true
    }
}

#[derive(Default, Debug)]
pub struct Comparison {
    pub restarted: bool,
    pub grant_keys: usize,
    pub adjacencies: usize,
    pub citation_keys: usize,
}

fn same(a: &BTreeSet<Document>, b: &BTreeSet<Document>, inspected: &mut usize) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(left, right)| {
            *inspected += 1;
            left == right
        })
}

#[path = "changes.rs"]
mod changes;
pub use changes::demonstrate_changes;
