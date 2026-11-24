//! Compare abstaining curriculum forecasts on trajectories of a saved forward model.
//! Every proposed action is scored, including actions the incumbent already gets wrong.
#[path = "planning_investigation/intervention.rs"]
mod intervention;
#[path = "planning_investigation/stress.rs"]
mod stress;

use serde_json::{json, Value};
use std::{collections::BTreeSet, env, fs};
use tutor_lab::campaign::forward::Experiment;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Skill {
    seen: bool,
    due: bool,
    paused: bool,
    unavailable: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    same_edition: bool,
    review: bool,
    goal: usize,
    // A bit set at index j means that skill j is a strict ancestor of i.
    ancestors: [u8; 3],
    skill: [Skill; 3],
}

impl Snapshot {
    fn read(row: &Value) -> Result<Self, String> {
        let facts = row["true_facts"]
            .as_array()
            .ok_or("missing planning facts")?
            .iter()
            .map(|v| v.as_str().ok_or("a fact must have a name"))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let skill = ["first component", "second component", "integration"].map(|name| {
            let has = |suffix: &str| facts.contains(format!("{name} {suffix}").as_str());
            Skill {
                seen: has("has a checkpoint"),
                due: has("is due"),
                paused: has("is paused"),
                unavailable: has("material unavailable"),
            }
        });
        let goal = if facts.contains("requested goal integrates both components") {
            2
        } else if facts.contains("requested goal is the first component") {
            0
        } else {
            1
        };
        Ok(Self {
            same_edition: !facts.contains("progress belongs to a different edition"),
            review: facts.contains("reserved return-check slot"),
            goal,
            ancestors: [
                0,
                u8::from(facts.contains("second skill builds on the first")),
                3,
            ],
            skill,
        })
    }

    fn is_ancestor(&self, candidate: usize, child: usize) -> bool {
        self.ancestors[child] & (1 << candidate) != 0
    }

    fn supported_parents(&self, i: usize) -> bool {
        self.skill.iter().enumerate().all(|(p, parent)| {
            !self.is_ancestor(p, i) || (parent.seen && !parent.due && !parent.unavailable)
        })
    }

    fn four_conditions(&self) -> bool {
        let first = self.skill[0];
        self.goal == 2 && !first.seen && !first.unavailable && !first.paused
    }

    // An unobserved ancestor gives evidence for teaching. An eligible overdue
    // sibling can still take the slot when it was reserved for a return check.
    fn slot_aware(&self) -> Option<&'static str> {
        if !self.same_edition || !self.teachable_prerequisite() {
            return None;
        }
        let review_available = self.skill.iter().enumerate().any(|(i, node)| {
            self.is_ancestor(i, self.goal)
                && self.supported_parents(i)
                && node.due
                && !node.paused
                && !node.unavailable
        });
        Some(if self.review && review_available {
            "recheck prerequisite"
        } else {
            "teach prerequisite"
        })
    }

    fn forecast(&self) -> [Option<&'static str>; 3] {
        [
            self.four_conditions().then_some("teach prerequisite"),
            self.teachable_prerequisite()
                .then_some("teach prerequisite"),
            self.slot_aware(),
        ]
    }
    fn teachable_prerequisite(&self) -> bool {
        self.skill.iter().enumerate().any(|(i, node)| {
            self.is_ancestor(i, self.goal)
                && self.supported_parents(i)
                && !node.seen
                && !node.paused
                && !node.unavailable
        })
    }
}

#[derive(Default)]
struct Score {
    observations: u32,
    fires: u32,
    wrong: u32,
    corrected: u32,
    broken: u32,
    first_wrong: Option<Value>,
}

impl Score {
    fn apply(&mut self, row: &Value, claim: Option<&str>) {
        self.observations += 1;
        let Some(claim) = claim else {
            return;
        };
        self.fires += 1;
        let before = row["predicted"] == row["observed"];
        let after = row["observed"] == claim;
        self.wrong += u32::from(!after);
        self.corrected += u32::from(!before && after);
        self.broken += u32::from(before && !after);
        if !after && self.first_wrong.is_none() {
            self.first_wrong = Some(row.clone());
        }
    }

    fn json(self) -> Value {
        json!({"observations": self.observations, "proposals": self.fires, "wrong_proposals": self.wrong, "fixed":self.corrected, "newly_broken":self.broken, "first_wrong":self.first_wrong})
    }
}

fn run() -> Result<Value, String> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: planning_investigation JOURNAL SEED...")?;
    let contents = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let records = contents
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let (header, rows) = records.split_first().ok_or("empty forward journal")?;
    let engine = Experiment::new()?;
    let session = engine.restore(header, rows)?;
    let seeds = args
        .map(|s| {
            s.parse::<u64>()
                .map_err(|_| format!("invalid workload seed {s}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if seeds.is_empty() {
        return Err("at least one workload seed is required".into());
    }
    let mut scores = vec![];
    for seed in seeds {
        let t = engine.simulate(&session, "planning", seed)?;
        let mut models: [Score; 3] = std::array::from_fn(|_| Score::default());
        for row in t["observations"]
            .as_array()
            .ok_or("empty planning trajectory")?
        {
            let state = Snapshot::read(row)?;
            for (counter, claim) in models.iter_mut().zip(state.forecast()) {
                counter.apply(row, claim);
            }
        }
        let [wider, scoped, slot] = models;
        scores.push(json!({"seed": seed, "incumbent": t["comparison"]["errors_candidate"], "four_conditions":wider.json(),"scoped": scoped.json(), "slot_aware":slot.json()}));
    }
    Ok(
        json!({"passes":rows.len(), "cohorts":scores, "adversarial":stress::evaluate()?, "paired_destinations":intervention::compare()?}),
    )
}

fn main() -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&run()?).map_err(|e| e.to_string())?
    );
    Ok(())
}
