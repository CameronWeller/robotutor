//! Hold the learners' requests constant while changing when the shared router learns
//! that work stopped, and when the final monetary charge becomes known.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::{
    context::{self, Authority, Count, Packet, Purpose},
    material::document_id,
    protocol::Question,
    routing::{
        Action, Completion, Intent, Model, Need, Options, Reason, Request, Returned, Reviewed,
        Role, Router, Ticket,
    },
};

const CAP: u64 = 14;
const ACTUAL: u64 = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiptOrder {
    Batch,
    StopFirst,
    Settled,
}

fn settle(router: &mut Router, ticket: Ticket) -> Result<(), String> {
    let first = router
        .settle(
            ticket,
            Completion {
                result: Returned::Draft(document_id(&ticket.request_id.to_be_bytes())),
                charged_micros: ACTUAL,
            },
        )
        .map_err(|e| format!("settlement failed: {e:?}"))?;
    if !first || router.draft(ticket).is_none() {
        return Err("missing new draft receipt".into());
    }
    Ok(())
}

fn router(seats: usize, slots: u16, budget: u64) -> Router {
    let caps = Model {
        revision: 1,
        maximum_micros: CAP,
        admission_ms: 4,
        slots,
        max_input_tokens: 512,
        max_output_tokens: 128,
    };
    Router::new(
        1,
        budget,
        1,
        seats,
        BTreeMap::from([(Role::Reasoner, caps)]),
    )
}

fn request_for(learner: u64) -> Result<(Request, Packet, Authority), String> {
    let (mut brief, _, mut records) = context::example(Purpose::Probe);
    brief.snapshot.scope.learner = learner;
    for record in &mut records {
        if record.learner.is_some() {
            record.learner = Some(learner);
        }
    }
    let authority =
        Authority::new(&brief, &records).map_err(|e| format!("invalid authority: {e:?}"))?;
    let packet = context::compile(&brief, &authority, &records, &|_| Count::Measured(96))
        .map_err(|e| format!("cannot build packet: {e:?}"))?;
    let need = Need {
        scope: packet.scope(),
        outcome: packet.outcome(),
        intent: Intent::Ask(Question::IndependentCheck),
        packet: packet.digest(),
    };
    let request = Request {
        id: learner,
        need,
        binding: need.packet,
        deadline_ms: 100,
        max_micros: CAP,
    };
    Ok((request, packet, authority))
}

fn episode(seats: usize, slots: u16, budget: u64, order: ReceiptOrder) -> Result<Value, String> {
    let role = Role::Reasoner;
    let mut router = router(seats, slots, budget);
    let preflight = router.available_profile(role, 1).is_some() && router.remaining_micros() >= CAP;
    let mut later = Vec::new();
    let mut counts = BTreeMap::new();
    let options = Options {
        permitted: BTreeSet::from([role]),
        ..Options::default()
    };
    for index in 0..seats {
        let learner = index as u64 + 1;
        let (request, packet, authority) = request_for(learner)?;
        let result = match router.route(request, &packet, &authority, &options, 0) {
            Action::Dispatch(ticket) => {
                match order {
                    ReceiptOrder::Batch => later.push(ticket),
                    ReceiptOrder::StopFirst => {
                        if !router
                            .confirm_stopped(ticket)
                            .map_err(|e| format!("stop receipt failed: {e:?}"))?
                        {
                            return Err("unexpected duplicate stop receipt".into());
                        }
                        later.push(ticket);
                    }
                    ReceiptOrder::Settled => settle(&mut router, ticket)?,
                }
                "dispatch"
            }
            Action::Abstain(Reason::NoCapacity) => "hold_for_capacity",
            Action::Abstain(Reason::Budget) => "hold_for_budget",
            action => return Err(format!("unexpected classroom action {action:?}")),
        };
        *counts.entry(result).or_insert(0usize) += 1;
    }
    let reserved_before_final_receipts = router.reserved_micros();
    for ticket in later {
        settle(&mut router, ticket)?;
    }
    if router.reserved_micros() != 0 {
        return Err("unsettled reservation".into());
    }
    Ok(
        json!({"seats":seats,"slots":slots,"budget":budget,"dispatches_forecast_from_shared_snapshot":if preflight {seats} else {0},
        "admissions":counts,"reserved_before_final_receipts":reserved_before_final_receipts,"settled_cost":router.spent_micros()}),
    )
}

pub(super) fn run(seats: &str, slots: &str, budget: &str) -> Result<(), String> {
    let seats = seats
        .parse()
        .map_err(|_| "expected a positive seat count")?;
    let slots = slots
        .parse()
        .map_err(|_| "expected a slot count in 0..=65535")?;
    let budget = budget
        .parse()
        .map_err(|_| "expected a nonnegative budget")?;
    if seats == 0 {
        return Err("expected at least one seat".into());
    }
    let mut out = BTreeMap::new();
    for (name, order) in [
        ("all_receipts_after_requests", ReceiptOrder::Batch),
        ("slot_released_before_invoice", ReceiptOrder::StopFirst),
        ("settled_between_requests", ReceiptOrder::Settled),
    ] {
        out.insert(name, episode(seats, slots, budget, order)?);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
    );
    Ok(())
}

// A dispatch ticket is a reservation. Test which rights and deadlines are still good at
// the worker, using the same ticket and compiled packet for all four counterfactuals.
pub(super) fn handoff(delay: &str) -> Result<(), String> {
    let now = delay
        .parse::<u64>()
        .map_err(|_| "expected nonnegative worker delay")?;
    let (request, packet, authority) = request_for(42)?;
    let (brief, _, mut records) = context::example(Purpose::Probe);
    records.retain(|r| r.kind != context::Kind::Material);
    let withdrawn =
        Authority::new(&brief, &records).map_err(|e| format!("invalid withdrawal: {e:?}"))?;
    let mut router = router(1, 1, CAP);
    let options = Options {
        permitted: BTreeSet::from([Role::Reasoner]),
        ..Options::default()
    };
    let Action::Dispatch(ticket) = router.route(request, &packet, &authority, &options, 0) else {
        return Err("missing initial reservation".into());
    };
    let new_local = Options {
        permitted: options.permitted.clone(),
        reviewed: Some(Reviewed {
            valid_until_ms: request.deadline_ms,
            need: request.need,
            artifact: document_id(b"reviewed card"),
            binding: request.binding,
        }),
        ..Options::default()
    };
    let no_role = Options::default();
    let mut outcomes = BTreeMap::new();
    for (name, current, rights) in [
        ("unchanged", &authority, &options),
        ("support_withdrawn", &withdrawn, &options),
        ("role_revoked", &authority, &no_role),
        ("local_form_released", &authority, &new_local),
    ] {
        let decision = match router.messages_for(ticket, &packet, current, rights, now) {
            Ok(_) => "eligible_to_send".into(),
            Err(reason) => format!("{reason:?}"),
        };
        outcomes.insert(name, decision);
    }
    let held = router.reserved_micros();
    // This episode is entirely local; the experiment owns the only worker and observed zero sends.
    if !router
        .settle(
            ticket,
            Completion {
                result: Returned::Failed,
                charged_micros: 0,
            },
        )
        .map_err(|e| format!("no-send receipt failed: {e:?}"))?
    {
        return Err("expected a new no-send receipt".into());
    }
    println!(
        "{}",
        json!({"worker_delay":now, "worker":outcomes,"reservation_after_preflight":held,
        "reservation_after_confirmed_no_send":router.reserved_micros(),"charged":router.spent_micros()})
    );
    Ok(())
}
