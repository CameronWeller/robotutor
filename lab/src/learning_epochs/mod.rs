//! Synthetic learner and calendar for longitudinal lab experiments. Response
//! probabilities, learning effects and attendance here are explicit assumptions.
//! A tutor gets observed receipts only. This module never calls a model or
//! issues a real learner credential.
use crate::{
    curriculum::{goal_evidence::check_quantity_text, OutcomeId, Span},
    history::{Attempt, Scope, Withdrawn},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{Assistance, Judgment, Question},
    retention::{self, Cell, CheckResult, Exposure, Moment, Prior},
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const TERM: u32 = 91;
pub const TRANSFER_RULE: &str = "A quarter-litre consists of two eighth-litres. The delivered volume equals the original volume minus the remainder.";
pub const DEFAULT_SEED: u64 = 20261118;
pub mod pilot;
const PRACTICE_ATTENDANCE: f64 = 0.80;
const CHECK_ATTENDANCE: f64 = 0.88;
const PRACTICE_HELP: f64 = 0.18;
const CHECK_HELP: f64 = 0.12;
pub const ARMS: [Arm; 3] = [Arm::Clustered, Arm::Regular, Arm::ChecksOnly];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    Clustered,
    Regular,
    ChecksOnly,
}

impl Arm {
    fn offsets(self) -> &'static [u32] {
        match self {
            Self::Clustered => &[21, 42, 63],
            Self::Regular => &[3, 33, 63],
            Self::ChecksOnly => &[],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Clustered => "clustered_21",
            Self::Regular => "regular_30",
            Self::ChecksOnly => "checks_only",
        }
    }
}

#[derive(Clone, Copy)]
enum Noise {
    InitialMemory,
    ApplyUnits,
    Attendance,
    Help,
    Retrieve,
    Convert,
    OutsideInvite,
    OutsideAttendance,
    OutsideReceipt,
}

pub struct Participant {
    pub seed: u64,
    pub id: u32,
}

impl Participant {
    // An opportunity's draws match across arms even when its appointment date differs.
    fn draw(&self, term: u32, slot: u8, channel: Noise) -> f64 {
        let mut key = [0; 18];
        key[..8].copy_from_slice(&self.seed.to_le_bytes());
        key[8..12].copy_from_slice(&self.id.to_le_bytes());
        key[12..16].copy_from_slice(&term.to_le_bytes());
        key[16] = slot;
        key[17] = channel as u8;
        let bits = u64::from_le_bytes(document_id(&key)[..8].try_into().unwrap()) >> 11;
        bits as f64 / (1_u64 << 53) as f64
    }
}

struct Memory {
    last_encounter: u32,
    stability_days: f64,
    conversion: f64,
    learns: bool,
}

impl Memory {
    fn recall(&self, day: u32) -> f64 {
        1.0 / (1.0 + f64::from(day - self.last_encounter) / (9.0 * self.stability_days))
    }
    fn encounter(&mut self, day: u32, independent_success: bool) {
        if self.learns {
            // Shared assumptions with retention_investigation. Cap growth explicitly.
            let gain = if independent_success {
                (1.2 * (1.0 - self.recall(day))).exp()
            } else {
                1.05
            };
            self.stability_days = (self.stability_days * gain).min(180.0);
            self.last_encounter = day;
        }
    }

    fn volume_answer(
        &self,
        p: &Participant,
        term: u32,
        slot: u8,
        day: u32,
        quarters: i64,
        remaining: i64,
    ) -> (bool, String, Judgment) {
        let assisted = p.draw(term, slot, Noise::Help) < CHECK_HELP;
        let recalls = p.draw(term, slot, Noise::Retrieve) < self.recall(day);
        let converts = p.draw(term, slot, Noise::Convert) < self.conversion;
        let actual = if assisted || (recalls && converts) {
            2 * quarters - remaining
        } else if !recalls {
            2 * quarters + remaining
        } else {
            quarters - remaining
        };
        let response = format!("{actual} eighths");
        let judgment = check_quantity_text(2 * quarters - remaining, "eighths", &response);
        (assisted, response, judgment)
    }
}

pub fn dispenser_question(quarters: i64, left: i64) -> String {
    format!("A dispenser held {quarters} quarter-litres. Now {left} eighth-litres remain. How many eighth-litres left the dispenser? Respond in eighths.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Scheduled,
    ReturnFromAbsence,
}

pub enum Events<'a> {
    Silent,
    Print,
    Capture(&'a mut Vec<Value>),
}

impl Events<'_> {
    fn record(&mut self, event: impl FnOnce(bool) -> Value) {
        match self {
            Self::Silent => {}
            Self::Print => println!("{}", event(false)),
            Self::Capture(history) => history.push(event(true)),
        }
    }

    fn receipt(&mut self, event: impl FnOnce() -> Value) {
        if let Self::Capture(history) = self {
            history.push(event());
        }
    }
}

pub struct TermSession<'a> {
    pub participant: &'a Participant,
    pub term: u32,
    pub arm: Arm,
    pub rule: &'a RuleSource<'a>,
}

impl Participant {
    pub fn away(&self, term: u32) -> bool {
        self.id.is_multiple_of(3) && (12..14).contains(&term)
    }
}

pub struct Learner {
    memory: Memory,
    anchor: Prior,
    contacts: Vec<Exposure>,
    coverage: bool,
    questions_seen: BTreeSet<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Counts {
    pub practice_offered: u32,
    pub practice_attended: u32,
    pub practice_assisted: u32,
    pub practice_errors: u32,
    pub missed_probes: u32,
    pub independent_probes: u32,
    pub independent_correct: u32,
    pub assisted_probes: u32,
    pub supported: u32,
    pub supported_unseen_family: u32,
    pub opposed: u32,
    pub unknown: u32,
    pub short_gap: u32,
}

impl Counts {
    pub fn report(&self) -> Value {
        json!({
            "practice_offered":self.practice_offered,"practice_attended":self.practice_attended,
            "practice_assisted":self.practice_assisted,"practice_errors":self.practice_errors,
            "missed_probes":self.missed_probes,"independent_probes":self.independent_probes,
            "independent_correct":self.independent_correct,"assisted_probes":self.assisted_probes,
            "supports_durability":self.supported,"opposes_durability":self.opposed,
            "supports_unseen_family":self.supported_unseen_family,
            "unknown_exposure":self.unknown,"insufficient_spacing":self.short_gap
        })
    }
}

fn moment(day: u32) -> Moment {
    Moment {
        day,
        sequence: u64::from(day),
    }
}

impl Learner {
    pub fn new(p: &Participant, learns: bool) -> Self {
        let cell = Cell {
            scope: Scope {
                learner: u64::from(p.id),
                revision: 1,
            },
            outcome: OutcomeId(2),
        };
        Self {
            memory: Memory {
                last_encounter: 0,
                stability_days: [4.0, 14.0, 40.0]
                    [(p.draw(0, 0, Noise::InitialMemory) * 3.0) as usize],
                conversion: 0.50 + 0.45 * p.draw(0, 0, Noise::ApplyUnits),
                learns,
            },
            // All learners enter with a known independent day-zero missing-part success.
            // The tutor has the same estimate for everyone and cannot see individual stability.
            anchor: Prior {
                cell,
                verified: moment(0),
                stability_days: 14.0,
                threshold: 0.85,
                had_durable_success: false,
            },
            contacts: Vec::new(),
            coverage: true,
            questions_seen: BTreeSet::new(),
        }
    }

    fn contact(&mut self, day: u32) {
        self.contacts.push(Exposure {
            cell: self.anchor.cell,
            at: moment(day),
        });
    }

    fn practice(
        &mut self,
        p: &Participant,
        term: u32,
        arm: Arm,
        source: &RuleSource<'_>,
        counts: &mut Counts,
        events: &mut Events<'_>,
    ) {
        for (slot, &offset) in arm.offsets().iter().enumerate() {
            counts.practice_offered += 1;
            let slot = slot as u8;
            let day = term * TERM + offset;
            if p.draw(term, slot, Noise::Attendance) >= PRACTICE_ATTENDANCE {
                events.receipt(|| json!({"kind":"missed_practice","day":day,"seed":p.seed,"learner":p.id,"policy":arm.label()}));
                continue;
            }
            counts.practice_attended += 1;
            let first = 1 + ((term + u32::from(slot)) % 5) as u16;
            let remaining = 1 + ((term * 3 + u32::from(slot)) % 7) as u16;
            let card =
                FractionCard::propose(FractionTask::MissingPart, first, remaining, 16).unwrap();
            let (prompt, _) = card.question(source).expect("a supported practice opening");
            let assisted = p.draw(term, slot, Noise::Help) < PRACTICE_HELP;
            let remembers = p.draw(term, slot, Noise::Retrieve) < self.memory.recall(day);
            // One modeled error is adding the removed part instead of subtracting it.
            let amount = if remembers || assisted {
                remaining
            } else {
                remaining + 2 * first
            };
            let response = format!("{amount}/16");
            let correct = card.assess(&response, source) == Some(Judgment::Correct);
            let worked = (assisted || !correct).then(|| {
                card.worked(source)
                    .expect("a supported worked correction")
                    .0
            });
            events.record(|_| json!({"kind":"synthetic_practice","seed":p.seed,"learner":p.id,"policy":arm.label(),
                    "day":day,"prompt":prompt,"response":response,"worked_before_answer":assisted,
                    "worked":worked,"correct":correct}));
            counts.practice_assisted += u32::from(assisted);
            counts.practice_errors += u32::from(!correct);
            self.memory.encounter(day, correct && !assisted);
            self.contact(day);
        }
    }

    fn outside(&mut self, p: &Participant, term: u32, endpoint: u32, events: &mut Events<'_>) {
        if p.draw(term, 3, Noise::OutsideInvite) >= 0.22 {
            return;
        }
        let attended = p.draw(term, 3, Noise::OutsideAttendance) < 0.55;
        let known = p.draw(term, 3, Noise::OutsideReceipt) < 0.55;
        events.receipt(|| {
            json!({"kind":"outside_invitation","day":endpoint-3,"seed":p.seed,"learner":p.id,
            "attendance":if !known {"unknown"} else if attended {"attended"} else {"absent"}})
        });
        if attended {
            self.memory.encounter(endpoint - 3, false);
            if known {
                self.contact(endpoint - 3);
            }
        }
        if !known {
            self.coverage = false;
        }
    }

    fn probe(
        &mut self,
        p: &Participant,
        term: u32,
        counts: &mut Counts,
        events: &mut Events<'_>,
        arm: Arm,
        probe: Probe,
    ) -> Option<bool> {
        let (day, slot) = match probe {
            Probe::Scheduled => ((term + 1) * TERM, 3),
            Probe::ReturnFromAbsence => (term * TERM + 1, 4),
        };
        if p.draw(term, slot, Noise::Attendance) >= CHECK_ATTENDANCE {
            counts.missed_probes += 1;
            events.record(|_|json!({"kind":"missed_probe","occasion":format!("{probe:?}"),"seed":p.seed,"learner":p.id,"policy":arm.label(),"day":day}));
            return None;
        }
        let form = 2 * term + u32::from(probe == Probe::ReturnFromAbsence);
        let quarters = 6 + i64::from(form / 4);
        let left = 1 + i64::from(form % 4);
        let question = dispenser_question(quarters, left);
        // Every volume probe is in the same family. A missed invitation never shows the task.
        let first_dispenser_task = self.questions_seen.is_empty();
        let fresh = self.questions_seen.insert(question.clone());
        let (assisted, response, judgment) = self
            .memory
            .volume_answer(p, term, slot, day, quarters, left);
        let cell = self.anchor.cell;
        let attempt = Attempt {
            scope: cell.scope,
            id: u64::from(day),
            sequence: u64::from(day),
            outcome: cell.outcome,
            item: u64::from(form) + 1,
            day,
            question: Question::ChangedExample,
            judgment,
            assistance: if assisted {
                Assistance::Reported
            } else {
                Assistance::NoneReported
            },
            fresh,
            sources: [document_id(TRANSFER_RULE.as_bytes())].into(),
        };
        let (_, finding) = retention::read(
            cell,
            moment(day),
            None,
            Some(self.anchor),
            self.coverage.then_some(&self.contacts[..]),
            Some(&attempt),
            &Withdrawn::new(cell.scope),
        )
        .expect("ordered evidence events");
        let correct = judgment == Judgment::Correct;
        counts.assisted_probes += u32::from(assisted);
        counts.independent_probes += u32::from(!assisted);
        counts.independent_correct += u32::from(correct && !assisted);
        counts.supported += u32::from(finding == CheckResult::SupportsDurability);
        counts.supported_unseen_family +=
            u32::from(first_dispenser_task && finding == CheckResult::SupportsDurability);
        counts.opposed += u32::from(finding == CheckResult::OpposesDurability);
        counts.unknown += u32::from(finding == CheckResult::UnknownExposure);
        counts.short_gap += u32::from(finding == CheckResult::InsufficientSpacing);
        events.record(|detailed| {
            let worked = (assisted || !correct).then(|| format!("{quarters} quarter-litres equals {} eighth-litres. Taking away the {left} eighth-litres that remain gives {} eighth-litres delivered.",2*quarters,2*quarters-left));
            let mut event = json!({"kind":"synthetic_probe","occasion":format!("{probe:?}"),"seed":p.seed,"learner":p.id,"policy":arm.label(),"day":day,
                "prompt":question,"response":response,"assisted":assisted,"judgment":format!("{judgment:?}"),
                "worked_before_answer":assisted,"worked":worked,"coverage":self.coverage,
                "first_dispenser_task":first_dispenser_task,"finding":format!("{finding:?}")});
            if detailed {
                event["fresh"] = json!(fresh);
                event["quarters"] = json!(quarters);
                event["remaining_eighths"] = json!(left);
            }
            event
        });
        if correct && !assisted && fresh {
            self.anchor.verified = moment(day);
            self.anchor.had_durable_success |= finding == CheckResult::SupportsDurability;
            self.contacts.clear();
            self.coverage = true;
        }
        // Repeated testing is an intervention, including in the checks-only arm.
        self.contact(day);
        self.memory.encounter(day, correct && !assisted);
        (!assisted).then_some(correct)
    }

    pub fn advance_term(
        &mut self,
        session: &TermSession<'_>,
        counts: &mut Counts,
        reentry: &mut Counts,
        events: &mut Events<'_>,
    ) -> Option<bool> {
        let p = session.participant;
        let term = session.term;
        let arm = session.arm;
        let day = (term + 1) * TERM;
        if p.away(term) {
            self.coverage = false;
            counts.missed_probes += 1;
            events.receipt(|| {
                json!({"kind":"term_absence","seed":p.seed,"learner":p.id,
                "policy":arm.label(),"day":day,"coverage":false})
            });
            return None;
        }
        if term == 14 && p.id.is_multiple_of(3) {
            self.probe(p, term, reentry, events, arm, Probe::ReturnFromAbsence);
        }
        self.practice(p, term, arm, session.rule, counts, events);
        self.outside(p, term, day, events);
        self.probe(p, term, counts, events, arm, Probe::Scheduled)
    }
}

pub fn practice_source(revoked: &BTreeSet<crate::curriculum::Document>) -> RuleSource<'_> {
    RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin: Span {
            document: document_id(UNIT_RULE.as_bytes()),
            start: 0,
            end: UNIT_RULE.len() as u32,
        },
        revoked,
    }
}

mod tracked;
pub use tracked::{
    transfer_question, worked_example, Lesson, TermObservation, TrackedLearner,
    INITIAL_EIGHTHS_QUESTION,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_positive_delayed_read_requires_complete_spaced_independent_work() {
        use telo::{
            monitor::Monitor,
            predicate::{ClosurePredicate, Predicates},
            Property,
        };
        let predictions = || {
            let mut builder = Predicates::builder();
            let earned = builder.new_predicate(ClosurePredicate::new(
                |pair: &(bool, bool)| pair.0 == pair.1,
                "observed support agrees with the recorded conditions",
            ));
            (builder.build(), Property::always(Property::atomic(earned)))
        };
        let (predicates, property) = predictions();
        let automaton = property.to_monitoring_automaton(&predicates);
        let mut positives = 0;
        let mut familiar_positives = 0;
        for id in 0..16 {
            let p = Participant {
                seed: DEFAULT_SEED,
                id,
            };
            let mut learner = Learner::new(&p, true);
            let mut monitor = Monitor::new(predictions().0, automaton.clone());
            for term in 0..24 {
                let day = (term + 1) * TERM;
                learner.outside(&p, term, day, &mut Events::Silent);
                let last_contact = learner
                    .contacts
                    .iter()
                    .map(|e| e.at.day)
                    .max()
                    .unwrap_or(learner.anchor.verified.day);
                // At the fixture's fixed threshold and S=14, 23 whole days are needed.
                let can_support = learner.coverage && day - last_contact >= 23;
                let first_dispenser_task = learner.questions_seen.is_empty();
                let mut c = Counts::default();
                let answer = learner.probe(
                    &p,
                    term,
                    &mut c,
                    &mut Events::Silent,
                    Arm::ChecksOnly,
                    Probe::Scheduled,
                );
                let earned = can_support && answer == Some(true);
                positives += u32::from(earned);
                assert!(monitor.next_state(&(c.supported == 1, earned)));
                assert_eq!(
                    c.supported_unseen_family,
                    u32::from(earned && first_dispenser_task)
                );
                familiar_positives += u32::from(earned && !first_dispenser_task);
            }
        }
        assert!(positives > 0);
        assert!(familiar_positives > 0);
        let mut stale = Monitor::new(predictions().0, automaton);
        assert!(!stale.next_state(&(true, false)));
    }
}
