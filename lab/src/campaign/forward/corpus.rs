//! Run the teaching kernels before encoding observations. Only pre-decision facts
//! enter the predictor. Labels and trajectory locators stay with the evaluator.
use super::{planning, retention, serving, teaching};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) const DOMAINS: [&str; 4] = ["retention", "planning", "serving", "teaching"];

pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(super) fn below(&mut self, n: u32) -> u32 {
        // SplitMix64; use the high half of a multiply for a bounded draw.
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((u128::from(x ^ (x >> 31)) * u128::from(n)) >> 64) as u32
    }
}

pub(super) struct Observation {
    domain: &'static str,
    locator: String,
    facts: Vec<(&'static str, bool)>,
    outcome: &'static str,
}

impl Observation {
    pub(super) fn new(
        domain: &'static str,
        locator: String,
        facts: Vec<(&'static str, bool)>,
        outcome: &'static str,
    ) -> Self {
        Self {
            domain,
            locator,
            facts,
            outcome,
        }
    }
}

#[derive(Clone)]
pub(super) struct Sample {
    pub(super) domain: usize,
    pub(super) bits: u64,
    pub(super) outcome: &'static str,
    pub(super) seed: u64,
    pub(super) locator: String,
}

impl Sample {
    pub(super) fn witness(&self, predicted: &str, schema: &[&str]) -> Value {
        let present: Vec<_> = schema
            .iter()
            .enumerate()
            .filter(|(i, _)| self.bits & (1 << i) != 0)
            .map(|(_, name)| *name)
            .collect();
        json!({"domain":DOMAINS[self.domain],"seed":self.seed,"step":self.locator,
            "true_facts":present,"predicted":predicted,"observed":self.outcome})
    }
}

fn observations(domain: usize, seed: u64) -> Result<Vec<Observation>, String> {
    match domain {
        0 => retention::cases(seed),
        1 => planning::cases(seed),
        2 => serving::cases(seed),
        3 => teaching::cases(seed),
        _ => Err("unknown reference workload".into()),
    }
}

pub(super) fn schemas() -> Result<Vec<Vec<&'static str>>, String> {
    (0..DOMAINS.len())
        .map(|d| {
            let sample = observations(d, 11)?
                .into_iter()
                .next()
                .ok_or("empty workload")?;
            let names: BTreeSet<_> = sample.facts.iter().map(|(k, _)| *k).collect();
            if names.len() != sample.facts.len() || names.is_empty() || names.len() > 64 {
                return Err(format!("invalid input predicates for {}", DOMAINS[d]));
            }
            Ok(names.into_iter().collect())
        })
        .collect()
}

pub(super) fn draw(domain: usize, seed: u64, schema: &[&str]) -> Result<Vec<Sample>, String> {
    let mut ids = BTreeSet::new();
    observations(domain, seed)?
        .into_iter()
        .map(|o| {
            if o.domain != DOMAINS[domain]
                || o.facts.len() != schema.len()
                || !ids.insert(o.locator.clone())
            {
                return Err(format!("inconsistent workload observation: {}", o.locator));
            }
            let mut bits = 0;
            let mut seen = 0;
            for (name, value) in o.facts {
                let pos = schema
                    .binary_search(&name)
                    .map_err(|_| format!("unknown input predicate {name}"))?;
                let mask = 1u64 << pos;
                if seen & mask != 0 {
                    return Err(format!("repeated predicate {name}"));
                }
                seen |= mask;
                if value {
                    bits |= mask;
                }
            }
            Ok(Sample {
                domain,
                seed,
                locator: o.locator,
                bits,
                outcome: o.outcome,
            })
        })
        .collect()
}
