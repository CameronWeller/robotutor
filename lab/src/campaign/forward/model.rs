//! Ordered exceptions over the simulator's declared decision inputs. Prediction
//! reads only the domain and feature bits. Outcome labels are training targets.

use super::{digest, Sample, DOMAINS};
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    mask: u64,
    values: u64,
    outcome: String,
}

impl Rule {
    fn matches(&self, bits: u64) -> bool {
        bits & self.mask == self.values
    }

    fn json(&self, schema: &[&str]) -> Value {
        let predicates: Vec<_> = schema
            .iter()
            .take(64)
            .enumerate()
            .filter(|(i, _)| self.mask & (1u64 << i) != 0)
            .map(|(i, feature)| json!({"feature":feature, "value":self.values & (1u64 << i) != 0}))
            .collect();
        json!({"predicates":predicates, "outcome":self.outcome})
    }
}

#[derive(Clone)]
pub(super) struct Model {
    defaults: [String; DOMAINS.len()],
    rules: [Vec<Rule>; DOMAINS.len()],
}

#[derive(Clone, Copy)]
enum Change {
    Put(u64),
    Remove(usize),
}

struct Candidate {
    change: Change,
    gain: i64,
    /// Net change in rule headers plus literals in the stored model.
    cost: i32,
    mask: u64,
}

impl Model {
    pub(super) fn baseline(rows: &[Sample]) -> Self {
        let mut counts: [BTreeMap<&str, usize>; DOMAINS.len()] =
            std::array::from_fn(|_| BTreeMap::new());
        for row in rows {
            *counts[row.domain].entry(row.outcome).or_default() += 1;
        }
        let defaults = counts.map(|counts| {
            counts
                .into_iter()
                .min_by_key(|(label, n)| (Reverse(*n), *label))
                .map_or(String::new(), |(label, _)| label.to_owned())
        });
        Self {
            defaults,
            rules: std::array::from_fn(|_| Vec::new()),
        }
    }

    pub(super) fn predict(&self, row: &Sample) -> &str {
        self.rules[row.domain]
            .iter()
            .find(|rule| rule.matches(row.bits))
            .map_or(&self.defaults[row.domain], |rule| &rule.outcome)
    }

    pub(super) fn size(&self) -> usize {
        self.rules.iter().map(Vec::len).sum()
    }

    pub(super) fn to_json(&self, schemas: &[Vec<&'static str>]) -> Value {
        let domains: Vec<_> = DOMAINS
            .iter()
            .enumerate()
            .map(|(d, name)| {
                let rules: Vec<_> = self.rules[d].iter().map(|r| r.json(&schemas[d])).collect();
                json!({"domain":name, "default":self.defaults[d], "first_matching_rule":rules})
            })
            .collect();
        json!({"type":"ordered_boolean_exceptions/1", "domains": domains})
    }

    /// The order of input rows cannot break ties. All single and pair guards
    /// matching the witness are scored, then a bounded beam of pairs is extended.
    /// The fixed regression suite and holdout are not required to generate an edit.
    pub(super) fn propose(
        &self,
        w: &Sample,
        training: &[Sample],
        schemas: &[Vec<&'static str>],
        tried: &BTreeSet<String>,
    ) -> Option<(Self, Value)> {
        const PAIR_BEAM: usize = 12;
        let schema = schemas.get(w.domain)?;
        if schema.len() > 64 || self.predict(w) == w.outcome {
            return None;
        }
        let rules = &self.rules[w.domain];
        let domain_rows: Vec<_> = training
            .iter()
            .filter(|r| r.domain == w.domain)
            .map(|row| (row, self.predict(row) == row.outcome))
            .collect();

        // A new first guard changes only matching rows. For each distinct input
        // difference from the witness, count newly correct minus newly wrong.
        let mut weights = BTreeMap::<u64, i64>::new();
        for &(row, was_correct) in &domain_rows {
            let delta = i64::from(row.outcome == w.outcome) - i64::from(was_correct);
            if delta != 0 {
                *weights.entry(w.bits ^ row.bits).or_default() += delta;
            }
        }
        let weights: Vec<_> = weights
            .into_iter()
            .filter(|(_, delta)| *delta != 0)
            .collect();
        let score = |mask: u64| {
            weights
                .iter()
                .filter(|(difference, _)| difference & mask == 0)
                .map(|(_, weight)| *weight)
                .sum::<i64>()
        };

        let mut guards = BTreeMap::new();
        let mut pairs = Vec::new();
        for i in 0..schema.len() {
            let bit = 1u64 << i;
            guards.insert(bit, score(bit));
            for j in i + 1..schema.len() {
                let mask = bit | (1u64 << j);
                let gain = score(mask);
                guards.insert(mask, gain);
                pairs.push((Reverse(gain), mask));
            }
        }
        pairs.sort_unstable();
        for (_, pair) in pairs.into_iter().take(PAIR_BEAM) {
            for i in 0..schema.len() {
                let mask = pair | (1u64 << i);
                guards.entry(mask).or_insert_with(|| score(mask));
            }
        }
        let mut candidates: Vec<_> = guards
            .into_iter()
            .map(|(mask, gain)| {
                let exists = rules.iter().any(|r| r.mask == mask && r.matches(w.bits));
                let cost = if exists {
                    0
                } else {
                    1 + mask.count_ones() as i32
                };
                Candidate {
                    change: Change::Put(mask),
                    gain,
                    cost,
                    mask,
                }
            })
            .collect();

        // Only the first matching rule can change this witness if removed.
        if let Some(i) = rules.iter().position(|r| r.matches(w.bits)) {
            let without = |bits| {
                rules
                    .iter()
                    .enumerate()
                    .find(|(j, r)| *j != i && r.matches(bits))
                    .map_or(self.defaults[w.domain].as_str(), |(_, r)| {
                        r.outcome.as_str()
                    })
            };
            if without(w.bits) == w.outcome {
                let gain = domain_rows
                    .iter()
                    .map(|(row, was_correct)| {
                        i64::from(without(row.bits) == row.outcome) - i64::from(*was_correct)
                    })
                    .sum();
                candidates.push(Candidate {
                    change: Change::Remove(i),
                    gain,
                    cost: -(1 + rules[i].mask.count_ones() as i32),
                    mask: rules[i].mask,
                });
            }
        }

        candidates.sort_by_key(|c| (Reverse(c.gain), c.cost, c.mask.count_ones(), c.mask));
        for candidate in candidates {
            let (op, mut edit) = match candidate.change {
                Change::Put(mask) => (
                    "put_first",
                    Rule {
                        mask,
                        values: mask & w.bits,
                        outcome: w.outcome.into(),
                    }
                    .json(schema),
                ),
                Change::Remove(i) => ("remove", rules[i].json(schema)),
            };
            edit["op"] = json!(op);
            edit["domain"] = json!(DOMAINS[w.domain]);
            if !tried.is_empty() && tried.contains(&digest(&edit)) {
                continue;
            }
            let next = self.apply(&edit, schemas).ok()?;
            debug_assert_eq!(next.predict(w), w.outcome);
            return Some((next, edit));
        }
        None
    }

    /// Apply a journaled edit against the supplied feature schema. Removal names
    /// the complete old guard and outcome, so it cannot silently delete a different rule.
    pub(super) fn apply(
        &self,
        edit: &Value,
        schemas: &[Vec<&'static str>],
    ) -> Result<Self, String> {
        if edit.as_object().is_none_or(|fields| fields.len() != 4) {
            return Err("expected op, domain, predicates and outcome in model edit".into());
        }
        let domain = DOMAINS
            .iter()
            .position(|name| edit["domain"] == *name)
            .ok_or("unknown edit domain")?;
        let schema = schemas
            .get(domain)
            .filter(|s| s.len() <= 64)
            .ok_or("missing or oversized feature schema")?;
        let predicates = edit["predicates"]
            .as_array()
            .filter(|p| !p.is_empty())
            .ok_or("a rule needs predicates")?;
        let outcome = edit["outcome"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("missing rule outcome")?;
        let mut rule = Rule {
            mask: 0,
            values: 0,
            outcome: outcome.into(),
        };
        for predicate in predicates {
            if predicate.as_object().is_none_or(|fields| fields.len() != 2) {
                return Err("expected feature and boolean value".into());
            }
            let feature = predicate["feature"]
                .as_str()
                .ok_or("missing feature name")?;
            let mut positions = schema
                .iter()
                .enumerate()
                .filter(|(_, name)| **name == feature);
            let (i, _) = positions
                .next()
                .ok_or("unknown predicate for this domain")?;
            let bit = 1u64 << i;
            if positions.next().is_some() || rule.mask & bit != 0 {
                return Err("ambiguous or repeated predicate".into());
            }
            rule.mask |= bit;
            if predicate["value"]
                .as_bool()
                .ok_or("missing predicate polarity")?
            {
                rule.values |= bit;
            }
        }
        let mut updated = self.clone();
        let rules = &mut updated.rules[domain];
        match edit["op"].as_str() {
            Some("put_first") => {
                rules.retain(|old| old.mask != rule.mask || old.values != rule.values);
                rules.insert(0, rule);
            }
            Some("remove") => {
                let i = rules
                    .iter()
                    .position(|old| old == &rule)
                    .ok_or("no matching old rule to remove")?;
                rules.remove(i);
            }
            _ => return Err("unknown model edit operation".into()),
        }
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(domain: usize, bits: u64, outcome: &'static str) -> Sample {
        Sample {
            domain,
            bits,
            outcome,
            seed: 0,
            locator: "training seat".into(),
        }
    }

    fn schemas() -> Vec<Vec<&'static str>> {
        vec![vec!["can attend", "exercise open", "used hint"]; DOMAINS.len()]
    }

    #[test]
    fn a_negative_literal_and_two_positive_literals_generalize_beyond_the_witness_identity() {
        let mut training: Vec<_> = (0..8)
            .map(|bits| row(0, bits, if bits == 3 { "attempt" } else { "wait" }))
            .collect();
        training.push(row(1, 3, "separate domain"));
        let m = Model::baseline(&training);
        let w = Sample {
            seed: 91,
            locator: "new seat".into(),
            ..row(0, 3, "attempt")
        };
        let schema = schemas();
        let (candidate, edit) = m.propose(&w, &training, &schema, &BTreeSet::new()).unwrap();
        assert_eq!(
            edit["predicates"],
            json!([{"feature":"can attend","value":true},{"feature":"exercise open","value":true},{"feature":"used hint","value":false}])
        );
        assert_eq!(candidate.size(), 1);
        let replay = m.apply(&edit, &schema).unwrap();
        for r in &training {
            assert_eq!(replay.predict(r), r.outcome);
        }
        assert_eq!(
            replay.predict(&row(0, 3, "different validation label")),
            "attempt"
        );
        assert_eq!(candidate.to_json(&schema), replay.to_json(&schema));
        training.reverse();
        assert_eq!(
            m.propose(&w, &training, &schema, &BTreeSet::new())
                .unwrap()
                .1,
            edit
        );
    }

    #[test]
    fn removal_wins_an_equal_gain_and_tried_edits_are_excluded_for_this_incumbent() {
        let schema = schemas();
        let training = vec![row(0, 1, "wait"), row(0, 0, "wait")];
        let base = Model::baseline(&training);
        let bad = json!({"domain":DOMAINS[0], "op":"put_first", "predicates":[{"feature":"can attend","value":true}], "outcome":"deliver"});
        let incumbent = base.apply(&bad, &schema).unwrap();
        let (repaired, edit) = incumbent
            .propose(&training[0], &training, &schema, &BTreeSet::new())
            .unwrap();
        assert_eq!(edit["op"], "remove");
        assert_eq!(repaired.size(), 0);
        assert_eq!(repaired.to_json(&schema), base.to_json(&schema));
        let tried = BTreeSet::from([digest(&edit)]);
        let (alternative, next) = incumbent
            .propose(&training[0], &training, &schema, &tried)
            .unwrap();
        assert_ne!(edit, next);
        assert_eq!(alternative.predict(&training[0]), "wait");
        assert!(base.apply(&edit, &schema).is_err());
        let mut typo = bad;
        typo["predicates"][0]["feature"] = json!("seat identity");
        assert!(base.apply(&typo, &schema).is_err());
    }

    #[test]
    fn a_negative_gain_candidate_can_be_tested_and_all_tried_moves_exhaust_the_search() {
        let schema = vec![vec!["can attend"]; DOMAINS.len()];
        let training = vec![row(0, 1, "deliver"), row(1, 0, "z"), row(1, 0, "a")];
        let baseline = Model::baseline(&training);
        assert_eq!(baseline.predict(&row(1, 0, "irrelevant")), "a");
        let w = row(0, 1, "wait");
        let (candidate, edit) = baseline
            .propose(&w, &training, &schema, &BTreeSet::new())
            .unwrap();
        assert_eq!(candidate.predict(&w), "wait");
        assert_ne!(candidate.predict(&training[0]), training[0].outcome);
        assert!(baseline
            .propose(&w, &training, &schema, &BTreeSet::from([digest(&edit)]))
            .is_none());
        assert!(candidate
            .propose(&w, &training, &schema, &BTreeSet::new())
            .is_none());
    }
}
