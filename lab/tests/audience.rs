use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::audience::*;
use tutor_lab::curriculum::{OutcomeId, Span};
use tutor_lab::history::Scope;
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::presentation::BindError;
use tutor_lab::protocol::{Assistance, Command, Event, Gate, Question};
use tutor_lab::supply::{Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable};

fn setup() -> (Roster, Assignment) {
    (
        Roster {
            class: 5,
            teachers: BTreeSet::from([500]),
            learners: BTreeSet::from([11, 12, 13, 14]),
        },
        Assignment {
            key: AssignmentKey {
                class: 5,
                activity: 7,
            },
            publication: 9,
            form: Form {
                class: 5,
                release: 8,
                outcome: OutcomeId(3),
                revision: 35,
                task: 350,
                family: 1,
                owner: None,
                card: FractionCard::propose(FractionTask::Sum, 1, 2, 4).unwrap(),
                source: Span {
                    document: document_id(UNIT_RULE.as_bytes()),
                    start: 0,
                    end: UNIT_RULE.len() as u32,
                },
            },
            question: Question::RequestedProbe,
            assigned: BTreeMap::from([(11, 1), (12, 1), (13, 1), (19, 1)]),
            held: BTreeSet::new(),
            corrected: false,
            returns: BTreeMap::from([(11, 730)]),
        },
    )
}

fn seat(a: &Assignment) -> Enrollment {
    Enrollment {
        scope: Scope {
            learner: 11,
            revision: a.form.release,
        },
        class: a.key.class,
    }
}

fn source<'a>(assignment: &Assignment, revoked: &'a BTreeSet<[u8; 32]>) -> RuleSource<'a> {
    RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin: assignment.form.source,
        revoked,
    }
}

fn control(p: &mut Participation, event: Event) {
    let after = p.revision();
    p.control(Command {
        after,
        record: after + 101,
        event,
    })
    .unwrap();
}

#[test]
fn learner_sees_a_question_then_a_checked_answer_after_its_feedback_receipt() {
    let (r, a) = setup();
    let active = BTreeSet::new();
    let src = source(&a, &active);
    let mut book = ExposureBook::new(seat(&a), true);
    let access = LearnerAccess {
        actor: 11,
        roster: &r,
        assignment: &a,
        source: &src,
    };
    let mut p = Participation::start(&a, 11);
    let receipt = p
        .present(access, &mut book, Requirement::FreshTask, 0, 1)
        .unwrap();
    match learner_view(11, &r, &a, &p, &src).unwrap() {
        LearnerView::Question { prompt, .. } => assert_eq!(prompt, "What is 1/4 + 2/4?"),
        other => panic!("unexpected {:?}", other),
    }
    assert_eq!(
        learner_view(12, &r, &a, &p, &src),
        Err(ViewError::AccessDenied)
    );
    p.submit(access, &receipt.handle, 2, "3/4", Assistance::NoneReported)
        .unwrap();
    assert_eq!(
        learner_view(11, &r, &a, &p, &src),
        Ok(LearnerView::AwaitingFeedback)
    );
    control(&mut p, Event::Feedback(Gate::Supported));
    let LearnerView::Feedback {
        worked_answer,
        result,
        why,
        next,
        return_day,
        ..
    } = learner_view(11, &r, &a, &p, &src).unwrap()
    else {
        panic!("no feedback");
    };
    assert!(worked_answer.unwrap().contains("3/4"));
    assert_eq!(result, "Your numerical answer matches.");
    assert!(why.contains("reasoning") && next.contains("changed example"));
    assert_eq!(return_day, Some(730));
    let report = teacher_view(500, &r, &a, &[&p], &src).unwrap();
    assert_eq!(
        report.results,
        BTreeMap::from([
            (ResultState::MatchedWithoutReportedHelp, 1),
            (ResultState::NoCurrentResult, 2)
        ])
    );
    control(&mut p, Event::Challenge);
    assert_eq!(
        learner_view(11, &r, &a, &p, &src),
        Ok(LearnerView::UnderReview)
    );
    let revoked = BTreeSet::from([src.pin.document]);
    assert_eq!(
        learner_view(11, &r, &a, &p, &source(&a, &revoked)),
        Ok(LearnerView::Updated)
    );
}

#[test]
fn current_class_roster_and_activity_jointly_determine_who_is_counted() {
    let (mut r, mut a) = setup();
    let active = BTreeSet::new();
    let src = source(&a, &active);
    let mut book = ExposureBook::new(seat(&a), true);
    let access = LearnerAccess {
        actor: 11,
        roster: &r,
        assignment: &a,
        source: &src,
    };
    let mut working = Participation::start(&a, 11);
    working
        .present(access, &mut book, Requirement::FreshTask, 0, 1)
        .unwrap();
    let excluded = Participation::start(&a, 14); // enrolled, not assigned
    let former = Participation::start(&a, 19); // assigned, no longer enrolled
    let (_, mut foreign_activity) = setup();
    foreign_activity.key.class = 6;
    let foreign = Participation::start(&foreign_activity, 12);
    let rows = [&working, &excluded, &former, &foreign];
    a.held.insert(13);
    let tv = teacher_view(500, &r, &a, &rows, &src).unwrap();
    assert_eq!(tv.assigned, 3);
    assert_eq!(tv.unassigned_enrolled, 1);
    assert_eq!(
        tv.counts,
        BTreeMap::from([
            (TeacherAction::ContinueActivity, 1),
            (TeacherAction::InviteToStart, 1),
            (TeacherAction::ReviewWithLearner, 1)
        ])
    );
    assert_eq!(tv.follow_up, vec![13]);
    assert_eq!(
        tv.results,
        BTreeMap::from([
            (ResultState::NoCurrentResult, 2),
            (ResultState::UnderReview, 1)
        ])
    );
    assert_eq!(
        teacher_view(500, &r, &a, &[&working, &working], &src),
        Err(ViewError::DuplicateParticipation)
    );
    for denied in [11, 14, 700] {
        assert_eq!(
            teacher_view(denied, &r, &a, &rows, &src),
            Err(ViewError::AccessDenied)
        );
    }
    assert_eq!(
        learner_view(19, &r, &a, &former, &src),
        Err(ViewError::AccessDenied)
    );
    assert_eq!(
        learner_view(14, &r, &a, &excluded, &src),
        Err(ViewError::AccessDenied)
    );
    assert_eq!(
        learner_view(12, &r, &a, &foreign, &src),
        Err(ViewError::AccessDenied)
    );
    a.form.owner = Some(12);
    assert_eq!(
        learner_view(11, &r, &a, &working, &src),
        Err(ViewError::AccessDenied)
    );
    assert_eq!(
        teacher_view(500, &r, &a, &rows, &src),
        Err(ViewError::AccessDenied)
    );
    a.form.owner = None;
    r.teachers.clear();
    assert_eq!(
        teacher_view(500, &r, &a, &rows, &src),
        Err(ViewError::AccessDenied)
    );
}

#[test]
fn corrections_opaque_inputs_and_replaced_publications_yield_no_old_solution() {
    let (r, mut a) = setup();
    let active = BTreeSet::new();
    let src = source(&a, &active);
    let mut book = ExposureBook::new(seat(&a), true);
    let access = LearnerAccess {
        actor: 11,
        roster: &r,
        assignment: &a,
        source: &src,
    };
    let mut p = Participation::start(&a, 11);
    let open = p
        .present(access, &mut book, Requirement::FreshTask, 0, 1)
        .unwrap();
    p.submit(
        access,
        &open.handle,
        2,
        "I meant a unit of length",
        Assistance::Unknown,
    )
    .unwrap();
    control(&mut p, Event::Feedback(Gate::Supported));
    assert!(matches!(
        learner_view(11, &r, &a, &p, &src),
        Ok(LearnerView::Feedback {
            worked_answer: None,
            ..
        })
    ));
    a.corrected = true;
    assert_eq!(learner_view(11, &r, &a, &p, &src), Ok(LearnerView::Updated));
    assert_eq!(
        teacher_view(500, &r, &a, &[&p], &src).unwrap().counts,
        BTreeMap::from([(TeacherAction::ReplaceMaterial, 3)])
    );
    a.corrected = false;
    a.publication += 1;
    assert_eq!(learner_view(11, &r, &a, &p, &src), Ok(LearnerView::Updated));
    assert_eq!(
        teacher_view(500, &r, &a, &[&p], &src).unwrap().counts,
        BTreeMap::from([(TeacherAction::InviteToStart, 3)])
    );
    let mut new = Participation::start(&a, 11);
    control(&mut new, Event::Teach(Gate::Supported));
    assert!(matches!(
        learner_view(11, &r, &a, &new, &src),
        Ok(LearnerView::Lesson {
            text: UNIT_RULE,
            ..
        })
    ));
    control(&mut new, Event::Pause);
    assert_eq!(
        learner_view(11, &r, &a, &new, &src),
        Ok(LearnerView::Paused)
    );
}

#[test]
fn two_authorized_lanes_cannot_claim_the_same_semantic_task_as_fresh() {
    let (roster, a) = setup();
    let (_, mut alias) = setup();
    alias.key.activity += 1;
    alias.form.revision += 1; // A distinct authorized edition of the same reviewed semantic task.
    let active = BTreeSet::new();
    let src = source(&a, &active);
    let access = LearnerAccess {
        actor: 11,
        roster: &roster,
        assignment: &a,
        source: &src,
    };
    let second = LearnerAccess {
        assignment: &alias,
        ..access
    };
    let mut first_lane = Participation::start(&a, 11);
    let mut second_lane = Participation::start(&alias, 11);
    let mut book = ExposureBook::new(seat(&a), true);
    assert!(first_lane
        .present(access, &mut book, Requirement::FreshFamily, 7, 11)
        .is_err());
    assert_eq!(book.exposure(a.form.task), None);
    let opening = first_lane
        .present(access, &mut book, Requirement::FreshFamily, 0, 11)
        .unwrap();
    assert_eq!(book.exposure(a.form.task), Some(Contact::Possible));
    // Asking for weaker freshness cannot turn a probe into supported practice.
    assert!(matches!(
        second_lane.present(second, &mut book, Requirement::Practice, 0, 21),
        Err(ParticipationError::Binding(BindError::Supply(
            Unavailable::NoFreshForm
        )))
    ));
    assert_eq!(second_lane.revision(), 0);
    book.record(seat(&a), &a.form, Contact::Delivered).unwrap();
    assert_eq!(book.exposure(a.form.task), Some(Contact::Delivered));
    control(&mut first_lane, Event::Pause);
    control(&mut first_lane, Event::Resume);
    assert!(first_lane
        .submit(access, &opening.handle, 14, "3/4", Assistance::NoneReported)
        .is_err());
    let resumed = first_lane
        .present(
            access,
            &mut book,
            Requirement::FreshFamily,
            first_lane.revision(),
            15,
        )
        .unwrap();
    let answer = first_lane
        .submit(access, &resumed.handle, 16, "3/4", Assistance::NoneReported)
        .unwrap();
    assert!(answer.presented_again);
    assert_eq!(answer.exposure_check, Some(Requirement::Practice));
    assert_eq!(book.exposure(a.form.task), Some(Contact::Delivered));
    alias.question = Question::Practice;
    let supported_access = LearnerAccess {
        assignment: &alias,
        ..access
    };
    let mut supported = Participation::start(&alias, 11);
    control(&mut supported, Event::Teach(Gate::Supported));
    let practice = supported
        .present(
            supported_access,
            &mut book,
            Requirement::Practice,
            supported.revision(),
            31,
        )
        .unwrap();
    let answer = supported
        .submit(
            supported_access,
            &practice.handle,
            32,
            "3/4",
            Assistance::Unknown,
        )
        .unwrap();
    assert_eq!(answer.exposure_check, Some(Requirement::Practice));
}

#[test]
fn held_revoked_and_changed_forms_leave_the_opening_and_exposure_frontier_untouched() {
    let (roster, mut a) = setup();
    let active = BTreeSet::new();
    let src = source(&a, &active);
    let revoked = BTreeSet::from([src.pin.document]);
    let unavailable = source(&a, &revoked);
    let mut lane = Participation::start(&a, 11);
    let mut book = ExposureBook::new(seat(&a), true);
    a.held.insert(11);
    let access = LearnerAccess {
        actor: 11,
        roster: &roster,
        assignment: &a,
        source: &src,
    };
    assert!(matches!(
        lane.present(access, &mut book, Requirement::FreshTask, 0, 1),
        Err(ParticipationError::Access(AccessStop::UnderReview))
    ));
    a.held.clear();
    let access = LearnerAccess {
        actor: 11,
        roster: &roster,
        assignment: &a,
        source: &unavailable,
    };
    assert!(matches!(
        lane.present(access, &mut book, Requirement::FreshTask, 0, 1),
        Err(ParticipationError::Access(AccessStop::Updated))
    ));
    a.form.family += 1;
    let changed = LearnerAccess {
        actor: 11,
        roster: &roster,
        assignment: &a,
        source: &src,
    };
    assert!(matches!(
        lane.present(changed, &mut book, Requirement::FreshTask, 0, 1),
        Err(ParticipationError::Access(AccessStop::Updated))
    ));
    assert_eq!(lane.revision(), 0);
    assert_eq!(book.exposure(a.form.task), None);
    a.form.family -= 1;
    let corrected = LearnerAccess {
        actor: 11,
        roster: &roster,
        assignment: &a,
        source: &src,
    };
    let open = lane
        .present(corrected, &mut book, Requirement::Practice, 0, 1)
        .unwrap();
    let result = lane
        .submit(corrected, &open.handle, 2, "3/4", Assistance::NoneReported)
        .unwrap();
    assert_eq!(result.exposure_check, Some(Requirement::FreshTask));
}

#[test]
fn authorized_successor_preserves_the_owed_question_and_seals_the_previous_activity() {
    let (roster, mut prior) = setup();
    let (_, mut successor) = setup();
    successor.key.activity += 1;
    successor.publication += 1;
    successor.form.revision += 1;
    successor.form.task += 1;
    successor.form.card = FractionCard::propose(FractionTask::Sum, 2, 3, 8).unwrap();
    successor.question = Question::ChangedExample;
    let active = BTreeSet::new();
    let src = source(&prior, &active);
    let original = LearnerAccess {
        actor: 11,
        roster: &roster,
        assignment: &prior,
        source: &src,
    };
    let later = LearnerAccess {
        assignment: &successor,
        ..original
    };
    let mut lane = Participation::start(&prior, 11);
    let mut book = ExposureBook::new(seat(&prior), true);
    let first = lane
        .present(original, &mut book, Requirement::FreshTask, 0, 1)
        .unwrap();
    lane.submit(original, &first.handle, 2, "3/4", Assistance::NoneReported)
        .unwrap();
    // Feedback debt cannot be discarded by jumping to another assignment.
    let requested = Requirement::Practice;
    assert!(lane
        .present_successor(original, later, &mut book, requested, 2, 3)
        .is_err());
    control(&mut lane, Event::Feedback(Gate::Supported));
    let after = lane.revision();

    let revoked = BTreeSet::from([src.pin.document]);
    let unavailable = source(&successor, &revoked);
    let (_, mut held) = setup();
    held.held.insert(11);
    let (_, mut wrong_phase) = setup();
    wrong_phase.key = successor.key;
    wrong_phase.form = successor.form;
    wrong_phase.publication = successor.publication;
    wrong_phase.question = Question::DelayedReview;
    for (from, to, revision) in [
        (
            original,
            LearnerAccess {
                source: &unavailable,
                ..later
            },
            after,
        ),
        (original, LearnerAccess { actor: 12, ..later }, after),
        (original, later, after - 1),
        (
            LearnerAccess {
                assignment: &held,
                ..original
            },
            later,
            after,
        ),
        (
            original,
            LearnerAccess {
                assignment: &wrong_phase,
                ..later
            },
            after,
        ),
    ] {
        assert!(lane
            .present_successor(from, to, &mut book, requested, revision, 4)
            .is_err());
        assert_eq!(lane.revision(), after);
        assert_eq!(book.exposure(successor.form.task), None);
    }
    let (mut next_lane, changed) = lane
        .present_successor(original, later, &mut book, requested, after, 4)
        .unwrap();
    assert_eq!(changed.prompt, "What is 2/8 + 3/8?");
    assert_eq!(book.exposure(successor.form.task), Some(Contact::Possible));
    assert!(lane
        .present_successor(original, later, &mut book, requested, after, 5)
        .is_err());
    assert!(next_lane
        .submit(later, &first.handle, 5, "3/4", Assistance::NoneReported)
        .is_err());
    let result = next_lane
        .submit(later, &changed.handle, 5, "5/8", Assistance::NoneReported)
        .unwrap();
    assert!(result.fresh_task());
    let old_tally = teacher_view(500, &roster, &prior, &[&lane, &next_lane], &src).unwrap();
    assert_eq!(old_tally.counts[&TeacherAction::FollowUpStarted], 1);
    assert_eq!(
        old_tally.results[&ResultState::MatchedWithoutReportedHelp],
        1
    );
    assert!(matches!(
        learner_view(11, &roster, &prior, &lane, &src).unwrap(),
        LearnerView::Feedback {
            next: "You continued in the next activity.",
            ..
        }
    ));
    prior.corrected = true;
    assert_eq!(
        learner_view(11, &roster, &prior, &lane, &src),
        Ok(LearnerView::Updated)
    );
}
