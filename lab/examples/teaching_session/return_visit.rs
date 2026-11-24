//! An adult's day-730 return with a deliberately small, fully tracked unit catalog.
//! The day-25 example is the only variation in the controlled day-30 comparison.
use std::collections::{BTreeMap, BTreeSet};

use tutor_lab::authoring::Template;
use tutor_lab::context::{self, Authority, Count, Kind, Purpose, RecordRef};
use tutor_lab::curriculum::{Decision, Outcome, OutcomeId, ReviewedUnit, Slot, Span};
use tutor_lab::history::{reconcile, Attempt, CheckpointProposal, Withdrawn};
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::presentation::{BoundLane, CatalogQuestion};
use tutor_lab::protocol::{Assistance, Command, Event, Gate, Judgment, Question};
use tutor_lab::retention::{self, Cell, CheckResult, Exposure, Moment, Prior};
use tutor_lab::routing::{Action, Intent, Model, Need, Options, Request, Role, Router};
use tutor_lab::supply::{
    select, Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable,
};

struct Visit<'a> {
    at: Moment,
    question: Question,
    answer: &'a str,
}

fn control(lane: &mut BoundLane, event: Event) {
    let after = lane.revision();
    lane.control(Command {
        after,
        record: after + 1,
        event,
    })
    .unwrap();
}

/// The selection is a suggestion. The bound opening checks freshness again and records possible delivery.
fn perform(
    seat: Enrollment,
    forms: &[Form],
    book: &mut ExposureBook,
    lane: &mut BoundLane,
    source: &RuleSource<'_>,
    visit: Visit<'_>,
) -> Attempt {
    let outcome = forms[0].outcome;
    let form = select(seat, outcome, Requirement::FreshTask, book, forms, source).unwrap();
    let after = lane.revision();
    let opening = lane
        .present_from_catalog(
            book,
            CatalogQuestion {
                form,
                question: visit.question,
                requirement: Requirement::FreshTask,
                source,
            },
            after,
            after + 1,
        )
        .unwrap();
    book.record(seat, form, Contact::Delivered).unwrap();
    let assessment = lane
        .submit(
            &opening.handle,
            lane.revision() + 1,
            visit.answer,
            Assistance::NoneReported,
            source,
        )
        .unwrap();
    control(lane, Event::Feedback(Gate::Supported));
    println!(
        "Day {}  {}  Learner answered {}. Numeric result {:?}.",
        visit.at.day, opening.prompt, visit.answer, assessment.judgment
    );
    Attempt {
        scope: assessment.scope,
        id: assessment.response_record,
        sequence: visit.at.sequence,
        outcome: assessment.outcome,
        item: assessment.item.revision,
        day: visit.at.day,
        question: assessment.item.question,
        judgment: assessment.judgment,
        assistance: assessment.assistance,
        fresh: assessment.fresh_task(),
        sources: BTreeSet::from([assessment.span.document]),
    }
}

pub fn demonstrate() {
    let (mut briefing, _, mut records) = context::example(Purpose::PrivateForm);
    // This chapter has its own learner so earlier walkthroughs cannot supply undeclared exposure.
    briefing.snapshot.scope.learner = 84;
    for record in &mut records {
        if record.learner.is_some() {
            record.learner = Some(briefing.snapshot.scope.learner);
        }
    }
    println!("A separate synthetic learner returns to a fractions unit after two years.");
    let scope = briefing.snapshot.scope;
    let seat = Enrollment {
        scope,
        class: briefing.snapshot.class,
    };
    let objective = briefing.snapshot.outcome;
    let cell = Cell {
        scope,
        outcome: objective,
    };
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let withdrawn = Withdrawn::new(scope);
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &withdrawn.sources,
    };
    // These reviewed task/family identities span the complete retained unit history in this fixture.
    let forms: Vec<_> = [
        (101, 1, FractionTask::Sum, 1, 2),
        (102, 1, FractionTask::Sum, 2, 3),
        (103, 2, FractionTask::MissingPart, 1, 4),
    ]
    .into_iter()
    .map(|(id, family, kind, a, b)| Form {
        class: seat.class,
        release: scope.revision,
        owner: None,
        outcome: objective,
        revision: id,
        task: id,
        family,
        source: pin,
        card: FractionCard::propose(kind, a, b, 8).unwrap(),
    })
    .collect();
    let mut book = ExposureBook::new(seat, true);
    let mut lane = BoundLane::for_enrollment(seat, objective);
    control(&mut lane, Event::Teach(Gate::Supported));
    let first = perform(
        seat,
        &forms,
        &mut book,
        &mut lane,
        &source,
        Visit {
            at: Moment {
                day: 0,
                sequence: 1,
            },
            question: Question::Practice,
            answer: "3/8",
        },
    );
    let confirm = perform(
        seat,
        &forms,
        &mut book,
        &mut lane,
        &source,
        Visit {
            at: Moment {
                day: 0,
                sequence: 2,
            },
            question: Question::ChangedExample,
            answer: "5/8",
        },
    );
    let prior = Prior {
        cell,
        verified: Moment {
            day: confirm.day,
            sequence: confirm.sequence,
        },
        // Stated fixture inputs to the day-resolution policy, not fitted to this person.
        stability_days: 3.0,
        threshold: 0.85,
        had_durable_success: false,
    };
    let guide = select(
        seat,
        objective,
        Requirement::Practice,
        &book,
        &forms,
        &source,
    )
    .unwrap();
    control(&mut lane, Event::RequestRefresher(Gate::Supported));
    let (worked, _) = guide.card.worked(&source).unwrap();
    println!("Day 25  The learner chose a worked example. {worked}");
    book.record(seat, guide, Contact::Delivered).unwrap();

    let at = Moment {
        day: 30,
        sequence: 4,
    };
    control(&mut lane, Event::ReviewDue);
    let check = perform(
        seat,
        &forms,
        &mut book,
        &mut lane,
        &source,
        Visit {
            at,
            question: Question::DelayedReview,
            answer: "4/8",
        },
    );
    let guided_contact = [Exposure {
        cell,
        at: Moment {
            day: 25,
            sequence: 3,
        },
    }];
    let compare = |contacts: &[Exposure]| {
        let (forecast, result) = retention::read(
            cell,
            at,
            Some(395),
            Some(prior),
            Some(contacts),
            Some(&check),
            &withdrawn,
        )
        .unwrap();
        let forecast = forecast.unwrap();
        (
            forecast.days_since_exposure.unwrap(),
            forecast.required_gap_days,
            result,
        )
    };
    let (cold_days, _, cold) = compare(&[]);
    let (warm_days, required_days, warm) = compare(&guided_contact);
    assert_eq!(
        (cold, warm),
        (
            CheckResult::SupportsDurability,
            CheckResult::InsufficientSpacing
        )
    );
    println!("Day 30  The same fresh numerical answer has two interpretations. With the day-25 lesson it follows {warm_days} clear days. A comparison history without that lesson gives {cold_days}. The spacing rule needs {required_days:.1} days for the stated day-395 target. Only the comparison history supplies the required interval.");

    // The supported day-0 pair schedules a day-30 return. The short-spaced day-30 observation
    // remains recorded and supplies no proposal to extend a durable-return date.
    let pair = CheckpointProposal {
        scope,
        outcome: objective,
        first: first.id,
        confirmation: confirm.id,
        due: 30,
    };
    let attempts = [first, confirm, check];
    let progress = reconcile(scope, 730, &attempts, &[pair], &withdrawn).unwrap();
    let goal = OutcomeId(objective.0 + 1);
    let unit = ReviewedUnit::build(
        scope.revision,
        vec![
            Outcome {
                id: objective,
                requires: BTreeSet::new(),
                support: BTreeSet::from([pin]),
            },
            Outcome {
                id: goal,
                requires: BTreeSet::from([objective]),
                support: BTreeSet::from([pin]),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        unit.next(goal, &progress, 730, &withdrawn.sources, Slot::Review),
        Ok(Decision::Recheck(objective))
    );
    let checkpoint = progress.checkpoints[&objective];
    assert_eq!(
        select(
            seat,
            objective,
            Requirement::FreshTask,
            &book,
            &forms,
            &source
        ),
        Err(Unavailable::NoFreshForm)
    );
    assert!(select(
        seat,
        objective,
        Requirement::Practice,
        &book,
        &forms,
        &source
    )
    .is_ok());
    assert_eq!(attempts[2].judgment, Judgment::Correct);
    println!("Day 730  Your answer on day {} matched. You took that check {} days after a worked example, so the longer-term question scheduled for day {} is still open. You can work through a familiar example now, ask for a new check, or postpone it.", attempts[2].day, warm_days, checkpoint.due);
    println!("Learner  I'd like a new check.\nTeacher  You've already seen the three approved forms available here. I'll request a different one.");

    briefing.input_limit = 4096;
    briefing.snapshot.turn = 730;
    briefing.snapshot.term = 5;
    for r in &mut records {
        r.turn = briefing.snapshot.turn;
        r.term = briefing.snapshot.term;
    }
    let brief_index = records.iter().position(|r| r.kind == Kind::Brief).unwrap();
    let seen: Vec<_> = forms
        .iter()
        .filter(|f| book.exposure(f.task).is_some())
        .map(|f| f.card.question(&source).unwrap().0)
        .collect();
    records[brief_index].text = format!("The learner requested a new equal-part sum within one whole. Supply three positive integer parameters. Avoid these previously delivered tasks and operand reversals: {}", seen.join(" "));
    let mut saved = records[brief_index].clone();
    saved.key = RecordRef { id: 3, revision: 1 };
    saved.kind = Kind::SavedReturn;
    saved.term = 1;
    saved.turn = attempts[2].sequence;
    saved.carry = true; // These two records retain an explicit grant into the return term.
    saved.text = format!("Return due day {} from the supported day-{} pair. Last numerical check matched on day {}. An intervening worked example left {} days of clear spacing.", checkpoint.due, checkpoint.observed, attempts[2].day, warm_days);
    let mut help = saved.clone();
    help.key = RecordRef { id: 4, revision: 1 };
    help.kind = Kind::Help;
    help.turn = guided_contact[0].at.sequence;
    help.text = format!(
        "A worked equal-parts example was delivered on day {}.",
        guided_contact[0].at.day
    );
    briefing.memory = [Some(help.key), None, Some(saved.key)];
    records.extend([saved, help]);
    let authority = Authority::new(&briefing, &records).unwrap();
    let packet = context::compile(&briefing, &authority, &records, &|m| {
        Count::Measured(m.wire().len() as u32)
    })
    .unwrap();
    let template = Template {
        brief: briefing.required[0],
        material: briefing.required[1],
        span: pin,
        task: FractionTask::Sum,
        edition: 104,
    };
    let request = Request {
        id: 730,
        need: Need {
            scope,
            outcome: objective,
            intent: Intent::PersonalizeForm(Question::DelayedReview),
            packet: packet.digest(),
        },
        binding: template.binding(),
        deadline_ms: 500,
        max_micros: 8,
    };
    let model = Model {
        revision: briefing.snapshot.profile,
        maximum_micros: 8,
        admission_ms: 10,
        slots: 1,
        max_input_tokens: 4096,
        max_output_tokens: 80,
    };
    let mut router = Router::new(1, 8, 1, 1, BTreeMap::from([(Role::Reasoner, model)]));
    let options = Options {
        permitted: BTreeSet::from([Role::Reasoner]),
        ..Options::default()
    };
    assert!(matches!(
        router.route(request, &packet, &authority, &options, 1),
        Action::Dispatch(_)
    ));
    println!("Day 730  Private request authorized by the offline router. Exact provider envelope follows. Provider calls sent: 0.\n{}", packet.dispatch(&authority).unwrap().wire());
}
