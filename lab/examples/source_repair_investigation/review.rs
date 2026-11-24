//! A replacement lead and a permission to teach have different lifetimes.
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::curriculum::{
    provenance::{Catalog, Claim, Decision, Lineage, Purpose, Release, Standing, Support, Use},
    Document, OutcomeId, Span,
};

/// One syllabus entry cites two copies of the same account. One more lead has
/// independent editorial provenance but is licensed only for classroom teaching.
pub fn demonstrate(
    original: Document,
    replacement: Document,
    separate_days: &[u32],
    shared_days: &[u32],
) {
    let excerpt = |document| Span {
        document,
        start: 0,
        end: 1,
    };
    let a = excerpt(original);
    let b = excerpt(replacement);
    let claim = Claim {
        course: 1,
        outcome: OutcomeId(2),
        criterion: 1,
        edition: original,
        convention: 1,
    };
    let teaching = BTreeMap::from([((7, Purpose::Instruction), (0, 5))]);
    let source = |root| Support {
        claim,
        lineage: Lineage::Original(root),
        grants: teaching.clone(),
        standing: Standing::Current,
    };
    let mut catalog = Catalog {
        generation: 1,
        spans: BTreeMap::from([(a, source(1)), (b, source(1))]),
    };
    let release = Release {
        revision: 1,
        claim,
        candidates: [a, b].into(),
        required: BTreeSet::from([b]),
        minimum_lineages: 2,
    };
    let request = Use {
        revision: 1,
        claim,
        audience: 7,
        purpose: Purpose::Instruction,
        day: 4,
    };
    let verdict = |catalog: &Catalog, request: Use| {
        matches!(
            catalog.inspect(&release, request, 32).decision,
            Decision::Approved(_)
        )
    };
    let mirrored = verdict(&catalog, request);
    catalog.spans.insert(b, source(2));
    catalog.generation += 1;
    let independent = verdict(&catalog, request);
    let assessment = verdict(
        &catalog,
        Use {
            purpose: Purpose::Assessment,
            ..request
        },
    );
    let home = verdict(
        &catalog,
        Use {
            audience: 8,
            ..request
        },
    );
    let late = verdict(&catalog, Use { day: 5, ..request });
    println!("candidate use: shared origin {mirrored}, independent classroom teaching {independent}, independent assessment {assessment}, independent home {home}, after grant expiry {late}");
    assert_eq!(
        [mirrored, independent, assessment, home, late],
        [false, true, false, false, false]
    );
    // All six requests can inspect the same collection. The four classroom
    // teaching requests, assessment request and home request have different uses.
    let evaluate = |days: &[u32]| {
        days.iter()
            .enumerate()
            .map(|(seat, &day)| {
                let audience = if seat == 5 { 8 } else { 7 };
                let purpose = if seat == 4 {
                    Purpose::Assessment
                } else {
                    Purpose::Instruction
                };
                verdict(
                    &catalog,
                    Use {
                        day,
                        audience,
                        purpose,
                        ..request
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let separately = evaluate(separate_days);
    let shared = evaluate(shared_days);
    let copied = vec![shared[0]; shared.len()];
    let mistaken_grants = copied
        .iter()
        .zip(&shared)
        .filter(|(assumed, actual)| **assumed && !**actual)
        .count();
    println!("per-seat discovery: ready days {separate_days:?}, approved {separately:?}");
    println!("shared discovery: ready days {shared_days:?}, approved {shared:?}; copying first permission yields {mistaken_grants} false grants");
    assert_eq!(separately, [true, false, false, false, false, false]);
    assert_eq!(shared, [true, true, true, true, false, false]);
}
