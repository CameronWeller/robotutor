//! Run a synthetic lesson from an authoring request to the two audience views.
//! The mock profile counts one token per UTF-8 byte of the wire envelope.
//! It has no network access or vendor tokenizer. Arguments: numeric response.
#[path = "teaching_session/reasoning.rs"]
mod reasoning;
#[path = "teaching_session/return_visit.rs"]
mod return_visit;
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::audience::{
    learner_view, teacher_view, Assignment, AssignmentKey, LearnerView, Participation, Roster,
};
use tutor_lab::authoring::{AuthoringJob, CandidateBytes, Catalog, Publisher, Template};
use tutor_lab::context::{self, Authority, Count, Kind, Messages, Purpose};
use tutor_lab::curriculum::Span;
use tutor_lab::material::{document_id, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::{Assistance, Command, Event, Gate, Question};
use tutor_lab::routing::profiles::{self, Contract, Qualification, Window};
use tutor_lab::routing::{
    self, Action, Completion, Intent, LocalGrant, Model, Need, Options, Request, Returned, Role,
    Router,
};
use tutor_lab::supply::{select, Contact, Enrollment, ExposureBook, Requirement};

fn meter(messages: &Messages) -> Count {
    Count::Measured(u32::try_from(messages.wire().len()).unwrap())
}

fn main() {
    let response = std::env::args().nth(1).unwrap_or_else(|| "5/8".into());
    if response.len() > context::MAX_RECORD_BYTES {
        eprintln!(
            "The walkthrough accepts one response up to {} UTF-8 bytes.",
            context::MAX_RECORD_BYTES
        );
        std::process::exit(2);
    }
    let (mut brief, _, catalog_records) = context::example(Purpose::Author);
    let model = Model {
        revision: 2,
        maximum_micros: 450,
        admission_ms: 100,
        slots: 1,
        max_input_tokens: 2048,
        max_output_tokens: 80,
    };
    let mut router = Router::new(1, 900, 1, 8, BTreeMap::from([(Role::Language, model)]));
    let options = Options {
        permitted: BTreeSet::from([Role::Language]),
        ..Options::default()
    };
    let prose = Model {
        revision: 1,
        maximum_micros: 40,
        ..model
    };
    router.register_profile(Role::Language, prose).unwrap();
    let contract = Contract {
        release: brief.snapshot.scope.revision,
        outcome: brief.snapshot.outcome,
        intent: Intent::AuthorForm,
        locale: "en",
        modality: "text",
        schema: "fraction-parameters/v1",
    };
    // Both synthetic profiles have a processing grant. Only one has a reviewed
    // qualification for the requested output schema, language, modality and release.
    let numeric = Qualification {
        contract,
        role: Role::Language,
        profile: model.revision,
        benchmark_epoch: 1,
        grant_epoch: brief.snapshot.epoch,
        from_ms: 0,
        until_ms: 900,
    };
    let prose_only = Qualification {
        contract: Contract {
            schema: "question-prose/v1",
            ..contract
        },
        profile: prose.revision,
        ..numeric
    };
    let window = Window {
        now_ms: 0,
        deadline_ms: 1000,
        available_micros: 450.min(router.remaining_micros()),
        benchmark_epoch: 1,
        grant_epoch: brief.snapshot.epoch,
    };
    let selected = profiles::select(
        contract,
        window,
        &[numeric, prose_only],
        |role, revision| {
            if options.permitted.contains(&role) {
                router.available_profile(role, revision)
            } else {
                None
            }
        },
    )
    .unwrap();
    brief.snapshot.profile = selected.model.revision;
    brief.input_limit = selected.model.max_input_tokens;
    let authority = Authority::new(&brief, &catalog_records).unwrap();
    let packet = context::compile(&brief, &authority, &catalog_records, &meter).unwrap();
    let seat = Enrollment {
        scope: brief.snapshot.scope,
        class: brief.snapshot.class,
    };
    let outcome = brief.snapshot.outcome;
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let revoked = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &revoked,
    };
    let template = Template {
        brief: catalog_records
            .iter()
            .find(|r| r.kind == Kind::Brief)
            .unwrap()
            .key,
        material: catalog_records
            .iter()
            .find(|r| r.kind == Kind::Material)
            .unwrap()
            .key,
        task: FractionTask::Sum,
        span: pin,
        edition: 32,
    };
    let job = AuthoringJob {
        template,
        request: Request {
            id: 9,
            need: Need {
                scope: seat.scope,
                outcome,
                intent: Intent::AuthorForm,
                packet: packet.digest(),
            },
            binding: template.binding(),
            deadline_ms: selected.deadline_ms,
            max_micros: 450,
        },
    };
    let Action::Dispatch(ticket) = router.route(job.request, &packet, &authority, &options, 0)
    else {
        panic!("fixture author unavailable")
    };
    println!(
        "Model role {:?}, profile revision {}, maximum charge {} microdollars\nExact provider envelope\n{}",
        ticket.role, ticket.model_revision, ticket.maximum_micros,
        router
            .messages_for(ticket, &packet, &authority, &options, 1)
            .unwrap()
            .wire()
    );
    let candidate = b"[2,3,8]";
    router
        .settle(
            ticket,
            Completion {
                result: Returned::Draft(document_id(candidate)),
                charged_micros: 240,
            },
        )
        .unwrap();
    let mut catalog = Catalog::default();
    let form = job
        .publish(
            Publisher {
                router: &router,
                catalog: &mut catalog,
            },
            CandidateBytes {
                ticket,
                bytes: candidate,
            },
            &packet,
            &authority,
            &source,
            200,
        )
        .unwrap();
    let mut book = ExposureBook::new(seat, true);
    let chosen = select(
        seat,
        outcome,
        Requirement::FreshTask,
        &book,
        std::slice::from_ref(&form),
        &source,
    )
    .unwrap();
    let learner = seat.scope.learner;
    let teacher = 97;
    let roster = Roster {
        class: seat.class,
        teachers: BTreeSet::from([teacher]),
        learners: BTreeSet::from([learner, 43, 44]),
    };
    let mut assignment = Assignment {
        key: AssignmentKey {
            class: seat.class,
            activity: 99,
        },
        publication: 1,
        form: *chosen,
        question: Question::Practice,
        assigned: roster.learners.iter().map(|&id| (id, 1)).collect(),
        held: BTreeSet::new(),
        corrected: false,
        returns: BTreeMap::new(),
    };
    let mut participation = Participation::start(&assignment, learner);
    participation
        .control(Command {
            after: 0,
            record: 1,
            event: Event::Teach(Gate::Supported),
        })
        .unwrap();
    let lesson = learner_view(learner, &roster, &assignment, &participation, &source).unwrap();
    let LearnerView::Lesson { text, next, .. } = lesson else {
        panic!("expected lesson")
    };
    println!("Learner lesson: {text}\n{next}");
    let access = tutor_lab::audience::LearnerAccess {
        actor: learner,
        roster: &roster,
        assignment: &assignment,
        source: &source,
    };
    let opening = participation
        .present(access, &mut book, Requirement::Practice, 1, 2)
        .unwrap();
    let LearnerView::Question { prompt, why, .. } =
        learner_view(learner, &roster, &assignment, &participation, &source).unwrap()
    else {
        panic!("expected question")
    };
    println!("Learner question: {prompt}\n{why}");
    book.record(seat, &form, Contact::Delivered).unwrap();

    // Create the local assessor's immutable turn scope. No provider messages are sent to grade the number.
    let (eval, _, mut evidence) = context::example(Purpose::Evaluate);
    for record in &mut evidence {
        match record.kind {
            Kind::Brief => record.text = format!("Check the numeric response to {prompt}"),
            Kind::Rubric => record.text = "Accept rational values equivalent to 5/8.".into(),
            Kind::Response => record.text.clone_from(&response),
            _ => {}
        }
    }
    let eval_auth = Authority::new(&eval, &evidence).unwrap();
    let binding = document_id(
        serde_json::json!([assignment.publication, form.revision, prompt, response])
            .to_string()
            .as_bytes(),
    );
    let grade = Request {
        id: 10,
        need: Need {
            scope: seat.scope,
            outcome,
            intent: Intent::GradeNumeric,
            packet: binding,
        },
        binding,
        deadline_ms: 1000,
        max_micros: 0,
    };
    assert_eq!(
        routing::route_local(
            grade,
            &eval_auth,
            LocalGrant {
                binding,
                valid_until_ms: 1000,
                snapshot: eval.snapshot,
                need: grade.need
            },
            &Options {
                exact_checker: true,
                ..Default::default()
            },
            250
        ),
        Action::CheckLocally
    );
    participation
        .submit(
            access,
            &opening.handle,
            3,
            &response,
            Assistance::NoneReported,
        )
        .unwrap();
    participation
        .control(Command {
            after: 3,
            record: 4,
            event: Event::Feedback(Gate::Supported),
        })
        .unwrap();
    if let LearnerView::Feedback {
        result,
        why,
        next,
        worked_answer,
        ..
    } = learner_view(learner, &roster, &assignment, &participation, &source).unwrap()
    {
        println!("Learner feedback: {result}\n{why}\n{next}");
        if let Some(worked) = worked_answer {
            println!("{worked}");
        }
    }
    let group = teacher_view(teacher, &roster, &assignment, &[&participation], &source).unwrap();
    println!("Teacher · {} assigned", group.assigned);
    for (action, count) in group.counts {
        println!("{} · {count}", action.label());
    }
    println!(
        "Mock usage: {} microdollars charged, {} reserved.",
        router.spent_micros(),
        router.reserved_micros()
    );
    assignment.corrected = true;
    let held = learner_view(learner, &roster, &assignment, &participation, &source).unwrap();
    println!(
        "Learner after a correction · {}",
        held.status_message().expect("held status")
    );
    reasoning::demonstrate();
    return_visit::demonstrate();
}
