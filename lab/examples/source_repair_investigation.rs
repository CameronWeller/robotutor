//! Rehearse interrupted discovery on a pinned classroom source index.
#[path = "source_repair_investigation/frontier.rs"]
mod frontier;
#[path = "source_repair_investigation/review.rs"]
mod review;

use frontier::Frontier;
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::curriculum::{crawl, CrawlBudget, Document};

fn document(index: u8) -> Document {
    [index; 32]
}

fn main() {
    let seed = document(0);
    let replacement = document(13);
    let edges = BTreeMap::from([(seed, (1..=13).map(document).collect())]);
    let allowed = BTreeSet::from([seed, replacement]);
    for allowance in [4, 8, 12, 16] {
        let found = crawl(
            seed,
            &edges,
            &allowed,
            CrawlBudget {
                max_nodes: 8,
                max_edges: allowance,
                max_depth: 2,
            },
        );
        println!(
            "edge allowance {allowance}, inspected {}, replacement {}, limited {}",
            found.edges_inspected,
            found.documents.contains(&replacement),
            found.limited
        );
    }
    let mut frontier = Frontier::start(seed, &edges, &allowed, 8, 2);
    for meeting in 1..=4 {
        let finished = frontier.advance(4);
        println!(
            "meeting {meeting}, cumulative edges {}, replacement {}, finished {finished}",
            frontier.edges,
            frontier.discovered.contains(&replacement)
        );
    }
    let arrivals = |jobs| {
        let mut spent: usize = 0;
        (0..jobs)
            .map(|_| {
                let mut cursor = Frontier::start(seed, &edges, &allowed, 8, 2);
                while !cursor.advance(1) {}
                assert!(cursor.discovered.contains(&replacement));
                spent += cursor.edges;
                u32::try_from(spent.div_ceil(4)).expect("fixture day fits in u32")
            })
            .collect::<Vec<_>>()
    };
    let separate = arrivals(6);
    let shared = vec![arrivals(1)[0]; separate.len()];
    review::demonstrate(seed, replacement, &separate, &shared);
    frontier::demonstrate_changes();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_with_edges(mask: u16, docs: &[Document; 3]) -> BTreeMap<Document, BTreeSet<Document>> {
        docs.iter()
            .enumerate()
            .map(|(i, &source)| {
                let adjacent = docs
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| mask & (1 << (3 * i + j)) != 0)
                    .map(|(_, &target)| target)
                    .collect();
                (source, adjacent)
            })
            .collect()
    }

    #[test]
    fn yielding_inside_an_adjacency_matches_a_single_bounded_crawl() {
        let docs = [document(0), document(1), document(2)];
        for graph in 0..512 {
            let edges = graph_with_edges(graph, &docs);
            for allowed in [
                BTreeSet::from(docs),
                BTreeSet::from([docs[0], docs[2]]),
                BTreeSet::from([docs[2]]),
            ] {
                for depth in 0..3 {
                    for cap in 0..4 {
                        let reference = crawl(
                            docs[0],
                            &edges,
                            &allowed,
                            CrawlBudget {
                                max_nodes: cap,
                                max_depth: depth,
                                max_edges: usize::MAX,
                            },
                        );
                        let mut cursor = Frontier::start(docs[0], &edges, &allowed, cap, depth);
                        // A zero-work turn can consume empty adjacency lists but never a link.
                        cursor.advance(0);
                        while !cursor.advance(1) {
                            assert!(cursor.edges <= 9);
                        }
                        assert_eq!(cursor.discovered, reference.documents.into_iter().collect());
                        assert_eq!(cursor.edges, reference.edges_inspected);
                        assert_eq!(cursor.limited, reference.limited);
                    }
                }
            }
        }
    }

    #[test]
    fn resuming_after_every_single_edit_matches_the_current_reference() {
        let docs = [document(0), document(1), document(2)];
        for graph in 0..512 {
            let index = graph_with_edges(graph, &docs);
            for old_grants in [
                BTreeSet::from(docs),
                [docs[0], docs[2]].into(),
                [docs[2]].into(),
            ] {
                for edit in 0..12 {
                    let mut grants = old_grants.clone();
                    let mut edited_index = index.clone();
                    if edit < 9 {
                        let row = edited_index.get_mut(&docs[edit / 3]).unwrap();
                        let target = docs[edit % 3];
                        if !row.remove(&target) {
                            row.insert(target);
                        }
                    } else {
                        let target = docs[edit - 9];
                        if !grants.remove(&target) {
                            grants.insert(target);
                        }
                    }
                    let reference = crawl(
                        docs[0],
                        &edited_index,
                        &grants,
                        CrawlBudget {
                            max_nodes: 2,
                            max_depth: 2,
                            max_edges: usize::MAX,
                        },
                    );
                    let desired: BTreeSet<_> = reference.documents.into_iter().collect();
                    for budget in [0, 1, 2, 9] {
                        let mut saved = Frontier::start(docs[0], &index, &old_grants, 2, 2);
                        saved.advance(budget);
                        let (mut resumed, _) = saved.rebase(&edited_index, &grants);
                        while !resumed.advance(1) {}
                        assert_eq!(
                            (&resumed.discovered, resumed.edges, resumed.limited),
                            (&desired, reference.edges_inspected, reference.limited),
                            "graph={graph} edit={edit} budget={budget}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn equal_cardinality_edits_are_detected_in_the_inspected_prefix() {
        let [root, a, b, c, d] = [0, 1, 2, 3, 4].map(document);
        let index = BTreeMap::from([(root, [a, c, d].into())]);
        let edited = BTreeMap::from([(root, [a, b, d].into())]);
        let allow: BTreeSet<_> = [root, b, d].into();
        let switched: BTreeSet<_> = [root, a, d].into();
        let mut old = Frontier::start(root, &index, &allow, 4, 2);
        old.advance(2);
        let (mut graph_change, check) = old.clone().rebase(&edited, &allow);
        assert!(check.restarted);
        assert!(graph_change.advance(3));
        assert_eq!(graph_change.discovered, allow);
        let (mut grant_change, check) = old.rebase(&index, &switched);
        assert!(check.restarted);
        assert!(grant_change.advance(3));
        assert_eq!(grant_change.discovered, switched);
    }
}
