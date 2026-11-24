//! Counterexample-guided identification of an executable teaching-service simulator.
//! One step proposes a structural edit to the last retained model, runs the checked
//! reference workloads, gates the change, and hands an actual remaining miss forward.
mod audit;
mod corpus;
mod model;
mod planning;
mod retention;
mod serving;
mod teaching;

use audit::RoundCheck;
use corpus::{draw, Observation, Rng, Sample, DOMAINS};
use model::Model;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const FIT: [u64; 3] = [11, 59, 107];
const REGRESSION: [u64; 2] = [2309, 6263];
const RESERVED: [u64; 3] = [35983, 49109, 63467];
const PROBE: u64 = 1 << 40;
const TRIAL: u64 = 2 << 40;
const FORMAT: &str = "tutor-reference-model/1";

fn digest(value: &Value) -> String {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, value).expect("JSON value");
    format!("{:x}", hash.finalize())
}

fn domain(name: &Value) -> Result<usize, String> {
    DOMAINS
        .iter()
        .position(|d| Some(*d) == name.as_str())
        .ok_or_else(|| "unknown domain in journal".into())
}

fn cohort(schemas: &[Vec<&str>], seeds: &[u64]) -> Result<Vec<Sample>, String> {
    let mut rows = Vec::new();
    for seed in seeds {
        for (d, schema) in schemas.iter().enumerate() {
            rows.extend(draw(d, *seed, schema)?);
        }
    }
    Ok(rows)
}

#[derive(Default, Clone, Copy)]
struct Change {
    count: u32,
    before: u32,
    fixed: u32,
    broken: u32,
}

impl Change {
    fn after(self) -> u32 {
        self.before - self.fixed + self.broken
    }
    fn observe(&mut self, correct_before: bool, correct_after: bool) {
        self.count += 1;
        self.before += u32::from(!correct_before);
        self.fixed += u32::from(!correct_before && correct_after);
        self.broken += u32::from(correct_before && !correct_after);
    }
    fn json(self) -> Value {
        json!({"observations":self.count,"errors_before":self.before,"errors_candidate":self.after(),"fixed":self.fixed,"new_errors":self.broken})
    }
}

fn compare(before: &Model, candidate: &Model, samples: &[Sample]) -> Change {
    let mut score = Change::default();
    for row in samples {
        score.observe(
            before.predict(row) == row.outcome,
            candidate.predict(row) == row.outcome,
        );
    }
    score
}

fn classroom_mask(domain: &str, schema: &[&str]) -> Result<Option<u64>, String> {
    let feature = match domain {
        "planning" => "class shares the lesson and source access",
        "serving" => "shared_classroom",
        "teaching" => "classroom seat",
        // A retention case describes one history without assigning its setting.
        "retention" => return Ok(None),
        _ => return Err(format!("unmapped reporting domain {domain}")),
    };
    schema
        .binary_search(&feature)
        .map(|i| Some(1u64 << i))
        .map_err(|_| format!("missing setting feature in {domain}"))
}

fn setting_counts(groups: &BTreeMap<&str, Change>) -> Value {
    Value::Object(
        groups
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.json()))
            .collect(),
    )
}

// Seed the simulator with inspectable working assumptions from the existing lab.
// These are a practical starting approximation to four kernels, not a trained oracle.
fn starting_model(fit: &[Sample], schema: &[Vec<&'static str>]) -> Result<Model, String> {
    let mut model = Model::baseline(fit);
    for (domain, outcome, predicates) in [
        (
            "retention",
            "refresh indicated",
            vec![
                ("query concerns modeled refresh", true),
                ("return at least 21 days later", true),
            ],
        ),
        (
            "retention",
            "booked return due",
            vec![
                ("query concerns a booked return", true),
                ("return at least 30 days later", true),
            ],
        ),
        (
            "planning",
            "teach prerequisite",
            vec![
                ("requested goal integrates both components", true),
                ("first component has a checkpoint", false),
            ],
        ),
        (
            "planning",
            "reconcile changed edition",
            vec![("progress belongs to a different edition", true)],
        ),
        (
            "serving",
            "serve_shared_form",
            vec![("shared_card_released", true), ("needs_fresh_task", false)],
        ),
        (
            "serving",
            "hold_for_authority",
            vec![("source_rights_active", false)],
        ),
        (
            "teaching",
            "offer an explanation",
            vec![("checker found numeric discrepancy", true)],
        ),
        (
            "teaching",
            "defer this move",
            vec![("learner paused", true)],
        ),
    ] {
        let guards: Vec<_> = predicates
            .into_iter()
            .map(|(f, v)| json!({"feature":f,"value":v}))
            .collect();
        model = model.apply(
            &json!({"domain":domain,"op":"put_first","predicates":guards,"outcome":outcome}),
            schema,
        )?;
    }
    Ok(model)
}

// Two particularly costly forecast errors: grade a return durable without the
// corresponding finding, or predict material delivery when the broker holds/refers it.
fn false_clear(row: &Sample, predicted: &str) -> bool {
    match DOMAINS[row.domain] {
        "retention" => predicted == "supports durability" && row.outcome != "supports durability",
        "serving" => {
            predicted.starts_with("serve_")
                && (row.outcome.starts_with("hold_") || row.outcome == "refer_to_teacher")
        }
        _ => false,
    }
}

pub struct Experiment {
    schema: Vec<Vec<&'static str>>,
    fit: Vec<Sample>,
    regression: Vec<Sample>,
    baseline: Model,
    header: Value,
}

pub struct Session {
    model: Model,
    model_id: String,
    last_change: u32,
    head: String,
    passes: u32,
    next: Value,
    tried: BTreeSet<String>,
    protected: Vec<Sample>,
    last_investigated: [u32; 4],
    history: Vec<RoundCheck>,
}

impl Experiment {
    pub fn new() -> Result<Self, String> {
        let schema = corpus::schemas()?;
        let fit = cohort(&schema, &FIT)?;
        let regression = cohort(&schema, &REGRESSION)?;
        let baseline = starting_model(&fit, &schema)?;
        // Pin ordered measurements and source semantics. A changed oracle needs its own
        // experiment; independent full replay also covers fresh probes and model selection.
        let signatures = |rows: &[Sample]| {
            let v: Vec<_> = rows
                .iter()
                .map(|r| json!([r.domain, r.seed, r.locator, r.bits, r.outcome]))
                .collect();
            digest(&Value::Array(v))
        };
        let header = json!({"format":FORMAT,"domains":DOMAINS,"input_facts":schema,"fit_seeds":FIT,
            "regression_seeds":REGRESSION,"reserved_seeds":RESERVED,
            "fit_sha256":signatures(&fit),"regression_sha256":signatures(&regression),
            "baseline":baseline.to_json(&schema)});
        Ok(Self {
            schema,
            fit,
            regression,
            baseline,
            header,
        })
    }

    pub fn header(&self) -> Value {
        self.header.clone()
    }

    pub fn start(&self) -> Session {
        let model = self.baseline.clone();
        let next = self
            .fit
            .iter()
            .find(|r| model.predict(r) != r.outcome)
            .map(|r| r.witness(model.predict(r), &self.schema[r.domain]))
            .unwrap_or(Value::Null);
        Session {
            model_id: digest(&model.to_json(&self.schema)),
            model,
            last_change: 0,
            head: digest(&self.header),
            passes: 0,
            next,
            tried: BTreeSet::new(),
            protected: Vec::new(),
            last_investigated: [0; 4],
            history: Vec::new(),
        }
    }

    fn witness(&self, record: &Value) -> Result<Sample, String> {
        let d = domain(&record["domain"])?;
        let seed = record["seed"].as_u64().ok_or("missing witness seed")?;
        let locator = record["step"].as_str().ok_or("missing witness locator")?;
        draw(d, seed, &self.schema[d])?
            .into_iter()
            .find(|r| r.locator == locator)
            .ok_or_else(|| "witness no longer exists in the reference trajectory".into())
    }

    fn next_probe(
        &self,
        session: &Session,
        model: &Model,
        pass: u32,
        just_tested: usize,
    ) -> Result<Value, String> {
        let mut opportunities = Vec::new();
        for (d, last_investigated) in session.last_investigated.iter().enumerate() {
            let errors = self
                .fit
                .iter()
                .filter(|r| r.domain == d && model.predict(r) != r.outcome)
                .count();
            let total = self.fit.iter().filter(|r| r.domain == d).count();
            let last = if d == just_tested {
                pass
            } else {
                *last_investigated
            };
            // Return to high-error domains, with age preventing starvation on easy domains.
            let priority =
                (errors as u64 + 1) * (1 + u64::from(pass - last)) * 10_000 / total as u64;
            opportunities.push((std::cmp::Reverse(priority), d));
        }
        opportunities.sort();
        let seed = PROBE + u64::from(pass);
        for (_, d) in opportunities {
            let probe = draw(d, seed, &self.schema[d])?;
            let mut rng = Rng::new(seed + d as u64);
            let first = rng.below(probe.len() as u32) as usize;
            for row in probe[first..].iter().chain(&probe[..first]) {
                let predicted = model.predict(row);
                if predicted != row.outcome {
                    return Ok(row.witness(predicted, &self.schema[d]));
                }
            }
        }
        // Completing a sampled probe is not proof of convergence. Preserve a known
        // development miss, or stop with an explicit need for another design workload.
        Ok(self
            .fit
            .iter()
            .find(|r| model.predict(r) != r.outcome)
            .map(|r| r.witness(model.predict(r), &self.schema[r.domain]))
            .unwrap_or(Value::Null))
    }

    pub fn step(&self, session: &mut Session) -> Result<Value, String> {
        if session.next.is_null() {
            return Err("no residual on the development probes; add a new workload and version its contract".into());
        }
        let pass = session
            .passes
            .checked_add(1)
            .ok_or("pass counter overflow")?;
        let w = self.witness(&session.next)?;
        let before = &session.model;
        if w.witness(before.predict(&w), &self.schema[w.domain]) != session.next
            || before.predict(&w) == w.outcome
        {
            return Err("previous analysis did not identify a reproducible incumbent error".into());
        }
        let (candidate, edit) = before
            .propose(&w, &self.fit, &self.schema, &session.tried)
            .ok_or("no untested structural edit fixes the witness; broaden the model language")?;
        let candidate_id = digest(&candidate.to_json(&self.schema));
        let trial_seed = TRIAL + u64::from(pass);
        let fresh = draw(w.domain, trial_seed, &self.schema[w.domain])?;
        let fit = compare(before, &candidate, &self.fit);
        let regression = compare(before, &candidate, &self.regression);
        let trial = compare(before, &candidate, &fresh);
        let locked = compare(before, &candidate, &session.protected);
        let fixed = candidate.predict(&w) == w.outcome;
        let new_false_clears = self
            .regression
            .iter()
            .filter(|r| false_clear(r, candidate.predict(r)) && !false_clear(r, before.predict(r)))
            .count();
        let reason = if !fixed {
            "witness_still_wrong"
        } else if locked.after() != 0 {
            "reopens_retained_counterexample"
        } else if new_false_clears > 0 {
            "new_false_clear_on_fixed_suite"
        } else if regression.after() > regression.before {
            "fixed_suite_loss"
        } else if trial.after() > trial.before {
            "fresh_trajectory_loss"
        } else if fit.after() < fit.before {
            "reduces_development_error"
        } else if fit.after() == fit.before && candidate.size() < before.size() {
            "simpler_at_equal_error"
        } else {
            "no_development_gain"
        };
        let promoted = matches!(
            reason,
            "reduces_development_error" | "simpler_at_equal_error"
        );
        let retained = if promoted { &candidate } else { before };
        let next = self.next_probe(session, retained, pass, w.domain)?;
        let body = json!({"pass":pass,"previous_record":session.head,"parent_pass":session.last_change,
            "before_model":session.model_id,"plan":{"counterexample":session.next,"feedback_from_pass":pass-1},
            "design":edit,"candidate_model":candidate_id,
            "simulation":{"fit":fit.json(),"fixed_regression":regression.json(),"fresh_domain":DOMAINS[w.domain],
                "new_false_clears_on_fixed_suite":new_false_clears,"fresh_seed":trial_seed,"fresh":trial.json(),"protected":locked.json(),"candidate_prediction":candidate.predict(&w)},
            "analysis":{"promoted":promoted,"reason":reason,"rules_before":before.size(),"rules_candidate":candidate.size(),
                "retained_model":if promoted{&candidate_id}else{&session.model_id},"next_counterexample":next}});
        let mut record = body.clone();
        record["record_sha256"] = json!(digest(&body));
        self.advance(session, &record, w, candidate)?;
        Ok(record)
    }

    fn advance(
        &self,
        s: &mut Session,
        r: &Value,
        w: Sample,
        candidate: Model,
    ) -> Result<(), String> {
        let pass = s.passes.checked_add(1).ok_or("pass counter overflow")?;
        let a = &r["analysis"];
        let sim = &r["simulation"];
        let promoted = a["promoted"].as_bool().ok_or("missing disposition")?;
        let truth = s.model.predict(&w);
        let candidate_id = digest(&candidate.to_json(&self.schema));
        let after = if promoted { &candidate_id } else { &s.model_id };
        if r["pass"] != pass
            || r["parent_pass"] != s.last_change
            || r["previous_record"] != s.head
            || r["before_model"] != s.model_id
            || r["candidate_model"] != candidate_id
            || a["retained_model"] != after.as_str()
            || r["plan"]["counterexample"] != s.next
            || r["plan"]["feedback_from_pass"] != s.passes
            || w.witness(truth, &self.schema[w.domain]) != s.next
            || sim["candidate_prediction"] != candidate.predict(&w)
            || a["rules_candidate"] != candidate.size()
            || a["rules_before"] != s.model.size()
            || r["design"]["domain"] != DOMAINS[w.domain]
        {
            return Err(format!("inconsistent model transition at pass {pass}"));
        }
        let count = |key: &str| {
            sim["fixed_regression"][key]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or("missing regression count")
        };
        let check = RoundCheck {
            pass,
            before: s.model_id.clone(),
            candidate: candidate_id.clone(),
            after: after.clone(),
            promoted,
            witness_matches_before: truth == w.outcome,
            witness_fixed: candidate.predict(&w) == w.outcome,
            regression_before: count("errors_before")?,
            regression_after: count("errors_candidate")?,
            candidate_replayed: sim["fresh"]["observations"].as_u64().is_some_and(|n| n > 0),
        };
        let start = s.history.last().map(std::slice::from_ref).unwrap_or(&[]);
        audit::verify(&[start, std::slice::from_ref(&check)].concat())?;
        if !s.tried.insert(digest(&r["design"])) {
            return Err(format!("duplicate hypothesis at pass {pass}"));
        }
        if promoted {
            s.model = candidate;
            s.model_id = candidate_id;
            s.last_change = pass;
            s.protected.push(w.clone());
            s.tried.clear();
        }
        s.last_investigated[w.domain] = pass;
        s.next = a
            .get("next_counterexample")
            .ok_or("missing next investigation")?
            .clone();
        s.passes = pass;
        s.history.push(check);
        s.head = r["record_sha256"]
            .as_str()
            .ok_or("missing record digest")?
            .to_owned();
        Ok(())
    }

    pub fn restore(&self, header: &Value, records: &[Value]) -> Result<Session, String> {
        if header != &self.header {
            return Err("journal uses a different reference workload or model format".into());
        }
        let mut session = self.start();
        for r in records {
            // Replay validates measurements, the promotion gate and the previous
            // pass's next witness before extending a journal from another process.
            if &self.step(&mut session)? != r {
                return Err(format!(
                    "reference replay disagrees at pass {}",
                    session.passes
                ));
            }
        }
        Ok(session)
    }

    /// Predict the reference decisions in a specified trajectory using the exact
    /// retained model. The reference drives the world history, not these predictions.
    pub fn simulate(
        &self,
        session: &Session,
        domain_name: &str,
        seed: u64,
    ) -> Result<Value, String> {
        let d = domain(&json!(domain_name))?;
        let samples = draw(d, seed, &self.schema[d])?;
        let observations: Vec<_> = samples
            .iter()
            .map(|r| r.witness(session.model.predict(r), &self.schema[d]))
            .collect();
        Ok(
            json!({"domain":domain_name,"seed":seed,"model_sha256":session.model_id,"passes":session.passes,
            "comparison":compare(&self.baseline,&session.model,&samples).json(),"observations":observations}),
        )
    }

    pub fn report(&self, session: &Session) -> Result<Value, String> {
        if !session.history.is_empty() {
            audit::verify(&session.history)?;
        }
        let reserved = cohort(&self.schema, &RESERVED)?;
        let mut domains = BTreeMap::new();
        let mut all_settings = BTreeMap::new();
        for (d, name) in DOMAINS.iter().enumerate() {
            let training: BTreeSet<_> = self
                .fit
                .iter()
                .filter(|r| r.domain == d)
                .map(|r| r.bits)
                .collect();
            let mut seen = Change::default();
            let mut unseen = Change::default();
            let mut losses = BTreeMap::new();
            let mut scores = Change::default();
            let mut domain_settings = BTreeMap::new();
            let mask = classroom_mask(name, &self.schema[d])?;
            let (mut baseline_false_clears, mut final_false_clears) = (0, 0);
            for row in reserved.iter().filter(|r| r.domain == d) {
                let predicted = session.model.predict(row);
                let baseline = self.baseline.predict(row);
                let (before, after) = (baseline == row.outcome, predicted == row.outcome);
                scores.observe(before, after);
                let setting = match mask {
                    Some(bit) if row.bits & bit != 0 => "classroom",
                    Some(_) => "individual",
                    None => "unassigned",
                };
                domain_settings
                    .entry(setting)
                    .or_insert_with(Change::default)
                    .observe(before, after);
                all_settings
                    .entry(setting)
                    .or_insert_with(Change::default)
                    .observe(before, after);
                (if training.contains(&row.bits) {
                    &mut seen
                } else {
                    &mut unseen
                })
                .observe(before, after);
                baseline_false_clears += u32::from(false_clear(row, baseline));
                final_false_clears += u32::from(false_clear(row, predicted));
                if predicted != row.outcome {
                    *losses
                        .entry(format!("{predicted} -> {}", row.outcome))
                        .or_insert(0u32) += 1;
                }
            }
            domains.insert(*name,json!({"reserved":scores.json(),"reserved_by_setting":setting_counts(&domain_settings),"input_vector_seen_in_fit":seen.json(),"input_vector_absent_from_fit":unseen.json(),
                "false_clears_before":baseline_false_clears,"false_clears_after":final_false_clears,"remaining_confusions":losses}));
        }
        Ok(
            json!({"format":FORMAT,"passes":session.passes,"promoted":session.protected.len(),
            "rejected":session.passes as usize-session.protected.len(),"model_sha256":session.model_id,
            "rules":session.model.size(),"domains":domains,"model":session.model.to_json(&self.schema),
            "fit":compare(&self.baseline,&session.model,&self.fit).json(),
            "fixed_regression":compare(&self.baseline,&session.model,&self.regression).json(),
            "reserved":compare(&self.baseline,&session.model,&reserved).json(),"reserved_by_setting":setting_counts(&all_settings),"head":session.head,
            "next_counterexample":session.next}),
        )
    }
}

#[cfg(test)]
mod setting_tests {
    use super::*;

    #[test]
    fn reserved_setting_counts_reconcile_with_each_domain_and_the_full_report() {
        let experiment = Experiment::new().unwrap();
        let mut session = experiment.start();
        experiment.step(&mut session).unwrap();
        let report = experiment.report(&session).unwrap();
        let domains = report["domains"].as_object().unwrap();
        let all = report["reserved_by_setting"].as_object().unwrap();
        // Each reserved seed has two individual and two five-seat planning cohorts
        // attending sixteen meetings. Serving alternates solo and classroom episodes.
        assert_eq!(
            domains["planning"]["reserved_by_setting"]["individual"]["observations"],
            96
        );
        assert_eq!(
            domains["planning"]["reserved_by_setting"]["classroom"]["observations"],
            480
        );
        assert_eq!(
            domains["serving"]["reserved_by_setting"]["individual"]["observations"],
            288
        );
        assert_eq!(
            domains["retention"]["reserved_by_setting"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(domains["retention"]["reserved"], all["unassigned"]);
        for key in [
            "observations",
            "errors_before",
            "errors_candidate",
            "new_errors",
            "fixed",
        ] {
            for domain in domains.values() {
                let subtotal: u64 = domain["reserved_by_setting"]
                    .as_object()
                    .unwrap()
                    .values()
                    .map(|v| v[key].as_u64().unwrap())
                    .sum();
                assert_eq!(subtotal, domain["reserved"][key].as_u64().unwrap());
            }
            for (setting, counts) in all {
                let subtotal: u64 = domains
                    .values()
                    .filter_map(|v| v["reserved_by_setting"][setting][key].as_u64())
                    .sum();
                assert_eq!(subtotal, counts[key].as_u64().unwrap());
            }
            let total: u64 = all.values().map(|v| v[key].as_u64().unwrap()).sum();
            assert_eq!(total, report["reserved"][key].as_u64().unwrap());
        }
    }
}
