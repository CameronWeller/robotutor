//! Fixed service workloads. Answers and source judgments are fixtures, never inferences about students.

use super::{Metrics, Policy};
use crate::curriculum::{
    Checkpoint, Decision, Outcome, OutcomeId, Progress, ReviewedUnit, Slot, Span,
};
use crate::protocol::{
    transition, Assistance, Effect, Event, Gate, Judgment, Next, Phase, Question, State,
};
use crate::temporal::check_trace;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

#[derive(Clone, Copy)]
pub(super) struct Workload {
    pub name: &'static str,
    pub seats: u32,
    days: &'static [u32],
    daily_attempts: u32,
    script: u32,
}

pub(super) const DESIGN: [Workload; 2] = [
    Workload {
        name: "solo",
        seats: 1,
        days: &[
            0, 1, 2, 3, 5, 6, 7, 8, 12, 13, 14, 24, 25, 26, 55, 56, 57, 364, 365, 366, 729, 730,
            731, 1824, 1825,
        ],
        daily_attempts: 3,
        script: 0,
    },
    Workload {
        name: "class24",
        seats: 24,
        days: &[
            0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, 16, 17, 18, 22, 23, 24, 28, 29, 30, 34, 35, 36,
        ],
        daily_attempts: 10,
        script: 1,
    },
];

pub(super) const REGRESSION: [Workload; 2] = [
    Workload {
        name: "solo-gap",
        seats: 1,
        days: &[0, 2, 6, 7, 8, 30, 32, 35, 92, 95, 303, 306, 730, 732, 1825],
        daily_attempts: 2,
        script: 2,
    },
    Workload {
        name: "class32",
        seats: 32,
        days: &[0, 1, 3, 4, 6, 7, 9, 10, 13, 14, 16, 17, 41, 42, 43, 45],
        daily_attempts: 8,
        script: 3,
    },
];

// Frozen when the backward-feedback incident was added. The previously inspected reserved pair
// remains a regression suite and plays no part in selection.
pub(super) const RESERVED: [Workload; 2] = [
    Workload {
        name: "solo-return",
        seats: 1,
        days: &[
            0, 1, 3, 5, 6, 9, 15, 18, 35, 37, 91, 93, 181, 183, 362, 369, 726, 1100, 1825,
        ],
        daily_attempts: 2,
        script: 4,
    },
    Workload {
        name: "class29",
        seats: 29,
        days: &[
            0, 1, 3, 5, 6, 8, 10, 11, 13, 15, 16, 18, 20, 21, 44, 46, 48, 49, 51,
        ],
        daily_attempts: 7,
        script: 5,
    },
];

fn unit() -> ReviewedUnit {
    let outcomes = [(1, vec![]), (2, vec![1]), (3, vec![1]), (4, vec![2, 3])];
    ReviewedUnit::build(
        1,
        outcomes
            .into_iter()
            .map(|(id, requires)| Outcome {
                id: OutcomeId(id),
                requires: requires.into_iter().map(OutcomeId).collect(),
                support: BTreeSet::from([Span {
                    document: [id as u8; 32],
                    start: 0,
                    end: 1,
                }]),
            })
            .collect(),
    )
    .expect("reviewed fixture graph")
}

#[derive(Default)]
struct Conversation {
    state: State,
    events: Vec<Event>,
    valid_returns: u32,
    replacement_due: bool,
}

struct Learner {
    progress: Progress,
    conversations: BTreeMap<OutcomeId, Conversation>,
}

impl Default for Learner {
    fn default() -> Self {
        Self {
            progress: Progress {
                revision: 1,
                checkpoints: BTreeMap::new(),
                postponed: BTreeSet::new(),
            },
            conversations: BTreeMap::new(),
        }
    }
}

const CONTESTED: OutcomeId = OutcomeId(2);
// The integrated scoring rubric in this fixture directly cites the contested rubric. An ordinary
// prerequisite edge alone would not justify withdrawing another independently assessed result.
const DEPENDENT_SCORE: OutcomeId = OutcomeId(4);

impl Learner {
    fn apply_instrument_review(
        &mut self,
        session: u32,
        metrics: &mut Metrics,
    ) -> Result<(), String> {
        for id in [CONTESTED, DEPENDENT_SCORE] {
            let Some(thread) = self.conversations.get_mut(&id) else {
                continue;
            };
            if session == 12 {
                if emit(&mut thread.state, &mut thread.events, Event::Challenge)
                    != Effect::Invalidated
                {
                    return Err("contested instrument remained active".into());
                }
                metrics.reviews -= thread.valid_returns;
                thread.valid_returns = 0;
                if self.progress.checkpoints.remove(&id).is_some() {
                    thread.replacement_due = true;
                    metrics.retracted += 1;
                    metrics.first -= 1;
                }
            } else if session == 14
                && thread.state.phase == Phase::Disputed
                && emit(&mut thread.state, &mut thread.events, Event::Resolve) != Effect::Reopened
            {
                return Err("replacement rubric could not reopen the lane".into());
            }
        }
        Ok(())
    }
}

// Compare complete lane histories from their common fresh state, including transitions across visits.
#[derive(Default)]
pub(super) struct Audit(HashSet<Vec<Event>>);

impl Audit {
    pub fn checked_traces(&self) -> usize {
        self.0.len()
    }

    fn check(&mut self, events: Vec<Event>) -> Result<(), String> {
        if !self.0.contains(&events) {
            check_trace(State::default(), &events)?;
            self.0.insert(events);
        }
        Ok(())
    }
}

// A family contains alternate teaching, practice and check forms. All cache entries are reviewed,
// public to this one synthetic class, and free of learner data. Revision changes evict the bundle.
type Pack = (OutcomeId, u32);
#[derive(Default)]
struct Preparation {
    recent: VecDeque<Pack>,
}

impl Preparation {
    fn gate(
        &mut self,
        pack: Pack,
        policy: Policy,
        visit: &Visit,
        remaining: &mut u32,
        metrics: &mut Metrics,
    ) -> Gate {
        if let Some(index) = self.recent.iter().position(|key| *key == pack) {
            self.recent.remove(index);
            self.recent.push_back(pack);
            metrics.hits += 1;
            return Gate::Supported;
        }
        let (OutcomeId(objective), family) = pack;
        for attempt in 0..policy.attempts {
            if *remaining == 0 {
                break;
            }
            *remaining -= 1;
            metrics.calls += 1;
            // Simulates verifier rejection, including two adjacent failures and a correlated outage.
            let case = visit.session * 7 + visit.seat * 3 + objective * 5 + family + visit.script;
            let outage = (visit.session + visit.script) % 13 == 7;
            if outage || (case + u32::from(attempt)) % 11 < 3 {
                continue;
            }
            if policy.cache > 0 {
                if self.recent.len() == usize::from(policy.cache) {
                    self.recent.pop_front();
                }
                self.recent.push_back(pack);
            }
            return Gate::Supported;
        }
        Gate::Unavailable
    }
}

struct Visit {
    review_due: u32,
    day: u32,
    session: u32,
    seat: u32,
    script: u32,
}

impl Visit {
    fn observation(&self, id: OutcomeId, turn: u32) -> (Judgment, Assistance) {
        let case =
            (self.seat * 11 + self.session * 7 + id.0 * 3 + turn * 5 + self.script * 13) % 31;
        let judgment = match case {
            0 => Judgment::Missing,
            1..=3 => Judgment::Ambiguous,
            4..=8 => Judgment::Incorrect,
            _ => Judgment::Correct,
        };
        let help = match case {
            11 | 12 => Assistance::Reported,
            13..=15 => Assistance::Unknown,
            _ => Assistance::NoneReported,
        };
        (judgment, help)
    }
}

fn emit(state: &mut State, events: &mut Vec<Event>, event: Event) -> Effect {
    let (after, effect) = transition(*state, event);
    *state = after;
    events.push(event);
    effect
}

fn ask_event(next: Option<Next>, requested: bool, gate: Gate) -> Event {
    let question = match next {
        None if requested => Question::RequestedProbe,
        None | Some(Next::Explain | Next::OfferBreak | Next::HumanReview) => {
            return Event::Teach(gate)
        }
        Some(Next::Clarify) => Question::Clarification,
        Some(Next::IndependentCheck) => Question::IndependentCheck,
        Some(Next::ChangedExample) => Question::ChangedExample,
        Some(Next::WaitReview) => Question::DelayedReview,
    };
    Event::Ask(question, gate)
}

// Preserve state and outstanding work across attended sessions, including multi-year gaps.
fn encounter(
    learner: &mut Learner,
    id: OutcomeId,
    gate: Gate,
    visit: &Visit,
    extra: u8,
    metrics: &mut Metrics,
) -> Result<(), String> {
    let thread = learner.conversations.entry(id).or_default();
    let start = thread.state;
    let events = &mut thread.events;
    for turn in 0..=u32::from(extra) {
        let requested = turn == 0 && (visit.seat + visit.session + visit.script) % 9 == 4;
        if thread.state.phase == Phase::FollowUp(Next::HumanReview)
            && emit(&mut thread.state, events, Event::Resolve) != Effect::Reopened
        {
            return Err("fixture failed to resolve a reviewed handoff".into());
        }
        // Wait to open the scheduled review until its packet exists. Withholding a delayed prompt
        // before ReviewDue would be an invalid command in some protocol states.
        let follow_up = match thread.state.phase {
            Phase::Ready => None,
            Phase::FollowUp(next) => Some(next),
            phase => return Err(format!("unexpected resumable phase {phase:?}")),
        };
        if follow_up == Some(Next::WaitReview) {
            if gate == Gate::Unavailable {
                metrics.unserved += 1;
                break;
            }
            if emit(&mut thread.state, events, Event::ReviewDue) != Effect::ReviewOpened {
                return Err("fixture attempted to open an ineligible delayed review".into());
            }
        }
        let initial = ask_event(follow_up, requested, gate);
        let effect = emit(&mut thread.state, events, initial);
        if effect == Effect::Withheld {
            metrics.unserved += 1;
            break;
        }
        let asked = if effect == Effect::Taught {
            emit(
                &mut thread.state,
                events,
                Event::Ask(Question::Practice, gate),
            )
        } else {
            effect
        };
        if !matches!(asked, Effect::Asked(_)) {
            return Err(format!(
                "fixture could not start {initial:?} from {start:?}, got {asked:?}"
            ));
        }
        if turn > 0 {
            metrics.extra += 1;
        }
        let (judgment, help) = visit.observation(id, turn);
        let analysis = emit(&mut thread.state, events, Event::Answer(judgment, help));
        let Effect::Analyzed(next) = analysis else {
            return Err(format!("unexpected analysis {analysis:?}"));
        };
        if emit(&mut thread.state, events, Event::Feedback(gate)) != Effect::GaveFeedback(next) {
            return Err("analysis and delivered feedback diverged".into());
        }
        if next == Next::WaitReview {
            if !learner.progress.checkpoints.contains_key(&id) {
                metrics.first += 1;
            }
            if asked == Effect::Asked(Question::DelayedReview) {
                metrics.reviews += 1;
                thread.valid_returns += 1;
            }
            if thread.replacement_due {
                metrics.rebuilt += 1;
                thread.replacement_due = false;
            }
            // A fixture checkpoint is merely a script reaching the scheduled-review handoff.
            learner.progress.checkpoints.insert(
                id,
                Checkpoint {
                    observed: visit.day,
                    due: visit.review_due,
                },
            );
            break;
        }
        if matches!(next, Next::HumanReview | Next::OfferBreak) {
            break;
        }
    }
    Ok(())
}

pub(super) fn simulate(
    work: &Workload,
    policy: Policy,
    audit: &mut Audit,
) -> Result<Metrics, String> {
    replay(work, policy, audit, 7, |_, _, _, _, _| {})
}

fn replay(
    work: &Workload,
    policy: Policy,
    audit: &mut Audit,
    review_days: u32,
    mut observe: impl FnMut(u32, u32, Decision, Option<Gate>, &Learner),
) -> Result<Metrics, String> {
    let unit = unit();
    let mut learners: Vec<_> = (0..work.seats).map(|_| Learner::default()).collect();
    let mut prep = Preparation::default();
    let mut metrics = Metrics::default();
    for (i, &day) in work.days.iter().enumerate() {
        let session = i as u32;
        let source_incident = session == 10;
        // Simulated catalog revision and later reinstatement. Even cache hits re-enter the planner.
        if source_incident || session == 11 {
            prep.recent.clear();
        }
        if session == 12 {
            prep.recent
                .retain(|(id, _)| *id != CONTESTED && *id != DEPENDENT_SCORE);
        }
        let revoked = if source_incident {
            BTreeSet::from([[1; 32]])
        } else {
            BTreeSet::new()
        };
        let mut remaining = work.daily_attempts;
        for offset in 0..work.seats {
            // Rotate the first learner with access to the shared capacity at every attendance.
            let seat = (offset + session) % work.seats;
            let learner = &mut learners[seat as usize];
            learner.apply_instrument_review(session, &mut metrics)?;
            metrics.visits += 1;
            if learner
                .progress
                .checkpoints
                .values()
                .any(|mark| mark.due <= day)
            {
                metrics.late += 1;
            }
            learner.progress.postponed = learner
                .conversations
                .iter()
                .filter(|(_, c)| {
                    c.state.phase == Phase::FollowUp(Next::HumanReview)
                        && !(seat + session).is_multiple_of(4)
                })
                .map(|(&id, _)| id)
                .collect();
            if (12..14).contains(&session) {
                learner
                    .progress
                    .postponed
                    .extend([CONTESTED, DEPENDENT_SCORE]);
            }
            let preferred = if (session + seat).is_multiple_of(u32::from(policy.period)) {
                Slot::Review
            } else {
                Slot::Forward
            };
            let decision = unit
                .next(OutcomeId(4), &learner.progress, day, &revoked, preferred)
                .map_err(|err| format!("invalid curriculum fixture: {err:?}"))?;
            let id = match decision {
                Decision::Teach(id) | Decision::Recheck(id) => id,
                Decision::RepairSources(_) | Decision::Postponed(_) => {
                    metrics.unserved += 1;
                    observe(day, seat, decision, None, learner);
                    continue;
                }
                Decision::CurrentUntil(_) => {
                    observe(day, seat, decision, None, learner);
                    continue;
                }
            };
            let visit = Visit {
                review_due: day + review_days,
                day,
                session,
                seat,
                script: work.script,
            };
            let family = (session + seat) % 3;
            let gate = prep.gate((id, family), policy, &visit, &mut remaining, &mut metrics);
            encounter(learner, id, gate, &visit, policy.extra, &mut metrics)?;
            observe(day, seat, decision, Some(gate), learner);
        }
    }
    for learner in learners {
        if learner.progress.checkpoints.contains_key(&OutcomeId(4)) {
            metrics.goals += 1;
        }
        if learner.progress.checkpoints.is_empty() {
            metrics.zero += 1;
        }
        for conversation in learner.conversations.into_values() {
            if !conversation.events.is_empty() {
                audit.check(conversation.events)?;
                metrics.audits += 1;
            }
        }
    }
    Ok(metrics)
}

/// Change one constraint at a time on design workloads. Preserve the first unsuccessful seat's
/// baseline trace so an observer can distinguish unavailable material from ineligible targets.
pub(super) fn diagnose(policy: Policy) -> Result<String, String> {
    use std::fmt::Write;
    let mut report = String::new();
    for original in DESIGN {
        for (label, review_days, attempts) in [
            ("unchanged", 7, original.daily_attempts),
            ("fixed-21-days", 21, original.daily_attempts),
            ("fixed-30-days", 30, original.daily_attempts),
            ("defer-due-past-horizon", 1826, original.daily_attempts),
            (
                "double-shared-attempt-budget",
                7,
                2 * original.daily_attempts,
            ),
        ] {
            let mut timelines = vec![String::new(); original.seats as usize];
            let mut complete = vec![false; original.seats as usize];
            let mut goal_selected_seats = BTreeSet::new();
            let mut goal_selected_visits = 0;
            let mut goal_supported_visits = 0;
            let workload = Workload {
                daily_attempts: attempts,
                ..original
            };
            let metrics = replay(
                &workload,
                policy,
                &mut Audit::default(),
                review_days,
                |day, seat, decision, gate, learner| {
                    // Planner eligibility precedes pack preparation. An abstention can therefore
                    // leave a learner with a selected goal and no supported exercise this visit.
                    if matches!(
                        decision,
                        Decision::Teach(OutcomeId(4)) | Decision::Recheck(OutcomeId(4))
                    ) {
                        goal_selected_seats.insert(seat);
                        goal_selected_visits += 1;
                        goal_supported_visits += u32::from(gate == Some(Gate::Supported));
                    }
                    if label != "unchanged" {
                        return;
                    }
                    let seat = seat as usize;
                    let phase = match decision {
                        Decision::Teach(id) | Decision::Recheck(id) => {
                            learner.conversations.get(&id).map(|c| c.state.phase)
                        }
                        _ => None,
                    };
                    complete[seat] = learner.progress.checkpoints.contains_key(&OutcomeId(4));
                    let latest: Vec<_> = learner
                        .progress
                        .checkpoints
                        .iter()
                        .map(|(id, mark)| (id.0, mark.due))
                        .collect();
                    writeln!(&mut timelines[seat], "day {day}: {decision:?} {gate:?} {phase:?}; marks (objective,due) {latest:?}").unwrap();
                },
            )?;
            writeln!(&mut report, "{} {label}: {metrics:?}", original.name).unwrap();
            writeln!(
                &mut report,
                "  fixed_days={review_days} daily_attempts={attempts} policy={policy} goal_selected_seats={} goal_selected_visits={goal_selected_visits} goal_supported_visits={goal_supported_visits}",
                goal_selected_seats.len(),
            ).unwrap();
            if label == "unchanged" {
                if let Some(seat) = complete.iter().position(|done| !done) {
                    writeln!(
                        &mut report,
                        "First unfinished seat {seat}\n{}",
                        timelines[seat]
                    )
                    .unwrap();
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_intervals_start_at_each_observation_on_the_same_attendance_trace() {
        for work in DESIGN {
            for days in [7, 21, 30] {
                let mut observed_visits = 0;
                let mut saw_checkpoint = false;
                let metrics = replay(
                    &work,
                    Policy::BASELINE,
                    &mut Audit::default(),
                    days,
                    |day, _, _, _, learner| {
                        assert_eq!(day, work.days[observed_visits / work.seats as usize]);
                        observed_visits += 1;
                        for mark in learner.progress.checkpoints.values() {
                            assert_eq!(mark.due - mark.observed, days);
                            saw_checkpoint = true;
                        }
                    },
                )
                .unwrap();
                assert!(saw_checkpoint);
                assert_eq!(observed_visits, work.days.len() * work.seats as usize);
                assert_eq!(metrics.visits as usize, observed_visits);
                if days == 7 {
                    assert_eq!(
                        metrics,
                        simulate(&work, Policy::BASELINE, &mut Audit::default()).unwrap()
                    );
                }
            }
        }
    }
}
