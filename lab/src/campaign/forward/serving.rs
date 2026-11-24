//! Shared inventory, per-learner exposures and a single accounting partition. A synthetic
//! model completion settles its reservation and remains a draft; it cannot refill inventory.
use super::{Observation, Rng};
use crate::{
    context::{self, Authority, Count, Packet, Purpose},
    curriculum::Span,
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::Question,
    routing::{
        Action, Completion, Intent, Model, Need, Options, Reason, Request, Returned, Reviewed,
        Role, Router, Ticket,
    },
    supply::{self, Contact, Enrollment, ExposureBook, Form, Requirement},
};
use std::collections::{BTreeMap, BTreeSet};

const LANGUAGE_CAP: u64 = 8;
const REASONER_CAP: u64 = 14;

struct Learner {
    seat: Enrollment,
    packet: Packet,
    authorized: Authority,
    withdrawn: Authority,
    history: ExposureBook,
    complete: bool,
    permitted: BTreeSet<Role>,
}

impl Learner {
    fn new(id: u64, rng: &mut Rng) -> Result<Self, String> {
        let (mut brief, _, mut records) = context::example(Purpose::Probe);
        brief.snapshot.scope.learner = id;
        for record in &mut records {
            if record.learner.is_some() {
                record.learner = Some(id);
            }
        }
        let authorized = Authority::new(&brief, &records)
            .map_err(|e| format!("invalid serving context: {e:?}"))?;
        let packet = context::compile(&brief, &authorized, &records, &|_| Count::Measured(96))
            .map_err(|e| format!("could not compile a probe packet: {e:?}"))?;
        // Remove only the material record. The precompiled packet must fail dispatch-time reauthorization.
        records.retain(|r| r.kind != context::Kind::Material);
        let withdrawn = Authority::new(&brief, &records)
            .map_err(|e| format!("invalid source withdrawal: {e:?}"))?;
        let seat = Enrollment {
            scope: brief.snapshot.scope,
            class: brief.snapshot.class,
        };
        let complete = rng.below(6) != 0;
        let mut permitted = BTreeSet::new();
        if rng.below(8) != 0 {
            permitted.insert(Role::Language);
        }
        if rng.below(5) != 0 {
            permitted.insert(Role::Reasoner);
        }
        Ok(Self {
            seat,
            packet,
            authorized,
            withdrawn,
            history: ExposureBook::new(seat, complete),
            complete,
            permitted,
        })
    }
}

fn model(maximum_micros: u64) -> Model {
    Model {
        revision: 1,
        maximum_micros,
        admission_ms: 4,
        slots: 1,
        max_input_tokens: 512,
        max_output_tokens: 128,
    }
}

fn complete_draft(router: &mut Router, ticket: Ticket) -> Result<(), String> {
    let first_receipt = router
        .settle(
            ticket,
            Completion {
                result: Returned::Draft(document_id(&ticket.request_id.to_be_bytes())),
                charged_micros: ticket.maximum_micros / 2,
            },
        )
        .map_err(|e| format!("bad serving receipt: {e:?}"))?;
    if !first_receipt {
        return Err("a dispatch completed twice".into());
    }
    Ok(())
}

pub(super) fn cases(seed: u64) -> Result<Vec<Observation>, String> {
    let mut rng = Rng::new(seed);
    let mut observations = Vec::with_capacity(192);
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let none = BTreeSet::new();
    let revoked = BTreeSet::from([pin.document]);
    for episode in 0..8 {
        let class = episode % 2 == 1;
        let seats = if class { 6 } else { 1 };
        let mut learners = (0..seats)
            .map(|i| Learner::new(u64::from(i + 1), &mut rng))
            .collect::<Result<Vec<_>, _>>()?;
        let target = learners[0].packet.outcome();
        let shared = Form {
            class: learners[0].seat.class,
            release: learners[0].seat.scope.revision,
            outcome: target,
            owner: None,
            revision: 1,
            task: 21,
            family: 1,
            source: pin,
            card: FractionCard::propose(FractionTask::Sum, 1, 2, 5).expect("valid fixture card"),
        };
        let same_family = rng.below(2) == 0;
        let private = Form {
            owner: Some(u64::from(1 + rng.below(seats))),
            revision: 2,
            task: 22,
            family: if same_family { 1 } else { 2 },
            card: FractionCard::propose(
                if same_family {
                    FractionTask::Sum
                } else {
                    FractionTask::MissingPart
                },
                2,
                3,
                7,
            )
            .expect("valid fixture card"),
            ..shared
        };
        let release_turn = match rng.below(3) {
            0 => 0,
            1 => 3 + rng.below(11),
            _ => 24,
        };
        let outage_start = 3 + rng.below(12);
        let outage_end = outage_start + 1 + rng.below(4);
        let mut router = Router::new(
            episode + 1,
            u64::from(16 + 7 * rng.below(6)),
            1,
            24,
            BTreeMap::from([
                (Role::Language, model(LANGUAGE_CAP)),
                (Role::Reasoner, model(REASONER_CAP)),
            ]),
        );
        let mut pending: Vec<(u64, Ticket)> = Vec::new();
        for turn in 0..24 {
            // Classmates make overlapping requests. A solo learner has longer between turns.
            let now = u64::from(turn) * if class { 2 } else { 12 };
            while let Some(i) = pending.iter().position(|(due, _)| *due <= now) {
                let (_, ticket) = pending.swap_remove(i);
                complete_draft(&mut router, ticket)?;
            }
            let index = rng.below(seats) as usize;
            let person = &learners[index];
            let active = !(outage_start..outage_end).contains(&turn);
            let source = RuleSource {
                bytes: Some(UNIT_RULE.as_bytes()),
                pin,
                revoked: if active { &none } else { &revoked },
            };
            let (question, requirement) = match rng.below(3) {
                0 => (Question::Practice, Requirement::Practice),
                1 => (Question::IndependentCheck, Requirement::FreshTask),
                _ => (Question::ChangedExample, Requirement::FreshFamily),
            };
            let teacher = rng.below(if class { 3 } else { 8 }) == 0;
            let short_deadline = rng.below(8) == 0;
            let has_shared = turn >= release_turn;
            let predicates = vec![
                ("shared_classroom", class),
                ("shared_card_released", has_shared),
                (
                    "private_card_owned",
                    private.owner == Some(person.seat.scope.learner),
                ),
                (
                    "shared_task_encountered",
                    person.history.exposure(shared.task).is_some(),
                ),
                (
                    "private_task_encountered",
                    person.history.exposure(private.task).is_some(),
                ),
                ("cards_share_a_family", same_family),
                ("exposure_history_complete", person.complete),
                ("needs_fresh_task", requirement != Requirement::Practice),
                ("needs_new_family", requirement == Requirement::FreshFamily),
                ("source_rights_active", active),
                ("teacher_on_duty", teacher),
                (
                    "language_role_permitted",
                    person.permitted.contains(&Role::Language),
                ),
                (
                    "reasoning_role_permitted",
                    person.permitted.contains(&Role::Reasoner),
                ),
                ("short_model_deadline", short_deadline),
                (
                    "language_slot_occupied",
                    pending.iter().any(|(_, t)| t.role == Role::Language),
                ),
                (
                    "reasoning_slot_occupied",
                    pending.iter().any(|(_, t)| t.role == Role::Reasoner),
                ),
                (
                    "balance_covers_language",
                    router.remaining_micros() >= LANGUAGE_CAP,
                ),
                (
                    "balance_covers_reasoner",
                    router.remaining_micros() >= REASONER_CAP,
                ),
            ];
            let catalog = [shared, private];
            let selected = match supply::select(
                person.seat,
                target,
                requirement,
                &person.history,
                if has_shared {
                    &catalog[..]
                } else {
                    &catalog[1..]
                },
                &source,
            ) {
                Ok(form) => Some(*form),
                Err(supply::Unavailable::NoSupportedForm | supply::Unavailable::NoFreshForm) => {
                    None
                }
                Err(supply::Unavailable::Scope) => {
                    return Err("learner's exposure scope changed".into())
                }
            };
            let need = Need {
                scope: person.seat.scope,
                outcome: target,
                intent: Intent::Ask(question),
                packet: person.packet.digest(),
            };
            let request = Request {
                id: u64::from(turn) + 1,
                need,
                binding: need.packet,
                deadline_ms: now + if short_deadline { 2 } else { 30 },
                max_micros: REASONER_CAP,
            };
            let reviewed = selected.map(|form| Reviewed {
                valid_until_ms: now + 30,
                need,
                artifact: document_id(
                    form.card
                        .question(&source)
                        .expect("selected form has current support")
                        .0
                        .as_bytes(),
                ),
                binding: request.binding,
            });
            let opts = Options {
                reviewed,
                human_available: teacher,
                permitted: person.permitted.clone(),
                ..Options::default()
            };
            let action = router.route(
                request,
                &person.packet,
                if active {
                    &person.authorized
                } else {
                    &person.withdrawn
                },
                &opts,
                now,
            );
            let label = match action {
                Action::UseReviewed(artifact) => {
                    if opts.reviewed.map(|grant| grant.artifact) != Some(artifact) {
                        return Err(
                            "router served a different artifact from the reviewed request".into(),
                        );
                    }
                    let form = selected
                        .ok_or("router served a form not selected from this learner's catalog")?;
                    // A projected shared practice example also changes classmates' exposure histories.
                    let project = class
                        && form.owner.is_none()
                        && question == Question::Practice
                        && rng.below(3) == 0;
                    for (i, learner) in learners.iter_mut().enumerate() {
                        if i == index || project {
                            let contact = if rng.below(5) == 0 {
                                Contact::Possible
                            } else {
                                Contact::Delivered
                            };
                            learner
                                .history
                                .record(learner.seat, &form, contact)
                                .map_err(|e| format!("bad classroom exposure: {e:?}"))?;
                        }
                    }
                    if form.owner.is_some() {
                        "serve_private_form"
                    } else {
                        "serve_shared_form"
                    }
                }
                Action::Dispatch(ticket) => {
                    pending.push((now + 4 + u64::from(rng.below(6)), ticket));
                    match ticket.role {
                        Role::Language => "dispatch_language_draft",
                        Role::Reasoner => "dispatch_reasoning_draft",
                        Role::Assessor => return Err("a probe reached an assessor".into()),
                    }
                }
                Action::Human(_) => "refer_to_teacher",
                Action::Abstain(Reason::Unauthorized) => "hold_for_authority",
                Action::Abstain(Reason::Budget) => "hold_for_budget",
                Action::Abstain(Reason::NoCapacity) => "hold_for_capacity",
                Action::Abstain(Reason::Deadline) => "hold_for_deadline",
                other => return Err(format!("unexpected serving action: {other:?}")),
            };
            observations.push(Observation::new(
                "serving",
                format!(
                    "{} episode {} turn {} seat {}",
                    if class { "class" } else { "solo" },
                    episode + 1,
                    turn + 1,
                    index + 1
                ),
                predicates,
                label,
            ));
        }
        for (_, ticket) in pending {
            complete_draft(&mut router, ticket)?;
        }
        if router.reserved_micros() != 0 {
            return Err("serving episode closed with an unsettled reservation".into());
        }
    }
    Ok(observations)
}
