use std::collections::BTreeSet;
use telo::monitor::Monitor;
use telo::predicate::{ClosurePredicate, Predicates};
use telo::Property;
use tutor_lab::curriculum::creation::{
    source_manifest, Batch, Candidate, Error, Pack, ReviewerGrant, Work, MAX_ROWS,
};
use tutor_lab::curriculum::{OutcomeId, Span};
use tutor_lab::material::document_id;

// An invented classroom charter. Its single rule is the entire domain of the fixture.
const CHARTER: &str = "The council may adopt park rules. A named park board may only give advice.";
fn span() -> Span {
    Span {
        document: document_id(CHARTER.as_bytes()),
        start: 0,
        end: CHARTER.len() as u32,
    }
}
fn candidate(id: u32) -> Candidate {
    Candidate {
        work: Work {
            release: 8,
            objective: OutcomeId(id),
            brief: document_id(format!("charter sources, objective {id}").as_bytes()),
            source_manifest: source_manifest(&[span()].into()),
        },
        author: 1,
        pack: Pack {
            lesson: "Find the actor and the verb that grants its power.".into(),
            worked: "The council can adopt a rule for the park because the charter says ‘may adopt.’".into(),
            practice: "Underline the operative verb for the council.".into(),
            check: "Does a park board recommendation itself adopt a rule? Defend your reading.".into(),
            criterion: "Distinguish advisory authority from the power to adopt a rule.".into(),
            rubric: "Credit identifying the board's advisory role and citing the limit in the charter. Accept equivalent wording.".into(),
            sources: [span()].into(),
        }
    }
}
fn manifest(objectives: &[u32]) -> Batch {
    Batch::new(8, objectives.iter().map(|&id| candidate(id).work)).unwrap()
}

#[test]
fn a_mixed_reply_keeps_usable_rows_and_leaves_missing_or_ambiguous_objectives_unfilled() {
    let mut batch = manifest(&[1, 2, 3, 4, 5, 6, 7]);
    let good = candidate(1);
    let mut empty = candidate(3);
    empty.pack.rubric.clear();
    let mut exposed = candidate(4);
    exposed.pack.worked = format!(
        "Worked example. {} Our answer was no.",
        exposed.pack.check.to_uppercase()
    );
    let mut stale = candidate(5);
    stale.work.release = 9;
    let mut ungrounded = candidate(6);
    ungrounded.pack.sources.clear();
    let result = batch
        .stage(vec![
            candidate(2),
            good.clone(),
            empty,
            candidate(2),
            exposed,
            stale,
            ungrounded,
            candidate(100),
        ])
        .unwrap();
    let digest = result[1].unwrap();
    assert_eq!(
        result,
        [
            Err(Error::DuplicateRow),
            Ok(digest),
            Err(Error::InvalidText),
            Err(Error::DuplicateRow),
            Err(Error::ExposedCheck),
            Err(Error::StaleWork),
            Err(Error::InvalidSources),
            Err(Error::UnknownObjective)
        ]
    );
    let sources = good.pack.sources.clone();
    assert_eq!(batch.available(good.work, &sources), Err(Error::Unapproved));
    for id in 2..=7 {
        assert_eq!(
            batch.available(candidate(id).work, &sources),
            Err(Error::MissingDraft)
        );
    }
    assert_eq!(batch.stage(vec![good.clone()]).unwrap(), [Ok(digest)]);
    let reviewer = ReviewerGrant {
        actor: 9,
        work: good.work,
    };
    batch
        .approve(good.work, digest, &reviewer, &sources)
        .unwrap();
    assert_eq!(batch.available(good.work, &sources), Ok(&good.pack));
    assert_eq!(batch.stage(vec![candidate(2)]).unwrap().len(), 1);
    assert_eq!(
        batch.available(candidate(2).work, &sources),
        Err(Error::Unapproved)
    );
}

#[test]
fn corrected_bytes_resembling_an_old_revision_cannot_reuse_its_signoff() {
    let mut batch = manifest(&[1]);
    let original = candidate(1);
    let work = original.work;
    let sources = original.pack.sources.clone();
    let v1 = batch.stage(vec![original.clone()]).unwrap()[0].unwrap();
    let mut revised = original.clone();
    revised.author = 2;
    revised
        .pack
        .rubric
        .push_str(" Award citation credit separately.");
    let v2 = batch.revise(v1, revised.clone()).unwrap();
    assert_eq!(batch.revise(v1, revised.clone()), Ok(v2));
    assert_eq!(
        batch.stage(vec![original.clone()]).unwrap(),
        [Err(Error::Conflict)]
    );
    let v3 = batch.revise(v2, original.clone()).unwrap();
    assert_ne!(v1, v3);
    let reviewer = ReviewerGrant { actor: 9, work };
    assert_eq!(
        batch.approve(
            work,
            v3,
            &ReviewerGrant {
                actor: revised.author,
                work
            },
            &sources
        ),
        Err(Error::UnauthorizedReviewer),
        "a contributor to a prior draft cannot adjudicate the repaired lineage"
    );
    assert_eq!(
        batch.approve(work, v1, &reviewer, &sources),
        Err(Error::WrongDigest)
    );
    assert_eq!(
        batch.approve(
            work,
            v3,
            &ReviewerGrant {
                actor: original.author,
                work
            },
            &sources
        ),
        Err(Error::UnauthorizedReviewer)
    );
    assert_eq!(
        batch.approve(
            work,
            v3,
            &ReviewerGrant {
                actor: 9,
                work: candidate(2).work
            },
            &sources
        ),
        Err(Error::UnauthorizedReviewer)
    );
    assert_eq!(
        batch.approve(work, v3, &reviewer, &BTreeSet::new()),
        Err(Error::SourceUnavailable)
    );
    batch.approve(work, v3, &reviewer, &sources).unwrap();
    batch.approve(work, v3, &reviewer, &sources).unwrap();
    assert_eq!(batch.revise(v3, revised), Err(Error::Conflict));
    assert_eq!(
        batch.available(work, &BTreeSet::new()),
        Err(Error::SourceUnavailable)
    );
    assert_eq!(
        batch.available(work, &[Span { start: 1, ..span() }].into()),
        Err(Error::SourceUnavailable)
    );
    assert_eq!(
        batch.available(
            Work {
                brief: document_id(b"different jurisdiction"),
                ..work
            },
            &sources
        ),
        Err(Error::StaleWork)
    );
    batch.withdraw(work, v3, &reviewer).unwrap();
    assert_eq!(
        batch.approve(work, v3, &reviewer, &sources),
        Err(Error::Withdrawn)
    );
    assert_eq!(batch.available(work, &sources), Err(Error::Withdrawn));
}

#[test]
fn oversized_replies_do_no_partial_work_but_one_oversized_artifact_is_local() {
    assert!(matches!(Batch::new(1, []), Err(Error::BatchSize)));
    let mut batch = manifest(&[1, 2]);
    let good = candidate(1);
    assert_eq!(
        batch.stage(vec![good.clone(); MAX_ROWS + 1]),
        Err(Error::BatchSize)
    );
    assert_eq!(
        batch.available(good.work, &good.pack.sources),
        Err(Error::MissingDraft)
    );
    let mut large = candidate(2);
    large.pack.lesson = "x".repeat(4097);
    let result = batch.stage(vec![large, good]).unwrap();
    assert_eq!(result[0], Err(Error::InvalidText));
    assert!(result[1].is_ok());
}

#[test]
fn a_licensed_span_from_another_brief_cannot_be_substituted_for_the_commissioned_one() {
    let original = candidate(1);
    let work = original.work;
    let other = Span {
        document: document_id(b"Another authorized charter."),
        start: 0,
        end: 10,
    };
    let rights = [span(), other].into();
    let mut wrong = original.clone();
    wrong.pack.sources = [other].into();
    let mut batch = manifest(&[1]);
    assert_eq!(
        batch.stage(vec![wrong.clone()]).unwrap(),
        [Err(Error::InvalidSources)]
    );
    wrong.work.source_manifest = source_manifest(&wrong.pack.sources);
    assert_eq!(
        batch.stage(vec![wrong.clone()]).unwrap(),
        [Err(Error::StaleWork)]
    );
    let receipt = batch.stage(vec![original]).unwrap()[0].unwrap();
    wrong.work = work;
    assert_eq!(batch.revise(receipt, wrong), Err(Error::InvalidSources));
    let reviewer = ReviewerGrant { actor: 9, work };
    batch.approve(work, receipt, &reviewer, &rights).unwrap();
    assert_eq!(
        batch.available(work, &rights).unwrap().sources,
        [span()].into()
    );
    assert!(matches!(
        Batch::new(8, [work, work]),
        Err(Error::DuplicateRow)
    ));
}

#[derive(Clone, Copy, Debug)]
enum Event {
    Stage,
    Approve,
    RevokeSource,
    ReinstateSource,
    HoldPack,
}

#[test]
fn temporal_release_requires_independent_approval_intact_sources_and_a_live_pack() {
    let alphabet = [
        Event::Stage,
        Event::Approve,
        Event::RevokeSource,
        Event::ReinstateSource,
        Event::HoldPack,
    ];
    // Each frame records whether the real catalog and an independent event-history oracle agree
    // on availability. G agreement checks both release and withholding on the observed prefixes.
    // Eventual review in a staffed service is a separate liveness claim.
    let make_predicates = || {
        let mut symbols = Predicates::builder();
        let agrees = symbols.new_predicate(ClosurePredicate::new(
            |agreement: &bool| *agreement,
            "catalog agrees with observed approval, rights and holds",
        ));
        (symbols.build(), Property::always(Property::atomic(agrees)))
    };
    let (symbols, rule) = make_predicates();
    let automaton = rule.to_monitoring_automaton(&symbols);
    let row = candidate(1);
    let reviewer = ReviewerGrant {
        actor: 9,
        work: row.work,
    };
    let mut visible = 0;
    for encoded in 0..alphabet.len().pow(4) {
        let mut index = encoded;
        let mut batch = manifest(&[1]);
        let mut monitor = Monitor::new(make_predicates().0, automaton.clone());
        let mut receipt = None;
        let (mut staged, mut approved, mut held, mut rights) = (false, false, false, true);
        for _ in 0..4 {
            let event = alphabet[index % alphabet.len()];
            index /= alphabet.len();
            match event {
                Event::Stage => {
                    receipt = Some(batch.stage(vec![row.clone()]).unwrap()[0].unwrap());
                    staged = true;
                }
                Event::Approve => {
                    if staged && rights && !held {
                        approved = true;
                    }
                    let current = if rights {
                        row.pack.sources.clone()
                    } else {
                        BTreeSet::new()
                    };
                    let _ =
                        batch.approve(row.work, receipt.unwrap_or([0; 32]), &reviewer, &current);
                }
                Event::RevokeSource => rights = false,
                Event::ReinstateSource => rights = true,
                Event::HoldPack => {
                    if staged {
                        held = true;
                    }
                    let _ = batch.withdraw(row.work, receipt.unwrap_or([0; 32]), &reviewer);
                }
            }
            let current = if rights {
                row.pack.sources.clone()
            } else {
                BTreeSet::new()
            };
            let actual = batch.available(row.work, &current).is_ok();
            assert!(
                monitor.next_state(&(actual == (approved && rights && !held))),
                "schedule {encoded}, event {event:?}"
            );
            visible += usize::from(actual);
        }
    }
    assert!(visible > 0);
}
