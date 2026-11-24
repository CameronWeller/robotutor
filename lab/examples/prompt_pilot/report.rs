use super::*;
use std::collections::BTreeMap;

fn rating(arm: &Value, visit: usize) -> Option<i8> {
    let score = &arm["outcomes"][visit]["score"];
    Some(i8::from(score["first"]["met"].as_bool()?) + i8::from(score["second"]["met"].as_bool()?))
}
#[derive(Default, Serialize)]
struct Counts {
    scheduled: u32,
    ceiling: u32,
    missing: u32,
    unsupported: u32,
    candidate_higher: u32,
    baseline_higher: u32,
    tied: u32,
}
pub fn compare(f: &Frozen, mut pairs: Vec<Value>, usage: Value) -> Value {
    let mut counts: BTreeMap<String, BTreeMap<String, Counts>> = BTreeMap::new();
    let mut case_results: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    let mut traces = json!({"candidate_advantage":null,"failure_or_inconclusive":null});
    for pair in &mut pairs {
        let subject = pair["subject"].as_str().unwrap().to_owned();
        let simulation = pair["simulation"].as_str().unwrap().to_owned();
        let case = pair["case"].as_str().unwrap().to_owned();
        let arms = &pair["arms"];
        let summary = counts
            .entry(simulation.clone())
            .or_default()
            .entry(subject)
            .or_default();
        let ceiling = (0..2).any(|v| rating(&arms["control"], v) == Some(2));
        let unsupported = simulation == "explicit"
            && ["baseline", "candidate"].iter().any(|arm| {
                arms[arm]["teaching_reviews"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["review"]["mapping"] == "unsupported")
            });
        let mut comparisons = vec![];
        let mut directions = vec![];
        for (visit, day) in [28, 84].into_iter().enumerate() {
            let base = rating(&arms["baseline"], visit);
            let candidate = rating(&arms["candidate"], visit);
            let control = rating(&arms["control"], visit);
            summary.scheduled += 1;
            let direction = if base.is_none() || candidate.is_none() || control.is_none() {
                summary.missing += 1;
                "missing"
            } else if ceiling {
                summary.ceiling += 1;
                "ceiling"
            } else if unsupported {
                summary.unsupported += 1;
                "unsupported"
            } else if candidate > base {
                summary.candidate_higher += 1;
                "candidate higher"
            } else if base > candidate {
                summary.baseline_higher += 1;
                "baseline higher"
            } else {
                summary.tied += 1;
                "tied"
            };
            comparisons.push(json!({"day":day,"baseline":base,"candidate":candidate,"control":control,"interpretation":direction}));
            directions.push(direction.to_owned());
        }
        for (visit, direction) in directions.iter().enumerate().rev() {
            let slot = if direction == "candidate higher" {
                "candidate_advantage"
            } else {
                "failure_or_inconclusive"
            };
            if traces[slot].is_null() {
                traces[slot] = json!({"case":case,"simulation":simulation,"selected_day":([28,84][visit]),"observations":comparisons,
                "baseline_path":arms["baseline"]["transcript"],"candidate_path":arms["candidate"]["transcript"],
                "baseline_review":arms["baseline"]["teaching_reviews"],"candidate_review":arms["candidate"]["teaching_reviews"]});
            }
        }
        case_results
            .entry(case)
            .or_default()
            .insert(simulation, comparisons.clone());
        pair["ceiling_case"] = json!(ceiling);
        pair["unsupported_mapping"] = json!(unsupported);
        pair["comparisons"] = json!(comparisons);
    }
    let mut disagreements = vec![];
    for (case, views) in case_results {
        if let (Some(explicit), Some(agent)) = (views.get("explicit"), views.get("agent")) {
            for (i, day) in [28, 84].into_iter().enumerate() {
                if explicit[i] != agent[i] {
                    disagreements.push(
                        json!({"case":case,"day":day,"explicit":explicit[i],"agent":agent[i]}),
                    );
                }
            }
        }
    }
    let old = &f.versions[0].1;
    let new = &f.versions[1].1;
    let diff = if let Some(suffix) = new.strip_prefix(old) {
        json!({"append_verbatim":suffix})
    } else {
        json!({"before":old,"after":new})
    };
    json!({"manifest":f.manifest,"exact_prompt_change":diff,"usage":usage,
        "counts_by_simulation_and_subject":counts,"paired_cases":pairs,"disagreements":disagreements,"traces":traces,
        "interpretation":{"explicit":"Outcomes under the stated recall, application, attendance and help assumptions.",
        "agent":"Performance by simulated participants with elapsed weeks supplied as context.",
        "counts":"Each row counts scheduled delayed comparisons. A full-score control flags the whole case. Missing first unaided responses, ceiling cases and unsupported modeled teaching moves remain visible."}})
}
