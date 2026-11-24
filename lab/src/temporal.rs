//! LTL safety and bounded-response properties over observations of actual reducer calls.
//! Each atomic observation contains the event, its pre/post states and the emitted effect.
use crate::protocol::{
    transition, Assistance, Effect, Event, Gate, Judgment, Next, Phase, Question, State,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;
use telo::monitor::Monitor;
use telo::predicate::{ClosurePredicate, Predicates};
use telo::Property;

struct Step {
    before: State,
    event: Event,
    after: State,
    effect: Effect,
}

impl Step {
    fn released(&self) -> bool {
        matches!(
            self.effect,
            Effect::Taught
                | Effect::RetaughtBeforeReview
                | Effect::Asked(_)
                | Effect::GaveFeedback(_)
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Deadline {
    Now,
    Next,
}

struct Rule {
    name: &'static str,
    deadline: Deadline,
    when: fn(&Step) -> bool,
    require: fn(&Step) -> bool,
}

const fn rule(
    name: &'static str,
    deadline: Deadline,
    when: fn(&Step) -> bool,
    require: fn(&Step) -> bool,
) -> Rule {
    Rule {
        name,
        deadline,
        when,
        require,
    }
}

use Deadline::{Next as NextEvent, Now};

const RULES: &[Rule] = &[
    rule(
        "release has current support",
        Now,
        |s| s.released(),
        |s| match s.effect {
            Effect::Taught => s.event == Event::Teach(Gate::Supported),
            Effect::RetaughtBeforeReview => s.event == Event::RequestRefresher(Gate::Supported),
            Effect::Asked(q) => s.event == Event::Ask(q, Gate::Supported),
            Effect::GaveFeedback(_) => s.event == Event::Feedback(Gate::Supported),
            _ => false,
        },
    ),
    rule(
        "pause suppresses teaching and assessment",
        Now,
        |s| s.before.paused,
        |s| !s.released() && !matches!(s.effect, Effect::Analyzed(_)),
    ),
    rule(
        "initial teaching is available",
        Now,
        |s| {
            !s.before.paused
                && s.before.phase == Phase::Ready
                && s.event == Event::Teach(Gate::Supported)
        },
        |s| s.effect == Effect::Taught && s.after.phase == Phase::Taught,
    ),
    rule(
        "learner can request an opening probe",
        Now,
        |s| {
            !s.before.paused
                && s.before.phase == Phase::Ready
                && s.event == Event::Ask(Question::RequestedProbe, Gate::Supported)
        },
        |s| s.effect == Effect::Asked(Question::RequestedProbe),
    ),
    rule(
        "practice follows teaching",
        Now,
        |s| s.effect == Effect::Asked(Question::Practice),
        |s| matches!(s.before.phase, Phase::Taught | Phase::Refreshed),
    ),
    rule(
        "challenge places this lane on hold",
        Now,
        |s| s.event == Event::Challenge,
        |s| {
            s.after.phase == Phase::Disputed
                && s.before.paused == s.after.paused
                && (s.before.phase == Phase::Disputed || s.effect == Effect::Invalidated)
        },
    ),
    rule(
        "hold persists until authorized resolution",
        Now,
        |s| s.before.phase == Phase::Disputed && s.event != Event::Resolve,
        |s| s.after.phase == Phase::Disputed && !s.released(),
    ),
    rule(
        "analyzed answer records feedback due",
        Now,
        |s| matches!(s.effect, Effect::Analyzed(_)),
        |s| matches!((s.effect, s.after.phase), (Effect::Analyzed(a), Phase::AwaitingFeedback(b)) if a == b),
    ),
    rule(
        "feedback due survives interruptions",
        Now,
        |s| {
            matches!(s.before.phase, Phase::AwaitingFeedback(_))
                && s.event != Event::Challenge
                && (s.event != Event::Feedback(Gate::Supported) || s.before.paused)
        },
        |s| s.after.phase == s.before.phase && !s.released(),
    ),
    rule(
        "eligible feedback immediately discharges the same obligation",
        Now,
        |s| {
            !s.before.paused
                && matches!(s.before.phase, Phase::AwaitingFeedback(_))
                && s.event == Event::Feedback(Gate::Supported)
        },
        |s| matches!((s.before.phase, s.effect, s.after.phase), (Phase::AwaitingFeedback(a), Effect::GaveFeedback(b), Phase::FollowUp(c)) if a == b && b == c),
    ),
    rule(
        "question cannot immediately follow analysis",
        NextEvent,
        |s| matches!(s.effect, Effect::Analyzed(_)),
        |s| !matches!(s.effect, Effect::Asked(_)),
    ),
    rule(
        "clarifying a response calls for a fresh independent check",
        Now,
        |s| {
            !s.before.paused
                && s.before.phase == Phase::AwaitingAnswer(Question::Clarification)
                && matches!(s.event, Event::Answer(Judgment::Correct, _))
        },
        |s| {
            s.effect == Effect::Analyzed(Next::IndependentCheck)
                && s.after.phase == Phase::AwaitingFeedback(Next::IndependentCheck)
        },
    ),
    rule(
        "loss of checkpoint support withdraws scheduled checks",
        Now,
        |s| {
            matches!(
                s.before.phase,
                Phase::FollowUp(Next::WaitReview)
                    | Phase::Refreshed
                    | Phase::ReviewDue
                    | Phase::AwaitingAnswer(Question::DelayedReview)
            ) && s.event == Event::CheckpointUnsupported
        },
        |s| {
            s.effect == Effect::RecheckRequired
                && s.after.phase == Phase::FollowUp(Next::IndependentCheck)
                && s.after.paused == s.before.paused
        },
    ),
    rule(
        "reported or unknown assistance cannot schedule retention",
        Now,
        |s| {
            matches!(
                s.event,
                Event::Answer(
                    Judgment::Correct,
                    Assistance::Reported | Assistance::Unknown
                )
            ) && matches!(s.effect, Effect::Analyzed(_))
        },
        |s| {
            matches!(
                s.effect,
                Effect::Analyzed(Next::IndependentCheck | Next::Explain)
            )
        },
    ),
    rule(
        "a due timer opens the review even during pause",
        Now,
        |s| {
            matches!(
                s.before.phase,
                Phase::FollowUp(Next::WaitReview) | Phase::Refreshed
            ) && s.event == Event::ReviewDue
        },
        |s| {
            s.effect == Effect::ReviewOpened
                && s.after.phase == Phase::ReviewDue
                && s.after.paused == s.before.paused
        },
    ),
    rule(
        "schedule follows a qualifying changed or delayed item",
        Now,
        |s| s.effect == Effect::Analyzed(Next::WaitReview),
        |s| {
            s.event == Event::Answer(Judgment::Correct, Assistance::NoneReported)
                && matches!(
                    s.before.phase,
                    Phase::AwaitingAnswer(Question::ChangedExample | Question::DelayedReview)
                )
        },
    ),
    rule(
        "only the scheduled timer opens a review",
        Now,
        |s| s.before.phase != Phase::ReviewDue && s.after.phase == Phase::ReviewDue,
        |s| {
            s.event == Event::ReviewDue
                && matches!(
                    s.before.phase,
                    Phase::FollowUp(Next::WaitReview) | Phase::Refreshed
                )
        },
    ),
    rule(
        "returning learner can request a grounded refresher",
        Now,
        |s| {
            !s.before.paused
                && matches!(
                    s.before.phase,
                    Phase::ReviewDue
                        | Phase::Refreshed
                        | Phase::AwaitingAnswer(Question::DelayedReview)
                        | Phase::FollowUp(Next::WaitReview)
                )
                && s.event == Event::RequestRefresher(Gate::Supported)
        },
        |s| s.effect == Effect::RetaughtBeforeReview && s.after.phase == Phase::Refreshed,
    ),
    rule(
        "delayed check requires an opened review",
        Now,
        |s| s.effect == Effect::Asked(Question::DelayedReview),
        |s| s.before.phase == Phase::ReviewDue,
    ),
    rule(
        "review cannot be presented on the tick after scheduling",
        NextEvent,
        |s| s.effect == Effect::GaveFeedback(Next::WaitReview),
        |s| s.effect != Effect::Asked(Question::DelayedReview),
    ),
    rule(
        "rejected or withheld commands preserve the projection",
        Now,
        |s| matches!(s.effect, Effect::Rejected | Effect::Withheld),
        |s| s.before == s.after,
    ),
];

// For G(p -> q), the residual obligation is always the same after an accepted
// symbol. For G(p -> X q), the only extra state is the preceding value of p.
// Tabulate telo's monitor on a shortest representative of each residual and q.
// This lets graph exploration copy finite monitor states and retain a witness.
type TruthTable = [[bool; 2]; 2];

fn monitor_factory(deadline: Deadline) -> impl Fn() -> Monitor<(bool, bool)> {
    let build_predicates = || {
        let mut builder = Predicates::builder();
        let p = builder.new_predicate(ClosurePredicate::new(|s: &(bool, bool)| s.0, "trigger"));
        let q = builder.new_predicate(ClosurePredicate::new(|s: &(bool, bool)| s.1, "consequence"));
        (builder.build(), Property::atomic(p), Property::atomic(q))
    };
    let (predicates, trigger, consequence) = build_predicates();
    let required = match deadline {
        Now => consequence,
        NextEvent => Property::next(consequence),
    };
    // Disjoint guards avoid overlapping edges during minimization in telo 0.1.0.
    let automaton = Property::always(Property::Or(
        Box::new(!trigger.clone()),
        Box::new(trigger & required),
    ))
    .to_monitoring_automaton(&predicates);
    move || Monitor::new(build_predicates().0, automaton.clone())
}

fn temporal_table(deadline: Deadline) -> TruthTable {
    let new_monitor = monitor_factory(deadline);
    let mut accepted = [[false; 2]; 2];
    for p in [false, true] {
        for q in [false, true] {
            let mut monitor = new_monitor();
            accepted[p as usize][q as usize] = match deadline {
                Now => monitor.next_state(&(p, q)),
                NextEvent => monitor.next_state(&(p, true)) && monitor.next_state(&(false, q)),
            };
        }
    }
    accepted
}

fn step_monitors(
    previous: &[bool],
    step: &Step,
    mut on_trigger: impl FnMut(usize),
) -> Result<Vec<bool>, &'static str> {
    static TABLES: OnceLock<[TruthTable; 2]> = OnceLock::new();
    let [now, next] = TABLES.get_or_init(|| [temporal_table(Now), temporal_table(NextEvent)]);
    let mut pending = Vec::with_capacity(RULES.len());
    for (i, property) in RULES.iter().enumerate() {
        let triggered = (property.when)(step);
        let required = (property.require)(step) as usize;
        let accepted = match property.deadline {
            Now => now[triggered as usize][required],
            NextEvent => next[previous[i] as usize][required],
        };
        if !accepted {
            return Err(property.name);
        }
        if triggered {
            on_trigger(i);
        }
        pending.push(property.deadline == NextEvent && triggered);
    }
    Ok(pending)
}

/// A successful prefix can still have an obligation outstanding at its last symbol.
pub fn check_trace(initial: State, events: &[Event]) -> Result<(), String> {
    let now = monitor_factory(Now);
    let next = monitor_factory(NextEvent);
    let mut monitors: Vec<_> = RULES
        .iter()
        .map(|r| match r.deadline {
            Now => now(),
            NextEvent => next(),
        })
        .collect();
    let mut before = initial;
    for (i, &event) in events.iter().enumerate() {
        let (after, effect) = transition(before, event);
        let step = Step {
            before,
            event,
            after,
            effect,
        };
        for (rule, monitor) in RULES.iter().zip(&mut monitors) {
            if !monitor.next_state(&((rule.when)(&step), (rule.require)(&step))) {
                return Err(format!(
                    "{} at step {}: {:?}",
                    rule.name,
                    i + 1,
                    &events[..=i]
                ));
            }
        }
        before = after;
    }
    Ok(())
}

#[derive(Debug)]
pub struct Report {
    pub states: usize,
    pub product_states: usize,
    pub transitions: usize,
    /// Reachable product edges activating each rule, including self-loops.
    pub activations: Vec<(&'static str, usize)>,
}

/// Check any event sequence, including cycles, starting at the fresh lane.
pub fn verify() -> Result<Report, String> {
    explore(transition)
}

fn explore(reduce: impl Fn(State, Event) -> (State, Effect)) -> Result<Report, String> {
    let alphabet = Event::all();
    let mut nodes = vec![(State::default(), vec![false; RULES.len()])];
    let mut ids = HashMap::from([(nodes[0].clone(), 0)]);
    let mut parents: Vec<Option<(usize, Event)>> = vec![None];
    let mut queue = VecDeque::from([0]);
    let mut activations: Vec<_> = RULES.iter().map(|r| (r.name, 0)).collect();
    let mut transitions = 0;
    while let Some(id) = queue.pop_front() {
        let (before, pending) = nodes[id].clone();
        for &event in &alphabet {
            let (after, effect) = reduce(before, event);
            let step = Step {
                before,
                event,
                after,
                effect,
            };
            let advanced =
                step_monitors(&pending, &step, |i| activations[i].1 += 1).map_err(|rule| {
                    let mut path = vec![event];
                    let mut ancestor = id;
                    while let Some((parent, edge)) = parents[ancestor] {
                        path.push(edge);
                        ancestor = parent;
                    }
                    path.reverse();
                    format!("{rule}: {path:?} -> {effect:?}, {after:?}")
                })?;
            let successor = (after, advanced);
            if let std::collections::hash_map::Entry::Vacant(entry) = ids.entry(successor.clone()) {
                let next_id = nodes.len();
                entry.insert(next_id);
                nodes.push(successor);
                parents.push(Some((id, event)));
                queue.push_back(next_id);
            }
            transitions += 1;
        }
    }
    if let Some((name, _)) = activations.iter().find(|(_, count)| *count == 0) {
        return Err(format!("unreachable antecedent: {name}"));
    }
    let states = nodes
        .iter()
        .map(|(state, _)| *state)
        .collect::<HashSet<_>>()
        .len();
    Ok(Report {
        states,
        product_states: nodes.len(),
        transitions,
        activations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residuals_agree_with_ltl_on_all_four_symbol_words() {
        for deadline in [Now, NextEvent] {
            let make_monitor = monitor_factory(deadline);
            let table = temporal_table(deadline);
            for word in 0u16..256 {
                let mut monitor = make_monitor();
                let mut previous = false;
                for index in 0..4 {
                    let p = (word >> (index * 2)) & 1 == 1;
                    let q = (word >> (index * 2 + 1)) & 1 == 1;
                    let due = if deadline == Now { p } else { previous };
                    let accepted = monitor.next_state(&(p, q));
                    assert_eq!(accepted, !due || q);
                    assert_eq!(accepted, table[due as usize][q as usize]);
                    if !accepted {
                        break;
                    }
                    previous = p;
                }
            }
        }
    }

    #[test]
    fn unavailable_feedback_has_a_reachable_stuttering_cycle() {
        let start = State::default();
        let prefix = [
            Event::Ask(Question::RequestedProbe, Gate::Supported),
            Event::Answer(Judgment::Correct, Assistance::Unknown),
        ];
        let pending = prefix
            .into_iter()
            .fold(start, |state, event| transition(state, event).0);
        assert_eq!(
            pending.phase,
            Phase::AwaitingFeedback(Next::IndependentCheck)
        );
        assert_eq!(
            transition(pending, Event::Feedback(Gate::Unavailable)),
            (pending, Effect::Withheld)
        );
    }

    #[test]
    fn reachable_graph() {
        let report = verify().unwrap();
        assert_eq!(
            report.transitions,
            report.product_states * Event::all().len()
        );
        println!("{report:#?}");
    }

    #[test]
    fn unsupported_instruction_counterexample() {
        let broken = |before, event| {
            if event == Event::Teach(Gate::Unavailable) {
                transition(before, Event::Teach(Gate::Supported))
            } else {
                transition(before, event)
            }
        };
        let witness = explore(broken).unwrap_err();
        assert!(witness.starts_with("release has current support: [Teach(Unavailable)]"));
    }

    #[test]
    fn dropped_feedback_counterexample() {
        let broken = |before: State, event| {
            if matches!(before.phase, Phase::AwaitingFeedback(_)) && event == Event::Pause {
                (
                    State {
                        phase: Phase::Ready,
                        paused: true,
                    },
                    Effect::Paused,
                )
            } else {
                transition(before, event)
            }
        };
        let witness = explore(broken).unwrap_err();
        assert!(witness.starts_with("feedback due survives interruptions:"));
        assert!(witness.contains("Pause]"));
    }

    #[test]
    fn next_obligation_is_carried_across_symbols() {
        let mut step = Step {
            before: State::default(),
            event: Event::Answer(Judgment::Missing, Assistance::Unknown),
            after: State {
                phase: Phase::AwaitingFeedback(Next::OfferBreak),
                paused: false,
            },
            effect: Effect::Analyzed(Next::OfferBreak),
        };
        let pending = step_monitors(&vec![false; RULES.len()], &step, |_| {}).unwrap();
        step.before = State::default();
        step.event = Event::Ask(Question::RequestedProbe, Gate::Supported);
        step.effect = Effect::Asked(Question::RequestedProbe);
        step.after.phase = Phase::AwaitingAnswer(Question::RequestedProbe);
        assert_eq!(
            step_monitors(&pending, &step, |_| {}).unwrap_err(),
            "question cannot immediately follow analysis"
        );
    }
}
