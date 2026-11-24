use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::{
    audience::Roster,
    context::{self, Authority, Count, Kind, Packet, Purpose, Record},
    interpretation::{
        Candidate, Criterion, Error, Finding, LearnerFeedback, OpenReview, ReviewBook, ReviewOrder,
        ReviewRevision, ReviewerPermit,
    },
    material::document_id,
    protocol::Assistance,
    routing::{
        Action, Completion, Intent, Model, Need, Options, Request, Returned, Role, Router, Ticket,
    },
};

struct Fixture {
    records: Vec<Record>,
    context: context::Request,
    authority: Authority,
    packet: Packet,
    router: Router,
    ticket: Ticket,
    request: Request,
    order: ReviewOrder,
    roster: Roster,
}

const RESPONSE: &str = "¼ + ¾ fills the same whole";
const QUOTED: &[u8] = br#"["not_yet","the same whole"]"#;

impl Fixture {
    fn new(bytes: Option<&[u8]>) -> Self {
        Self::with_order(bytes, 42, 73, None)
    }

    fn with_order(
        bytes: Option<&[u8]>,
        learner: u64,
        operation: u64,
        replaces: Option<ReviewRevision>,
    ) -> Self {
        Self::with_material(bytes, learner, operation, replaces, |_| {})
    }

    fn with_material(
        bytes: Option<&[u8]>,
        learner: u64,
        operation: u64,
        replaces: Option<ReviewRevision>,
        specialize: impl FnOnce(&mut [Record]),
    ) -> Self {
        Self::for_presentation(bytes, learner, operation, replaces, 25, specialize)
    }

    fn for_presentation(
        bytes: Option<&[u8]>,
        learner: u64,
        operation: u64,
        replaces: Option<ReviewRevision>,
        presentation: u64,
        specialize: impl FnOnce(&mut [Record]),
    ) -> Self {
        Self::for_objective(
            bytes,
            learner,
            operation,
            replaces,
            presentation,
            10,
            specialize,
        )
    }

    fn for_objective(
        bytes: Option<&[u8]>,
        learner: u64,
        operation: u64,
        replaces: Option<ReviewRevision>,
        presentation: u64,
        outcome: u32,
        specialize: impl FnOnce(&mut [Record]),
    ) -> Self {
        let (mut context, _, mut records) = context::example(Purpose::Evaluate);
        context.snapshot.scope.learner = learner;
        context.snapshot.outcome = tutor_lab::curriculum::OutcomeId(outcome);
        for record in &mut records {
            record.outcome = context.snapshot.outcome;
            if record.learner.is_some() {
                record.learner = Some(learner);
            }
        }
        context.input_limit = 4096;
        records[0].text = "Evaluate the explanation for adding equal fractional units.".into();
        records[2].text = "Explains why the result counts parts of a common whole.".into();
        records[3].text = RESPONSE.into();
        specialize(&mut records);
        let authority = Authority::new(&context, &records).unwrap();
        // This fixture meters bytes as units on a synthetic model profile.
        let packet = context::compile(&context, &authority, &records, &|m| {
            Count::Measured(m.wire().len() as u32)
        })
        .unwrap();
        let criterion = Criterion {
            id: 123,
            presentation,
            rubric: records[2].key,
            response: records[3].key,
            assistance: Assistance::Unknown,
        };
        let order = ReviewOrder {
            criterion,
            replaces,
        };
        let request = Request {
            id: operation,
            binding: order.binding(),
            max_micros: 10,
            deadline_ms: 18,
            need: Need {
                scope: packet.scope(),
                outcome: packet.outcome(),
                intent: Intent::AssessOpen,
                packet: packet.digest(),
            },
        };
        let model = Model {
            revision: 1,
            maximum_micros: 10,
            admission_ms: 2,
            slots: 1,
            max_input_tokens: 4096,
            max_output_tokens: 100,
        };
        let mut router = Router::new(1, 20, 1, 1, BTreeMap::from([(Role::Assessor, model)]));
        let options = Options {
            permitted: BTreeSet::from([Role::Assessor]),
            ..Options::default()
        };
        let Action::Dispatch(ticket) = router.route(request, &packet, &authority, &options, 1)
        else {
            panic!("expected assessor")
        };
        if let Some(bytes) = bytes {
            router
                .settle(
                    ticket,
                    Completion {
                        result: Returned::Draft(document_id(bytes)),
                        charged_micros: 5,
                    },
                )
                .unwrap();
        }
        let roster = Roster {
            class: context.snapshot.class,
            teachers: BTreeSet::from([8, 9]),
            learners: BTreeSet::from([context.snapshot.scope.learner, 100]),
        };
        Self {
            records,
            context,
            authority,
            packet,
            router,
            ticket,
            request,
            order,
            roster,
        }
    }

    fn admit<'a>(
        &self,
        cases: &'a mut ReviewBook,
        bytes: &[u8],
        now: u64,
    ) -> Result<&'a mut OpenReview, Error> {
        let candidate = Candidate {
            ticket: self.ticket,
            bytes,
            rubric: &self.records[2],
            response: &self.records[3],
        };
        cases.admit(
            self.order,
            candidate,
            &self.router,
            &self.packet,
            &self.authority,
            now,
        )
    }
}

#[test]
fn exact_quote_can_support_a_wrong_suggestion_and_only_teacher_feedback_is_disclosed() {
    let mut cases = ReviewBook::default();
    let f = Fixture::new(Some(QUOTED));
    let review = f.admit(&mut cases, QUOTED, 17).unwrap();
    let learner = f.packet.scope().learner;
    let permit = ReviewerPermit {
        teacher: 8,
        request: f.request,
    };
    let suggested = review
        .inspect(8, &permit, &f.roster, &f.packet, &f.authority)
        .unwrap()
        .unwrap();
    assert_eq!(suggested.proposed, Finding::NotYet);
    assert_eq!(&RESPONSE[suggested.start..suggested.end], "the same whole");
    assert_eq!(
        review
            .learner_view(learner, &f.roster, &f.packet, &f.authority)
            .unwrap(),
        LearnerFeedback::Pending
    );
    assert_eq!(
        review.learner_view(100, &f.roster, &f.packet, &f.authority),
        Err(Error::Unauthorized)
    );

    let feedback = "You accounted for the same whole with one quarter and three quarters.";
    assert_eq!(
        review.adjudicate(
            9,
            &permit,
            &f.roster,
            &f.packet,
            &f.authority,
            (Finding::Meets, feedback)
        ),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        review.adjudicate(
            learner,
            &permit,
            &f.roster,
            &f.packet,
            &f.authority,
            (Finding::Meets, feedback)
        ),
        Err(Error::Unauthorized)
    );
    for _ in 0..2 {
        review
            .adjudicate(
                8,
                &permit,
                &f.roster,
                &f.packet,
                &f.authority,
                (Finding::Meets, feedback),
            )
            .unwrap();
    }
    assert_eq!(
        review
            .learner_view(learner, &f.roster, &f.packet, &f.authority)
            .unwrap(),
        LearnerFeedback::Reviewed {
            criterion: 123,
            presentation: 25,
            unit_revision: f.packet.scope().revision,
            assistance: Assistance::Unknown,
            finding: Finding::Meets,
            text: feedback,
        }
    );
    assert_eq!(
        review.adjudicate(
            8,
            &permit,
            &f.roster,
            &f.packet,
            &f.authority,
            (Finding::NotYet, "Try again.")
        ),
        Err(Error::AlreadyReviewed)
    );
}

#[test]
fn invalid_candidate_or_missing_receipt_cannot_enter_the_review_queue() {
    let mut cases = ReviewBook::default();
    for bytes in [
        br#"["met",""]"#.as_slice(),
        br#"["not_yet","  "]"#,
        br#"["met","e"]"#,
        br#"["met","a false quote"]"#,
        br#"["grade","whole"]"#,
        br#"{"text":"looks convincing"}"#,
        br#"["met","whole","extra"]"#,
    ] {
        let f = Fixture::new(Some(bytes));
        assert!(matches!(
            f.admit(&mut cases, bytes, 17),
            Err(Error::InvalidOutput)
        ));
    }
    let f = Fixture::new(None);
    assert!(matches!(
        f.admit(&mut cases, QUOTED, 17),
        Err(Error::MissingReceipt)
    ));
    let f = Fixture::new(Some(b"null"));
    assert!(matches!(
        f.admit(&mut cases, QUOTED, 17),
        Err(Error::InvalidOutput)
    ));
    assert!(matches!(
        f.admit(&mut cases, b"null", 18),
        Err(Error::Stale)
    ));
    let review = f.admit(&mut cases, b"null", 17).unwrap();
    let permit = ReviewerPermit {
        teacher: 8,
        request: f.request,
    };
    assert_eq!(
        review
            .inspect(8, &permit, &f.roster, &f.packet, &f.authority)
            .unwrap(),
        None
    );
}

#[test]
fn substituted_records_and_withdrawn_rubrics_hide_prior_feedback() {
    let mut cases = ReviewBook::default();
    let mut f = Fixture::new(Some(QUOTED));
    let review = f.admit(&mut cases, QUOTED, 17).unwrap();
    let permit = ReviewerPermit {
        teacher: 8,
        request: f.request,
    };
    let learner = f.packet.scope().learner;
    review
        .adjudicate(
            8,
            &permit,
            &f.roster,
            &f.packet,
            &f.authority,
            (Finding::Meets, "You identified the common whole."),
        )
        .unwrap();
    let key = review.key();
    f.order.criterion.assistance = Assistance::Reported;
    assert!(matches!(
        f.admit(&mut cases, QUOTED, 17),
        Err(Error::WrongContract)
    ));
    f.order.criterion.assistance = Assistance::Unknown;
    f.records[3].text.push('.');
    assert!(matches!(
        f.admit(&mut cases, QUOTED, 17),
        Err(Error::WrongContract)
    ));
    f.records[3].text = RESPONSE.into();
    let review = cases.current(key).unwrap();
    let original = f.records.clone();
    for kind in [Kind::Rubric, Kind::Material, Kind::Response] {
        let mut changed = original.clone();
        let record = changed.iter_mut().find(|r| r.kind == kind).unwrap();
        record.key.revision += 1;
        let changed_authority = Authority::new(&f.context, &changed).unwrap();
        assert_eq!(
            review
                .learner_view(learner, &f.roster, &f.packet, &changed_authority)
                .unwrap(),
            LearnerFeedback::UnderReview
        );
    }
    // Restore the exact old bytes in a new epoch. The withdrawn job stays fenced.
    f.context.snapshot.epoch += 1;
    let after_rollback = Authority::new(&f.context, &original).unwrap();
    assert_eq!(
        review
            .learner_view(learner, &f.roster, &f.packet, &after_rollback)
            .unwrap(),
        LearnerFeedback::UnderReview
    );
    assert_eq!(
        review.inspect(8, &permit, &f.roster, &f.packet, &after_rollback),
        Err(Error::Stale)
    );
    f.roster.learners.remove(&learner);
    assert_eq!(
        review.learner_view(learner, &f.roster, &f.packet, &f.authority),
        Err(Error::Unauthorized)
    );
}

#[test]
fn replay_resolves_one_signed_case_and_only_its_named_successor_can_reopen_it() {
    let f = Fixture::new(Some(QUOTED));
    let mut cases = ReviewBook::default();
    let permit = ReviewerPermit {
        teacher: 8,
        request: f.request,
    };
    let review = f.admit(&mut cases, QUOTED, 17).unwrap();
    let key = review.key();
    let revision = review.revision();
    let original = (Finding::Meets, "Your response identified the same whole.");
    review
        .adjudicate(8, &permit, &f.roster, &f.packet, &f.authority, original)
        .unwrap();

    let replay = f.admit(&mut cases, QUOTED, 17).unwrap();
    replay
        .adjudicate(8, &permit, &f.roster, &f.packet, &f.authority, original)
        .unwrap();
    assert_eq!(
        replay.adjudicate(
            8,
            &permit,
            &f.roster,
            &f.packet,
            &f.authority,
            (Finding::NotYet, "Different judgment.")
        ),
        Err(Error::AlreadyReviewed)
    );
    let view = replay
        .learner_view(key.learner, &f.roster, &f.packet, &f.authority)
        .unwrap();
    assert!(matches!(
        view,
        LearnerFeedback::Reviewed {
            finding: Finding::Meets,
            ..
        }
    ));

    let other = Fixture::with_order(Some(b"null"), key.learner, 74, None);
    assert!(matches!(
        other.admit(&mut cases, b"null", 17),
        Err(Error::CaseConflict)
    ));
    let next = Fixture::with_order(Some(b"null"), key.learner, 75, Some(revision));
    let replacement = next.admit(&mut cases, b"null", 17).unwrap();
    assert_eq!(replacement.key(), key);
    assert_eq!(
        replacement
            .learner_view(key.learner, &next.roster, &next.packet, &next.authority)
            .unwrap(),
        LearnerFeedback::Pending
    );
    assert_eq!(
        replacement.adjudicate(8, &permit, &f.roster, &f.packet, &f.authority, original),
        Err(Error::Unauthorized)
    );
    let next_permit = ReviewerPermit {
        teacher: 8,
        request: next.request,
    };
    let correction = (
        Finding::Unresolved,
        "The reference whole needs clarification.",
    );
    replacement
        .adjudicate(
            8,
            &next_permit,
            &next.roster,
            &next.packet,
            &next.authority,
            correction,
        )
        .unwrap();

    let obsolete = Fixture::with_order(Some(b"null"), key.learner, 76, Some(revision));
    assert!(matches!(
        obsolete.admit(&mut cases, b"null", 17),
        Err(Error::CaseConflict)
    ));
    assert!(matches!(
        f.admit(&mut cases, QUOTED, 17),
        Err(Error::CaseConflict)
    ));
    let current = cases.current(key).unwrap();
    let view = current
        .learner_view(key.learner, &next.roster, &next.packet, &next.authority)
        .unwrap();
    assert!(
        matches!(view, LearnerFeedback::Reviewed { finding: Finding::Unresolved, text, .. } if text == correction.1)
    );
}

#[test]
fn identical_presentation_and_criterion_numbers_on_another_seat_open_an_independent_case() {
    let f = Fixture::new(Some(QUOTED));
    let other = Fixture::with_order(Some(b"null"), 100, 77, None);
    let mut cases = ReviewBook::default();
    let first = f.admit(&mut cases, QUOTED, 17).unwrap();
    let key = first.key();
    let permit = ReviewerPermit {
        teacher: 8,
        request: f.request,
    };
    first
        .adjudicate(
            8,
            &permit,
            &f.roster,
            &f.packet,
            &f.authority,
            (Finding::Meets, "You identified the reference whole."),
        )
        .unwrap();

    let second = other.admit(&mut cases, b"null", 17).unwrap();
    assert_ne!(key, second.key());
    assert_eq!(
        second
            .learner_view(100, &other.roster, &other.packet, &other.authority)
            .unwrap(),
        LearnerFeedback::Pending
    );
    assert_eq!(
        second.inspect(8, &permit, &other.roster, &other.packet, &other.authority),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        cases
            .current(key)
            .unwrap()
            .learner_view(100, &f.roster, &f.packet, &f.authority),
        Err(Error::Unauthorized)
    );
}

#[test]
fn voluntary_working_can_be_reviewed_as_an_equation_without_sending_its_private_preamble() {
    const PERSONAL: &str = "I learned this while caring for my uncle.";
    const EQUATION: &str = "3 - 2 = 1";
    const CANDIDATE: &[u8] = br#"["met","3 - 2 = 1"]"#;
    let full = format!("{PERSONAL} {EQUATION}");
    for (response, whole) in [(full.as_str(), true), (EQUATION, false)] {
        let f = Fixture::with_material(Some(CANDIDATE), 42, 73, None, |records| {
            records[0].text =
                "Consider only whether the working shows conversion of quarters to eighths.".into();
            records[1].text = "Twice as many eighths as quarters cover the same length.".into();
            records[2].text =
                "The writing must visibly express quarter and eighth amounts for the same length."
                    .into();
            records[3].text = response.into();
        });
        let wire = f.packet.dispatch(&f.authority).unwrap().wire();
        assert_eq!(wire.contains(PERSONAL), whole);
        let mut cases = ReviewBook::default();
        let review = f.admit(&mut cases, CANDIDATE, 17).unwrap();
        let permit = ReviewerPermit {
            teacher: 8,
            request: f.request,
        };
        let inspected = review
            .inspect(8, &permit, &f.roster, &f.packet, &f.authority)
            .unwrap()
            .unwrap();
        assert_eq!(inspected.proposed, Finding::Meets);
        assert_eq!(&response[inspected.start..inspected.end], EQUATION);
        assert_eq!(
            review.learner_view(42, &f.roster, &f.packet, &f.authority),
            Ok(LearnerFeedback::Pending)
        );
        assert_eq!(
            review.inspect(9, &permit, &f.roster, &f.packet, &f.authority),
            Err(Error::Unauthorized)
        );
        let note="Your line shows a subtraction. It does not tell me which units you used. You can add the units or leave that question open.";
        review
            .adjudicate(
                8,
                &permit,
                &f.roster,
                &f.packet,
                &f.authority,
                (Finding::Unresolved, note),
            )
            .unwrap();
        assert!(
            matches!(review.learner_view(42,&f.roster,&f.packet,&f.authority),Ok(LearnerFeedback::Reviewed {finding:Finding::Unresolved,text,..}) if text==note)
        );
        assert_eq!(
            review.learner_view(100, &f.roster, &f.packet, &f.authority),
            Err(Error::Unauthorized)
        );
        let visible: Vec<_> = f
            .records
            .iter()
            .filter(|r| r.kind != Kind::Response)
            .cloned()
            .collect();
        let withdrawn = Authority::new(&f.context, &visible).unwrap();
        assert_eq!(
            review.learner_view(42, &f.roster, &f.packet, &withdrawn),
            Ok(LearnerFeedback::UnderReview)
        );
        println!("whole={whole} assessor_bytes={} equation_span={}..{} before=Pending signed=Unresolved after_withdrawal=UnderReview",wire.len(),inspected.start,inspected.end);
    }
}

#[test]
fn optional_signed_review_and_access_to_parallel_instruction_compete_for_different_resources() {
    use tutor_lab::{
        curriculum::Span,
        interpretation::CaseKey,
        material::{FractionCard, FractionTask, RuleSource, UNIT_RULE},
    };

    struct Case {
        fixture: Fixture,
        key: CaseKey,
        withdrawn: Authority,
    }

    #[derive(Debug)]
    struct Results {
        signed: Vec<(u64, u64)>,
        first_instruction: Vec<Option<usize>>,
        withheld_turns: usize,
        pending: usize,
        under_review: usize,
    }

    // All learners opt into the reviewed worked example. Tick zero precedes
    // the first signature. A learner can see each update after a service slot.
    fn simulate(
        seats: usize,
        older: usize,
        slots: usize,
        learner_priority: bool,
        block: bool,
        withdrawal: Option<u64>,
    ) -> Results {
        let mut book = ReviewBook::default();
        let mut cases = Vec::new();
        let mut current = Vec::new();
        for seat in 1..=seats {
            let count = if seat == 1 { older + 1 } else { 1 };
            for _ in 0..count {
                let id = cases.len() as u64 + 1;
                let f = Fixture::for_presentation(Some(QUOTED), seat as u64, id, None, id, |_| {});
                let key = f.admit(&mut book, QUOTED, 17).unwrap().key();
                // The queue indexes this stable case key. Repeated submissions
                // from the same opening reuse the same admitted review.
                assert_eq!(key, f.admit(&mut book, QUOTED, 17).unwrap().key());
                let remaining: Vec<_> = f
                    .records
                    .iter()
                    .filter(|r| r.kind != Kind::Response)
                    .cloned()
                    .collect();
                let withdrawn = Authority::new(&f.context, &remaining).unwrap();
                cases.push(Case {
                    fixture: f,
                    key,
                    withdrawn,
                });
            }
            current.push(cases.len() - 1);
        }
        let pin = Span {
            document: document_id(UNIT_RULE.as_bytes()),
            start: 0,
            end: UNIT_RULE.len() as u32,
        };
        let active = BTreeSet::new();
        let source = RuleSource {
            bytes: Some(UNIT_RULE.as_bytes()),
            pin,
            revoked: &active,
        };
        let card = FractionCard::propose(FractionTask::Sum, 1, 3, 8).unwrap();
        let mut results = Results {
            signed: Vec::new(),
            first_instruction: vec![None; seats],
            withheld_turns: 0,
            pending: 0,
            under_review: 0,
        };
        let mut served = vec![0; seats];
        for now in 0..=slots {
            let mut waiting = Vec::new();
            results.pending = 0;
            results.under_review = 0;
            for (i, case) in cases.iter().enumerate() {
                let seat = case.key.learner;
                let s = seat as usize - 1;
                let f = &case.fixture;
                let authority = if withdrawal == Some(seat) && now >= 1 {
                    &case.withdrawn
                } else {
                    &f.authority
                };
                let review = book.current(case.key).unwrap();
                let view = review
                    .learner_view(seat, &f.roster, &f.packet, authority)
                    .unwrap();
                let can_proceed = matches!(view, LearnerFeedback::Reviewed { .. }) || !block;
                if i == current[s] && results.first_instruction[s].is_none() {
                    if can_proceed {
                        let (example, _) = card.worked(&source).unwrap();
                        assert!(example.contains("4/8"));
                        results.first_instruction[s] = Some(now);
                    } else if now < slots {
                        results.withheld_turns += 1;
                    }
                }
                match view {
                    LearnerFeedback::Pending => {
                        results.pending += 1;
                        waiting.push((if learner_priority { served[s] } else { 0 }, i));
                    }
                    LearnerFeedback::UnderReview => {
                        results.under_review += 1;
                    }
                    LearnerFeedback::Reviewed {
                        finding: Finding::Unresolved,
                        ..
                    } => {}
                    _ => panic!("unexpected finding"),
                }
            }
            if now == slots {
                break;
            }
            if let Some((_, i)) = waiting.iter().min() {
                let case = &cases[*i];
                let seat = case.key.learner;
                let f = &case.fixture;
                let permit = ReviewerPermit {
                    teacher: 8,
                    request: f.request,
                };
                let active = book.current(case.key).unwrap();
                active
                    .adjudicate(
                        8,
                        &permit,
                        &f.roster,
                        &f.packet,
                        &f.authority,
                        (
                            Finding::Unresolved,
                            "This line does not show your units. You may leave this open.",
                        ),
                    )
                    .unwrap();
                results.signed.push((seat, case.key.presentation));
                served[seat as usize - 1] += 1;
            }
        }
        results
    }

    for (seats, older, slots, withdrawn) in [
        (1, 0, 1, None),
        (6, 2, 6, None),
        (6, 2, 6, Some(3)),
        (6, 2, 0, None),
    ] {
        for learner_priority in [false, true] {
            let gated = simulate(seats, older, slots, learner_priority, true, withdrawn);
            let parallel = simulate(seats, older, slots, learner_priority, false, withdrawn);
            assert_eq!(gated.signed, parallel.signed);
            assert_eq!(
                (gated.pending, gated.under_review),
                (parallel.pending, parallel.under_review)
            );
            assert_eq!(parallel.first_instruction, vec![Some(0); seats]);
            assert_eq!(parallel.withheld_turns, 0);
            assert_eq!(
                parallel.pending + parallel.under_review + parallel.signed.len(),
                seats + older
            );
            if seats == 6 && slots == 6 && withdrawn.is_none() {
                let recipients: BTreeSet<_> = gated.signed.iter().map(|s| s.0).collect();
                assert_eq!(recipients.len(), if learner_priority { 6 } else { 4 });
            }
            println!("seats={seats} older={older} slots={slots} learner_priority={learner_priority} withdrawal={withdrawn:?} signed={:?} pending={} under_review={} gated_first_instruction={:?} gated_wait={} parallel_first_instruction={:?}",
                gated.signed,gated.pending,gated.under_review,gated.first_instruction,gated.withheld_turns,parallel.first_instruction);
        }
    }
}

#[test]
fn a_class_can_offer_instruction_while_only_two_hall_responses_have_adjudicable_evidence() {
    use serde_json::Value;
    use tutor_lab::rehearsal::{Action, Session};
    let mut session = Session::new().unwrap();
    let goal = |session: &mut Session, words: &[&str]| {
        session
            .apply(Action::Goal(words.iter().map(|s| s.to_string()).collect()))
            .unwrap()
            .1
    };
    for words in [
        &["choose", "101"][..],
        &["visit"],
        &["move", "101", "check"],
        &["move", "101", "private"],
        &["visit"],
        &["request", "101"],
    ] {
        goal(&mut session, words);
    }
    session.apply(Action::CaseReview("hall".into())).unwrap();
    let hall = goal(&mut session, &["move", "101", "check"]);
    let check = hall["display"]["case"].as_str().unwrap();
    assert!(check.contains("dry hall"));
    let pack: Value = serde_json::from_str(include_str!("../fixtures/goals/worship.json")).unwrap();
    let base = &pack["first_pack"];
    let roster = Roster {
        class: 7,
        teachers: [71].into(),
        learners: [11, 12, 13, 14, 15, 16].into(),
    };
    let make = |seat, answer: &str, quoted: &[u8]| {
        Fixture::for_objective(Some(quoted), seat, seat, None, seat + 100, 101, |records| {
            records[0].text = check.into();
            records[1].text = base["excerpt"].as_str().unwrap().into();
            records[2].text = base["rubric"].as_str().unwrap().into();
            records[3].text = answer.into();
        })
    };
    let quoted_a = br#"["not_yet","Stop the rain"]"#;
    let quoted_b = br#"["not_yet","dry hall"]"#;
    let a = make(15, "Stop the rain so we can sing outdoors.", quoted_a);
    let b=make(16,"Thank you for this dry hall. 'Give thanks' led me to name what the choir already received.",quoted_b);
    let mut cases = ReviewBook::default();
    let mut proposed_not_yet = 0;
    let mut signed = BTreeMap::new();
    let mut b_key = None;
    for (f, candidate, seat) in [(&a, quoted_a.as_slice(), 15), (&b, quoted_b.as_slice(), 16)] {
        let rev = f.admit(&mut cases, candidate, 17).unwrap();
        let permit = ReviewerPermit {
            teacher: 71,
            request: f.request,
        };
        assert_eq!(
            rev.inspect(71, &permit, &roster, &f.packet, &f.authority)
                .unwrap()
                .unwrap()
                .proposed,
            Finding::NotYet
        );
        proposed_not_yet += 1;
        assert_eq!(
            rev.signed_finding(71, &permit, &roster, &f.packet, &f.authority),
            Ok(None)
        );
        if seat == 15 {
            rev.adjudicate(
                71,
                &permit,
                &roster,
                &f.packet,
                &f.authority,
                (
                    Finding::NotYet,
                    "That asks for a change. Which gift has the fictional choir already received?",
                ),
            )
            .unwrap();
            signed.insert(
                seat,
                rev.signed_finding(71, &permit, &roster, &f.packet, &f.authority)
                    .unwrap()
                    .unwrap(),
            );
        } else {
            b_key = Some(rev.key());
        }
    }
    assert_eq!(signed.len(), 1);
    println!("6 enrolled in A; 2 privately submitted; proposals not_yet={proposed_not_yet}; before second signature: one signed not_yet, five without a signed judgment");
    let rev = cases.current(b_key.unwrap()).unwrap();
    let permit = ReviewerPermit {
        teacher: 71,
        request: b.request,
    };
    rev.adjudicate(
        71,
        &permit,
        &roster,
        &b.packet,
        &b.authority,
        (
            Finding::Meets,
            "The fictional singer names a received gift and connects it to the verse.",
        ),
    )
    .unwrap();
    signed.insert(
        16,
        rev.signed_finding(71, &permit, &roster, &b.packet, &b.authority)
            .unwrap()
            .unwrap(),
    );
    assert_eq!(signed[&16], Finding::Meets);
    assert_eq!(signed[&15], Finding::NotYet);
    assert_eq!(
        rev.signed_finding(91, &permit, &roster, &b.packet, &b.authority),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        rev.signed_finding(14, &permit, &roster, &b.packet, &b.authority),
        Err(Error::Unauthorized)
    );
    let without_artifact: Vec<_> = b
        .records
        .iter()
        .filter(|r| r.kind != Kind::Response)
        .cloned()
        .collect();
    let withdrawn = Authority::new(&b.context, &without_artifact).unwrap();
    assert_eq!(
        rev.signed_finding(71, &permit, &roster, &b.packet, &withdrawn),
        Err(Error::Stale)
    );
    // A worked example can be shown when invited, independently of interpreting someone else's writing.
    let example = goal(&mut session, &["move", "101", "repeat"]);
    assert_eq!(example["display"]["worked"], base["worked"]);
    let public = example["display"].to_string();
    assert!(!public.contains("Stop the rain") && !public.contains("dry hall"));
    println!("after second signature: one of two signed not_yet, four without a signed judgment; peer reads=denied; outside teacher reads=denied; withdrawn response excluded; worked example available");
}
