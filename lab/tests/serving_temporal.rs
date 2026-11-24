//! Bounded temporal traces over the actual authoring, delivery and audience entry points.
use std::collections::{BTreeMap, BTreeSet};
use telo::monitor::Monitor;
use telo::predicate::{ClosurePredicate, Predicates};
use telo::Property;
use tutor_lab::audience::{
    learner_view, teacher_view, AssignedHandle, Assignment, AssignmentKey, LearnerAccess,
    LearnerView, Participation, ResultState, Roster, ViewError,
};
use tutor_lab::authoring::{AuthoringJob, CandidateBytes, Catalog, Publisher, Template};
use tutor_lab::context::{compile, example, Authority, Count, Purpose};
use tutor_lab::curriculum::Span;
use tutor_lab::material::{document_id, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::{Assistance, Command, Effect, Event, Gate, Question, Receipt};
use tutor_lab::routing::{
    Action, Completion, Intent, Model, Need, Options, Request, Returned, Role, Router, Ticket,
};
use tutor_lab::supply::{Enrollment, ExposureBook, Requirement};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    RemoveLearner,
    RemoveTeacher,
    RevokeSource,
    ReplaceRelease,
    Challenge,
    CorrectItem,
    Deadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Route,
    SettleFirst,
    SettleSecond,
    Publish,
    Open,
    Submit,
    Feedback,
    Interrupt(Fault),
    Read,
}

const PATH: [Step; 7] = [
    Step::Route,
    Step::SettleFirst,
    Step::SettleSecond,
    Step::Publish,
    Step::Open,
    Step::Submit,
    Step::Feedback,
];
const FAULTS: [Fault; 7] = [
    Fault::RemoveLearner,
    Fault::RemoveTeacher,
    Fault::RevokeSource,
    Fault::ReplaceRelease,
    Fault::Challenge,
    Fault::CorrectItem,
    Fault::Deadline,
];
const A: &[u8] = b"[1,2,4]";
const B: &[u8] = b"[2,2,4]";

#[derive(Clone, Copy, Debug)]
struct Frame {
    step: Step,
    publication_allowed: bool,
    published_now: bool,
    has_publication: bool,
    can_show: bool,
    question: bool,
    worked: bool,
    submitted: bool,
    released: bool,
    class_authorized: bool,
    teacher_sees: bool,
    named_in_denominator: bool,
    enrolled: bool,
    class_result: bool,
    peer_denied: bool,
    dispatched: bool,
    dispatch_allowed: bool,
    callbacks: u64,
    charged: u64,
}

fn observe(
    shared: bool,
    second_request_first: bool,
    interrupt: Option<(usize, Fault)>,
) -> Vec<Frame> {
    let purpose = if shared {
        Purpose::Author
    } else {
        Purpose::PrivateForm
    };
    let (mut brief, _, records) = example(purpose);
    brief.input_limit = 4096;
    let mut authority = Authority::new(&brief, &records).unwrap();
    let packet = compile(&brief, &authority, &records, &|m| {
        Count::Measured(m.wire().len() as u32)
    })
    .unwrap();
    let learner = brief.snapshot.scope.learner;
    let teacher = 99;
    let class = brief.snapshot.class;
    let mut book = ExposureBook::new(
        Enrollment {
            class,
            scope: packet.scope(),
        },
        true,
    );
    let mut roster = Roster {
        class,
        learners: BTreeSet::from([learner, learner + 1]),
        teachers: BTreeSet::from([teacher]),
    };
    let span = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let template = Template {
        brief: brief.required[0],
        material: brief.required[1],
        span,
        task: FractionTask::Sum,
        edition: 12,
    };
    let role = if shared {
        Role::Language
    } else {
        Role::Reasoner
    };
    let intent = if shared {
        Intent::AuthorForm
    } else {
        Intent::PersonalizeForm(Question::RequestedProbe)
    };
    let need = Need {
        scope: packet.scope(),
        outcome: packet.outcome(),
        intent,
        packet: packet.digest(),
    };
    let job = AuthoringJob {
        template,
        request: Request {
            id: 71,
            need,
            deadline_ms: 100,
            max_micros: 10,
            binding: template.binding(),
        },
    };
    let model = Model {
        revision: brief.snapshot.profile,
        maximum_micros: 10,
        admission_ms: 1,
        slots: 2,
        max_input_tokens: 4096,
        max_output_tokens: 80,
    };
    let mut router = Router::new(4, 20, 1, 2, BTreeMap::from([(role, model)]));
    let options = Options {
        permitted: BTreeSet::from([role]),
        ..Options::default()
    };
    let mut catalog = Catalog::default();
    let mut tickets: [Option<Ticket>; 2] = [None, None];
    let mut settled = [false, false];
    let mut callbacks = 0;
    let mut revoked = BTreeSet::new();
    let mut now = 1;
    let mut active_fault = None;
    let mut assignment: Option<Assignment> = None;
    let mut person: Option<Participation> = None;
    let mut handle: Option<AssignedHandle> = None;
    let mut submitted = false;
    let mut released = false;
    let mut steps = PATH.to_vec();
    if let Some((at, fault)) = interrupt {
        steps.insert(at, Step::Interrupt(fault));
    }
    steps.push(Step::Read);
    let mut trace = vec![];
    for step in steps {
        if let Step::Interrupt(fault) = step {
            active_fault = Some(fault);
            match fault {
                Fault::RemoveLearner => {
                    roster.learners.remove(&learner);
                }
                Fault::RemoveTeacher => {
                    roster.teachers.remove(&teacher);
                }
                Fault::RevokeSource => {
                    revoked.insert(span.document);
                }
                Fault::ReplaceRelease => {
                    brief.snapshot.scope.revision += 1;
                    if let Some(a) = &mut assignment {
                        a.form.release += 1;
                        a.publication += 1;
                    }
                }
                Fault::CorrectItem => {
                    if let Some(a) = &mut assignment {
                        a.corrected = true;
                    }
                }
                Fault::Deadline => now = 100,
                Fault::Challenge => {
                    if let Some(p) = &mut person {
                        p.control(Command {
                            record: 90,
                            after: p.revision(),
                            event: Event::Challenge,
                        })
                        .unwrap();
                    }
                }
            }
            let grant_withdrawn = (fault == Fault::RemoveLearner && !shared)
                || (fault == Fault::RemoveTeacher && shared);
            if grant_withdrawn || matches!(fault, Fault::ReplaceRelease | Fault::CorrectItem) {
                brief.snapshot.epoch += 1;
                authority = Authority::new(&brief, &records).unwrap();
            } else if fault == Fault::RevokeSource {
                authority = Authority::new(&brief, &records[..1]).unwrap();
            }
        }
        let source = RuleSource {
            bytes: Some(UNIT_RULE.as_bytes()),
            pin: span,
            revoked: &revoked,
        };
        let mut published_now = false;
        match step {
            Step::Route => {
                for (i, slot) in tickets.iter_mut().enumerate() {
                    let request = Request {
                        id: job.request.id + i as u64,
                        ..job.request
                    };
                    if let Action::Dispatch(t) =
                        router.route(request, &packet, &authority, &options, now)
                    {
                        *slot = Some(t);
                    }
                }
            }
            Step::SettleFirst | Step::SettleSecond => {
                let i = usize::from((step == Step::SettleFirst) == second_request_first);
                if let Some(ticket) = tickets[i] {
                    let bytes = [A, B][i];
                    let done = Completion {
                        result: Returned::Draft(document_id(bytes)),
                        charged_micros: 3,
                    };
                    assert_eq!(router.settle(ticket, done), Ok(true));
                    assert_eq!(router.settle(ticket, done), Ok(false));
                    callbacks += 1;
                    settled[i] = true;
                    // An authenticated completion of another job cannot fulfill this job.
                    if i == 1 {
                        assert!(job
                            .publish(
                                Publisher {
                                    router: &router,
                                    catalog: &mut catalog
                                },
                                CandidateBytes { ticket, bytes },
                                &packet,
                                &authority,
                                &source,
                                now
                            )
                            .is_err());
                        if !settled[0] {
                            assert!(job
                                .publish(
                                    Publisher {
                                        router: &router,
                                        catalog: &mut catalog
                                    },
                                    CandidateBytes {
                                        ticket: tickets[0].unwrap(),
                                        bytes: A
                                    },
                                    &packet,
                                    &authority,
                                    &source,
                                    now
                                )
                                .is_err());
                        }
                    }
                }
            }
            Step::Publish => {
                if let Some(ticket) = tickets[0] {
                    if let Ok(form) = job.publish(
                        Publisher {
                            router: &router,
                            catalog: &mut catalog,
                        },
                        CandidateBytes { ticket, bytes: A },
                        &packet,
                        &authority,
                        &source,
                        now,
                    ) {
                        published_now = true;
                        let a = Assignment {
                            key: AssignmentKey {
                                class: form.class,
                                activity: 3,
                            },
                            publication: form.revision,
                            form,
                            question: Question::RequestedProbe,
                            assigned: BTreeMap::from([(learner, 1)]),
                            corrected: false,
                            returns: BTreeMap::new(),
                            held: if active_fault == Some(Fault::Challenge) {
                                BTreeSet::from([learner])
                            } else {
                                BTreeSet::new()
                            },
                        };
                        person = Some(Participation::start(&a, learner));
                        assignment = Some(a);
                    }
                }
            }
            Step::Open | Step::Submit | Step::Feedback => {
                if let (Some(a), Some(p)) = (&assignment, &mut person) {
                    let access = LearnerAccess {
                        actor: learner,
                        roster: &roster,
                        assignment: a,
                        source: &source,
                    };
                    match step {
                        Step::Open => {
                            handle = p
                                .present(access, &mut book, Requirement::FreshTask, p.revision(), 3)
                                .ok()
                                .map(|r| r.handle);
                        }
                        Step::Submit => {
                            if let Some(h) = &handle {
                                submitted = p
                                    .submit(access, h, 4, "3/4", Assistance::NoneReported)
                                    .is_ok();
                            }
                        }
                        Step::Feedback => {
                            let receipt = p.control(Command {
                                after: p.revision(),
                                record: 5,
                                event: Event::Feedback(Gate::Supported),
                            });
                            released =
                                matches!(receipt, Ok(Receipt::Applied(Effect::GaveFeedback(_))));
                        }
                        _ => unreachable!(),
                    }
                }
            }
            _ => {}
        }
        let enrolled = roster.learners.contains(&learner);
        let class_authorized = shared && roster.teachers.contains(&teacher);
        let has_publication = assignment.is_some();
        let can_show = has_publication
            && enrolled
            && !matches!(
                active_fault,
                Some(
                    Fault::Challenge
                        | Fault::CorrectItem
                        | Fault::ReplaceRelease
                        | Fault::RevokeSource
                )
            );
        let publisher_eligible = (if shared {
            roster.teachers.contains(&teacher)
        } else {
            enrolled
        }) && !matches!(
            active_fault,
            Some(
                Fault::RevokeSource | Fault::ReplaceRelease | Fault::CorrectItem | Fault::Deadline
            )
        );
        let mut frame = Frame {
            step,
            publication_allowed: settled[0] && publisher_eligible,
            published_now,
            has_publication,
            can_show,
            question: false,
            worked: false,
            submitted,
            released,
            class_authorized,
            teacher_sees: false,
            named_in_denominator: false,
            enrolled,
            class_result: false,
            peer_denied: true,
            dispatched: tickets[0].is_some(),
            dispatch_allowed: publisher_eligible,
            callbacks,
            charged: router.spent_micros(),
        };
        if let (Some(a), Some(p)) = (&assignment, &person) {
            let view = learner_view(learner, &roster, a, p, &source);
            frame.question = matches!(view, Ok(LearnerView::Question { .. }));
            frame.worked = matches!(
                view,
                Ok(LearnerView::Feedback {
                    worked_answer: Some(_),
                    ..
                })
            );
            frame.peer_denied =
                learner_view(learner + 1, &roster, a, p, &source) == Err(ViewError::AccessDenied);
            if let Ok(summary) = teacher_view(teacher, &roster, a, &[p], &source) {
                frame.teacher_sees = true;
                frame.named_in_denominator = summary.assigned != 0;
                frame.class_result = summary
                    .results
                    .contains_key(&ResultState::MatchedWithoutReportedHelp);
            }
        }
        trace.push(frame);
    }
    trace
}

struct Rule {
    label: &'static str,
    next: bool,
    when: fn(&Frame) -> bool,
    require: fn(&Frame) -> bool,
}

const fn rule(label: &'static str, when: fn(&Frame) -> bool, require: fn(&Frame) -> bool) -> Rule {
    Rule {
        label,
        next: false,
        when,
        require,
    }
}

const RULES: &[Rule] = &[
    rule(
        "authorized dispatch",
        |f| f.step == Step::Route,
        |f| f.dispatched == f.dispatch_allowed,
    ),
    rule(
        "live access before accepting a response",
        |f| f.step == Step::Submit && f.submitted,
        |f| f.can_show,
    ),
    rule(
        "admitted publication",
        |f| f.step == Step::Publish,
        |f| f.published_now == f.publication_allowed,
    ),
    rule(
        "current learner grant",
        |f| f.question || f.worked,
        |f| f.can_show,
    ),
    rule(
        "answer after released assessment",
        |f| f.worked,
        |f| f.has_publication && f.submitted && f.released,
    ),
    rule(
        "teacher publication grant",
        |f| f.has_publication,
        |f| f.teacher_sees == f.class_authorized,
    ),
    rule(
        "current class denominator",
        |f| f.teacher_sees,
        |f| f.named_in_denominator == f.enrolled,
    ),
    rule(
        "interpretable class result",
        |f| f.class_result,
        |f| f.can_show && f.submitted && f.released,
    ),
    rule(
        "foreign learner denied",
        |f| f.has_publication,
        |f| f.peer_denied,
    ),
    rule(
        "callbacks billed once",
        |f| f.dispatched,
        |f| f.charged == 3 * f.callbacks,
    ),
    Rule {
        label: "withdrawal suppresses subsequent learner material",
        next: true,
        when: |f| {
            matches!(
                f.step,
                Step::Interrupt(
                    Fault::RemoveLearner
                        | Fault::RevokeSource
                        | Fault::ReplaceRelease
                        | Fault::Challenge
                        | Fault::CorrectItem
                )
            )
        },
        require: |f| !f.question && !f.worked,
    },
];

/// Build once per operator. Each trace/rule receives an independent actual telo monitor.
fn monitor_factory(next: bool) -> impl Fn() -> Monitor<(bool, bool)> {
    let predicates = || {
        let mut builder = Predicates::builder();
        let p = builder.new_predicate(ClosurePredicate::new(|s: &(bool, bool)| s.0, "trigger"));
        let q = builder.new_predicate(ClosurePredicate::new(|s: &(bool, bool)| s.1, "consequence"));
        (builder.build(), Property::atomic(p), Property::atomic(q))
    };
    let (symbols, p, q) = predicates();
    let consequence = if next { Property::next(q) } else { q };
    let formula = Property::always(Property::Or(
        Box::new(!p.clone()),
        Box::new(p & consequence),
    ));
    let automaton = formula.to_monitoring_automaton(&symbols);
    move || Monitor::new(predicates().0, automaton.clone())
}

#[test]
fn serving_pipeline_respects_publication_and_visibility_in_each_bounded_interleaving() {
    let always = monitor_factory(false);
    let next = monitor_factory(true);
    let mut triggers = vec![0; RULES.len()];
    let (mut traces, mut frames, mut publications, mut worked) = (0, 0, 0, 0);
    for shared in [false, true] {
        for reversed in [false, true] {
            let cuts = FAULTS
                .into_iter()
                .flat_map(|fault| (0..=PATH.len()).map(move |i| Some((i, fault))));
            for fault in [None].into_iter().chain(cuts) {
                let trace = observe(shared, reversed, fault);
                for (i, property) in RULES.iter().enumerate() {
                    let mut monitor = if property.next { next() } else { always() };
                    for f in &trace {
                        let p = (property.when)(f);
                        let q = (property.require)(f);
                        triggers[i] += usize::from(p);
                        assert!(monitor.next_state(&(p,q)), "{}; shared={shared}, reversed={reversed}, fault={fault:?}, prefix={:?}",
                            property.label, &trace[..=trace.iter().position(|s| s.step == f.step).unwrap()]);
                    }
                }
                // Non-vacuity: allow a complete trajectory in both audience modes and both callback orders.
                if fault.is_none() {
                    assert!(trace.last().unwrap().worked);
                    assert_eq!(trace.last().unwrap().class_result, shared);
                }
                publications += trace.iter().filter(|f| f.published_now).count();
                worked += trace.iter().filter(|f| f.worked).count();
                traces += 1;
                frames += trace.len();
            }
        }
    }
    assert!(triggers.iter().all(|n| *n > 0));
    assert_eq!(traces, 228);
    println!("{traces} traces, {frames} observed boundaries, {publications} publications, {worked} worked answers visible; property triggers {triggers:?}");
}

#[test]
fn monitor_rejects_a_cached_worked_answer_after_a_learner_challenges_it() {
    let trace = observe(true, true, Some((PATH.len(), Fault::Challenge)));
    let rule = RULES.last().unwrap();
    let mut monitor = monitor_factory(true)();
    let mut cached = false;
    for mut f in trace {
        cached |= f.worked;
        if f.step == Step::Read {
            assert!(cached && !f.worked);
            f.worked = cached; // Fault injection: a renderer replays its last acknowledged answer.
            assert!(!monitor.next_state(&((rule.when)(&f), (rule.require)(&f))));
        } else {
            assert!(monitor.next_state(&((rule.when)(&f), (rule.require)(&f))));
        }
    }
}

/// Reissue one person's seat in a shared activity. Both generations may reuse the same
/// class, publication, form, command numbers and lane revision. The assignment adapter
/// issues a new individual grant when enrollment or an assignment is terminated and renewed.
fn renewal_trace(remove_membership: bool, reuse_retired_grant: bool) -> bool {
    use tutor_lab::curriculum::OutcomeId;
    use tutor_lab::history::Scope;
    use tutor_lab::material::FractionCard;
    use tutor_lab::supply::Form;
    let (learner, peer, class) = (11, 12, 5);
    let mut roster = Roster {
        class,
        learners: BTreeSet::from([learner, peer]),
        teachers: BTreeSet::from([99]),
    };
    let live = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin: Span {
            document: document_id(UNIT_RULE.as_bytes()),
            start: 0,
            end: UNIT_RULE.len() as u32,
        },
        revoked: &live,
    };
    let mut assignment = Assignment {
        key: AssignmentKey { class, activity: 7 },
        publication: 9,
        form: Form {
            class,
            release: 8,
            outcome: OutcomeId(3),
            owner: None,
            revision: 35,
            task: 350,
            family: 1,
            source: source.pin,
            card: FractionCard::propose(FractionTask::Sum, 1, 2, 4).unwrap(),
        },
        // Familiar supported practice can be explicitly reissued under the new grant.
        question: Question::Practice,
        assigned: BTreeMap::from([(learner, 1), (peer, 1)]),
        held: BTreeSet::new(),
        corrected: false,
        returns: BTreeMap::new(),
    };
    fn access<'a>(
        actor: u64,
        roster: &'a Roster,
        assignment: &'a Assignment,
        source: &'a RuleSource<'a>,
    ) -> LearnerAccess<'a> {
        LearnerAccess {
            actor,
            roster,
            assignment,
            source,
        }
    }
    let start = |grant: LearnerAccess<'_>, book: &mut ExposureBook| {
        let mut row = Participation::start(grant.assignment, grant.actor);
        row.control(Command {
            after: 0,
            record: 1,
            event: Event::Teach(Gate::Supported),
        })
        .unwrap();
        let opened = row
            .present(grant, book, Requirement::Practice, 1, 2)
            .unwrap();
        (row, opened.handle)
    };
    let history = |id| {
        ExposureBook::new(
            Enrollment {
                class,
                scope: Scope {
                    learner: id,
                    revision: 8,
                },
            },
            true,
        )
    };
    let mut book = history(learner);
    let (mut old, original) = start(access(learner, &roster, &assignment, &source), &mut book);
    let (mut peer_row, peer_opening) = start(
        access(peer, &roster, &assignment, &source),
        &mut history(peer),
    );
    let stale_suppressed = |row: &mut Participation, grant: LearnerAccess<'_>| {
        let visible = matches!(
            learner_view(
                grant.actor,
                grant.roster,
                grant.assignment,
                row,
                grant.source
            ),
            Ok(LearnerView::Question { .. })
        );
        let accepted = row
            .submit(grant, &original, 3, "3/4", Assistance::NoneReported)
            .is_ok();
        !visible && !accepted
    };
    // G(retired -> X(old prompt withheld and original handle refused)).
    // The first authority check happens at withdrawal. The extra final read discharges X.
    let mut monitor = monitor_factory(true)();
    if remove_membership {
        roster.learners.remove(&learner);
    }
    assignment.assigned.remove(&learner);
    let withdrawn = access(learner, &roster, &assignment, &source);
    let stopped_at_withdrawal = stale_suppressed(&mut old, withdrawn);
    assert!(stopped_at_withdrawal);
    assert!(monitor.next_state(&(true, stopped_at_withdrawal)));
    roster.learners.insert(learner);
    let no_assignment = access(learner, &roster, &assignment, &source);
    assert!(monitor.next_state(&(true, stale_suppressed(&mut old, no_assignment))));
    assignment
        .assigned
        .insert(learner, if reuse_retired_grant { 1 } else { 2 });
    let renewed = access(learner, &roster, &assignment, &source);
    if !monitor.next_state(&(true, stale_suppressed(&mut old, renewed))) {
        return false;
    }
    // The learner is assigned again but has yet to start the renewed activity. A peer
    // holds the same original grant and opening throughout the renewal.
    let tally = teacher_view(99, &roster, &assignment, &[&old, &peer_row], &source).unwrap();
    assert_eq!(
        tally.counts[&tutor_lab::audience::TeacherAction::InviteToStart],
        1
    );
    let (mut new, reissued) = start(renewed, &mut book);
    let rejected_on_new_row = new
        .submit(renewed, &original, 3, "3/4", Assistance::NoneReported)
        .is_err();
    assert!(monitor.next_state(&(
        true,
        rejected_on_new_row && stale_suppressed(&mut old, renewed)
    )));
    let accepted = new
        .submit(renewed, &reissued, 3, "3/4", Assistance::NoneReported)
        .unwrap();
    assert!(!accepted.fresh_task());
    let unchanged = access(peer, &roster, &assignment, &source);
    assert!(peer_row
        .submit(unchanged, &peer_opening, 3, "3/4", Assistance::NoneReported)
        .is_ok());
    assert!(monitor.next_state(&(false, stale_suppressed(&mut old, renewed))));
    true
}

#[test]
fn remove_then_reissue_fences_old_handles_and_preserves_an_unaffected_peer() {
    for remove_membership in [true, false] {
        assert!(renewal_trace(remove_membership, false));
        // A generation-reusing adapter reproduces the original permission resurrection.
        assert!(!renewal_trace(remove_membership, true));
    }
}
