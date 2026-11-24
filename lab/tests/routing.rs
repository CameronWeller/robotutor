use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::context::{self, Authority, Count, Packet, Purpose};
use tutor_lab::protocol::Question;
use tutor_lab::routing::*;

const MODEL: Model = Model {
    revision: 1,
    maximum_micros: 12,
    admission_ms: 4,
    slots: 1,
    max_input_tokens: 200,
    max_output_tokens: 512,
};

fn router(budget: u64, attempts: u8, requests: usize, model: Model) -> Router {
    let models = BTreeMap::from([
        (Role::Language, model),
        (Role::Reasoner, model),
        (Role::Assessor, model),
    ]);
    Router::new(2, budget, attempts, requests, models)
}

fn episode(purpose: Purpose, intent: Intent) -> (Packet, Authority, Request, Options) {
    let (context, authority, records) = context::example(purpose);
    let packet =
        context::compile(&context, &authority, &records, &|_| Count::Measured(100)).unwrap();
    let request = Request {
        binding: [12; 32],
        id: 8,
        max_micros: 24,
        deadline_ms: 100,
        need: Need {
            scope: packet.scope(),
            outcome: packet.outcome(),
            intent,
            packet: packet.digest(),
        },
    };
    (
        packet,
        authority,
        request,
        Options {
            permitted: BTreeSet::from([Role::Language, Role::Reasoner, Role::Assessor]),
            ..Options::default()
        },
    )
}

fn ticket(action: Action) -> Ticket {
    let Action::Dispatch(ticket) = action else {
        panic!("expected dispatch, got {action:?}")
    };
    ticket
}

#[test]
fn current_packet_selects_one_capability_and_old_output_is_quarantined() {
    for (purpose, intent, role) in [
        (Purpose::Teach, Intent::Explain, Role::Language),
        (Purpose::Author, Intent::AuthorForm, Role::Language),
        (
            Purpose::PrivateForm,
            Intent::PersonalizeForm(Question::ChangedExample),
            Role::Reasoner,
        ),
        (
            Purpose::PrivateForm,
            Intent::PersonalizeForm(Question::Practice),
            Role::Language,
        ),
        (
            Purpose::Probe,
            Intent::Ask(Question::DelayedReview),
            Role::Reasoner,
        ),
        (Purpose::Evaluate, Intent::AssessOpen, Role::Assessor),
    ] {
        let (packet, mut authority, request, mut opts) = episode(purpose, intent);
        let mut router = router(20, 2, 1, MODEL);
        let t = ticket(router.route(request, &packet, &authority, &opts, 10));
        assert_eq!(t.role, role);
        assert_eq!(router.request_for(t), Some(request));
        assert_eq!(router.request_for(Ticket { epoch: 1, ..t }), None);
        assert_eq!(router.reserved_micros(), 12);
        assert_eq!(
            router.route(request, &packet, &authority, &opts, 11),
            Action::InFlight(t)
        );
        let receipt = Completion {
            result: Returned::Draft([8; 32]),
            charged_micros: 7,
        };
        assert_eq!(router.settle(t, receipt), Ok(true));
        assert_eq!(router.settle(t, receipt), Ok(false));
        assert_eq!((router.spent_micros(), router.reserved_micros()), (7, 0));
        let draft = Draft {
            ticket: t,
            artifact: [8; 32],
        };
        assert_eq!(
            router.route(request, &packet, &authority, &opts, 12),
            Action::ReviewDraft(draft)
        );
        assert_eq!(router.draft(t), Some(draft));
        assert_eq!(router.draft(Ticket { epoch: 1, ..t }), None);
        assert_eq!(
            router.settle(
                t,
                Completion {
                    charged_micros: 6,
                    ..receipt
                }
            ),
            Err(SettlementError::ConflictingReceipt)
        );
        opts.permitted.clear();
        assert_eq!(
            router.route(request, &packet, &authority, &opts, 12),
            Action::Abstain(Reason::Unauthorized)
        );
        authority.snapshot.epoch += 1;
        assert_eq!(
            router.route(request, &packet, &authority, &opts, 13),
            Action::Abstain(Reason::Unauthorized)
        );
    }
}

#[test]
fn transport_termination_releases_a_slot_and_preserves_the_retry_fence_and_money() {
    let (p, a, r, o) = episode(Purpose::Teach, Intent::Explain);
    let mut router = router(24, 2, 4, MODEL);
    let next = Request { id: 9, ..r };
    let first = ticket(router.route(r, &p, &a, &o, 2));
    assert_eq!(
        router.route(next, &p, &a, &o, 2),
        Action::Abstain(Reason::NoCapacity)
    );
    assert_eq!(router.confirm_stopped(first), Ok(true));
    assert_eq!(router.confirm_stopped(first), Ok(false));
    assert_eq!(router.route(r, &p, &a, &o, 3), Action::InFlight(first));
    let second = ticket(router.route(next, &p, &a, &o, 3));
    assert_eq!(router.reserved_micros(), 24);
    assert_eq!(router.confirm_stopped(second), Ok(true));
    assert_eq!(
        router.route(Request { id: 10, ..r }, &p, &a, &o, 5),
        Action::Abstain(Reason::Budget)
    );
    assert_eq!(
        router.route(r, &p, &a, &o, 101),
        Action::Abstain(Reason::Deadline)
    );
    assert_eq!(
        router.settle(
            first,
            Completion {
                result: Returned::Failed,
                charged_micros: 4
            }
        ),
        Ok(true)
    );
    assert_eq!(
        router.settle(
            second,
            Completion {
                result: Returned::Draft([0; 32]),
                charged_micros: 3
            }
        ),
        Ok(true)
    );
    assert_eq!((router.spent_micros(), router.reserved_micros()), (7, 0));
}

#[test]
fn limits_apply_before_issuance_and_declined_work_does_not_consume_receipts() {
    let (p, a, r, mut o) = episode(Purpose::Teach, Intent::Explain);
    for (model, expected) in [
        (
            Model {
                revision: 2,
                ..MODEL
            },
            Reason::ProfileMismatch,
        ),
        (
            Model {
                max_input_tokens: 99,
                ..MODEL
            },
            Reason::TokenLimit,
        ),
        (
            Model {
                max_output_tokens: 0,
                ..MODEL
            },
            Reason::TokenLimit,
        ),
        (
            Model {
                admission_ms: 101,
                ..MODEL
            },
            Reason::Deadline,
        ),
        (Model { slots: 0, ..MODEL }, Reason::NoCapacity),
        (
            Model {
                maximum_micros: 25,
                ..MODEL
            },
            Reason::Budget,
        ),
    ] {
        let mut router = router(30, 2, 1, model);
        assert_eq!(router.route(r, &p, &a, &o, 1), Action::Abstain(expected));
        assert_eq!(router.reserved_micros(), 0);
    }
    let mut router = router(30, 2, 1, MODEL);
    o.permitted.clear();
    o.human_available = true;
    assert_eq!(
        router.route(r, &p, &a, &o, 1),
        Action::Human(Reason::Unauthorized)
    );
    o.permitted.insert(Role::Language);
    let other = Request {
        id: 10,
        max_micros: 12,
        ..r
    };
    let t = ticket(router.route(other, &p, &a, &o, 1));
    assert_eq!(
        router.route(
            Request {
                deadline_ms: 99,
                ..other
            },
            &p,
            &a,
            &o,
            1
        ),
        Action::Abstain(Reason::ChangedRequest)
    );
    assert_eq!(
        router.route(r, &p, &a, &o, 1),
        Action::Human(Reason::ReceiptCapacity)
    );
    router
        .settle(
            t,
            Completion {
                result: Returned::Retryable,
                charged_micros: 1,
            },
        )
        .unwrap();
    assert_eq!(
        router.route(other, &p, &a, &o, 4),
        Action::Human(Reason::Budget)
    );
}

#[test]
fn adjudicated_failures_allow_a_bounded_retry_and_billing_anomaly_stops_new_calls() {
    let (p, a, r, o) = episode(Purpose::Probe, Intent::Ask(Question::RequestedProbe));
    let mut router = router(50, 2, 3, MODEL);
    let first = ticket(router.route(r, &p, &a, &o, 1));
    let failed = Completion {
        result: Returned::Retryable,
        charged_micros: 2,
    };
    assert_eq!(router.settle(first, failed), Ok(true));
    let second = ticket(router.route(r, &p, &a, &o, 4));
    assert_eq!(second.attempt, 2);
    assert_eq!(router.settle(first, failed), Ok(false));
    assert_eq!(router.settle(second, failed), Ok(true));
    assert_eq!(
        router.route(r, &p, &a, &o, 5),
        Action::Abstain(Reason::AttemptLimit)
    );
    let third = ticket(router.route(Request { id: 9, ..r }, &p, &a, &o, 5));
    assert_eq!(
        router.settle(
            third,
            Completion {
                charged_micros: 13,
                ..failed
            }
        ),
        Err(SettlementError::ChargeAboveReservation)
    );
    assert_eq!(router.reserved_micros(), 12);
    assert_eq!(
        router.route(Request { id: 10, ..r }, &p, &a, &o, 5),
        Action::Abstain(Reason::BillingHold)
    );
}

#[test]
fn local_work_needs_the_current_broker_scope_and_no_provider_packet() {
    for (purpose, intent) in [
        (Purpose::Probe, Intent::Ask(Question::Practice)),
        (Purpose::Evaluate, Intent::GradeNumeric),
        (Purpose::Evaluate, Intent::Dispute),
    ] {
        let (_, mut a, _) = context::example(purpose);
        let request = Request {
            binding: [12; 32],
            id: 1,
            deadline_ms: 10,
            max_micros: 0,
            need: Need {
                scope: a.snapshot.scope,
                outcome: a.snapshot.outcome,
                intent,
                packet: [5; 32],
            },
        };
        let opts = Options {
            reviewed: Some(Reviewed {
                valid_until_ms: 4,
                binding: request.binding,
                need: request.need,
                artifact: [4; 32],
            }),
            exact_checker: true,
            human_available: true,
            ..Options::default()
        };
        let grant = || LocalGrant {
            valid_until_ms: 6,
            binding: request.binding,
            snapshot: a.snapshot,
            need: request.need,
        };
        let expected = match intent {
            Intent::GradeNumeric => Action::CheckLocally,
            Intent::Dispute => Action::Human(Reason::Dispute),
            _ => Action::UseReviewed([4; 32]),
        };
        assert_eq!(route_local(request, &a, grant(), &opts, 1), expected);
        assert_eq!(
            route_local(request, &a, grant(), &opts, 6),
            Action::Abstain(Reason::Unauthorized)
        );
        if matches!(intent, Intent::Ask(_)) {
            assert_eq!(
                route_local(request, &a, grant(), &opts, 4),
                Action::Abstain(Reason::Unauthorized)
            );
        }
        let stale = grant();
        a.snapshot.epoch += 1;
        assert_eq!(
            route_local(request, &a, stale, &opts, 1),
            Action::Abstain(Reason::Unauthorized)
        );
    }
}

#[test]
fn four_operation_histories_balance_against_an_independent_provider_journal() {
    #[derive(Clone, Copy)]
    struct ProviderCall {
        ticket: Ticket,
        running: bool,
        completion: Option<Completion>,
    }
    let (p, a, r, o) = episode(Purpose::Teach, Intent::Explain);
    // Two callers may dispatch, confirm a stop, or settle a success/failure in
    // any order, including duplicate callbacks. Each prefix gets checked too.
    for sequence in 0..8_u32.pow(4) {
        let mut router = router(20, 2, 2, MODEL);
        let mut calls: [Vec<ProviderCall>; 2] = Default::default();
        let mut word = sequence;
        for now in 0..4 {
            let letter = word % 8;
            word /= 8;
            let caller = (letter % 2) as usize;
            let journal = &mut calls[caller];
            if letter / 2 == 0 {
                let request = Request {
                    id: caller as u64,
                    ..r
                };
                if let Action::Dispatch(ticket) = router.route(request, &p, &a, &o, now) {
                    assert!(journal.last().is_none_or(|c| c
                        .completion
                        .is_some_and(|receipt| receipt.result == Returned::Retryable)));
                    assert_eq!(usize::from(ticket.attempt), journal.len() + 1);
                    journal.push(ProviderCall {
                        ticket,
                        running: true,
                        completion: None,
                    });
                }
            } else if let Some(call) = journal.last_mut() {
                if letter / 2 == 1 {
                    assert_eq!(router.confirm_stopped(call.ticket), Ok(call.running));
                    call.running = false;
                } else {
                    let receipt = Completion {
                        result: if letter / 2 == 2 {
                            Returned::Retryable
                        } else {
                            Returned::Draft([2; 32])
                        },
                        charged_micros: 5,
                    };
                    let expected = match call.completion {
                        None => Ok(true),
                        Some(prior) if prior == receipt => Ok(false),
                        Some(_) => Err(SettlementError::ConflictingReceipt),
                    };
                    assert_eq!(router.settle(call.ticket, receipt), expected);
                    if expected == Ok(true) {
                        call.completion = Some(receipt);
                        call.running = false;
                    }
                }
            }
            let mut spent = 0;
            let mut reserved = 0;
            let mut running = 0;
            for call in calls.iter().flatten() {
                if let Some(receipt) = call.completion {
                    spent += receipt.charged_micros;
                } else {
                    reserved += call.ticket.maximum_micros;
                }
                running += u8::from(call.running);
            }
            assert_eq!(
                (router.spent_micros(), router.reserved_micros()),
                (spent, reserved)
            );
            assert!(running <= 1 && spent + reserved <= 20);
        }
    }
}

#[test]
fn outbox_worker_must_validate_at_send_after_queue_delay_or_revocation() {
    let (packet, mut authority, request, mut o) = episode(Purpose::Author, Intent::AuthorForm);
    let mut router = router(20, 2, 1, MODEL);
    let t = ticket(router.route(request, &packet, &authority, &o, 0));
    assert_eq!(
        router.messages_for(t, &packet, &authority, &o, 96).unwrap(),
        packet.dispatch(&authority).unwrap()
    );
    assert_eq!(
        router.messages_for(t, &packet, &authority, &o, 97),
        Err(Reason::Deadline)
    );
    o.permitted.clear();
    assert_eq!(
        router.messages_for(t, &packet, &authority, &o, 4),
        Err(Reason::Unauthorized)
    );
    o.permitted.insert(Role::Language);
    authority.snapshot.epoch += 1;
    assert_eq!(
        router.messages_for(t, &packet, &authority, &o, 4),
        Err(Reason::Unauthorized)
    );
    router.confirm_stopped(t).unwrap();
    assert_eq!(
        router.messages_for(t, &packet, &authority, &o, 4),
        Err(Reason::NoPendingCall)
    );
    assert_eq!(router.reserved_micros(), 12);
}

#[test]
fn switching_profiles_retains_the_old_attempts_liability_and_requires_a_new_packet() {
    let (packet, authority, original, opts) = episode(Purpose::Teach, Intent::Explain);
    let mut router = router(20, 1, 4, MODEL);
    let alternate = Model {
        revision: 2,
        ..MODEL
    };
    assert_eq!(router.register_profile(Role::Language, alternate), Ok(()));
    assert_eq!(
        router.register_profile(
            Role::Language,
            Model {
                maximum_micros: 1,
                ..alternate
            }
        ),
        Err(Reason::ProfileMismatch)
    );
    let first = ticket(router.route(original, &packet, &authority, &opts, 0));
    assert_eq!(router.available_profile(Role::Language, 1), None);
    assert_eq!(router.available_profile(Role::Language, 2), Some(alternate));

    let (mut changed, _, inputs) = context::example(Purpose::Teach);
    changed.snapshot.profile = alternate.revision;
    let current = Authority::new(&changed, &inputs).unwrap();
    let new_packet =
        context::compile(&changed, &current, &inputs, &|_| Count::Measured(120)).unwrap();
    let other = Request {
        id: original.id + 1,
        need: Need {
            packet: new_packet.digest(),
            ..original.need
        },
        ..original
    };
    assert_ne!(other.need.packet, original.need.packet);
    assert_eq!(
        router.messages_for(first, &packet, &current, &opts, 1),
        Err(Reason::Unauthorized)
    );
    assert_eq!(
        router.route(other, &new_packet, &current, &opts, 1),
        Action::Abstain(Reason::Budget)
    );
    assert_eq!(router.remaining_micros(), 8);
    assert_eq!(router.confirm_stopped(first), Ok(true));
    assert_eq!(router.remaining_micros(), 8);
    router
        .settle(
            first,
            Completion {
                result: Returned::Failed,
                charged_micros: 3,
            },
        )
        .unwrap();
    let second = ticket(router.route(other, &new_packet, &current, &opts, 2));
    assert_eq!(second.model_revision, alternate.revision);
    assert_eq!(router.remaining_micros(), 5);
    assert_eq!(router.available_profile(Role::Language, 2), None);
    assert_eq!(router.available_profile(Role::Language, 1), Some(MODEL));
}
