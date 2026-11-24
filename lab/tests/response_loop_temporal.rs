//! Exhaustive graph over a fixed original item and two candidate item IDs.
//! The policy tests item equality only; this partition includes a stale reply
//! to a different candidate after a valid opening. Catalog semantics lie outside it.
use std::collections::{HashSet, VecDeque};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    protocol::{Assistance, Gate, Judgment},
    response_loop::{Attempt, Choice, Offer, Refusal, ResponseLoop, ReturnView, Stage},
};

#[derive(Clone, Copy, Debug)]
enum Input {
    Choose(Choice),
    Open(u64, Gate),
    Record(Attempt, Gate),
}

impl Input {
    fn apply(self, state: &mut ResponseLoop) -> Result<(), Refusal> {
        match self {
            Self::Choose(choice) => state.choose(choice),
            Self::Open(id, gate) => state.open_example(id, gate),
            Self::Record(attempt, gate) => state.record_check(attempt, gate),
        }
    }
}

fn attempts(item: u64) -> Vec<Attempt> {
    let mut cases = Vec::new();
    for judgment in [
        Judgment::Correct,
        Judgment::Incorrect,
        Judgment::Ambiguous,
        Judgment::Missing,
    ] {
        for disclosure in [
            Assistance::Unknown,
            Assistance::Reported,
            Assistance::NoneReported,
        ] {
            for opened in [false, true] {
                cases.push(Attempt::new(item, judgment, disclosure, opened));
            }
        }
    }
    cases
}

#[derive(Debug)]
struct Edge {
    before: ReturnView,
    input: Input,
    accepted: bool,
    after: ReturnView,
}

fn evidence_matches_submission(e: &Edge) -> bool {
    if e.after.original != e.before.original {
        return false;
    }
    match e.input {
        Input::Record(observation, Gate::Supported)
            if e.before.stage == Stage::OpenExample(observation.item) =>
        {
            e.accepted && e.after.follow_up == Some(observation) && e.after.stage == Stage::Closed
        }
        Input::Record(..) => !e.accepted && e.after == e.before,
        _ => e.after.follow_up == e.before.follow_up,
    }
}

fn opening_follows_choice(e: &Edge) -> bool {
    match (e.before.stage, e.input) {
        (Stage::Feedback, Input::Choose(Choice::NewExample))
            if e.before.original.judgment == Judgment::Correct =>
        {
            e.accepted && e.after.stage == Stage::LookingForExample
        }
        (_, Input::Choose(Choice::NewExample)) => !e.accepted && e.after == e.before,
        (Stage::LookingForExample, Input::Open(item, Gate::Supported))
            if item != e.before.original.item =>
        {
            e.accepted && e.after.stage == Stage::OpenExample(item)
        }
        (_, Input::Open(..)) => !e.accepted && e.after == e.before,
        (_, Input::Choose(Choice::Stop)) => e.accepted && e.after.stage == Stage::Closed,
        (Stage::Closed, _) => e.after == e.before,
        _ => {
            e.after.stage == e.before.stage
                || !matches!(
                    e.after.stage,
                    Stage::LookingForExample | Stage::OpenExample(_)
                )
        }
    }
}

fn monitor() -> Monitor<Edge> {
    let mut predicates = Predicates::builder();
    let evidence = predicates.new_predicate(ClosurePredicate::new(
        evidence_matches_submission,
        "faithful evidence",
    ));
    let choice = predicates.new_predicate(ClosurePredicate::new(
        opening_follows_choice,
        "optional fresh opening",
    ));
    let predicates = predicates.build();
    let property = Property::always(Property::atomic(evidence) & Property::atomic(choice));
    let automaton = property.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

#[test]
fn any_reachable_prefix_respects_choice_and_evidence() {
    let initial: Vec<_> = attempts(1).into_iter().map(ResponseLoop::new).collect();
    let mut discovered: HashSet<_> = initial.iter().copied().collect();
    let mut pending = VecDeque::from(initial);
    let mut alphabet = [
        Choice::NewExample,
        Choice::Discuss,
        Choice::Explain,
        Choice::Stop,
    ]
    .map(Input::Choose)
    .to_vec();
    for item in [1, 2, 3] {
        for gate in [Gate::Supported, Gate::Unavailable] {
            alphabet.push(Input::Open(item, gate));
            alphabet.extend(
                attempts(item)
                    .into_iter()
                    .map(|attempt| Input::Record(attempt, gate)),
            );
        }
    }
    // For G(predicate) the only accepting residual is itself. Checking every
    // reachable edge once covers every path length including self-loops.
    let mut safety = monitor();
    let (mut edges, mut recorded) = (0, 0);
    while let Some(before) = pending.pop_front() {
        for &input in &alphabet {
            let mut after = before;
            let accepted = input.apply(&mut after).is_ok();
            let edge = Edge {
                before: before.return_view(),
                input,
                accepted,
                after: after.return_view(),
            };
            assert!(safety.next_state(&edge), "{edge:?}");
            if !accepted {
                assert_eq!(after, before);
            }
            if accepted && matches!(input, Input::Record(..)) {
                recorded += 1;
            }
            edges += 1;
            if discovered.insert(after) {
                pending.push_back(after);
            }
        }
    }
    assert_eq!(recorded, 6 * 2 * 24);
    println!(
        "{} states, {} edges, {} accepted check observations",
        discovered.len(),
        edges,
        recorded
    );
}

#[test]
fn did_open_cannot_silently_become_reported_help_on_return() {
    let before = ResponseLoop::new(Attempt::new(
        1,
        Judgment::Correct,
        Assistance::Unknown,
        true,
    ));
    assert_eq!(before.offer(), Some(Offer::OptionalNewExample));
    assert_eq!(before.return_view().original.feedback(),
        "This response was marked correct. I don't have an assistance report. A reference opened during this item.");
    let mut corrupted = before.return_view();
    corrupted.original.disclosure = Assistance::Reported;
    corrupted.stage = Stage::Closed;
    assert!(!monitor().next_state(&Edge {
        before: before.return_view(),
        input: Input::Choose(Choice::Stop),
        accepted: true,
        after: corrupted
    }));
}

#[test]
fn a_learner_can_request_explanation_after_opening_and_abandon_scoring() {
    let prior = Attempt::new(1, Judgment::Correct, Assistance::Reported, false);
    let mut state = ResponseLoop::new(prior);
    state.choose(Choice::NewExample).unwrap();
    state.open_example(2, Gate::Supported).unwrap();
    state.choose(Choice::Explain).unwrap();
    assert_eq!(
        state.record_check(
            Attempt::new(2, Judgment::Correct, Assistance::NoneReported, false),
            Gate::Supported
        ),
        Err(Refusal::OutOfOrder)
    );
    assert_eq!(
        state.return_view(),
        ReturnView {
            original: prior,
            follow_up: None,
            stage: Stage::RequestedExplanation
        }
    );
}

#[test]
fn return_after_optional_arithmetic_example_carries_both_actual_judgments() {
    use std::collections::BTreeSet;
    use tutor_lab::curriculum::{OutcomeId, Span};
    use tutor_lab::history::Scope;
    use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
    use tutor_lab::supply::{select, Contact, Enrollment, ExposureBook, Form, Requirement};

    let bytes = UNIT_RULE.as_bytes();
    let pin = Span {
        document: document_id(bytes),
        start: 0,
        end: bytes.len() as u32,
    };
    let source = RuleSource {
        bytes: Some(bytes),
        pin,
        revoked: &BTreeSet::new(),
    };
    let seat = Enrollment {
        class: 1,
        scope: Scope {
            learner: 7,
            revision: 5,
        },
    };
    let original = Form {
        class: seat.class,
        release: seat.scope.revision,
        outcome: OutcomeId(1),
        owner: None,
        revision: 10,
        task: 1,
        family: 1,
        source: pin,
        card: FractionCard::propose(FractionTask::Sum, 3, 1, 4).unwrap(),
    };
    let candidate = Form {
        task: 2,
        revision: 11,
        card: FractionCard::propose(FractionTask::Sum, 3, 2, 8).unwrap(),
        ..original
    };
    let catalog = [original, candidate];
    let first = Attempt::new(
        original.task,
        original.card.assess("1", &source).unwrap(),
        Assistance::Unknown,
        true,
    );
    assert_eq!(first.judgment, Judgment::Correct);
    let mut run = ResponseLoop::new(first);
    let mut exposure = ExposureBook::new(seat, true);
    exposure
        .record(seat, &original, Contact::Delivered)
        .unwrap();
    run.choose(Choice::NewExample).unwrap();
    let new = select(
        seat,
        original.outcome,
        Requirement::FreshTask,
        &exposure,
        &catalog,
        &source,
    )
    .unwrap();
    run.open_example(new.task, Gate::Supported).unwrap();
    exposure.record(seat, &new, Contact::Delivered).unwrap();
    let second = Attempt::new(
        new.task,
        new.card.assess("5/16", &source).unwrap(),
        Assistance::NoneReported,
        false,
    );
    run.record_check(second, Gate::Supported).unwrap();
    let returned = run.return_view();
    assert_eq!(returned.original, first);
    assert_eq!(returned.follow_up, Some(second));
    assert_eq!(second.judgment, Judgment::Incorrect);
    assert_eq!(
        ResponseLoop::new(second).offer(),
        Some(Offer::ReasoningOrExplanation)
    );
}
