//! Prospective, finite-horizon intervention appointments. Attendance and responses never enter
//! the sampling key. A missed appointment may be deferred within its band; term 24 is follow-up.

use std::cell::{Cell, RefCell};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::material::document_id;

pub const TERMS: u32 = 24;
pub const TERM_DAYS: u32 = 91;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Sparse,
    Finishing,
}

impl Mode {
    fn bands(self) -> [(u32, u32); 3] {
        match self {
            Self::Sparse => [(2, 8), (9, 16), (17, 23)],
            Self::Finishing => [(2, 16), (17, 20), (21, 23)],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::Finishing => "finishing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub term: u32,
    pub day: u32,
    pub ordinal: u8,
    pub specificity: u8,
}

/// Select one opportunity per disjoint band without replacement. Among successive eight-term
/// epochs Sparse assigns 1/1/1 calls; Finishing assigns one in the first sixteen terms and two
/// in the last eight. All appointments are strictly inside the six-year observation window.
pub fn schedule(mode: Mode, seed: u64, learner: u32) -> [Sample; 3] {
    let bands = mode.bands();
    std::array::from_fn(|index| {
        let (first, last) = bands[index];
        // Each candidate has the same selection rule independent of its term and attendance.
        let term = (first..=last)
            .min_by_key(|term| {
                let key = format!("hybrid-schedule/1/{}/{seed}/{learner}/{term}", mode.label());
                document_id(key.as_bytes())
            })
            .expect("all three term bands are nonempty");
        let ordinal = (index + 1) as u8;
        Sample {
            term,
            day: term * TERM_DAYS,
            ordinal,
            specificity: if mode == Mode::Sparse { 3 } else { ordinal },
        }
    })
}

/// Present each prospective appointment at the first attended endpoint on or after the
/// sampled term, within its own band. The caller advances one real visit at a time. A missed
/// opportunity is returned so it can be recorded, but it never triggers a dispatch. Consuming
/// the appointment on first attendance also prevents a failed provider call from being retried.
pub fn at_visit(
    mode: Mode,
    appointments: &[Sample; 3],
    term: u32,
    attended: bool,
    handled: &mut [bool; 3],
) -> Option<Sample> {
    for (i, &sample) in appointments.iter().enumerate() {
        if !handled[i] && (sample.term..=mode.bands()[i].1).contains(&term) {
            handled[i] = attended;
            return Some(Sample {
                term,
                day: term * TERM_DAYS,
                ..sample
            });
        }
    }
    None
}

/// One row per actual simulated term endpoint. A missed endpoint still has a calendar row.
/// `effect` refers to intervention-induced learner updates, excluding the baseline simulator's
/// regular practice and assessment. Specificity records the appointment even on a skipped call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceStep {
    pub term: u32,
    pub day: u32,
    pub attended: bool,
    pub called: bool,
    pub specificity: Option<u8>,
    pub validated: bool,
    pub displayed: bool,
    pub effect: bool,
}

fn always<T: 'static>(condition: fn(&T) -> bool, label: &'static str) -> Monitor<T> {
    let mut builder = Predicates::builder();
    let predicate = builder.new_predicate(ClosurePredicate::new(condition, label));
    let predicates = builder.build();
    let machine =
        Property::always(Property::atomic(predicate)).to_monitoring_automaton(&predicates);
    Monitor::new(predicates, machine)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Permission {
    None,
    Call,
    Display,
}

#[derive(Clone, Copy)]
enum Boundary {
    Request,
    Release,
    Endpoint,
}

struct Appointment {
    step: TraceStep,
    clock_term: u32,
    expected: Option<Sample>,
    absent_for_term: bool,
    permission: Permission,
    boundary: Boundary,
}

type Rule = (&'static str, fn(&Appointment) -> bool);
const RULES: [Rule; 6] = [
    ("term/day order", |a| {
        a.step.term == a.clock_term && a.step.day == a.clock_term * TERM_DAYS
    }),
    ("scheduled specificity", |a| {
        a.step.specificity == a.expected.map(|s| s.specificity)
    }),
    ("structural absence", |a| {
        !a.absent_for_term || !a.step.attended
    }),
    ("call admission", |a| {
        !a.step.called || a.step.attended && a.expected.is_some()
    }),
    ("validated display before effect", |a| {
        (!a.step.validated || a.step.called)
            && (!a.step.displayed || a.step.validated)
            && (!a.step.effect || a.step.displayed)
    }),
    ("permission for this term", |a| match a.boundary {
        Boundary::Request => a.permission == Permission::None,
        Boundary::Release => a.permission == Permission::Call,
        Boundary::Endpoint => {
            a.step.called == (a.permission != Permission::None)
                && (!a.step.displayed || a.permission == Permission::Display)
        }
    }),
];

/// Per-run Telo monitors. Admission and display are pre-effect checks; observations include
/// actual receipts. The attendance prefix contains only completed terms. No replay or future
/// attendance lookup is needed as each term (including an absence) is committed.
pub struct Gate {
    mode: Mode,
    learner: u32,
    plan: [Sample; 3],
    history: Vec<bool>,
    monitors: RefCell<[Monitor<Appointment>; RULES.len()]>,
    permission: Cell<Permission>,
    failed: Option<String>,
}

impl Gate {
    pub fn new(mode: Mode, seed: u64, learner: u32) -> Self {
        Self {
            mode,
            learner,
            plan: schedule(mode, seed, learner),
            history: Vec::with_capacity(TERMS as usize),
            monitors: RefCell::new(RULES.map(|(name, condition)| always(condition, name))),
            permission: Cell::new(Permission::None),
            failed: None,
        }
    }

    fn check(&self, step: TraceStep, boundary: Boundary) -> Result<(), String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let index = self.history.len();
        if index >= TERMS as usize {
            return Err("observation past the final term".into());
        }
        let clock_term = index as u32 + 1;
        // Independently reconstruct the prospective appointment from the acknowledged prefix.
        let expected = self.plan.iter().copied().enumerate().find_map(|(i, p)| {
            ((p.term..=self.mode.bands()[i].1).contains(&clock_term)
                && !self.history[(p.term - 1) as usize..].contains(&true))
            .then_some(p)
        });
        let event = Appointment {
            step,
            clock_term,
            expected,
            absent_for_term: self.learner.is_multiple_of(3) && (13..=14).contains(&clock_term),
            permission: self.permission.get(),
            boundary,
        };
        // Each property is G(P). Sampling hypothetical sends/releases is stutter invariant:
        // an accepted precondition leaves the same obligation; a rejected proposal is withheld.
        for ((name, _), monitor) in RULES.iter().zip(self.monitors.borrow_mut().iter_mut()) {
            if !monitor.next_state(&event) {
                return Err(format!("{} term {clock_term}: {name}", self.mode.label()));
            }
        }
        Ok(())
    }

    /// Call exactly once immediately before the transport is invoked. A failed proposal
    /// grants no call and leaves the learner free to proceed through the inexpensive path.
    pub fn admit(
        &self,
        term: u32,
        day: u32,
        attended: bool,
        specificity: Option<u8>,
    ) -> Result<(), String> {
        self.check(
            TraceStep {
                term,
                day,
                attended,
                specificity,
                called: true,
                validated: false,
                displayed: false,
                effect: false,
            },
            Boundary::Request,
        )?;
        self.permission.set(Permission::Call);
        Ok(())
    }

    /// Evaluate the *proposed* display against the live validator outcome and this term's
    /// admission before any card is shown or the synthetic learner is updated.
    pub fn allow_display(
        &self,
        term: u32,
        day: u32,
        attended: bool,
        specificity: u8,
        validated: bool,
    ) -> Result<(), String> {
        self.check(
            TraceStep {
                term,
                day,
                attended,
                specificity: Some(specificity),
                called: true,
                validated,
                displayed: true,
                effect: false,
            },
            Boundary::Release,
        )?;
        self.permission.set(Permission::Display);
        Ok(())
    }

    pub fn observe(&mut self, step: TraceStep) -> Result<(), String> {
        if let Err(error) = self.check(step, Boundary::Endpoint) {
            self.failed = Some(error.clone());
            return Err(error);
        }
        self.history.push(step.attended);
        self.permission.set(Permission::None);
        Ok(())
    }

    pub fn finish(&self) -> Result<(), String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if self.history.len() != TERMS as usize {
            return Err(format!(
                "expected {TERMS} term endpoints, received {}",
                self.history.len()
            ));
        }
        Ok(())
    }
}

/// Replay the same guards from a recorded trace as a test oracle for the live gate.
#[cfg(test)]
fn verify(mode: Mode, seed: u64, learner: u32, trace: &[TraceStep]) -> Result<(), String> {
    let mut gate = Gate::new(mode, seed, learner);
    for &s in trace {
        if s.called {
            gate.admit(s.term, s.day, s.attended, s.specificity)?;
        }
        if s.displayed {
            gate.allow_display(
                s.term,
                s.day,
                s.attended,
                s.specificity.ok_or("unscheduled display")?,
                s.validated,
            )?;
        }
        gate.observe(s)?;
    }
    gate.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(mode: Mode, seed: u64, learner: u32, absent: &[u32]) -> Vec<TraceStep> {
        let plan = schedule(mode, seed, learner);
        let mut handled = [false; 3];
        (1..=TERMS)
            .map(|term| {
                let attended = !absent.contains(&term)
                    && !(learner.is_multiple_of(3) && (13..=14).contains(&term));
                let specificity =
                    at_visit(mode, &plan, term, attended, &mut handled).map(|p| p.specificity);
                let called = attended && specificity.is_some();
                TraceStep {
                    term,
                    day: term * TERM_DAYS,
                    attended,
                    called,
                    specificity,
                    validated: called,
                    displayed: called,
                    effect: called,
                }
            })
            .collect()
    }

    #[test]
    fn seeded_bands_keep_a_post_intervention_endpoint_and_raise_finishing_density() {
        for seed in [0, 1, 20261118, u64::MAX] {
            let sparse = schedule(Mode::Sparse, seed, 1);
            assert_eq!(sparse, schedule(Mode::Sparse, seed, 1));
            assert_eq!(sparse.map(|p| p.specificity), [3, 3, 3]);
            assert!((2..=8).contains(&sparse[0].term));
            assert!((9..=16).contains(&sparse[1].term));
            assert!((17..=23).contains(&sparse[2].term));
            let finishing = schedule(Mode::Finishing, seed, 1);
            assert_eq!(finishing.map(|p| p.specificity), [1, 2, 3]);
            assert!((2..=16).contains(&finishing[0].term));
            assert!((17..=20).contains(&finishing[1].term));
            assert!((21..=23).contains(&finishing[2].term));
        }
        let variants: std::collections::BTreeSet<_> = (0..32)
            .map(|seed| schedule(Mode::Sparse, seed, 1).map(|p| p.term))
            .collect();
        assert!(variants.len() > 1);
    }

    #[test]
    fn verifier_rejects_clock_specificity_attendance_and_release_mutations() {
        for mode in [Mode::Sparse, Mode::Finishing] {
            let seed = 3;
            let learner = 1;
            let steps = observed(mode, seed, learner, &[]);
            let index = (schedule(mode, seed, learner)[1].term - 1) as usize;
            let check = |trace: &[TraceStep]| verify(mode, seed, learner, trace);
            check(&steps).unwrap();
            assert!(check(&steps[..23]).is_err());
            for change in [
                |s: &mut TraceStep| s.day -= 1,
                |s: &mut TraceStep| s.term -= 1,
                |s: &mut TraceStep| s.specificity = Some(99),
                |s: &mut TraceStep| s.attended = false,
                |s: &mut TraceStep| s.called = false,
                |s: &mut TraceStep| s.validated = false,
                |s: &mut TraceStep| s.displayed = false,
            ] {
                let mut mutant = steps.clone();
                change(&mut mutant[index]);
                assert!(check(&mutant).is_err());
            }
            let mut unscheduled = steps.clone();
            unscheduled[0].called = true;
            assert!(check(&unscheduled).is_err());
            let mut rejected = steps;
            for (validated, displayed) in [(false, false), (true, false)] {
                rejected[index].validated = validated;
                rejected[index].displayed = displayed;
                rejected[index].effect = false;
                check(&rejected).unwrap();
            }
        }
    }

    #[test]
    fn stale_calls_and_unvalidated_displays_are_stopped_before_side_effects() {
        let mode = Mode::Finishing;
        let mut gate = Gate::new(mode, 20261118, 1);
        let rows = observed(mode, 20261118, 1, &[]);
        let first = rows.iter().position(|s| s.called).unwrap();
        for &step in &rows[..first] {
            gate.observe(step).unwrap();
        }
        let mut step = rows[first];
        let level = step.specificity.unwrap();
        let calls = Cell::new(0);
        let dispatch = |day, specificity| -> Result<(), String> {
            gate.admit(step.term, day, step.attended, specificity)?;
            calls.set(calls.get() + 1);
            Ok(())
        };
        assert!(dispatch(step.day - 1, step.specificity).is_err());
        assert!(dispatch(step.day, Some(level + 1)).is_err());
        assert_eq!(calls.get(), 0);
        assert!(gate
            .allow_display(step.term, step.day, step.attended, level, true)
            .is_err());
        dispatch(step.day, step.specificity).unwrap();
        assert!(dispatch(step.day, step.specificity).is_err());
        assert_eq!(calls.get(), 1);
        let mut displays = 0;
        let release = gate
            .allow_display(step.term, step.day, step.attended, level, false)
            .map(|()| displays += 1);
        assert!(release.is_err());
        assert_eq!(displays, 0);
        step.validated = false;
        step.displayed = false;
        step.effect = false;
        gate.observe(step).unwrap();
        for &later in &rows[first + 1..] {
            if later.called {
                gate.admit(later.term, later.day, later.attended, later.specificity)
                    .unwrap();
                let release = || {
                    gate.allow_display(
                        later.term,
                        later.day,
                        later.attended,
                        later.specificity.unwrap(),
                        true,
                    )
                };
                release().unwrap();
                assert!(release().is_err());
            }
            gate.observe(later).unwrap();
        }
        gate.finish().unwrap();
    }

    #[test]
    fn actual_display_without_a_release_latches_the_first_failure() {
        let mode = Mode::Sparse;
        let mut gate = Gate::new(mode, 20261118, 1);
        let rows = observed(mode, 20261118, 1, &[]);
        let first = rows.iter().position(|s| s.called).unwrap();
        for &step in &rows[..first] {
            gate.observe(step).unwrap();
        }
        assert!(gate.finish().is_err());
        let step = rows[first];
        gate.admit(step.term, step.day, true, step.specificity)
            .unwrap();
        let error = gate.observe(step).unwrap_err();
        assert!(error.contains("permission for this term"));
        // Even a corrected receipt or fresh proposal cannot conceal the earlier displayed card.
        assert_eq!(
            gate.observe(TraceStep {
                displayed: false,
                effect: false,
                ..step
            })
            .unwrap_err(),
            error
        );
        assert_eq!(
            gate.admit(step.term, step.day, true, step.specificity)
                .unwrap_err(),
            error
        );
        assert_eq!(gate.finish().unwrap_err(), error);
    }

    #[test]
    fn structural_absence_defers_within_its_band_without_calling_until_attendance() {
        // Find an appointment which falls in known absence; sampling must not peek ahead to avoid it.
        let learner = 0;
        let mode = Mode::Sparse;
        let seed = (0..1000)
            .find(|&seed| (13..=14).contains(&schedule(mode, seed, learner)[1].term))
            .unwrap();
        let mut trace = observed(mode, seed, learner, &[]);
        verify(mode, seed, learner, &trace).unwrap();
        assert!(!trace[12].called && !trace[13].called);
        assert!(trace[14].called);
        assert_eq!(trace.iter().filter(|s| s.called).count(), 3);
        trace[12].attended = true;
        assert!(verify(mode, seed, learner, &trace).is_err());
    }

    #[test]
    fn missed_appointments_never_leak_into_another_band_or_the_final_probe() {
        let seed = 20261118;
        let learner = 1;
        let mode = Mode::Sparse;
        let mut trace = observed(mode, seed, learner, &[6, 7, 8, 21, 22, 23]);
        verify(mode, seed, learner, &trace).unwrap();
        assert_eq!(trace.iter().filter(|s| s.called).count(), 1);
        for illegal_term in [9, 17, 24] {
            let i = illegal_term - 1;
            trace[i].called = true;
            assert!(verify(mode, seed, learner, &trace).is_err());
            trace[i].called = false;
        }
    }

    #[test]
    fn default_control_and_deferred_appointments_remain_reproducible() {
        let seed = 20261118;
        let learner = 1;
        // Attendance at these precommitted appointments in the regular 30-day arm.
        // Test both the original fixed control and the independent temporal verdict on deferral.
        for (mode, fixed, actual) in [
            (Mode::Sparse, [6, 16, 21], [8, 16, 22]),
            (Mode::Finishing, [2, 20, 21], [2, 20, 22]),
        ] {
            assert_eq!(schedule(mode, seed, learner).map(|s| s.term), fixed);
            let trace = observed(mode, seed, learner, &[6, 7, 21]);
            verify(mode, seed, learner, &trace).unwrap();
            let calls: Vec<_> = trace.iter().filter(|s| s.called).map(|s| s.term).collect();
            assert_eq!(calls, actual);
            let mut duplicate = trace;
            duplicate[22].called = true;
            assert!(verify(mode, seed, learner, &duplicate).is_err());
        }
    }
}
