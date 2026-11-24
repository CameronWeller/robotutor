//! Replay feedback scheduling through the tutor and search for starvation witnesses.
#[path = "verification_investigation/diagnosis.rs"]
mod diagnosis;
#[path = "verification_investigation/eligibility.rs"]
mod eligibility;
#[path = "verification_investigation/return_read.rs"]
mod return_read;

use std::collections::{HashMap, VecDeque};

use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::protocol::{
    transition, Assistance, Effect, Event, Gate, Judgment, Next, Phase, Question, State,
};
use tutor_lab::temporal::check_trace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Policy {
    FirstEligible,
    CalendarSeat,
    AfterDelivery,
    OldestRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Queue {
    cursor: usize,
    // At most one outstanding analyzed answer per seat. This is an enqueue order,
    // including absent seats. A completed seat's next answer arrives at the tail.
    pending: Vec<usize>,
    skips: Vec<usize>,
}

#[derive(Clone, Copy)]
struct Tick {
    available: usize, // bit i means seat i is present and the feedback has current support
    staffed: bool,
}

fn eligible(tick: Tick, seat: usize) -> bool {
    tick.available & (1 << seat) != 0
}

impl Queue {
    fn new(seats: usize) -> Self {
        Self {
            cursor: 0,
            pending: (0..seats).collect(),
            skips: vec![0; seats],
        }
    }

    fn dispatch(&mut self, policy: Policy, tick: Tick) -> Option<usize> {
        let n = self.pending.len();
        let grant = if !tick.staffed || tick.available == 0 {
            None
        } else {
            match policy {
                Policy::FirstEligible => (0..n).find(|&seat| eligible(tick, seat)),
                Policy::CalendarSeat => eligible(tick, self.cursor).then_some(self.cursor),
                Policy::AfterDelivery => (0..n)
                    .map(|offset| (self.cursor + offset) % n)
                    .find(|&seat| eligible(tick, seat)),
                Policy::OldestRequest => self
                    .pending
                    .iter()
                    .copied()
                    .find(|&seat| eligible(tick, seat)),
            }
        };
        for (seat, skips) in self.skips.iter_mut().enumerate() {
            if grant == Some(seat) {
                *skips = 0;
            } else if tick.staffed && eligible(tick, seat) {
                *skips += 1;
            }
        }
        match (policy, grant) {
            (Policy::CalendarSeat, _) => self.cursor = (self.cursor + 1) % n,
            (Policy::AfterDelivery, Some(seat)) => self.cursor = (seat + 1) % n,
            (Policy::OldestRequest, Some(seat)) => {
                self.pending.retain(|&other| other != seat);
                self.pending.push(seat);
            }
            _ => {}
        }
        grant
    }
}

#[derive(Default)]
struct Lane {
    state: State,
    events: Vec<Event>,
    receipts: usize,
    elapsed: usize,
    longest_elapsed: usize,
    eligible_slots: usize,
    longest_eligible: usize,
    missing_capacity: usize,
    last_submission: usize,
}

impl Lane {
    fn step(&mut self, event: Event, expected: Effect) {
        let (next, effect) = transition(self.state, event);
        assert_eq!(effect, expected, "unexpected reference effect on {event:?}");
        self.state = next;
        self.events.push(event);
    }

    fn submit(&mut self) {
        self.step(Event::Teach(Gate::Supported), Effect::Taught);
        self.step(
            Event::Ask(Question::Practice, Gate::Supported),
            Effect::Asked(Question::Practice),
        );
        self.step(
            Event::Answer(Judgment::Incorrect, Assistance::NoneReported),
            Effect::Analyzed(Next::Explain),
        );
    }
}

#[derive(Clone, Copy)]
struct Boundary {
    receipt: bool,
    owed: bool,
    can_dispatch: bool,
    last_opportunity: bool,
}

fn promise() -> Monitor<Boundary> {
    let mut builder = Predicates::builder();
    let good = builder.new_predicate(ClosurePredicate::new(
        |b: &Boundary| {
            (!b.receipt || (b.owed && b.can_dispatch)) && (!b.last_opportunity || b.receipt)
        },
        "every receipt is eligible and every final eligible opportunity yields its receipt",
    ));
    let predicates = builder.build();
    let automaton = Property::always(Property::atomic(good)).to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

#[derive(Clone, Copy)]
enum Reopen {
    Continuous,
    FromRoster,
    FromAdmissionOrder,
}

fn replay(
    name: &str,
    policy: Policy,
    seats: usize,
    ticks: &[Tick],
    reopen: Reopen,
) -> Result<(), String> {
    let mut lanes: Vec<Lane> = (0..seats).map(|_| Lane::default()).collect();
    let mut monitors: Vec<_> = (0..seats).map(|_| promise()).collect();
    let mut queue = Queue::new(seats);
    let mut first_failure = None;
    let mut next_submission = seats;
    for (sequence, lane) in lanes.iter_mut().enumerate() {
        lane.submit();
        lane.last_submission = sequence;
    }
    for (i, &tick) in ticks.iter().enumerate() {
        if i > 0 && i % 2 == 0 {
            match reopen {
                Reopen::Continuous => {}
                Reopen::FromRoster => queue = Queue::new(seats),
                Reopen::FromAdmissionOrder => {
                    queue = Queue::new(seats);
                    queue
                        .pending
                        .sort_by_key(|&seat| lanes[seat].last_submission);
                }
            }
        }
        let chosen = queue.dispatch(policy, tick);
        for (id, lane) in lanes.iter_mut().enumerate() {
            let owed = matches!(lane.state.phase, Phase::AwaitingFeedback(_));
            let can_dispatch = tick.staffed && eligible(tick, id);
            let last_opportunity = owed && can_dispatch && lane.eligible_slots + 1 >= seats;
            lane.elapsed += 1;
            lane.longest_elapsed = lane.longest_elapsed.max(lane.elapsed);
            lane.eligible_slots += usize::from(owed && can_dispatch);
            lane.longest_eligible = lane.longest_eligible.max(lane.eligible_slots);
            lane.missing_capacity += usize::from(owed && !tick.staffed && eligible(tick, id));
            let mut receipt = false;
            if chosen == Some(id) {
                lane.step(
                    Event::Feedback(Gate::Supported),
                    Effect::GaveFeedback(Next::Explain),
                );
                receipt = true;
                lane.receipts += 1;
                lane.elapsed = 0;
                lane.eligible_slots = 0;
                lane.submit();
                lane.last_submission = next_submission;
                next_submission += 1;
            }
            if !monitors[id].next_state(&Boundary {
                receipt,
                owed,
                can_dispatch,
                last_opportunity,
            }) && first_failure.is_none()
            {
                first_failure = Some((i + 1, id + 1));
            }
        }
    }
    for lane in &lanes {
        check_trace(State::default(), &lane.events)?;
    }
    let deliveries: Vec<_> = lanes.iter().map(|l| l.receipts).collect();
    println!("{name} {policy:?} receipts={deliveries:?} total={} pending={} max_elapsed={} max_eligible={} lost_capacity_seat_slots={} tutor_lanes_ok={} first_class_violation={first_failure:?}",
        deliveries.iter().sum::<usize>(), lanes.iter().filter(|l| matches!(l.state.phase, Phase::AwaitingFeedback(_))).count(),
        lanes.iter().map(|l| l.longest_elapsed).max().unwrap(),
        lanes.iter().map(|l| l.longest_eligible).max().unwrap(),
        lanes.iter().map(|l| l.missing_capacity).sum::<usize>(), lanes.len());
    Ok(())
}

// Each node is the pending queue, cursor and per-request count of unserved
// eligible opportunities. At N skips the invariant has already failed.
fn search(n: usize, policy: Policy) -> (usize, Option<Vec<Tick>>) {
    let start = Queue::new(n);
    let mut ids = HashMap::from([(start.clone(), 0)]);
    let mut parents: Vec<Option<(usize, Tick)>> = vec![None];
    let mut queue = VecDeque::from([(start, 0)]);
    while let Some((before, id)) = queue.pop_front() {
        for available in 0..(1 << n) {
            let tick = Tick {
                available,
                staffed: true,
            };
            let mut after = before.clone();
            after.dispatch(policy, tick);
            if after.skips.iter().any(|&misses| misses >= n) {
                let mut trace = vec![tick];
                let mut previous = id;
                while let Some((parent, tick)) = parents[previous] {
                    trace.push(tick);
                    previous = parent;
                }
                trace.reverse();
                return (ids.len(), Some(trace));
            }
            if let std::collections::hash_map::Entry::Vacant(entry) = ids.entry(after.clone()) {
                let next_id = parents.len();
                entry.insert(next_id);
                parents.push(Some((id, tick)));
                queue.push_back((after, next_id));
            }
        }
    }
    (ids.len(), None)
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "return") {
        return return_read::main(&args[1..]);
    }
    if args.as_slice() == ["diagnosis"] {
        return diagnosis::run();
    }
    let arg = args.first();
    let n = arg
        .map(String::as_str)
        .unwrap_or("4")
        .parse::<usize>()
        .map_err(|e| e.to_string())?;
    if !(1..=5).contains(&n) {
        return Err("choose 1–5 seats for exhaustive search".into());
    }
    let full = (1 << n) - 1;
    let open = Tick {
        available: full,
        staffed: true,
    };
    let regular = vec![open; 3 * n];
    let intermittent: Vec<_> = (0..3 * n)
        .map(|i| Tick {
            available: if i % n == n - 1 {
                full ^ (1 << (n - 1))
            } else {
                full
            },
            staffed: true,
        })
        .collect();
    let mut interrupted = vec![
        Tick {
            available: full,
            staffed: false
        };
        2 * n
    ];
    interrupted.extend(&regular);

    for policy in [
        Policy::FirstEligible,
        Policy::CalendarSeat,
        Policy::AfterDelivery,
        Policy::OldestRequest,
    ] {
        replay("continuous", policy, n, &regular, Reopen::Continuous)?;
        replay("intermittent", policy, n, &intermittent, Reopen::Continuous)?;
        if policy == Policy::OldestRequest {
            replay("outage", policy, n, &interrupted, Reopen::Continuous)?;
            replay("reopen-roster", policy, n, &regular, Reopen::FromRoster)?;
            replay(
                "reopen-receipts",
                policy,
                n,
                &regular,
                Reopen::FromAdmissionOrder,
            )?;
        }
        let (states, witness) = search(n, policy);
        if let Some(ticks) = witness {
            println!(
                "search {policy:?} seats={n} states_discovered={states} counterexample_masks={:?}",
                ticks.iter().map(|s| s.available).collect::<Vec<_>>()
            );
            replay("counterexample", policy, n, &ticks, Reopen::Continuous)?;
        } else {
            println!(
                "search {policy:?} seats={n} exhaustively_reached={states} counterexample=none"
            );
        }
    }
    eligibility::investigate()
}
