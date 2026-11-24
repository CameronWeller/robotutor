//! Compare the cost of detecting an edit with the cost of running the search again.
use super::Frontier;
use crate::document;
use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::time::Instant;
use tutor_lab::curriculum::{crawl, CrawlBudget};

#[derive(Clone, Copy)]
enum Edit {
    AddCitation(u8, u8),
    Grant(u8),
    Revoke(u8),
}

fn observe(label: &str, edit: Edit, already_spent: usize, unrelated_rows: u8) {
    let root = document(0);
    // Leave document 3 before the cursor out of the old list.
    let mut old_index =
        BTreeMap::from([(root, (1..=14).filter(|i| *i != 3).map(document).collect())]);
    for row in 0..unrelated_rows {
        old_index.insert(document(row + 31), [document(210)].into());
    }
    let old_grants: BTreeSet<_> = [0, 1, 3, 14].into_iter().map(document).collect();
    let mut spent = Frontier::start(root, &old_index, &old_grants, 8, 2);
    assert!(!spent.advance(already_spent));
    let mut index = old_index.clone();
    let mut grants = old_grants.clone();
    // This mutation is the only change between the two snapshots. A writer committing
    // the mutation can identify its changed keys without finding them by a global diff.
    let mut grant_edited = false;
    let mut changed_row = None;
    match edit {
        Edit::AddCitation(a, b) => {
            let a = document(a);
            if index.entry(a).or_default().insert(document(b)) {
                changed_row = Some(a);
            }
        }
        Edit::Grant(d) => grant_edited = grants.insert(document(d)),
        Edit::Revoke(d) => grant_edited = grants.remove(&document(d)),
    }
    let reference = crawl(
        root,
        &index,
        &grants,
        CrawlBudget {
            max_nodes: 8,
            max_depth: 2,
            max_edges: usize::MAX,
        },
    );
    let desired: BTreeSet<_> = reference.documents.into_iter().collect();
    let (checked, cost) = spent.clone().rebase(&index, &grants);
    let from_delta = |cursor: Frontier<'_>| {
        let changed =
            grant_edited || changed_row.is_some_and(|row| cursor.discovered.contains(&row));
        cursor.on_snapshot(&index, &grants, changed)
    };
    let affected_row_keys = usize::from(changed_row.is_some() && !grant_edited);
    let global_index_links: usize = index.values().map(BTreeSet::len).sum();
    println!("{label}, saved {} edges; snapshot comparison {} grant ids, {} adjacency list pairs, {} citation ids; restarted={}; publisher edit: {} grant changes and {} changed row lookups; full index contains {} links",
        spent.edges,cost.grant_keys,cost.adjacencies,cost.citation_keys,cost.restarted,usize::from(grant_edited),affected_row_keys,global_index_links);
    for (name, mut cursor) in [
        (
            "unchecked",
            spent.clone().on_snapshot(&index, &grants, false),
        ),
        ("restart", spent.clone().on_snapshot(&index, &grants, true)),
        ("scoped comparison", checked),
        ("publisher delta", from_delta(spent.clone())),
    ] {
        let prior = cursor.edges;
        while !cursor.advance(4) {}
        let omitted: Vec<_> = desired
            .difference(&cursor.discovered)
            .map(|d| d[0])
            .collect();
        let stale: Vec<_> = cursor
            .discovered
            .difference(&desired)
            .map(|d| d[0])
            .collect();
        println!(
            "  {name}: {} new edge inspections, missing {omitted:?}, stale {stale:?}",
            cursor.edges - prior
        );
        if name != "unchecked" {
            assert_eq!(
                (cursor.discovered, cursor.limited, cursor.edges),
                (
                    desired.clone(),
                    reference.limited,
                    reference.edges_inspected
                )
            );
        }
    }
    if std::env::args().any(|arg| arg == "--timing") {
        let restart = time_recovery(&spent, |c| c.on_snapshot(&index, &grants, true));
        let compare = time_recovery(&spent, |c| c.rebase(&index, &grants).0);
        let delta = time_recovery(&spent, from_delta);
        println!("  warm median ns per recovery and remaining crawl: restart {restart}, scoped comparison {compare}, publisher delta {delta}");
    }
}

/// Prebuild each batch of saved states. A real rebase consumes one owned cursor.
fn time_recovery<'old, 'new>(
    original: &Frontier<'old>,
    mut recover: impl FnMut(Frontier<'old>) -> Frontier<'new>,
) -> u128 {
    const BATCH: usize = 512;
    let mut timings = [0; 7];
    for timing in &mut timings {
        let saved = vec![original.clone(); BATCH];
        let start = Instant::now();
        for state in saved {
            let mut current = recover(black_box(state));
            while !current.advance(4) {}
            black_box(current);
        }
        *timing = start.elapsed().as_nanos() / BATCH as u128;
    }
    timings.sort_unstable();
    timings[timings.len() / 2]
}

pub fn demonstrate_changes() {
    observe(
        "new eligible citation before cursor",
        Edit::AddCitation(0, 3),
        4,
        200,
    );
    observe(
        "previously denied citation now permitted",
        Edit::Grant(2),
        4,
        200,
    );
    observe("discovered candidate revoked", Edit::Revoke(1), 4, 200);
    observe("seed revoked", Edit::Revoke(0), 4, 200);
    observe(
        "unrelated source edited before work",
        Edit::AddCitation(41, 201),
        0,
        200,
    );
    observe(
        "unrelated source edited early",
        Edit::AddCitation(41, 201),
        4,
        200,
    );
    observe(
        "unrelated source edited late",
        Edit::AddCitation(41, 201),
        12,
        200,
    );
    observe(
        "new grant for an unreachable document",
        Edit::Grant(201),
        12,
        200,
    );
    observe(
        "new citation after cursor",
        Edit::AddCitation(0, 17),
        12,
        200,
    );
}
