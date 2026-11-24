//! Timings contain only durations, provider usage and stage names.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, path::Path, time::Instant};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub micros: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Timing {
    #[serde(default)]
    pub entry: Option<String>,
    pub kind: String,
    pub delivered: bool,
    pub feedback_us: Option<u64>,
    pub actionable_us: Option<u64>,
    pub stages: Vec<Stage>,
}
struct Clock {
    start: Instant,
    stages: Vec<Stage>,
    feedback: Option<u64>,
}
thread_local! {static ACTIVE:RefCell<Option<Clock>>=const {RefCell::new(None)};}
pub fn begin() {
    ACTIVE.with_borrow_mut(|c| {
        *c = Some(Clock {
            start: Instant::now(),
            stages: vec![],
            feedback: None,
        })
    });
}
pub fn feedback() {
    ACTIVE.with_borrow_mut(|clock| {
        if let Some(c) = clock {
            c.feedback
                .get_or_insert(c.start.elapsed().as_micros() as u64);
        }
    });
}
fn record(name: &str, start: Instant, usage: Option<Value>) {
    ACTIVE.with_borrow_mut(|clock| {
        if let Some(c) = clock {
            c.stages.push(Stage {
                name: name.into(),
                micros: start.elapsed().as_micros() as u64,
                usage,
            })
        }
    });
}
pub fn stage<T>(name: &str, op: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let (result, usage) = tutor_lab::openrouter::with_usage(op);
    let mut calls = usage.into_iter();
    record(name, start, calls.next());
    for usage in calls {
        ACTIVE.with_borrow_mut(|clock| {
            if let Some(c) = clock {
                c.stages.push(Stage {
                    name: format!("{name}/additional_call"),
                    micros: 0,
                    usage: Some(usage),
                })
            }
        });
    }
    result
}
pub fn provider(name: &str, op: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
    stage(name, op)
}
pub fn end(kind: &str, delivered: bool, actionable: bool) -> Option<Timing> {
    ACTIVE.with_borrow_mut(|c| c.take()).map(|c| {
        let elapsed = c.start.elapsed().as_micros() as u64;
        Timing {
            entry: None,
            kind: kind.into(),
            delivered,
            feedback_us: delivered.then_some(c.feedback.unwrap_or(elapsed)),
            actionable_us: (delivered && actionable).then_some(elapsed),
            stages: c.stages,
        }
    })
}
pub fn percentiles(mut samples: Vec<u64>) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    samples.sort_unstable();
    let n = samples.len();
    let median = (samples[(n - 1) / 2] as f64 + samples[n / 2] as f64) / 2.0;
    json!({"n":n,"median_us":median,"p95_us":samples[(n * 95).div_ceil(100) - 1]})
}
pub fn report(path: &Path) -> Result<Value, String> {
    let paths = if path.is_dir() {
        crate::replay::journals(path)?
    } else {
        vec![path.into()]
    };
    let mut groups: BTreeMap<String, Vec<Timing>> = BTreeMap::new();
    let (mut prepared, mut used) = (0, 0);
    for p in paths {
        let visit = crate::session::replay(&p)?;
        for timing in visit.timings {
            groups.entry(timing.kind.clone()).or_default().push(timing);
        }
        for e in visit.practice_history {
            prepared += e.prepared;
            used += e.used;
        }
    }
    let entry_times = ["cold", "return", "ongoing"].map(|e| {
        (
            e,
            percentiles(
                groups
                    .values()
                    .flatten()
                    .filter(|s| s.entry.as_deref() == Some(e))
                    .filter_map(|s| s.actionable_us)
                    .collect(),
            ),
        )
    });
    let mut reports = BTreeMap::new();
    for (kind, samples) in groups {
        let mut stages: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
        let (mut total_tokens, mut reported_cost, mut calls, mut unknown_cost) = (0, 0.0, 0, 0);
        for s in samples.iter().flat_map(|t| &t.stages) {
            stages.entry(&s.name).or_default().push(s.micros);
            if let Some(u) = &s.usage {
                calls += 1;
                total_tokens += u["total_tokens"].as_u64().unwrap_or(0);
                if let Some(cost) = u["cost"].as_f64() {
                    reported_cost += cost
                } else {
                    unknown_cost += 1
                }
            }
        }
        reports.insert(kind,json!({"attempts":samples.len(),"failed":samples.iter().filter(|s|!s.delivered).count(),
            "submit_to_feedback":percentiles(samples.iter().filter_map(|s|s.feedback_us).collect()),"submit_to_exercise":percentiles(samples.iter().filter_map(|s|s.actionable_us).collect()),
            "stages":stages.into_iter().map(|(key,t)|(key,percentiles(t))).collect::<BTreeMap<_,_>>(),
            "provider_calls":calls,"reported_tokens":total_tokens,"reported_cost_usd":reported_cost,"calls_without_reported_cost":unknown_cost}));
    }
    let background = if path.is_dir() && path.join("supervision.sqlite3").exists() {
        Some(crate::preparation::totals(path)?)
    } else {
        None
    };
    Ok(
        json!({"by_entry":entry_times,"ordinary_preparation":background,"turns":reports,"prepared_branches_on_submissions":prepared,"selected_prepared_branches":used}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_median_uses_both_middle_observations() {
        let result = percentiles(vec![100, 5, 7, 8]);
        assert_eq!(result["median_us"], 7.5);
        assert_eq!(result["p95_us"], 100);
        assert!(percentiles(vec![]).is_null());
    }
}
