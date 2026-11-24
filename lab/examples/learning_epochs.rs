//! Paired cohort CLI. The same learner engine also supports a single in-flight history.
use serde_json::json;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
use tutor_lab::learning_epochs::*;
use tutor_lab::{
    material::{FractionCard, FractionTask},
    protocol::Judgment,
};

#[derive(Clone)]
struct Design {
    epochs: u32,
    terms: u32,
    learners: u32,
    seeds: u32,
    seed: u64,
    learns: bool,
    trace: Option<u32>,
}

impl Default for Design {
    fn default() -> Self {
        Self {
            epochs: 3,
            terms: 8,
            learners: 96,
            seeds: 3,
            seed: DEFAULT_SEED,
            learns: true,
            trace: None,
        }
    }
}

impl Design {
    fn parse() -> Result<Self, String> {
        let mut d = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            if flag == "--no-learning" {
                d.learns = false;
                continue;
            }
            let raw = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            if flag == "--seed" {
                d.seed = raw.parse().map_err(|_| "invalid seed")?;
                continue;
            }
            let n = raw
                .parse()
                .map_err(|_| format!("invalid value for {flag}"))?;
            match flag.as_str() {
                "--epochs" => d.epochs = n,
                "--terms-per-epoch" => d.terms = n,
                "--learners" => d.learners = n,
                "--seeds" => d.seeds = n,
                "--trace" => d.trace = Some(n),
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        if !(1..=24).contains(&d.epochs)
            || !(1..=16).contains(&d.terms)
            || !(1..=1024).contains(&d.learners)
            || !(1..=32).contains(&d.seeds)
            || d.trace.is_some_and(|id| id >= d.learners)
            || d.seed.checked_add(u64::from(d.seeds - 1)).is_none()
        {
            return Err("expected 1–24 epochs, 1–16 terms/epoch, 1–1024 learners, 1–32 seeds, a supported seed range and a trace learner inside the cohort".into());
        }
        Ok(d)
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Epoch {
    arms: [Counts; 3],
    reentry: [Counts; 3],
    final_independent: [u32; 3],
    final_correct: [u32; 3],
    wins_21: u32,
    wins_30: u32,
    matched: u32,
    absent_for_term: u32,
}
impl Epoch {
    fn pair(&mut self, first: Option<bool>, second: Option<bool>) {
        match (first, second) {
            (Some(a), Some(b)) => {
                self.matched += 1;
                self.wins_21 += u32::from(a && !b);
                self.wins_30 += u32::from(b && !a);
            }
            (None, None) => {}
            _ => unreachable!("paired attendance and help choices agree"),
        }
    }
}

fn run_seed(d: &Design, seed: u64, deadline: Instant) -> Result<Vec<Epoch>, String> {
    let revoked = BTreeSet::new();
    let rule = practice_source(&revoked);
    let entry = FractionCard::propose(FractionTask::MissingPart, 3, 2, 16).unwrap();
    assert_eq!(entry.assess("2/16", &rule), Some(Judgment::Correct));
    let mut epochs: Vec<_> = (0..d.epochs).map(|_| Epoch::default()).collect();
    for id in 0..d.learners {
        let participant = Participant { seed, id };
        let mut learners = ARMS.map(|_| Learner::new(&participant, d.learns));
        for (index, epoch) in epochs.iter_mut().enumerate() {
            if Instant::now() >= deadline {
                return Err("ten-minute local simulation deadline reached".into());
            }
            for number in 0..d.terms {
                let term = index as u32 * d.terms + number;
                epoch.absent_for_term += u32::from(participant.away(term));
                let mut observed = [None; 3];
                for (i, arm) in ARMS.into_iter().enumerate() {
                    let session = TermSession {
                        participant: &participant,
                        term,
                        arm,
                        rule: &rule,
                    };
                    let mut events = if d.trace == Some(id) {
                        Events::Print
                    } else {
                        Events::Silent
                    };
                    observed[i] = learners[i].advance_term(
                        &session,
                        &mut epoch.arms[i],
                        &mut epoch.reentry[i],
                        &mut events,
                    );
                    if number + 1 == d.terms {
                        epoch.final_independent[i] += u32::from(observed[i].is_some());
                        epoch.final_correct[i] += u32::from(observed[i] == Some(true));
                    }
                }
                epoch.pair(observed[0], observed[1]);
            }
        }
    }
    Ok(epochs)
}

fn main() -> Result<(), String> {
    let d = Design::parse()?;
    let deadline = Instant::now() + Duration::from_secs(600);
    for sample in 0..d.seeds {
        let seed = d.seed + u64::from(sample);
        for (i, e) in run_seed(&d, seed, deadline)?.into_iter().enumerate() {
            let arms: serde_json::Map<_, _> = ARMS
                .into_iter()
                .zip(&e.arms)
                .map(|(p, c)| (p.label().to_owned(), c.report()))
                .collect();
            println!(
                "{}",
                json!({"kind":"epoch","seed":seed,"learners":d.learners,"learning_effect":d.learns,
                "epoch":i+1,"through_day":(i as u32+1)*d.terms*TERM,"unavailable_learner_terms":e.absent_for_term,
                "policy_order":ARMS.map(Arm::label),
                "final_day_independent":e.final_independent,"final_day_correct":e.final_correct,
                "reentry":e.reentry.map(|c|c.report()),
                "paired_independent":e.matched,"correct_only_clustered":e.wins_21,"correct_only_regular":e.wins_30,"arms":arms})
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(options: Design) -> Vec<Epoch> {
        run_seed(&options, 91, Instant::now() + Duration::from_secs(30)).unwrap()
    }
    #[test]
    fn grouping_epochs_does_not_reset_the_learners_or_change_their_prefix() {
        let d = Design {
            learners: 12,
            epochs: 3,
            terms: 2,
            ..Design::default()
        };
        let epochs = run(d.clone());
        let fine = run(Design {
            epochs: 6,
            terms: 1,
            ..d.clone()
        });
        for (i, combined) in epochs.iter().enumerate() {
            for arm in 0..3 {
                assert_eq!(
                    combined.arms[arm].independent_correct,
                    fine[2 * i].arms[arm].independent_correct
                        + fine[2 * i + 1].arms[arm].independent_correct
                );
            }
        }
        assert_eq!(&epochs[..1], run(Design { epochs: 1, ..d }));
    }

    #[test]
    fn null_and_attendance_controls_hold_with_errors_help_and_a_long_absence() {
        let d = Design {
            learners: 18,
            epochs: 3,
            terms: 8,
            learns: false,
            ..Design::default()
        };
        for (i, e) in run(d).into_iter().enumerate() {
            assert_eq!(e.wins_21 + e.wins_30, 0);
            assert_eq!(e.arms[0].independent_correct, e.arms[2].independent_correct);
            assert_eq!(e.arms[0].practice_attended, e.arms[1].practice_attended);
            for c in e.arms {
                assert_eq!(
                    c.independent_probes + c.assisted_probes + c.missed_probes,
                    18 * 8
                );
                assert!(
                    c.assisted_probes > 0
                        && c.missed_probes > 0
                        && c.independent_correct < c.independent_probes
                );
                assert!(c.supported <= c.independent_correct);
            }
            assert_eq!(e.absent_for_term, if i == 1 { 12 } else { 0 });
            for count in e.reentry {
                assert_eq!(
                    count.independent_probes + count.assisted_probes + count.missed_probes,
                    if i == 1 { 6 } else { 0 }
                );
                assert_eq!(count.supported, 0);
                assert_eq!(count.independent_correct, count.unknown);
            }
        }
    }
}
