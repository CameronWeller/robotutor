//! Short and longitudinal tutoring experiments. Paid transport is opt-in.
#[path = "bounded_probes/hybrid.rs"]
mod hybrid;
#[path = "bounded_probes/hybrid_cases.rs"]
mod hybrid_cases;
#[path = "bounded_probes/hybrid_schedule.rs"]
mod hybrid_schedule;
use tutor_lab::learning_epochs;
#[path = "bounded_probes/scenarios.rs"]
mod scenarios;

use hybrid_schedule::Mode;
use serde_json::{json, Value};
use std::{
    env, fs,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::ExitCode,
};
use tutor_lab::openrouter::{
    assistant_text, decision_choice, experiment::Campaign, matches_reported, Config, Role,
    DEFAULT_JEV, DEFAULT_LUNA,
};

const HELP: &str = "bounded_probes preview | hybrid sparse|finishing [SEED] | hybrid compare FIRST_SEED COUNT | [suite] live DIRECTORY --budget-usd DOLLARS [--credit-only-account] | [suite] replay DIRECTORY | status DIRECTORY";

fn dollars(text: &str) -> Result<u64, String> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("expected a positive dollar amount with at most six decimal places".into());
    }
    let micros = format!("{whole}{fraction:0<6}")
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0);
    micros.ok_or_else(|| "invalid dollar ceiling".into())
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Jev => "jev",
        Role::Luna => "luna",
    }
}

/// Keep only public output consumed by the test. Excludes provider diagnostics,
/// internal reasoning, headers and account metadata.
fn public_reply(role: Role, request: &Value, reply: &Value) -> Value {
    let Some(model) = reply["model"].as_str().filter(|returned| {
        request["model"]
            .as_str()
            .is_some_and(|m| matches_reported(m, returned))
    }) else {
        return json!({"error":true});
    };
    let rejected = || json!({"model":model,"error":true});
    match role {
        Role::Jev => {
            let Some((key, question)) = request["questions"]
                .as_object()
                .and_then(|q| q.iter().next())
            else {
                return rejected();
            };
            if reply["answers"]
                .as_object()
                .is_none_or(|answers| answers.len() != 1)
            {
                return rejected();
            }
            let Some(criteria) = question["criteria"].as_object() else {
                return rejected();
            };
            let choices: Vec<_> = criteria.keys().map(String::as_str).collect();
            match decision_choice(reply, model, key, &choices) {
                Ok(choice) => {
                    json!({"model":model, "answers":{(key):{"type":"choice","choice":choice}}})
                }
                Err(_) => rejected(),
            }
        }
        Role::Luna => match assistant_text(reply) {
            Ok(text) => json!({"model":model,"choices":[{"finish_reason":"stop",
                "message":{"role":"assistant","content":text}}]}),
            Err(_) => rejected(),
        },
    }
}

fn case_request(case: &str) -> Result<(Role, Value), String> {
    let mut packet = None;
    let outcome = scenarios::run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |role, request| {
        packet = Some((role, request.clone()));
        Err("planning ends before dispatch".into())
    });
    packet.ok_or_else(|| outcome.err().unwrap_or("missing request".into()))
}

fn preview() -> Result<Value, String> {
    let tasks = scenarios::CASES
        .iter()
        .map(|case| {
            let (role, request) = case_request(case)?;
            Ok(json!({"case":case,"role":role_name(role),"request":request}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(json!({"mode":"preview","provider_calls":0,"tests":tasks}))
}

fn load_report(path: &Path) -> Result<Value, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(1_048_577).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > 1_048_576 {
        return Err("case report too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid case report".into())
}

fn replay_case(case: &str, report: &Value) -> Result<Value, String> {
    if report["format"] != "tutor-probe/1" || report["case"] != case {
        return Err("case report does not match the requested test".into());
    }
    let mut used = false;
    let result = scenarios::run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |role, request| {
        if used || report["role"] != role_name(role) || request != &report["request"] {
            return Err("stored response belongs to a different request".into());
        }
        used = true;
        Ok(public_reply(role, request, &report["reply"]))
    })?;
    if !used || result != report["score"] {
        return Err("replayed score differs from the recorded observation".into());
    }
    Ok(result)
}

fn io_error(e: std::io::Error) -> String {
    e.to_string()
}

fn finalize_report(dir: &Path, pending: &Path, path: &Path) -> Result<(), String> {
    tutor_lab::platform::replace_staged(dir, pending, path).map_err(io_error)
}

/// A settled charge fences another send even when the previous process died
/// before it durably captured the observation. Never turn a missing reply into a model result.
fn recovered_score(dir: &Path, case: &str, billed: bool) -> Result<Option<Value>, String> {
    let path = dir.join(format!("{case}.json"));
    let pending = dir.join(format!("{case}.pending"));
    let complete = path.try_exists().map_err(io_error)?;
    let partial = pending.try_exists().map_err(io_error)?;
    if !billed {
        return if complete || partial {
            Err(format!(
                "{case}: report present without a matching settled charge in this campaign"
            ))
        } else {
            Ok(None)
        };
    }
    let unavailable = |reason: &str| {
        Some(json!({"case":case,"passed":false,
        "status":"paid_observation_unavailable","reason":reason}))
    };
    if !complete && !partial {
        return Ok(unavailable("no response record survived"));
    }
    let file = if complete { &path } else { &pending };
    let score = match load_report(file).and_then(|v| replay_case(case, &v)) {
        Ok(s) => s,
        Err(_) => {
            return Ok(unavailable(
                "stored observation could not be read or replayed",
            ))
        }
    };
    if !complete {
        OpenOptions::new()
            .write(true)
            .open(&pending)
            .and_then(|f| f.sync_all())
            .map_err(io_error)?;
        finalize_report(dir, &pending, &path)?;
    }
    Ok(Some(score))
}

fn run_live(dir: &Path, budget: u64, credit_only: bool) -> Result<Value, String> {
    let config = Config::from_env()?;
    fs::create_dir_all(dir).map_err(io_error)?;
    let mut campaign = Campaign::open(&dir.join("meter.json"), &config, budget)?;
    if credit_only {
        campaign.confirm_credit_only_account();
    }
    let tests = run_short(dir, &mut campaign)?;
    Ok(json!({"mode":"live","tests":tests,"meter":campaign.summary()}))
}

fn run_short(dir: &Path, campaign: &mut Campaign<'_>) -> Result<Vec<Value>, String> {
    let mut tests = Vec::new();
    for case in scenarios::CASES {
        let path = dir.join(format!("{case}.json"));
        let (role, request) = case_request(case)?;
        if let Some(score) = recovered_score(dir, case, campaign.request_billed(role, &request)?)? {
            tests.push(score);
            continue;
        }
        let mut record = None;
        let score = scenarios::run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |role, request| {
            if record.is_some() {
                return Err("test already used its one request".into());
            }
            let raw = campaign.complete(role, request)?;
            let reply = public_reply(role, request, &raw);
            record = Some(
                json!({"format":"tutor-probe/1","case":case,"role":role_name(role),
                "request":request,"reply":reply}),
            );
            Ok(reply)
        })?;
        let mut record = record.ok_or("test ended without a model observation")?;
        record["score"] = score.clone();
        let pending = dir.join(format!("{case}.pending"));
        // The meter durably fences each dispatch. Only completed observations
        // reach this file, so a failed preflight consumes no case opportunity.
        write_report(dir, &pending, &path, &record)?;
        tests.push(score);
    }
    Ok(tests)
}

fn write_report(dir: &Path, pending: &Path, path: &Path, record: &Value) -> Result<(), String> {
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts
        .open(pending)
        .map_err(|e| format!("{}: {e}", pending.display()))?;
    serde_json::to_writer_pretty(&mut file, record).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(io_error)?;
    finalize_report(dir, pending, path)
}

fn replay_short(dir: &Path) -> Result<Vec<Value>, String> {
    let mut tests = Vec::new();
    for case in scenarios::CASES {
        let path = dir.join(format!("{case}.json"));
        let source = if path.try_exists().map_err(io_error)? {
            path
        } else {
            dir.join(format!("{case}.pending"))
        };
        let score = load_report(&source)
            .and_then(|report| replay_case(case, &report))
            .unwrap_or_else(|error| {
                json!({"case":case,"passed":false,
                "status":"observation_unavailable","error":error})
            });
        tests.push(score);
    }
    Ok(tests)
}

fn execute(args: &[String]) -> Result<Value, String> {
    match args {
        [mode] if mode == "preview" => preview(),
        [mode, dir] if mode == "status" => Campaign::status(&Path::new(dir).join("meter.json")),
        [mode, dir] if mode == "replay" => {
            Ok(json!({"mode":"replay","provider_calls":0,"tests":replay_short(Path::new(dir))?}))
        }
        [study, mode, dir] if study == "suite" && mode == "replay" => {
            hybrid::suite_replay(Path::new(dir))
        }
        [study, rest @ ..] if study == "suite" => match rest {
            [mode, dir, opt, amount] if mode == "live" && opt == "--budget-usd" => {
                hybrid::suite_live(Path::new(dir), dollars(amount)?, false)
            }
            [mode, dir, opt, amount, flag]
                if mode == "live" && opt == "--budget-usd" && flag == "--credit-only-account" =>
            {
                hybrid::suite_live(Path::new(dir), dollars(amount)?, true)
            }
            _ => Err(HELP.into()),
        },
        [group, mode, seed, count] if group == "hybrid" && mode == "compare" => hybrid::compare(
            seed.parse().map_err(|_| "invalid unsigned seed")?,
            count.parse().map_err(|_| "invalid seed count")?,
        ),
        [group, mode, extra @ ..] if group == "hybrid" => {
            let mode = match mode.as_str() {
                "sparse" => Mode::Sparse,
                "finishing" => Mode::Finishing,
                _ => return Err(HELP.into()),
            };
            let seed = match extra {
                [] => hybrid::STUDY_SEED,
                [value] => value.parse().map_err(|_| "invalid unsigned seed")?,
                _ => return Err(HELP.into()),
            };
            hybrid::offline(mode, seed)
        }
        [mode, dir, opt, amount] if mode == "live" && opt == "--budget-usd" => {
            run_live(Path::new(dir), dollars(amount)?, false)
        }
        [mode, dir, opt, amount, funding]
            if mode == "live" && opt == "--budget-usd" && funding == "--credit-only-account" =>
        {
            run_live(Path::new(dir), dollars(amount)?, true)
        }
        _ => Err(HELP.into()),
    }
}

fn main() -> ExitCode {
    match execute(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("JSON value")
            );
            if result["passed"] == false
                || result["tests"]
                    .as_array()
                    .is_some_and(|tests| tests.iter().any(|t| t["passed"] == false))
            {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod runner_tests {
    use super::*;
    #[test]
    fn budgets_are_exact_and_output_omits_private_provider_metadata() {
        assert_eq!(dollars("20").unwrap(), 20_000_000);
        assert_eq!(dollars("0.000001").unwrap(), 1);
        for bad in ["", "0", "-5", "1e4", "2.0000001", "18446744073710"] {
            assert!(dollars(bad).is_err());
        }
        let s = public_reply(
            Role::Luna,
            &json!({"model":DEFAULT_LUNA}),
            &json!({"model":DEFAULT_LUNA,"choices":[{
            "finish_reason":"stop","message":{"role":"assistant","content":"Public.","reasoning":"private detail"}}],"usage":{"provider_private":"internal"}}),
        );
        assert!(s.to_string().contains("Public."));
        assert!(!s.to_string().contains("private"));
    }

    #[test]
    fn scoring_and_replay_preserve_rejections_without_private_diagnostics() {
        for case in [scenarios::CASES[0], scenarios::CASES[2]] {
            let (role, request) = case_request(case).unwrap();
            let model = &request["model"];
            let rejected = if role == Role::Jev {
                let key = request["questions"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap();
                let decision = json!({"type":"choice","choice":"worked_example"});
                vec![
                    json!({"model":model,"answers":{(key):decision},"error":"private detail"}),
                    json!({"model":model,"answers":{(key):decision,"private unexpected key":decision}}),
                    json!({"model":model,"answers":{(key):{"type":"choice","choice":"private unoffered choice"}}}),
                ]
            } else {
                let choice = json!({"finish_reason":"stop","message":{"role":"assistant","content":"private text"}});
                let mut refused = choice.clone();
                refused["message"]["refusal"] = json!("private reason");
                vec![
                    json!({"model":model,"choices":[choice,choice]}),
                    json!({"model":model,"choices":[refused]}),
                ]
            };
            for raw in rejected {
                let reply = public_reply(role, &request, &raw);
                assert!(reply.to_string().find("private").is_none());
                assert_eq!(reply["error"], true);
                let score =
                    scenarios::run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |_, _| Ok(reply.clone()))
                        .unwrap();
                assert_eq!(score["passed"], false);
                let record = json!({"format":"tutor-probe/1","case":case,"role":role_name(role),
                    "request":request,"reply":reply,"score":score});
                assert_eq!(replay_case(case, &record).unwrap(), score);
            }
        }
    }

    #[test]
    fn restart_recovers_a_complete_observation_and_keeps_a_paid_loss_separate_from_unseen_work() {
        let dir = env::temp_dir().join(format!("bounded-probe-recovery-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let case = scenarios::CASES[0];
        let (role, request) = case_request(case).unwrap();
        let key = request["questions"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap();
        let reply = json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":"worked_example"}}});
        let score =
            scenarios::run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |_, _| Ok(reply.clone())).unwrap();
        let record = json!({"format":"tutor-probe/1","case":case,"role":role_name(role),
            "request":request,"reply":reply,"score":score});
        let pending = dir.join(format!("{case}.pending"));
        let path = dir.join(format!("{case}.json"));
        let lost = recovered_score(&dir, case, true).unwrap().unwrap();
        assert_eq!(lost["status"], "paid_observation_unavailable");
        assert_eq!(lost["passed"], false);
        assert_eq!(
            recovered_score(&dir, scenarios::CASES[1], false).unwrap(),
            None
        );
        fs::write(&pending, "{").unwrap();
        assert_eq!(
            recovered_score(&dir, case, true).unwrap().unwrap()["status"],
            "paid_observation_unavailable"
        );
        fs::write(&pending, record.to_string()).unwrap();
        assert!(recovered_score(&dir, case, false).is_err());
        assert!(!path.exists());
        assert_eq!(
            recovered_score(&dir, case, true).unwrap(),
            Some(score.clone())
        );
        assert!(path.exists() && !pending.exists());
        assert_eq!(recovered_score(&dir, case, true).unwrap(), Some(score));
        // A partial historical campaign can be inspected without a credential.
        // This path makes no assertion about billing from an absent file.
        let archive = execute(&["replay".into(), dir.to_str().unwrap().into()]).unwrap();
        assert_eq!(archive["provider_calls"], 0);
        assert_eq!(archive["tests"][0]["passed"], true);
        assert_eq!(archive["tests"][1]["status"], "observation_unavailable");
        assert_eq!(archive["tests"][2]["status"], "observation_unavailable");
        fs::remove_dir_all(dir).unwrap();
    }
}
