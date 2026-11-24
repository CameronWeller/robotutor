//! Exact checked planner as the observer for a small finite model. The forecast
//! sees only goal, graph, current/due state, holds, access, revision and slot.
use super::{Skill, Snapshot};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::curriculum::{
    Checkpoint, Decision, Document, Outcome, OutcomeId, PlanError, Progress, ReviewedUnit, Slot,
    Span,
};

const DAY: u32 = 30;
const IDS: [OutcomeId; 3] = [OutcomeId(1), OutcomeId(2), OutcomeId(3)];

fn doc(id: OutcomeId) -> Document {
    [id.0 as u8; 32]
}

pub(super) fn release(chain: bool) -> Result<ReviewedUnit, String> {
    let mut skills = Vec::new();
    for (i, id) in IDS.into_iter().enumerate() {
        let requires = match i {
            1 if chain => BTreeSet::from([IDS[0]]),
            2 => BTreeSet::from([IDS[0], IDS[1]]),
            _ => BTreeSet::new(),
        };
        skills.push(Outcome {
            id,
            requires,
            support: BTreeSet::from([Span {
                document: doc(id),
                start: 0,
                end: 1,
            }]),
        });
    }
    ReviewedUnit::build(1, skills).map_err(|e| format!("invalid checked unit: {e:?}"))
}

fn snapshot(
    goal: usize,
    chain: bool,
    p: &Progress,
    revoked: &BTreeSet<Document>,
    slot: Slot,
) -> Snapshot {
    Snapshot {
        goal,
        ancestors: [0, u8::from(chain), 3],
        same_edition: p.revision == 1,
        review: slot == Slot::Review,
        skill: IDS.map(|id| Skill {
            seen: p.checkpoints.contains_key(&id),
            due: p.checkpoints.get(&id).is_some_and(|c| c.due <= DAY),
            paused: p.postponed.contains(&id),
            unavailable: revoked.contains(&doc(id)),
        }),
    }
}

fn label(result: Result<Decision, PlanError>, goal: OutcomeId) -> Result<&'static str, String> {
    Ok(match result {
        Ok(Decision::Teach(id)) if id == goal => "teach requested objective",
        Ok(Decision::Teach(_)) => "teach prerequisite",
        Ok(Decision::Recheck(id)) if id == goal => "recheck requested objective",
        Ok(Decision::Recheck(_)) => "recheck prerequisite",
        Ok(Decision::Postponed(_)) => "respect activity hold",
        Ok(Decision::RepairSources(_)) => "repair source access",
        Ok(Decision::CurrentUntil(_)) => "wait for scheduled return",
        Err(PlanError::WrongRevision) => "reconcile changed edition",
        Err(e) => return Err(format!("invalid planning snapshot: {e:?}")),
    })
}

#[derive(Default)]
struct Census {
    proposals: u32,
    incorrect: BTreeMap<&'static str, u32>,
}

fn early_deadline_pair(unit: &ReviewedUnit) -> Result<Value, String> {
    let due_pair = |first, second| Progress {
        revision: 1,
        postponed: BTreeSet::new(),
        checkpoints: BTreeMap::from([
            (
                IDS[0],
                Checkpoint {
                    observed: 1,
                    due: first,
                },
            ),
            (
                IDS[1],
                Checkpoint {
                    observed: 1,
                    due: second,
                },
            ),
        ]),
    };
    let a = due_pair(10, 20);
    let b = due_pair(20, 10);
    let revoked = BTreeSet::new();
    let target = |p: &Progress| match unit.next(IDS[2], p, DAY, &revoked, Slot::Review) {
        Ok(Decision::Recheck(id)) => Ok(id.0),
        other => Err(format!("expected a recheck, got {other:?}")),
    };
    Ok(
        json!({"day":DAY,"goal":IDS[2].0,"due_dates":[[10,20],[20,10]],
        "projected_snapshots_identical":snapshot(2,false,&a,&revoked,Slot::Review)==snapshot(2,false,&b,&revoked,Slot::Review),
        "exact_reference_targets":[target(&a)?,target(&b)?]}),
    )
}

pub(super) fn evaluate() -> Result<Value, String> {
    let mut cases = 0;
    let mut census: [Census; 3] = std::array::from_fn(|_| Census::default());
    for chain in [false, true] {
        let unit = release(chain)?;
        // Each skill may be unseen, independently current, or due. Holds
        // and source withdrawals are orthogonal (12 possibilities per skill).
        for encoded in 0_u32..12_u32.pow(3) {
            let mut digits = encoded;
            let mut p = Progress {
                revision: 1,
                checkpoints: BTreeMap::new(),
                postponed: BTreeSet::new(),
            };
            let mut revoked = BTreeSet::new();
            for id in IDS {
                let digit = digits % 12;
                digits /= 12;
                match digit % 3 {
                    1 => {
                        p.checkpoints.insert(
                            id,
                            Checkpoint {
                                observed: 1,
                                due: 40,
                            },
                        );
                    }
                    2 => {
                        p.checkpoints.insert(
                            id,
                            Checkpoint {
                                observed: 1,
                                due: 20,
                            },
                        );
                    }
                    _ => (),
                }
                if (digit / 3) & 1 != 0 {
                    p.postponed.insert(id);
                }
                if (digit / 3) & 2 != 0 {
                    revoked.insert(doc(id));
                }
            }
            for revision in [1, 2] {
                p.revision = revision;
                for (i, goal) in IDS.into_iter().enumerate() {
                    for slot in [Slot::Forward, Slot::Review] {
                        let reference = label(unit.next(goal, &p, DAY, &revoked, slot), goal)?;
                        let view = snapshot(i, chain, &p, &revoked, slot);
                        for (stat, claim) in census.iter_mut().zip(view.forecast()) {
                            if let Some(claim) = claim {
                                stat.proposals += 1;
                                if claim != reference {
                                    *stat.incorrect.entry(reference).or_default() += 1;
                                }
                            }
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    let measurements = census.map(|c| json!({"proposals":c.proposals,"incorrect":c.incorrect}));
    Ok(
        json!({"finite_snapshots":cases,"four_conditions":measurements[0],"scoped":measurements[1],"slot_aware":measurements[2],"order_collision":early_deadline_pair(&release(false)?)?}),
    )
}
