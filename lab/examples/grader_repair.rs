//! Replay a corrected criterion from a submitted answer to a later curriculum decision.
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::{
    audience::Roster,
    context::{self, Authority, Count, Kind, Purpose},
    curriculum::{Decision, Outcome, OutcomeId, ReviewedUnit, Slot, Span},
    history::{
        adjudication::{DisputedAssessment, ReviewAccess},
        reconcile, Attempt, CheckpointProposal, Scope, Withdrawn,
    },
    interpretation::{
        Candidate, Criterion, Error, Finding, LearnerFeedback, ReviewBook, ReviewOrder,
        ReviewerPermit,
    },
    material::document_id,
    protocol::{Assistance, Judgment, Question},
    routing::{Action, Completion, Intent, Model, Need, Options, Request, Returned, Role, Router},
};

const CLASSIFY: OutcomeId = OutcomeId(1);
const ADAPT: OutcomeId = OutcomeId(2);
const UNIT: u64 = 1;
const TEACHER: u64 = 80;
const CLASS: u64 = 7;
const RULE: &str = "An assessment serves a formative purpose when its evidence changes ongoing teaching or learning.";
const VIGNETTE: &str = "A teacher gives a quiz, does not show students their scores, and uses the results to plan small groups.";
const DIFFERENT: &str = "A lecturer files final scores after the course ends. What evidence would show that the scores guided instruction during this course?";
const NEGATION: &str = "Does that prevent it from counting as formative assessment?";
const SUMMATIVE: &str = "Does it follow that the assessment is purely summative?";
const DENIAL_REPAIR: &str = "Your no answered the question as written. You traced how the quiz changed the grouping. I marked that answer incorrectly.";
const CREDIT_REPAIR: &str = "I credited a conclusion that conflicts with your example. The teacher changed the groups during instruction, which gives the quiz a formative use.";

#[derive(Clone, Copy)]
struct Case {
    learner: u64,
    item: u64,
    stem: &'static str,
    answer: &'static str,
    legacy: Judgment,
}

struct History {
    scope: Scope,
    class: u64,
    attempts: Vec<Attempt>,
    pairs: Vec<CheckpointProposal>,
    withdrawn: Withdrawn,
}

impl History {
    fn first_visit(case: Case, span: Span) -> Self {
        let scope = Scope {
            learner: case.learner,
            revision: UNIT,
        };
        // Two distinct published tasks in the day-one visit. The first case describes
        // sorting exit tickets to change tomorrow's lesson, with no yes/no wording.
        let first = Attempt {
            scope,
            id: 1,
            sequence: 1,
            outcome: CLASSIFY,
            item: 100,
            day: 1,
            question: Question::IndependentCheck,
            judgment: Judgment::Correct,
            assistance: Assistance::NoneReported,
            fresh: true,
            sources: [span.document].into(),
        };
        let check = Attempt {
            id: 2,
            sequence: 2,
            item: case.item,
            question: Question::ChangedExample,
            judgment: case.legacy,
            ..first.clone()
        };
        Self {
            scope,
            class: CLASS,
            attempts: vec![first, check],
            pairs: vec![CheckpointProposal {
                scope,
                outcome: CLASSIFY,
                first: 1,
                confirmation: 2,
                due: 10,
            }],
            withdrawn: Withdrawn::new(scope),
        }
    }

    fn plan(&self, unit: &ReviewedUnit, day: u32) -> Decision {
        let progress = reconcile(
            self.scope,
            day,
            &self.attempts,
            &self.pairs,
            &self.withdrawn,
        )
        .unwrap();
        unit.next(
            ADAPT,
            &progress,
            day,
            &self.withdrawn.sources,
            Slot::Forward,
        )
        .unwrap()
    }
}

fn open_dispute(
    case: Case,
    decision: Option<(Finding, &str)>,
    history: &mut History,
    unit: &ReviewedUnit,
    roster: &Roster,
    router: &mut Router,
    book: &mut ReviewBook,
) {
    let (mut brief, _, mut records) = context::example(Purpose::Evaluate);
    brief.snapshot.scope = history.scope;
    brief.snapshot.outcome = CLASSIFY;
    brief.snapshot.class = roster.class;
    brief.input_limit = 4096;
    for record in &mut records {
        record.outcome = CLASSIFY;
        record.learner = record.learner.map(|_| case.learner);
        record.text = match record.kind {
            Kind::Brief => format!("{VIGNETTE} {}", case.stem),
            Kind::Material => RULE.into(),
            Kind::Rubric => "Classify the stated *use* of the quiz and justify the answer for this exact proposition.".into(),
            Kind::Response => case.answer.into(),
            _ => unreachable!(),
        };
        // Scope mutable examples' record IDs to the actual issue, including paraphrases.
        record.key.id += case.learner * 10;
    }
    brief.required = records.iter().map(|r| r.key).collect();
    let authority = Authority::new(&brief, &records).unwrap();
    let packet = context::compile(&brief, &authority, &records, &|m| {
        Count::Measured(m.wire().len() as u32)
    })
    .unwrap();
    let rubric = records.iter().find(|r| r.kind == Kind::Rubric).unwrap();
    let response = records.iter().find(|r| r.kind == Kind::Response).unwrap();
    let original = history.attempts[1].clone();
    let disputed = DisputedAssessment {
        class: roster.class,
        criterion: Criterion {
            id: 1,
            presentation: original.sequence,
            rubric: rubric.key,
            response: response.key,
            assistance: original.assistance,
        },
        original,
    };
    let order = ReviewOrder {
        criterion: disputed.criterion,
        replaces: None,
    };
    let request = Request {
        id: case.learner,
        deadline_ms: 100,
        max_micros: 10,
        binding: order.binding(),
        need: Need {
            scope: history.scope,
            outcome: CLASSIFY,
            intent: Intent::AssessOpen,
            packet: packet.digest(),
        },
    };
    let options = Options {
        permitted: [Role::Assessor].into(),
        ..Options::default()
    };
    let Action::Dispatch(ticket) = router.route(request, &packet, &authority, &options, 1) else {
        panic!("expected scoped reviewer job");
    };
    // The simulated provider abstains. Only the specified human adjudication can release a revision.
    let bytes = b"null";
    router
        .settle(
            ticket,
            Completion {
                result: Returned::Draft(document_id(bytes)),
                charged_micros: 4,
            },
        )
        .unwrap();
    let review = book
        .admit(
            order,
            Candidate {
                ticket,
                bytes,
                rubric,
                response,
            },
            router,
            &packet,
            &authority,
            2,
        )
        .unwrap();
    let permit = ReviewerPermit {
        teacher: TEACHER,
        request,
    };
    let access = ReviewAccess {
        actor: TEACHER,
        permit: &permit,
        roster,
        packet: &packet,
        authority: &authority,
    };

    history.withdrawn.attempts.insert(disputed.original.id);
    history.withdrawn.postponed.insert(CLASSIFY);
    assert!(disputed
        .replacement(3, None, review, &access)
        .unwrap()
        .is_none());
    assert_eq!(history.plan(unit, 7), Decision::Postponed(CLASSIFY));
    assert_eq!(
        review.learner_view(44, roster, &packet, &authority),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        review.learner_view(case.learner, roster, &packet, &authority),
        Ok(LearnerFeedback::Pending)
    );
    let Some((finding, note)) = decision else {
        return;
    };
    review
        .adjudicate(
            TEACHER,
            &permit,
            roster,
            &packet,
            &authority,
            (finding, note),
        )
        .unwrap();
    assert!(
        matches!(review.learner_view(case.learner,roster,&packet,&authority),
        Ok(LearnerFeedback::Reviewed{text,..}) if text==note)
    );
    let unrelated = DisputedAssessment {
        criterion: Criterion {
            id: 2,
            ..disputed.criterion
        },
        class: disputed.class,
        original: disputed.original.clone(),
    };
    assert!(matches!(
        unrelated.replacement(3, None, review, &access),
        Err(Error::WrongContract)
    ));
    let correction = disputed
        .replacement(3, None, review, &access)
        .unwrap()
        .unwrap();
    let pair = correction.rebind(history.pairs[0]).unwrap();
    assert_eq!(pair.due, 10);
    assert_eq!(
        (correction.attempt().day, correction.attempt().sequence),
        (1, 2)
    );
    history.pairs.push(pair);
    history.attempts.push(correction.attempt().clone());
    history.withdrawn.postponed.remove(&CLASSIFY);
    println!(
        "Private feedback for synthetic learner {}: {note}",
        case.learner
    );
}

/// The classroom view projects the assigned learners' next steps and carries no response text.
fn counts(
    actor: u64,
    roster: &Roster,
    histories: &[History],
    unit: &ReviewedUnit,
    day: u32,
) -> Option<[usize; 3]> {
    if !roster.teachers.contains(&actor) {
        return None;
    }
    let mut counts = [0; 3];
    for h in histories
        .iter()
        .filter(|h| h.class == roster.class && roster.learners.contains(&h.scope.learner))
    {
        match h.plan(unit, day) {
            Decision::Teach(ADAPT) => counts[0] += 1,
            Decision::Teach(CLASSIFY) => counts[1] += 1,
            Decision::Postponed(CLASSIFY) => counts[2] += 1,
            _ => panic!("unexpected day-seven activity"),
        }
    }
    Some(counts)
}

fn main() {
    let span = Span {
        document: document_id(RULE.as_bytes()),
        start: 0,
        end: RULE.len() as u32,
    };
    let unit = ReviewedUnit::build(
        UNIT,
        vec![
            Outcome {
                id: CLASSIFY,
                requires: BTreeSet::new(),
                support: [span].into(),
            },
            Outcome {
                id: ADAPT,
                requires: [CLASSIFY].into(),
                support: [span].into(),
            },
        ],
    )
    .unwrap();
    let cases = [
        Case {
            learner: 41,
            item: 101,
            stem: NEGATION,
            answer: "No. The teacher uses it to plan groups.",
            legacy: Judgment::Incorrect,
        },
        Case {
            learner: 42,
            item: 102,
            stem: SUMMATIVE,
            answer: "Yes. The teacher uses it to plan groups.",
            legacy: Judgment::Correct,
        },
        Case {
            learner: 43,
            item: 102,
            stem: SUMMATIVE,
            answer: "Yes. The teacher uses it to plan groups.",
            legacy: Judgment::Correct,
        },
        Case {
            learner: 44,
            item: 103,
            stem: "Is it formative assessment?",
            answer: "Yes. The teacher uses it to plan groups.",
            legacy: Judgment::Correct,
        },
    ];
    let roster = Roster {
        class: CLASS,
        teachers: [TEACHER].into(),
        learners: cases.iter().map(|c| c.learner).collect(),
    };
    let mut histories: Vec<_> = cases
        .iter()
        .map(|&c| History::first_visit(c, span))
        .collect();
    let before = counts(TEACHER, &roster, &histories, &unit, 7).unwrap();
    let mut book = ReviewBook::default();
    let model = Model {
        revision: 1,
        maximum_micros: 10,
        admission_ms: 1,
        slots: 1,
        max_input_tokens: 4096,
        max_output_tokens: 100,
    };
    let reviews = [
        Some((Finding::Meets, DENIAL_REPAIR)),
        Some((Finding::NotYet, CREDIT_REPAIR)),
        None,
    ];
    let mut router = Router::new(
        1,
        30,
        1,
        reviews.len(),
        BTreeMap::from([(Role::Assessor, model)]),
    );
    // The affirmative control has no dispute and shares the same roster.
    for ((history, case), decision) in histories.iter_mut().zip(cases).zip(reviews) {
        open_dispute(
            case,
            decision,
            history,
            &unit,
            &roster,
            &mut router,
            &mut book,
        );
    }
    let after = counts(TEACHER, &roster, &histories, &unit, 7).unwrap();
    assert_eq!(before, [3, 1, 0]);
    assert_eq!(after, [2, 1, 1]);
    assert!(counts(41, &roster, &histories, &unit, 7).is_none());
    for (h, want) in histories.iter().zip([
        Decision::Teach(ADAPT),
        Decision::Teach(CLASSIFY),
        Decision::Postponed(CLASSIFY),
        Decision::Teach(ADAPT),
    ]) {
        let selected = h.plan(&unit, 7);
        assert_eq!(selected, want);
        let instruction = match selected {
            Decision::Teach(ADAPT) => "Six learners added the denominators on an exit ticket. Tomorrow you show two quarter-lengths on a ruler, then ask them to combine eighths. Which response would tell you that the example helped?",
            Decision::Teach(CLASSIFY) => "In the quiz case, the teacher changed the groups. Trace the sequence: quiz, grouping decision, next lesson. Now suppose a school uses a final exam to award a certificate. What decision did that evidence change?",
            Decision::Postponed(CLASSIFY) => "Your earlier judgment is under review. You can revisit the lesson on using evidence or move to another activity.",
            _ => unreachable!(),
        };
        println!(
            "Day 7, synthetic learner {}: {instruction}",
            h.scope.learner
        );
    }
    assert_eq!(histories[0].plan(&unit, 10), Decision::Recheck(CLASSIFY));
    println!("Independent return for learner 41 on day 10: {DIFFERENT}");
    println!("Class counts (forward lesson, foundational lesson, review pending): {before:?} -> {after:?}");
}
