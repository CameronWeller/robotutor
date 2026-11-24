#![doc = "Paired fictional teaching prompt pilot. See docs/foundation/prompt-pilot.md."]
#[path = "prompt_pilot/ledger.rs"]
mod ledger;
#[path = "prompt_pilot/promotion.rs"]
mod promotion;
#[path = "prompt_pilot/report.rs"]
mod report;
#[path = "prompt_pilot/study.rs"]
mod study;
#[cfg(test)]
#[path = "prompt_pilot/tests.rs"]
mod tests;
use ledger::{hash, Journal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};
use tutor_lab::openrouter::credentials;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MODEL: &str = "openai/gpt-6-luna-20260922";
const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/prompt-pilot");
const FORMAT: &str = include_str!("../src/delivery-format.txt");
const LEARNER: &str = "Play only the fictional learner in the brief. Use only your own visible experience and the current task. Elapsed weeks describe the scenario. Give your first response in your own words. You can ask for help, decline, correct a premise, miss a visit, or change your goal. Report the event faithfully. A request for help does not supply a hint.";
const REALIZE: &str = "Voice the fictional learner's response to the current teaching move under the supplied modeled condition. On recall_error show a concrete mistaken recollection. On success use the reasoning in your own words. If help is requested ask for it. Do not change the modeled condition.";
const QUALITY: &str = "Review the visible lesson. Describe two interpretations the first tutor move could discriminate, or say none. Quote the learner's reply. Describe what the next move did with it. For the modeled action classify the episode from its actual words. Use worked if a solution, hint or worked correction was supplied, retrieval if the learner actually attempted the reasoning without that help, and unsupported if neither represents what happened. Flag ambiguity about that mapping in the explanation.";
const SCORE: &str = "Score only the first submitted answer on the two frozen criteria in order. Accept equivalent reasoning. For each true finding quote supporting words exactly from the response. For each false finding explain what is missing. Judge only the requested written artifact.";
const ROUTE: &str = "Rust offers the frozen opening and a practice visit, then two common delayed tasks. In teaching visits, respond to the submitted work and use the learner's reply for the follow-up. A request to stop closes the visit.";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    baseline: String,
    candidate: String,
    simulations: Vec<String>,
    cases: Vec<String>,
    seed: u64,
    call_limit: u64,
    token_limit: u64,
    budget_microusd: u64,
}
#[derive(Serialize, Deserialize)]
struct Case {
    id: String,
    subject: String,
    brief: String,
    opening: String,
    answer: String,
    practice: String,
    changed: String,
    transfer: String,
    changed_criteria: [String; 2],
    transfer_criteria: [String; 2],
    conclusion: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Version {
    id: String,
    parent: Option<String>,
    sha256: String,
    diff: String,
    prediction: String,
    against: String,
}
struct Frozen {
    settings: Settings,
    versions: [(Version, String); 2],
    cases: Vec<Case>,
    answers: Value,
    manifest: Value,
}

fn read(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?.replace("\r\n", "\n"))
}
fn freeze(settings: Settings, mode: &str) -> Result<Frozen> {
    let root = Path::new(DATA);
    let versions: Vec<Version> = serde_json::from_str(&read(&root.join("versions.json"))?)?;
    let version = |id: &str| -> Result<(Version, String)> {
        if !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err("Invalid version ID".into());
        }
        let matches: Vec<_> = versions.iter().filter(|v| v.id == id).collect();
        if matches.len() != 1 {
            return Err(format!("Missing or duplicate version {id}").into());
        }
        let v = matches[0];
        let prompt = read(&root.join(format!("{id}.txt")))?;
        if hash(prompt.as_bytes()) != v.sha256 {
            return Err(format!("Prompt hash mismatch for {id}").into());
        }
        Ok((v.clone(), prompt))
    };
    for start in [&settings.baseline, &settings.candidate] {
        let mut ancestor = Some(start.as_str());
        let mut visited = BTreeSet::new();
        while let Some(id) = ancestor {
            if !visited.insert(id) {
                return Err("Cyclic prompt lineage".into());
            }
            let v = versions
                .iter()
                .find(|v| v.id == id)
                .ok_or("Unknown prompt ancestor")?;
            ancestor = v.parent.as_deref();
        }
    }
    let versions = [version(&settings.baseline)?, version(&settings.candidate)?];
    if versions[1].0.parent.is_none() || versions[0].0.sha256 == versions[1].0.sha256 {
        return Err(
            "Comparison needs two distinct prompt texts and a registered candidate parent".into(),
        );
    }
    let cases_json = read(&root.join("cases.json"))?;
    let cases: Vec<Case> = serde_json::from_str(&cases_json)?;
    let materials = read(&root.join("model-answers.json"))?;
    let answers: Value = serde_json::from_str(&materials)?;
    for values in [&settings.cases, &settings.simulations] {
        if values.is_empty() || values.iter().collect::<BTreeSet<_>>().len() != values.len() {
            return Err("Empty or repeated selection".into());
        }
    }
    if settings
        .simulations
        .iter()
        .any(|s| !matches!(s.as_str(), "agent" | "explicit"))
        || settings
            .cases
            .iter()
            .any(|id| cases.iter().filter(|c| c.id == *id).count() != 1)
    {
        return Err("Unknown simulation or case".into());
    }
    for id in &settings.cases {
        for task in ["changed", "transfer"] {
            for outcome in ["success", "recall_error", "application_error"] {
                if answers[id][task][outcome]
                    .as_str()
                    .is_none_or(|s| s.trim().is_empty())
                {
                    return Err("Incomplete frozen model answers".into());
                }
            }
        }
    }
    let policy: Vec<_> = [
        include_str!("prompt_pilot/study.rs"),
        include_str!("prompt_pilot/ledger.rs"),
        include_str!("prompt_pilot/report.rs"),
        include_str!("prompt_pilot.rs"),
        include_str!("../src/learning_epochs/mod.rs"),
        include_str!("../src/learning_epochs/pilot.rs"),
        include_str!("../src/material.rs"),
        include_str!("../src/openrouter.rs"),
        include_str!("../Cargo.lock"),
    ]
    .iter()
    .map(|s| hash(s.replace("\r\n", "\n").as_bytes()))
    .collect();
    let manifest = json!({"format":1,"mode":mode,"settings":settings,"versions":versions,"policy_source_sha256":policy,
        "fixture_sha256":hash(cases_json.as_bytes()),"response_bank_sha256":hash(materials.as_bytes()),
        "role_instructions":{"format":FORMAT,"learner":LEARNER,"realizer":REALIZE,"routing":ROUTE,"quality":QUALITY,"score":SCORE},
        "model":MODEL,"provider":provider(),"sampling":{"reasoning":"low","temperature":"provider default","seed":"same case/day/stage in both arms","max_tokens":640}});
    Ok(Frozen {
        settings,
        versions,
        cases,
        answers,
        manifest,
    })
}

fn provider() -> Value {
    json!({"only":["openai"],"allow_fallbacks":false,"require_parameters":true,"max_price":{"prompt":1,"completion":3,"request":0}})
}
fn request(instructions: &[&str], input: Value, schema: Value, seed: u32) -> Value {
    let mut messages: Vec<_> = instructions
        .iter()
        .map(|s| json!({"role":"system","content":s}))
        .collect();
    messages.push(json!({"role":"user","content":input.to_string()}));
    json!({"model":MODEL,"provider":provider(),"reasoning":{"effort":"low","exclude":true},"seed":seed,
        "max_tokens":640,"messages":messages,"response_format":{"type":"json_schema","json_schema":{"name":"pilot_turn","strict":true,"schema":schema}}})
}
fn estimate(f: &Frozen) -> Value {
    let cases = f.settings.cases.len();
    let per_case: usize = f
        .settings
        .simulations
        .iter()
        .map(|s| if s == "agent" { 30 } else { 24 })
        .sum();
    json!({"cases":cases,"max_inference_calls":cases*per_case,"configured_limits":{"calls":f.settings.call_limit,"tokens":f.settings.token_limit,"usd":f.settings.budget_microusd as f64/1e6},"max_completion_tokens_per_call":640})
}
fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let mode = args.first().and_then(|s| s.to_str()).unwrap_or("");
    let registry = Path::new(DATA).join("promotions");
    if mode == "canonical" && args.len() == 1 {
        println!("{}", promotion::canonical(&registry)?);
        return Ok(());
    }
    let count = match mode {
        "estimate" => 2,
        "run" | "replay" => 3,
        "promote" => 4,
        _ => 0,
    };
    if count == 0 || args.len() != count {
        return Err("Usage: prompt_pilot canonical | estimate CONFIG | run CONFIG RUN_DIR | replay CONFIG RUN_DIR | promote CONFIG RUN_DIR APPROVAL.json".into());
    }
    let f = freeze(serde_json::from_str(&read(Path::new(&args[1]))?)?, "live")?;
    if mode == "estimate" {
        println!("{}", serde_json::to_string_pretty(&estimate(&f))?);
        return Ok(());
    }
    let dir = PathBuf::from(&args[2]);
    let mut journal = Journal::open(&dir, &f.manifest, f.settings.clone(), mode != "run")?;
    let cfg = if mode == "run" {
        Some(
            credentials::existing()?
                .ok_or("Live pilot needs the Robotutor OpenRouter credential")?,
        )
    } else {
        None
    };
    let report = study::run(&f, &mut journal, &mut |r| {
        Ok(cfg
            .as_ref()
            .ok_or("Unexpected replay dispatch")?
            .pilot_completion(r)?)
    })?;
    let path = dir.join("comparison.json");
    let bytes = serde_json::to_vec_pretty(&report)?;
    if mode != "run" && path.exists() && fs::read(&path)? != bytes {
        return Err("Report differs on replay".into());
    }
    if mode == "run" || (mode == "replay" && !path.exists()) {
        let staged = dir.join("comparison.json.tmp");
        fs::write(&staged, bytes)?;
        fs::rename(staged, &path)?;
    }
    if mode == "promote" {
        promotion::record(&registry, Path::new(&args[3]), &f, &report)?;
    }
    println!("{}\n{}", path.display(), journal.usage());
    Ok(())
}
