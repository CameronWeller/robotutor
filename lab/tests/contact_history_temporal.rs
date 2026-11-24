//! Reopen the same reviewed performances after a classroom report arrives
//! late. The oracle uses event order alone, independently of the contact map.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    curriculum::{
        goal_evidence::{
            contact_history::{ContactError, ContactHistory, Dispatch, TaskCatalog},
            delayed_on_reviewed_change, Evidence, Presentation, PublishedCase, Spacing, Task,
        },
        program::{Frame, Proposal},
        rehearsal_release::publish,
        Document, OutcomeId,
    },
    history::{Scope, Withdrawn},
    material::document_id,
    protocol::{Assistance, Judgment},
    retention::{Cell, CheckResult, Moment},
};

const CELL: Cell = Cell {
    scope: Scope {
        learner: 51,
        revision: 7,
    },
    outcome: OutcomeId(101),
};
const GOALS: [&str; 5] = [
    include_str!("../fixtures/goals/writer.json"),
    include_str!("../fixtures/goals/violin.json"),
    include_str!("../fixtures/goals/chemical-engineering.json"),
    include_str!("../fixtures/goals/better-person.json"),
    include_str!("../fixtures/goals/worship.json"),
];
const fn at(day: u32, sequence: u64) -> Moment {
    Moment { day, sequence }
}
fn spacing() -> Spacing {
    Spacing {
        stability_days: 3.0,
        threshold: 0.85,
        target_day: Some(365),
    }
}

struct Pair {
    performances: [Evidence; 2],
    openings: [Dispatch; 2],
    aliases: BTreeMap<PublishedCase, Document>,
    comparison: BTreeSet<(PublishedCase, PublishedCase)>,
    projected_alias: Dispatch,
    related_lesson: Dispatch,
}

impl Pair {
    fn new(fixture: &str, return_open: Moment, response: &str) -> Self {
        let goal: Value = serde_json::from_str(fixture).unwrap();
        let frame = Frame::from_json(&goal).unwrap();
        let proposal = Proposal::from_json(&goal["proposal"]).unwrap();
        let make = |name: &str, id, opening, answer, branch| {
            let row = &goal[name];
            let lesson = publish(&frame, &proposal, frame.review_digest(&proposal), row).unwrap();
            let pack = lesson.material(true).unwrap();
            let task = Task::from_pack(
                CELL,
                pack.digest(),
                goal["evidence_mode"].as_str().unwrap(),
                goal["criterion_claim"].as_str().unwrap(),
                row,
            )
            .unwrap()
            .with_published_sources(pack)
            .unwrap();
            let case = task.published_case().unwrap();
            let assessment = task
                .scripted(
                    Presentation {
                        attempt: id,
                        at: answer,
                        fresh: true,
                        assistance: Assistance::NoneReported,
                    },
                    branch,
                    82,
                )
                .unwrap();
            let dispatch = Dispatch {
                cell: CELL,
                at: opening,
                case,
                attempt: Some(id),
            };
            (assessment, dispatch)
        };
        let (first, a) = make("first_pack", 101, at(0, 1), at(0, 2), "demonstrated");
        let (later, b) = make("return_pack", 202, return_open, at(30, 6), response);
        // A second rendering of the return prompt and a different guided task
        // are synthetic classroom releases. Their semantic equivalence is an
        // editorial input independent of the assessment model.
        let variant = PublishedCase {
            pack: document_id(b"large-type return card"),
            ..b.case
        };
        let related = PublishedCase {
            pack: document_id(b"guided worked example"),
            ..b.case
        };
        Self {
            performances: [first, later],
            openings: [a, b],
            aliases: BTreeMap::from([
                (a.case, a.case.pack),
                (b.case, b.case.pack),
                (variant, b.case.pack),
                (related, related.pack),
            ]),
            comparison: BTreeSet::from([(a.case, b.case)]),
            projected_alias: Dispatch {
                at: at(5, 3),
                case: variant,
                attempt: None,
                ..b
            },
            related_lesson: Dispatch {
                at: at(29, 4),
                case: related,
                attempt: None,
                ..b
            },
        }
    }
    fn history(&self) -> ContactHistory {
        let mut history = ContactHistory::new(CELL);
        for event in self.openings {
            history.record(event).unwrap();
        }
        history
    }
    fn read(&self, history: &ContactHistory, complete: bool, holds: &Withdrawn) -> CheckResult {
        history
            .delayed(
                &self.performances[0],
                &self.performances[1],
                complete.then_some(at(30, 6)),
                TaskCatalog {
                    aliases: &self.aliases,
                    approved: &self.comparison,
                },
                holds,
                spacing(),
            )
            .unwrap()
    }
}

// G(ingest -> X(read_matches_prefix_and_original_grade_survives)).
#[derive(Default)]
struct Boundary {
    ingest: bool,
    correct_read: bool,
}
fn monitor() -> Monitor<Boundary> {
    let mut p = Predicates::builder();
    let ingest =
        Property::atomic(p.new_predicate(ClosurePredicate::new(|s: &Boundary| s.ingest, "ingest")));
    let read = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |s: &Boundary| !s.ingest && s.correct_read,
        "correct next read",
    )));
    let property = Property::always(Property::Or(
        Box::new(!ingest.clone()),
        Box::new(ingest & Property::next(read)),
    ));
    let predicates = p.build();
    let automaton = property.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Report {
    Alias,
    RelatedLesson,
    Gap,
    Backfill,
    Withdraw,
    DuplicateReceipt,
}
use Report::*;
const REPORTS: [Report; 6] = [
    Alias,
    RelatedLesson,
    Gap,
    Backfill,
    Withdraw,
    DuplicateReceipt,
];

#[test]
fn current_read_reflects_late_reports_and_preserves_original_artifact_judgment() {
    let mut histories = 0;
    let mut reads = 0;
    let mut supported = 0;
    // Every history ends with the read that discharges its last obligation.
    let mut audit = monitor();
    for fixture in GOALS {
        let pair = Pair::new(fixture, at(30, 5), "demonstrated");
        for mut word in 0..REPORTS.len().pow(3) {
            let mut book = pair.history();
            let mut holds = Withdrawn::new(CELL.scope);
            let mut complete = true;
            let reports = [(); 3].map(|_| {
                let e = REPORTS[word % REPORTS.len()];
                word /= REPORTS.len();
                e
            });
            assert_eq!(
                pair.read(&book, true, &holds),
                CheckResult::SupportsDurability
            );
            for (i, &event) in reports.iter().enumerate() {
                assert!(audit.next_state(&Boundary {
                    ingest: true,
                    correct_read: false
                }));
                match event {
                    Alias => book.record(pair.projected_alias).unwrap(),
                    RelatedLesson => book.record(pair.related_lesson).unwrap(),
                    Gap => complete = false,
                    Backfill => complete = true,
                    Withdraw => {
                        holds.sources.insert(pair.openings[1].case.pack);
                    }
                    DuplicateReceipt => book.record(pair.openings[1]).unwrap(),
                }
                let prefix = &reports[..=i];
                let expected = !prefix
                    .iter()
                    .any(|e| matches!(e, Alias | RelatedLesson | Withdraw))
                    && prefix.iter().rev().find(|e| matches!(e, Gap | Backfill)) != Some(&Gap);
                let actual = pair.read(&book, complete, &holds) == CheckResult::SupportsDurability;
                assert!(
                    audit.next_state(&Boundary {
                        ingest: false,
                        correct_read: actual == expected
                            && pair.performances[1].judgment() == Judgment::Correct
                    }),
                    "{prefix:?}"
                );
                reads += 1;
                supported += usize::from(actual);
            }
            histories += 1;
        }
    }
    println!("{histories} three-report histories; {reads} post-report reads; {supported} support");
    assert_eq!((histories, reads), (1080, 3240));
}

#[test]
fn answer_only_and_submission_clock_mutants_have_concrete_counterexamples() {
    for (return_open, prior_projection, expected) in [
        (at(30, 5), true, CheckResult::Ineligible),
        (at(1, 3), false, CheckResult::InsufficientSpacing),
    ] {
        let pair = Pair::new(GOALS[0], return_open, "demonstrated");
        let mut book = pair.history();
        let holds = Withdrawn::new(CELL.scope);
        if prior_projection {
            book.record(pair.projected_alias).unwrap();
        }
        assert_eq!(pair.read(&book, true, &holds), expected);
        // The earlier evidence-only projection drops the unanswered display and
        // computes the gap to submission. In both histories it advances.
        let stale = delayed_on_reviewed_change(
            &pair.performances[0],
            &pair.performances[1],
            Some(&[]),
            &holds,
            spacing(),
            &pair.comparison,
        )
        .unwrap();
        assert_eq!(stale, CheckResult::SupportsDurability);
        let mut audit = monitor();
        assert!(audit.next_state(&Boundary {
            ingest: true,
            correct_read: false
        }));
        assert!(!audit.next_state(&Boundary {
            ingest: false,
            correct_read: stale == expected
        }));
    }
    let pair = Pair::new(GOALS[0], at(1, 3), "revisit");
    assert_eq!(
        pair.read(&pair.history(), true, &Withdrawn::new(CELL.scope)),
        CheckResult::ReviewFailure
    );
}

#[test]
fn incompleteness_and_conflicting_receipts_cannot_restore_freshness() {
    let pair = Pair::new(GOALS[0], at(30, 5), "demonstrated");
    let holds = Withdrawn::new(CELL.scope);
    let mut book = ContactHistory::new(CELL);
    book.record(pair.openings[0]).unwrap();
    assert_eq!(pair.read(&book, true, &holds), CheckResult::UnknownExposure);
    book.record(pair.openings[1]).unwrap();
    let peer = Cell {
        scope: Scope {
            learner: 52,
            ..CELL.scope
        },
        ..CELL
    };
    assert_eq!(
        book.record(Dispatch {
            cell: peer,
            ..pair.projected_alias
        }),
        Err(ContactError::WrongCell)
    );
    assert_eq!(
        pair.read(&book, true, &holds),
        CheckResult::SupportsDurability
    );
    assert_eq!(
        book.record(Dispatch {
            attempt: None,
            ..pair.openings[1]
        }),
        Err(ContactError::ConflictingEvent)
    );
    assert_eq!(pair.read(&book, true, &holds), CheckResult::UnknownExposure);
}

#[test]
fn an_unmapped_rendering_and_an_inflight_hint_change_the_read_at_the_right_time() {
    let mut pair = Pair::new(GOALS[0], at(30, 3), "demonstrated");
    let holds = Withdrawn::new(CELL.scope);
    let mut book = pair.history();
    // A late delivery after the learner submitted cannot explain an earlier answer.
    book.record(Dispatch {
        at: at(30, 7),
        ..pair.projected_alias
    })
    .unwrap();
    assert_eq!(
        pair.read(&book, true, &holds),
        CheckResult::SupportsDurability
    );
    // A relevant example entered just before submission, after the check opened.
    book.record(Dispatch {
        at: at(30, 4),
        ..pair.related_lesson
    })
    .unwrap();
    assert_eq!(pair.read(&book, true, &holds), CheckResult::Ineligible);
    let mut unanswered = pair.history();
    unanswered.record(pair.projected_alias).unwrap();
    pair.aliases.remove(&pair.projected_alias.case);
    assert_eq!(
        pair.read(&unanswered, true, &holds),
        CheckResult::UnknownExposure
    );
}
