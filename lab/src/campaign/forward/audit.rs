//! Finite-trace safety checks on recorded trial results. Domain replay supplies
//! the witness and error counts; an internally consistent journal cannot establish
//! that a domain oracle is correct or that the change helped an actual learner.

use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};

#[derive(Clone, Debug)]
pub(super) struct RoundCheck {
    pub(super) pass: u32,
    pub(super) before: String,
    pub(super) candidate: String,
    pub(super) after: String,
    pub(super) promoted: bool,
    pub(super) witness_matches_before: bool,
    pub(super) witness_fixed: bool,
    pub(super) regression_before: u32,
    /// Measured on the candidate, including rejected candidates.
    pub(super) regression_after: u32,
    pub(super) candidate_replayed: bool,
}

fn disposition_monitor() -> Monitor<RoundCheck> {
    let mut p = Predicates::builder();
    let promoted = p.new_predicate(ClosurePredicate::new(
        |r: &RoundCheck| r.promoted,
        "promoted",
    ));
    let justified = p.new_predicate(ClosurePredicate::new(
        |r: &RoundCheck| {
            !r.witness_matches_before
                && r.witness_fixed
                && r.candidate_replayed
                && r.regression_after <= r.regression_before
                && r.candidate != r.before
                && r.after == r.candidate
        },
        "replayed candidate fixed the incumbent miss, passed the regression gate and was installed",
    ));
    let retained = p.new_predicate(ClosurePredicate::new(
        |r: &RoundCheck| r.after == r.before,
        "incumbent retained",
    ));
    let promoted = Property::atomic(promoted);
    let rule = Property::always(Property::Or(
        Box::new(promoted.clone() & Property::atomic(justified)),
        Box::new(!promoted & Property::atomic(retained)),
    ));
    let predicates = p.build();
    let automaton = rule.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

#[derive(Debug, Clone, Copy)]
struct Boundary {
    has_successor: bool,
    follows_predecessor: bool,
}

fn history_monitor() -> Monitor<Boundary> {
    let mut p = Predicates::builder();
    let continues = p.new_predicate(ClosurePredicate::new(
        |b: &Boundary| b.has_successor,
        "another trial follows",
    ));
    let linked = p.new_predicate(ClosurePredicate::new(
        |b: &Boundary| b.follows_predecessor,
        "next trial uses the previous outcome",
    ));
    let continues = Property::atomic(continues);
    // G(continues -> X linked). Disjoint guards keep telo's automaton deterministic.
    let rule = Property::always(Property::Or(
        Box::new(!continues.clone()),
        Box::new(continues & Property::next(Property::atomic(linked))),
    ));
    let predicates = p.build();
    let automaton = rule.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

/// Validate a nonempty batch. Its first ID may be any positive pass number.
/// The caller checks run completeness and anchors the first incumbent externally.
pub(super) fn verify(rounds: &[RoundCheck]) -> Result<(), String> {
    let first = rounds.first().ok_or("no recorded trials to verify")?;
    if first.pass == 0 {
        return Err("trial numbers start at one".into());
    }
    let mut dispositions = disposition_monitor();
    let mut history = history_monitor();
    let mut prior: Option<&RoundCheck> = None;

    for (index, trial) in rounds.iter().enumerate() {
        let follows_predecessor = prior.is_none_or(|previous| {
            let retained_errors = if previous.promoted {
                previous.regression_after
            } else {
                previous.regression_before
            };
            previous.pass.checked_add(1) == Some(trial.pass)
                && previous.after == trial.before
                && retained_errors == trial.regression_before
        });
        if !history.next_state(&Boundary {
            has_successor: index + 1 < rounds.len(),
            follows_predecessor,
        }) {
            return Err(format!("trial {} breaks the pass number, incumbent, or fixed regression baseline of its predecessor", trial.pass));
        }
        if !dispositions.next_state(trial) {
            return Err(format!(
                "trial {} invalid disposition: promoted={}, incumbent matched witness={}, candidate fixed witness={}, replayed={}, fixed-suite errors {} -> {}, installed candidate={}, retained incumbent={}",
                trial.pass, trial.promoted, trial.witness_matches_before, trial.witness_fixed, trial.candidate_replayed, trial.regression_before,
                trial.regression_after, trial.after == trial.candidate, trial.after == trial.before
            ));
        }
        prior = Some(trial);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promoted(pass: u32, before: &str, candidate: &str, from: u32, to: u32) -> RoundCheck {
        RoundCheck {
            pass,
            before: before.into(),
            candidate: candidate.into(),
            after: candidate.into(),
            promoted: true,
            witness_matches_before: false,
            witness_fixed: true,
            regression_before: from,
            regression_after: to,
            candidate_replayed: true,
        }
    }

    fn history() -> [RoundCheck; 3] {
        let mut held = promoted(12, "v2", "discard", 4, 7);
        held.promoted = false;
        held.after = "v2".into();
        [
            promoted(11, "v1", "v2", 5, 4),
            held,
            promoted(13, "v2", "v3", 4, 4),
        ]
    }

    #[test]
    fn equal_fixed_suite_errors_can_admit_a_witness_fix_and_a_rejection_keeps_its_prior_baseline() {
        assert!(verify(&history()).is_ok());
        let mut rejected = history()[1].clone();
        rejected.witness_fixed = false;
        rejected.candidate_replayed = false;
        assert!(verify(&[rejected]).is_ok());
    }

    #[test]
    fn each_missing_promotion_condition_fails_at_the_commit_pass() {
        let [first, held, last] = history();
        for mutate in [
            (|r: &mut RoundCheck| r.witness_matches_before = true) as fn(&mut RoundCheck),
            |r| r.witness_fixed = false,
            |r| r.candidate_replayed = false,
            |r| r.regression_after = r.regression_before + 1,
            |r| r.after = r.before.clone(),
            |r| {
                r.candidate = r.before.clone();
                r.after = r.before.clone();
            },
        ] {
            let mut broken = last.clone();
            mutate(&mut broken);
            let err = verify(&[first.clone(), held.clone(), broken]).unwrap_err();
            assert!(err.contains("trial 13 invalid disposition"), "{err}");
        }
    }

    #[test]
    fn leaking_rejected_candidates_and_broken_phase_history_are_detected() {
        let [first, held, last] = history();
        let mut leaked = held.clone();
        leaked.after = leaked.candidate.clone();
        assert!(verify(&[first.clone(), leaked])
            .unwrap_err()
            .contains("trial 12 invalid disposition"));
        for mutate in [
            (|r: &mut RoundCheck| r.before = "discard".into()) as fn(&mut RoundCheck),
            |r| r.regression_before = 7,
            |r| r.pass = 12,
            |r| r.pass = 14,
        ] {
            let mut broken = last.clone();
            mutate(&mut broken);
            assert!(verify(&[first.clone(), held.clone(), broken])
                .unwrap_err()
                .contains("breaks the pass number"));
        }
        let mut stale = held;
        stale.before = first.before.clone();
        stale.after = stale.before.clone();
        assert!(verify(&[first, stale]).is_err());
        assert!(verify(&[]).is_err());
    }
}
