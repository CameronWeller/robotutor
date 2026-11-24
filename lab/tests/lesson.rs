//! A synthetic one-person lesson followed through source admission, numeric assessment,
//! feedback, a two-year return, and correction of an item used by two checkpoints.
use std::collections::BTreeSet;
use tutor_lab::curriculum::{Decision, Outcome, OutcomeId, ReviewedUnit, Slot, Span};
use tutor_lab::history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn};
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::presentation::{Assessment, BoundLane, CatalogQuestion, Item};
use tutor_lab::protocol::{Assistance, Command, Effect, Event, Gate, Next, Question, Receipt};
use tutor_lab::supply::{Enrollment, ExposureBook, Form, Requirement};

const SEAT: Enrollment = Enrollment {
    class: 7,
    scope: Scope {
        learner: 42,
        revision: 1,
    },
};
const SUM: OutcomeId = OutcomeId(10);

fn control(lane: &mut BoundLane, event: Event, expected: Effect) {
    let after = lane.revision();
    let command = Command {
        after,
        record: after + 1,
        event,
    };
    assert_eq!(lane.control(command).unwrap(), Receipt::Applied(expected));
}

fn perform(
    lane: &mut BoundLane,
    book: &mut ExposureBook,
    item: Item,
    source: &RuleSource<'_>,
    response: &str,
    next: Next,
) -> Assessment {
    let version = lane.revision();
    let form = Form {
        class: SEAT.class,
        release: SEAT.scope.revision,
        owner: None,
        outcome: SUM,
        revision: item.revision,
        task: item.revision,
        family: 1,
        source: source.pin,
        card: item.card,
    };
    let delivery = lane
        .present_from_catalog(
            book,
            CatalogQuestion {
                form: &form,
                question: item.question,
                requirement: Requirement::FreshTask,
                source,
            },
            version,
            version + 1,
        )
        .unwrap();
    assert!(!delivery.prompt.is_empty());
    let assessment = lane
        .submit(
            &delivery.handle,
            lane.revision() + 1,
            response,
            Assistance::NoneReported,
            source,
        )
        .unwrap();
    assert_eq!(assessment.effect, Effect::Analyzed(next));
    control(
        lane,
        Event::Feedback(Gate::Supported),
        Effect::GaveFeedback(next),
    );
    assessment
}

fn observation(assessment: Assessment, day: u32) -> Attempt {
    Attempt {
        scope: assessment.scope,
        id: assessment.response_record,
        sequence: assessment.presentation_record,
        outcome: assessment.outcome,
        item: assessment.item.revision,
        day,
        question: assessment.item.question,
        judgment: assessment.judgment,
        assistance: assessment.assistance,
        fresh: assessment.fresh_task(),
        sources: BTreeSet::from([assessment.span.document]),
    }
}

#[test]
fn contested_item_changes_the_next_lesson_after_a_two_year_gap() {
    let scope = SEAT.scope;
    let sum = SUM;
    let endpoint = OutcomeId(20);
    let source_bytes = UNIT_RULE.as_bytes();
    let pin = Span {
        document: document_id(source_bytes),
        start: 0,
        end: source_bytes.len() as u32,
    };
    let active = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(source_bytes),
        pin,
        revoked: &active,
    };
    let unit = ReviewedUnit::build(
        scope.revision,
        vec![
            Outcome {
                id: sum,
                requires: BTreeSet::new(),
                support: BTreeSet::from([pin]),
            },
            Outcome {
                id: endpoint,
                requires: BTreeSet::from([sum]),
                support: BTreeSet::from([pin]),
            },
        ],
    )
    .unwrap();
    let mut lane = BoundLane::for_enrollment(SEAT, sum);
    let mut book = ExposureBook::new(SEAT, true);
    let mut withdrawn = Withdrawn::new(scope);
    let mut attempts = Vec::new();
    let mut proposals = Vec::new();
    let plan =
        |day, attempts: &[Attempt], proposals: &[CheckpointProposal], withdrawn: &Withdrawn| {
            let progress = reconcile(scope, day, attempts, proposals, withdrawn).unwrap();
            unit.next(endpoint, &progress, day, &withdrawn.sources, Slot::Review)
                .unwrap()
        };
    assert_eq!(
        plan(0, &attempts, &proposals, &withdrawn),
        Decision::Teach(sum)
    );

    let example = FractionCard::propose(FractionTask::Sum, 1, 1, 4).unwrap();
    assert_eq!(example.worked(&source).unwrap().1, pin);
    control(&mut lane, Event::Teach(Gate::Supported), Effect::Taught);
    let practice = Item {
        revision: 11,
        question: Question::Practice,
        card: FractionCard::propose(FractionTask::Sum, 3, 1, 4).unwrap(),
    };
    let first = perform(
        &mut lane,
        &mut book,
        practice,
        &source,
        "1",
        Next::ChangedExample,
    );
    attempts.push(observation(first, 0));
    assert_eq!(
        plan(0, &attempts, &proposals, &withdrawn),
        Decision::Teach(sum)
    );

    let changed = Item {
        revision: 12,
        question: Question::ChangedExample,
        card: FractionCard::propose(FractionTask::MissingPart, 1, 3, 8).unwrap(),
    };
    let second = perform(
        &mut lane,
        &mut book,
        changed,
        &source,
        "6/16",
        Next::WaitReview,
    );
    attempts.push(observation(second, 0));
    proposals.push(CheckpointProposal {
        scope,
        outcome: sum,
        first: first.response_record,
        confirmation: second.response_record,
        due: 8,
    });
    assert_eq!(
        plan(0, &attempts, &proposals, &withdrawn),
        Decision::Teach(endpoint)
    );
    assert_eq!(
        plan(730, &attempts, &proposals, &withdrawn),
        Decision::Recheck(sum)
    );

    control(&mut lane, Event::ReviewDue, Effect::ReviewOpened);
    let delayed = Item {
        revision: 13,
        question: Question::DelayedReview,
        card: FractionCard::propose(FractionTask::Sum, 3, 2, 8).unwrap(),
    };
    let third = perform(
        &mut lane,
        &mut book,
        delayed,
        &source,
        "5/8",
        Next::WaitReview,
    );
    attempts.push(observation(third, 730));
    proposals.push(CheckpointProposal {
        scope,
        outcome: sum,
        first: second.response_record,
        confirmation: third.response_record,
        due: 758,
    });
    assert_eq!(
        plan(730, &attempts, &proposals, &withdrawn),
        Decision::Teach(endpoint)
    );

    // The reconciler learns of the instrument defect before the next return.
    // Its transaction also cancels the timer for the unsupported checkpoint.
    withdrawn.items.insert(second.item.revision);
    let corrected = reconcile(scope, 730, &attempts, &proposals, &withdrawn).unwrap();
    assert!(!corrected.checkpoints.contains_key(&sum));
    control(
        &mut lane,
        Event::CheckpointUnsupported,
        Effect::RecheckRequired,
    );
    control(&mut lane, Event::Challenge, Effect::Invalidated);
    withdrawn.postponed.insert(sum);
    assert_eq!(
        plan(730, &attempts, &proposals, &withdrawn),
        Decision::Postponed(sum)
    );
    control(&mut lane, Event::Resolve, Effect::Reopened);
    withdrawn.postponed.clear();
    assert_eq!(
        plan(730, &attempts, &proposals, &withdrawn),
        Decision::Teach(sum)
    );
    withdrawn.sources.insert(pin.document);
    assert_eq!(
        plan(730, &attempts, &proposals, &withdrawn),
        Decision::RepairSources(sum)
    );
}
