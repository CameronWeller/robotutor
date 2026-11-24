use std::collections::BTreeSet;
use tutor_lab::curriculum::{provenance::*, OutcomeId, Span};
use tutor_lab::material::document_id;

const A: &str = "Course handout: 2.345 rounds to 2.34 at two places under ties-to-even.";
const B: &str =
    "Instructor worksheet: an exact tie keeps the adjacent even hundredth, giving 2.34.";
const X: &str = "Reference derivation: between 2.34 and 2.35 the even hundredth is 2.34.";
const Y: &str = "Independent solution: 234.5 cents ties between 234 and 235. Select 234 cents.";
const Z: &str = "Ties away from zero gives 235 cents for 234.5 cents.";
fn span(text: &str) -> Span {
    Span {
        document: document_id(text.as_bytes()),
        start: 0,
        end: text.len() as u32,
    }
}

fn node(claim: Claim, lineage: Lineage) -> Support {
    Support {
        claim,
        lineage,
        grants: [((7, Purpose::Assessment), (10, 90))].into(),
        standing: Standing::Current,
    }
}
fn derived(text: &[&str]) -> Lineage {
    Lineage::Derived(text.iter().map(|s| span(s)).collect())
}
fn fixture() -> (Release, Use, Catalog) {
    let claim = Claim {
        course: 6,
        outcome: OutcomeId(8),
        criterion: 1,
        edition: document_id(b"2019 course reference"),
        convention: 1,
    };
    let release = Release {
        revision: 4,
        claim,
        candidates: [span(A), span(B)].into(),
        required: BTreeSet::new(),
        minimum_lineages: 2,
    };
    let request = Use {
        revision: 4,
        claim,
        audience: 7,
        purpose: Purpose::Assessment,
        day: 30,
    };
    let catalog = Catalog {
        generation: 9,
        spans: [
            (span(A), node(claim, derived(&[X]))),
            (span(B), node(claim, derived(&[Y]))),
            (span(X), node(claim, Lineage::Original(11))),
            (span(Y), node(claim, Lineage::Original(12))),
            (
                span(Z),
                node(
                    Claim {
                        convention: 2,
                        ..claim
                    },
                    Lineage::Original(13),
                ),
            ),
        ]
        .into(),
    };
    (release, request, catalog)
}

#[test]
fn two_citations_can_lose_their_independence_without_changing_historical_work() {
    let (release, request, mut catalog) = fixture();
    let prior = catalog.inspect(&release, request, 100);
    let Decision::Approved(selected) = &prior.decision else {
        panic!("fixture needs two independent witnesses")
    };
    assert_eq!(
        selected.iter().copied().collect::<BTreeSet<_>>(),
        release.candidates
    );
    let for_b = prior.traced.iter().find(|w| w.citation == span(B)).unwrap();
    assert_eq!(for_b.dependencies, [span(B), span(Y)].into());
    assert_eq!(for_b.origins, [12].into());
    // New editorial finding: the independent-looking cents example used X as its underlying source.
    catalog.spans.get_mut(&span(Y)).unwrap().lineage = derived(&[X]);
    catalog.generation += 1;
    let current = catalog.inspect(&release, request, 100);
    assert_eq!(current.decision, Decision::Insufficient);
    assert!(current.traced.iter().all(|w| w.origins == [11].into()));
    assert_eq!(current.generation, 10);
    // The receipt bound to the learner's original answer remains a historical artifact.
    let original = ("2.34", prior.generation, prior.decision);
    assert!(matches!(original, ("2.34", 9, Decision::Approved(_))));
}

#[test]
fn an_underlying_withdrawal_and_an_uncertain_erratum_have_distinct_effective_dates() {
    let (release, request, mut catalog) = fixture();
    catalog.spans.get_mut(&span(X)).unwrap().standing = Standing::HeldFrom(31);
    assert!(matches!(
        catalog.inspect(&release, request, 100).decision,
        Decision::Approved(_)
    ));
    let later = Use { day: 31, ..request };
    let held = catalog.inspect(&release, later, 100);
    assert_eq!(held.excluded.get(&span(A)), Some(&Issue::UnderReview));
    assert_eq!(held.decision, Decision::Withheld(Issue::UnderReview));
    catalog.generation += 1;
    catalog.spans.get_mut(&span(X)).unwrap().standing = Standing::WithdrawnFrom(31);
    let withdrawn = catalog.inspect(&release, later, 100);
    assert_eq!(withdrawn.excluded.get(&span(A)), Some(&Issue::Withdrawn));
    assert_eq!(withdrawn.decision, Decision::Insufficient);
}

#[test]
fn scope_rights_and_convention_are_checked_on_every_relevant_dependency() {
    let (mut release, request, mut catalog) = fixture();
    release.candidates = [span(A), span(Z)].into();
    let mismatched = catalog.inspect(&release, request, 100);
    assert_eq!(mismatched.decision, Decision::Insufficient);
    assert_eq!(
        mismatched.excluded.get(&span(Z)),
        Some(&Issue::Inapplicable)
    );
    let wrong_edition = Use {
        claim: Claim {
            edition: document_id(b"2026 edition"),
            ..request.claim
        },
        ..request
    };
    assert_eq!(
        catalog.inspect(&release, wrong_edition, 100).decision,
        Decision::Withheld(Issue::WrongScope)
    );
    release.candidates = [span(A)].into();
    release.minimum_lineages = 1;
    catalog
        .spans
        .get_mut(&span(X))
        .unwrap()
        .grants
        .insert((7, Purpose::Assessment), (10, 30));
    for denied in [
        request,
        Use { day: 9, ..request },
        Use {
            audience: 8,
            ..request
        },
        Use {
            purpose: Purpose::Instruction,
            ..request
        },
    ] {
        assert_eq!(
            catalog
                .inspect(&release, denied, 100)
                .excluded
                .get(&span(A)),
            Some(&Issue::NoGrant)
        );
    }
    assert!(matches!(
        catalog
            .inspect(&release, Use { day: 10, ..request }, 100)
            .decision,
        Decision::Approved(_)
    ));
}

#[test]
fn unresolved_lineage_cycles_and_traversal_exhaustion_are_never_independent_evidence() {
    let (release, request, mut catalog) = fixture();
    for (lineage, issue) in [
        (derived(&[A]), Issue::Cycle),
        (Lineage::Unknown, Issue::UnknownOrigin),
        (derived(&["unavailable primary source"]), Issue::Missing),
    ] {
        catalog.spans.get_mut(&span(X)).unwrap().lineage = lineage;
        let receipt = catalog.inspect(&release, request, 100);
        assert_eq!(receipt.decision, Decision::Withheld(issue));
    }
    let stopped = catalog.inspect(&release, request, 1);
    assert_eq!(stopped.steps, 1);
    assert_eq!(stopped.decision, Decision::Withheld(Issue::Budget));
}

#[test]
fn separate_observations_require_disjoint_dependencies_not_a_union_count() {
    let (mut release, request, mut catalog) = fixture();
    catalog.spans.get_mut(&span(A)).unwrap().lineage = derived(&[X, Y]);
    catalog.spans.get_mut(&span(B)).unwrap().lineage = derived(&[X, Y]);
    let merged = catalog.inspect(&release, request, 100);
    assert_eq!(merged.decision, Decision::Insufficient);
    assert!(merged.traced.iter().all(|w| w.origins == [11, 12].into()));
    release.candidates.extend([span(X), span(Y)]);
    let satisfied = catalog.inspect(&release, request, 100);
    let Decision::Approved(selected) = satisfied.decision else {
        panic!("two separate primary sources exist")
    };
    assert_eq!(
        selected.into_iter().collect::<BTreeSet<_>>(),
        [span(X), span(Y)].into()
    );
}

#[test]
fn an_independent_spare_witness_cannot_replace_withdrawn_wording_in_the_exact_form() {
    let (mut release, request, mut catalog) = fixture();
    release.minimum_lineages = 1;
    catalog.spans.get_mut(&span(X)).unwrap().standing = Standing::WithdrawnFrom(30);
    let spare = catalog.inspect(&release, request, 100);
    assert_eq!(spare.decision, Decision::Approved(vec![span(B)]));
    release.required.insert(span(A));
    let exact = catalog.inspect(&release, request, 100);
    assert_eq!(exact.decision, Decision::Withheld(Issue::Withdrawn));
}
