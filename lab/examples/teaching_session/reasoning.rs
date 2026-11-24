//! Independent synthetic assessment of the explanation criterion on a fractions problem.
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::{
    audience::Roster,
    context::{self, Authority, Count, Kind, Purpose},
    interpretation::{
        Candidate, Criterion, Finding, LearnerFeedback, ReviewBook, ReviewOrder, ReviewerPermit,
    },
    material::document_id,
    protocol::Assistance,
    routing::{Action, Completion, Intent, Model, Need, Options, Request, Returned, Role, Router},
};

pub fn demonstrate() {
    let (mut brief, _, mut records) = context::example(Purpose::Evaluate);
    brief.input_limit = 2048;
    for record in &mut records {
        let wording = match record.kind {
            Kind::Brief => "For 2/8 plus 3/8, assess the learner's explanation of the unit.",
            Kind::Rubric => "Identifies the shared unit and combines the counts. Words, diagrams and mathematical notation can express the method.",
            Kind::Response => "I walked five eighth-size steps on the same strip, two for the first length and three for the second.",
            _ => continue,
        };
        record.text = wording.into();
    }
    let authority = Authority::new(&brief, &records).unwrap();
    let packet = context::compile(&brief, &authority, &records, &|m| {
        Count::Measured(m.wire().len() as u32)
    })
    .unwrap();
    let rubric = records.iter().find(|r| r.kind == Kind::Rubric).unwrap();
    let response = records.iter().find(|r| r.kind == Kind::Response).unwrap();
    let criterion = Criterion {
        id: 2,
        presentation: 73,
        rubric: rubric.key,
        response: response.key,
        assistance: Assistance::NoneReported,
    };
    let order = ReviewOrder {
        criterion,
        replaces: None,
    };
    let request = Request {
        id: 120,
        deadline_ms: 50,
        max_micros: 20,
        binding: order.binding(),
        need: Need {
            scope: packet.scope(),
            outcome: packet.outcome(),
            intent: Intent::AssessOpen,
            packet: packet.digest(),
        },
    };
    let model = Model {
        revision: brief.snapshot.profile,
        maximum_micros: 20,
        admission_ms: 5,
        slots: 1,
        max_input_tokens: brief.input_limit,
        max_output_tokens: brief.output_limit,
    };
    let mut router = Router::new(1, 20, 1, 1, BTreeMap::from([(Role::Assessor, model)]));
    let options = Options {
        permitted: BTreeSet::from([Role::Assessor]),
        ..Options::default()
    };
    let Action::Dispatch(ticket) = router.route(request, &packet, &authority, &options, 0) else {
        panic!("synthetic assessor unavailable");
    };
    let messages = router
        .messages_for(ticket, &packet, &authority, &options, 1)
        .unwrap();
    println!("Simulated assessor's authorized input\n{}", messages.wire());
    // An exact quotation is authentic even when the interpretation of it is wrong.
    let bytes = br#"["not_yet","five eighth-size steps on the same strip"]"#;
    router
        .settle(
            ticket,
            Completion {
                result: Returned::Draft(document_id(bytes)),
                charged_micros: 11,
            },
        )
        .unwrap();
    let mut cases = ReviewBook::default();
    let review = cases
        .admit(
            order,
            Candidate {
                ticket,
                bytes,
                rubric,
                response,
            },
            &router,
            &packet,
            &authority,
            2,
        )
        .unwrap();
    let learner = packet.scope().learner;
    let teacher = 97;
    let roster = Roster {
        class: brief.snapshot.class,
        learners: BTreeSet::from([learner]),
        teachers: BTreeSet::from([teacher]),
    };
    let permit = ReviewerPermit { teacher, request };
    assert_eq!(
        review
            .learner_view(learner, &roster, &packet, &authority)
            .unwrap(),
        LearnerFeedback::Pending
    );
    println!("The learner sees “Your explanation is awaiting review.”");
    let suggestion = review
        .inspect(teacher, &permit, &roster, &packet, &authority)
        .unwrap()
        .unwrap();
    println!(
        "Qualified teacher sees proposed {:?} on the passage “{}”.",
        suggestion.proposed,
        &response.text[suggestion.start..suggestion.end]
    );
    review.adjudicate(teacher, &permit, &roster, &packet, &authority,
        (Finding::Meets, "Your steps all measure one eighth of the same strip. Counting two and then three more explains why the combined length is five eighths.")).unwrap();
    let LearnerFeedback::Reviewed { text, .. } = review
        .learner_view(learner, &roster, &packet, &authority)
        .unwrap()
    else {
        panic!("expected reviewed feedback");
    };
    println!("After review the learner sees “{text}”");
    println!(
        "Separate assessor fixture charged {} simulated microdollars and retains {} in reservations.",
        router.spent_micros(), router.reserved_micros()
    );
}
