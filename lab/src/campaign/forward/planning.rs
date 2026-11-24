//! Small longitudinal reference courses: two component skills and one synthesis.
//! Checkpoints stand for independently accepted synthetic performances. Neither the
//! session calendar nor selecting a lesson establishes a checkpoint by itself.
use super::{Observation, Rng};
use crate::curriculum::{
    Checkpoint, Decision, Document, Outcome, OutcomeId, PlanError, Progress, ReviewedUnit, Slot,
    Span,
};
use std::collections::{BTreeMap, BTreeSet};

const FIRST: OutcomeId = OutcomeId(1);
const SECOND: OutcomeId = OutcomeId(2);
const SYNTHESIS: OutcomeId = OutcomeId(3);
const SKILLS: [OutcomeId; 3] = [FIRST, SECOND, SYNTHESIS];

fn source(skill: OutcomeId) -> Document {
    [skill.0 as u8; 32]
}

// In one edition the two operations can be taught in parallel. In the other,
// the second builds on the first. Both culminate in performing them together.
fn release(edition: u64, sequence_components: bool) -> Result<ReviewedUnit, String> {
    let nodes = SKILLS
        .into_iter()
        .map(|id| {
            let requires = match id {
                SYNTHESIS => BTreeSet::from([FIRST, SECOND]),
                SECOND if sequence_components => BTreeSet::from([FIRST]),
                _ => BTreeSet::new(),
            };
            Outcome {
                id,
                requires,
                support: BTreeSet::from([Span {
                    document: source(id),
                    start: 0,
                    end: 8,
                }]),
            }
        })
        .collect();
    ReviewedUnit::build(edition, nodes).map_err(|e| format!("planning release: {e:?}"))
}

fn empty(edition: u64) -> Progress {
    Progress {
        revision: edition,
        checkpoints: BTreeMap::new(),
        postponed: BTreeSet::new(),
    }
}

fn prior(edition: u64, today: u32, rng: &mut Rng) -> Progress {
    let mut learner = empty(edition);
    for skill in SKILLS {
        if rng.below(3) != 0 {
            let observed = today - rng.below(30);
            learner.checkpoints.insert(
                skill,
                Checkpoint {
                    observed,
                    due: observed + 7 + rng.below(29),
                },
            );
        }
    }
    learner
}

struct Meeting {
    day: u32,
    elapsed: u32,
    classroom: bool,
    slot: Slot,
    goal: OutcomeId,
    edition: u64,
    sequence_components: bool,
}

// All predictors are known before asking the reference. In particular, no predictor
// is the selected objective, next action, success of a future attempt or a seat ID.
fn facts(
    m: &Meeting,
    learner: &Progress,
    unavailable: &BTreeSet<Document>,
) -> Vec<(&'static str, bool)> {
    let seen = |skill| learner.checkpoints.contains_key(&skill);
    let due = |skill| {
        learner
            .checkpoints
            .get(&skill)
            .is_some_and(|c| c.due <= m.day)
    };
    let blocked = |skill| unavailable.contains(&source(skill));
    let paused = |id| learner.postponed.contains(&id);
    vec![
        ("class shares the lesson and source access", m.classroom),
        ("reserved return-check slot", m.slot == Slot::Review),
        ("at least four weeks since last meeting", m.elapsed >= 28),
        (
            "progress belongs to a different edition",
            learner.revision != m.edition,
        ),
        ("second skill builds on the first", m.sequence_components),
        ("requested goal is the first component", m.goal == FIRST),
        (
            "requested goal integrates both components",
            m.goal == SYNTHESIS,
        ),
        ("first component has a checkpoint", seen(FIRST)),
        ("second component has a checkpoint", seen(SECOND)),
        ("integration has a checkpoint", seen(SYNTHESIS)),
        ("first component is due", due(FIRST)),
        ("second component is due", due(SECOND)),
        ("integration is due", due(SYNTHESIS)),
        ("first component material unavailable", blocked(FIRST)),
        ("second component material unavailable", blocked(SECOND)),
        ("integration material unavailable", blocked(SYNTHESIS)),
        ("first component is paused", paused(FIRST)),
        ("second component is paused", paused(SECOND)),
        ("integration is paused", paused(SYNTHESIS)),
        (
            "one component is due while the other is unobserved",
            (due(FIRST) && !seen(SECOND)) || (due(SECOND) && !seen(FIRST)),
        ),
        (
            "an observed integration has a component due",
            seen(SYNTHESIS) && (due(FIRST) || due(SECOND)),
        ),
    ]
}

pub(super) fn cases(seed: u64) -> Result<Vec<Observation>, String> {
    let mut rng = Rng::new(seed);
    let mut observations = Vec::with_capacity(192);
    // Each classroom contributes five separate learners; both solo timetables
    // and both classrooms experience sixteen meetings and one edition change.
    for (cohort, seats) in [1, 5, 1, 5].into_iter().enumerate() {
        let classroom = seats > 1;
        let mut day = 90 + rng.below(90);
        let mut edition = 1;
        let mut chain = rng.below(2) == 0;
        let mut unit = release(edition, chain)?;
        let mut learners: Vec<_> = (0..seats).map(|_| prior(edition, day, &mut rng)).collect();
        let mut unavailable = BTreeSet::new();
        for visit in 0..16 {
            let elapsed = if rng.below(8) == 0 {
                28 + rng.below(90)
            } else if classroom {
                1 + rng.below(4)
            } else {
                1 + rng.below(10)
            };
            day += elapsed;
            if visit == 8 {
                edition += 1;
                chain = !chain;
                unit = release(edition, chain)?;
            }
            // A temporary rights or availability change is common to all seats this meeting.
            if rng.below(3) == 0 {
                let doc = source(SKILLS[rng.below(3) as usize]);
                if !unavailable.remove(&doc) {
                    unavailable.insert(doc);
                }
            }
            let slot = if classroom {
                if visit % 3 == 0 {
                    Slot::Review
                } else {
                    Slot::Forward
                }
            } else if rng.below(2) == 0 {
                Slot::Review
            } else {
                Slot::Forward
            };
            let goal = match rng.below(5) {
                0 => FIRST,
                1 => SECOND,
                _ => SYNTHESIS,
            };
            let meeting = Meeting {
                day,
                elapsed,
                classroom,
                slot,
                goal,
                edition,
                sequence_components: chain,
            };
            let mut repair = None;
            for (seat, learner) in learners.iter_mut().enumerate() {
                if rng.below(4) == 0 {
                    let skill = SKILLS[rng.below(3) as usize];
                    if !learner.postponed.remove(&skill) {
                        learner.postponed.insert(skill);
                    }
                }
                let inputs = facts(&meeting, learner, &unavailable);
                let actual = unit.next(goal, learner, day, &unavailable, slot);
                let label = match actual {
                    Ok(Decision::Teach(id)) if id == goal => "teach requested objective",
                    Ok(Decision::Teach(_)) => "teach prerequisite",
                    Ok(Decision::Recheck(id)) if id == goal => "recheck requested objective",
                    Ok(Decision::Recheck(_)) => "recheck prerequisite",
                    Ok(Decision::CurrentUntil(_)) => "wait for scheduled return",
                    Ok(Decision::RepairSources(_)) => "repair source access",
                    Ok(Decision::Postponed(_)) => "respect activity hold",
                    Err(PlanError::WrongRevision) => "reconcile changed edition",
                    Err(e) => return Err(format!("unexpected planning reference failure: {e:?}")),
                };
                observations.push(Observation::new(
                    "planning",
                    format!(
                        "cohort {} meeting {} seat {} day {day}",
                        cohort + 1,
                        visit + 1,
                        seat + 1
                    ),
                    inputs,
                    label,
                ));
                // A successful subsequent independent observation can move the frontier.
                // Declines and unsuccessful attempts leave the dates untouched.
                match actual {
                    Ok(Decision::Teach(id) | Decision::Recheck(id)) if rng.below(4) != 0 => {
                        learner.checkpoints.insert(
                            id,
                            Checkpoint {
                                observed: day,
                                due: day + 7 + rng.below(29),
                            },
                        );
                    }
                    Ok(Decision::RepairSources(id)) if repair.is_none() => {
                        repair = Some(source(id))
                    }
                    Err(PlanError::WrongRevision) => *learner = empty(edition),
                    _ => {}
                }
            }
            // One reviewer can restore at most one document after everyone has been seen.
            if let Some(doc) = repair {
                if rng.below(2) == 0 {
                    unavailable.remove(&doc);
                }
            }
        }
    }
    Ok(observations)
}
