//! A paid intervention can only see the current visit and earlier observations.
//! The next term is simulated after the response is checked and any material delivered.
use super::{
    finalize_report, hybrid_cases, io_error, load_report, public_reply, replay_short, role_name,
    run_short, write_report, Campaign, Config, Role, DEFAULT_JEV, DEFAULT_LUNA,
};
use crate::{
    hybrid_cases::Focus,
    hybrid_schedule::{at_visit, schedule, Gate, Mode, Sample, TraceStep, TERMS},
    learning_epochs::{Lesson, TrackedLearner},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

pub const STUDY_SEED: u64 = crate::learning_epochs::DEFAULT_SEED;
const LEARNER: u32 = 1;

fn context(
    history: &[Value],
    sample: &Sample,
    prior: &[Value],
    mode: Mode,
) -> Result<Value, String> {
    let now = history.last().ok_or("missing observed visit")?;
    if now["kind"] != "synthetic_probe" || now["day"] != sample.day {
        return Err("an insertion needs an observed answer from this visit".into());
    }
    let mut packet = json!({"mode":mode.label(),"stage":sample.ordinal,"visit":sample.term,
        "specificity":sample.specificity});
    for key in [
        "day",
        "prompt",
        "response",
        "judgment",
        "assisted",
        "coverage",
        "quarters",
        "remaining_eighths",
    ] {
        packet[key] = now[key].clone();
    }
    let memory_count = match sample.specificity {
        1 => 0,
        2 => 1,
        _ => 3,
    };
    let mut earlier: Vec<_> = history.iter().rev()
        .filter(|row| row["kind"] == "synthetic_probe" && row["day"].as_u64().is_some_and(|d| d < u64::from(sample.day)))
        .take(memory_count)
        .map(|row| json!({"day":row["day"],"response":row["response"],"judgment":row["judgment"],
            "prompt":row["prompt"],"quarters":row["quarters"],"remaining_eighths":row["remaining_eighths"],
            "assisted":row["assisted"],"coverage":row["coverage"]})).collect();
    earlier.reverse();
    packet["history"] = json!(earlier);
    if sample.specificity >= 2 {
        packet["prior_route"] = prior
            .iter()
            .filter(|p| p["validated"] == true)
            .find_map(|p| p["selected"].as_str())
            .into();
        packet["previous_interventions"] = json!(prior);
    }
    Ok(packet)
}

fn operands(v: &Value) -> Result<(u16, u16), String> {
    let parse = |key: &str| {
        v[key]
            .as_u64()
            .and_then(|n| n.try_into().ok())
            .ok_or_else(|| format!("invalid verified quantity {key}"))
    };
    Ok((parse("quarters")?, parse("remaining_eighths")?))
}

fn deliver(learner: &mut TrackedLearner, score: &Value) -> Result<Option<Value>, String> {
    if score["passed"] != true {
        return Ok(None);
    }
    if score["selected"] == "request_intermediate" {
        return learner.ask_initial_eighths().map(Some);
    }
    if !score["verified_transfer"].is_null() {
        let (q, r) = operands(&score["verified_transfer"])?;
        return learner.attempt_transfer(q, r).map(Some);
    }
    let lesson = match score["verified_lesson"].as_str() {
        Some("compare_units") => Lesson::CompareUnits,
        Some("rehearse_subtraction") => Lesson::RehearseSubtraction,
        None => return Ok(None),
        _ => return Err("unknown verified lesson".into()),
    };
    if score["verified_worked"].is_object() {
        let (q, r) = operands(&score["verified_worked"])?;
        learner.teach_worked(lesson, q, r).map(Some)
    } else {
        learner.teach(lesson).map(Some)
    }
}

fn run(
    mode: Mode,
    seed: u64,
    provenance: &str,
    deadline: Instant,
    mut exchange: impl FnMut(&str, Role, &Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let chosen = schedule(mode, seed, LEARNER);
    let mut learner = TrackedLearner::new(seed, LEARNER);
    let mut baseline = TrackedLearner::new(seed, LEARNER);
    let mut handled = [false; 3];
    let mut gate = Gate::new(mode, seed, LEARNER);
    let mut trace = Vec::new();
    let mut insertions = Vec::new();
    let mut previous = Vec::new();
    let mut trajectory = Vec::new();
    let mut changed = Vec::new();
    let mut scores_ok = true;
    for term in 1..=TERMS {
        if Instant::now() >= deadline {
            return Err("ten-minute study window expired".into());
        }
        let visit = learner.advance();
        let comparison = baseline.advance();
        let answer = &learner.history().last().ok_or("empty learner history")?["response"];
        let comparison_answer =
            &baseline.history().last().ok_or("empty reference history")?["response"];
        if answer != comparison_answer {
            changed.push(term);
        }
        trajectory.push(json!({"term":term,"day":visit.day,"attended":visit.attended,
            "independent_correct":visit.independent_correct,"baseline_independent_correct":comparison.independent_correct,
            "response":answer,"baseline_response":comparison_answer}));
        let appointment = at_visit(mode, &chosen, term, visit.attended, &mut handled);
        let mut step = TraceStep {
            term,
            day: visit.day,
            attended: visit.attended,
            called: false,
            specificity: appointment.map(|s| s.specificity),
            validated: false,
            displayed: false,
            effect: false,
        };
        if let Some(ref sample) = appointment {
            let id = format!("{}-{}", mode.label(), sample.ordinal);
            let planned = chosen[usize::from(sample.ordinal - 1)].term;
            let mut report = json!({"slot":id,"scheduled_term":planned,"visit":term,"day":sample.day,"specificity":sample.specificity,"status":"missed"});
            if visit.attended {
                let ctx = context(learner.history(), sample, &previous, mode)?;
                let focus = match sample.ordinal {
                    1 => Focus::Route,
                    2 => Focus::Worked,
                    3 => Focus::Transfer,
                    _ => return Err("unrecognized insertion".into()),
                };
                let score =
                    hybrid_cases::run(focus, &ctx, DEFAULT_JEV, DEFAULT_LUNA, |role, request| {
                        if step.called {
                            return Err("insertion attempted a second model request".into());
                        }
                        gate.admit(term, visit.day, visit.attended, Some(sample.specificity))?;
                        step.called = true;
                        exchange(&id, role, request)
                    })?;
                if !step.called {
                    return Err("no model observation for attended appointment".into());
                }
                step.validated = score["passed"] == true;
                scores_ok &= step.validated;
                if step.validated {
                    gate.allow_display(term, visit.day, visit.attended, sample.specificity, true)?;
                }
                let mut delivered = match deliver(&mut learner, &score) {
                    Ok(receipt) => receipt,
                    Err(reason) => {
                        scores_ok = false;
                        report["withheld_reason"] = json!(reason);
                        None
                    }
                };
                step.effect = delivered.is_some();
                if step.validated
                    && matches!(
                        score["selected"].as_str(),
                        Some("continue_without_coaching")
                    )
                {
                    delivered = Some(
                        json!({"kind":"guidance","day":visit.day,"display":score["display"],
                        "assessed":false,"durability_credit":false}),
                    );
                }
                step.displayed = delivered.is_some();
                let receipt = delivered.as_ref().unwrap_or(&Value::Null);
                previous.push(
                    json!({"day":visit.day,"selected":score["selected"],"validated":step.validated,
                    "delivered":step.displayed,"lesson":score["verified_lesson"],
                    "display":receipt.get("display").or_else(||receipt.get("prompt")),
                    "kind":receipt["kind"],"judgment":receipt["judgment"],"assisted":receipt["assisted"],
                    "skipped":receipt["skipped"],"assessed":receipt["assessed"],
                    "quarters":receipt["quarters"],"remaining_eighths":receipt["remaining_eighths"],
                    "response":receipt["response"]}),
                );
                report["context"] = ctx;
                report["score"] = score;
                report["delivery"] = json!(delivered);
                report["status"] = json!(if step.displayed {
                    "delivered"
                } else if !report["withheld_reason"].is_null() {
                    "withheld"
                } else if step.validated {
                    "held"
                } else {
                    "rejected"
                });
            }
            insertions.push(report);
        }
        gate.observe(step)?;
        trace.push(step);
    }
    gate.finish()?;
    let observations = trace.iter().filter(|s| s.called).count();
    Ok(
        json!({"mode":mode.label(),"provenance":provenance,"seed":seed,"learner":LEARNER,
        "elapsed_simulated_days":trajectory.last().and_then(|t|t["day"].as_u64()),"observations":observations,
        "displayed_interventions":trace.iter().filter(|s|s.displayed).count(),"temporal_verification":"passed",
        "passed":scores_ok,"changed_answer_terms":changed,"trajectory":trajectory,"insertions":insertions}),
    )
}

pub fn offline(mode: Mode, seed: u64) -> Result<Value, String> {
    let mut result = run(
        mode,
        seed,
        "scripted_fixture",
        Instant::now() + Duration::from_secs(600),
        |_, role, req| hybrid_cases::scripted(role, req),
    )?;
    result["provider_calls"] = json!(0);
    Ok(result)
}

fn read_observation(path: &Path, slot: &str, role: Role, request: &Value) -> Result<Value, String> {
    let record = load_report(path)?;
    if record["format"] != "tutor-hybrid/1"
        || record["slot"] != slot
        || record["role"] != role_name(role)
        || &record["request"] != request
    {
        return Err(format!(
            "{slot}: saved observation belongs to a different visit or request"
        ));
    }
    Ok(public_reply(role, request, &record["reply"]))
}

enum CaptureError {
    LostObservation(String),
    Campaign(String),
}

impl From<String> for CaptureError {
    fn from(message: String) -> Self {
        Self::Campaign(message)
    }
}

fn capture(
    dir: &Path,
    campaign: &mut Campaign<'_>,
    slot: &str,
    role: Role,
    request: &Value,
) -> Result<Value, CaptureError> {
    let path = dir.join(format!("{slot}.json"));
    let pending = dir.join(format!("{slot}.pending"));
    let settled = campaign.request_billed(role, request)?;
    let complete = path.try_exists().map_err(io_error)?;
    let partial = pending.try_exists().map_err(io_error)?;
    if settled {
        if !complete && !partial {
            return Err(CaptureError::LostObservation(format!("{slot}: settled charge but no surviving output; cannot continue this learner history")));
        }
        let source = if complete { &path } else { &pending };
        let result =
            read_observation(source, slot, role, request).map_err(CaptureError::LostObservation)?;
        if !complete {
            fs::OpenOptions::new()
                .write(true)
                .open(&pending)
                .and_then(|f| f.sync_all())
                .map_err(io_error)?;
            finalize_report(dir, &pending, &path)?;
        }
        return Ok(result);
    }
    if complete || partial {
        return Err(format!(
            "{slot}: saved output without matching charge; stopping before dispatch"
        )
        .into());
    }
    let response = campaign.complete(role, request)?;
    let reply = public_reply(role, request, &response);
    let record = json!({"format":"tutor-hybrid/1","slot":slot,"role":role_name(role),"request":request,"reply":reply});
    write_report(dir, &pending, &path, &record)?;
    Ok(reply)
}

pub fn suite_live(dir: &Path, budget: u64, credit_only: bool) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(600);
    let config = Config::from_env()?;
    fs::create_dir_all(dir).map_err(io_error)?;
    let mut campaign = Campaign::open_suite(&dir.join("meter.json"), &config, budget)?;
    if credit_only {
        campaign.confirm_credit_only_account();
    }
    let mut tests = run_short(dir, &mut campaign)?;
    let histories = run_live_histories(deadline, |slot, role, request| {
        capture(dir, &mut campaign, slot, role, request)
    })?;
    tests.extend(histories);
    Ok(json!({"mode":"suite_live","tests":tests,"meter":campaign.summary()}))
}

fn run_live_histories(
    deadline: Instant,
    mut exchange: impl FnMut(&str, Role, &Value) -> Result<Value, CaptureError>,
) -> Result<Vec<Value>, String> {
    let mut tests = Vec::new();
    for mode in [Mode::Sparse, Mode::Finishing] {
        let mut has_lost_observation = false;
        let result = run(
            mode,
            STUDY_SEED,
            "openrouter",
            deadline,
            |slot, role, request| {
                exchange(slot, role, request).map_err(|error| match error {
                    CaptureError::LostObservation(message) => {
                        has_lost_observation = true;
                        message
                    }
                    CaptureError::Campaign(message) => message,
                })
            },
        );
        tests.push(match result {
            Ok(run) => run,
            Err(message) if has_lost_observation => json!({"mode":mode.label(),"passed":false,
                "status":"paid_observation_unavailable","error":message}),
            Err(error) => return Err(error),
        });
    }
    Ok(tests)
}

pub fn suite_replay(dir: &Path) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut tests = replay_short(dir)?;
    for mode in [Mode::Sparse, Mode::Finishing] {
        let observed = run(
            mode,
            STUDY_SEED,
            "saved_reply",
            deadline,
            |slot, role, request| {
                let path = dir.join(format!("{slot}.json"));
                let source = if path.try_exists().map_err(io_error)? {
                    path
                } else {
                    dir.join(format!("{slot}.pending"))
                };
                read_observation(&source, slot, role, request)
            },
        );
        let result = observed.unwrap_or_else(|error| {
            json!({"mode":mode.label(), "passed":false, "status":"observation_unavailable", "error":error})
        });
        tests.push(result);
    }
    Ok(json!({"mode":"suite_replay","provider_calls":0,"tests":tests}))
}

/// Predeclare a consecutive group so that a favorable learner cannot be selected after the run.
pub fn compare(first: u64, count: u32) -> Result<Value, String> {
    if count == 0 || first.checked_add(u64::from(count - 1)).is_none() {
        return Err("the consecutive seed range must be nonempty and fit in u64".into());
    }
    let mut models = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(600);
    for mode in [Mode::Sparse, Mode::Finishing] {
        let mut fixed_attended = 0_u64;
        let mut evaluated = 0_u64;
        let mut graded_better = 0_u64;
        let mut graded_worse = 0_u64;
        let mut changed_people = 0;
        let mut reached_transfer = 0;
        let mut withheld = 0_u64;
        let mut rejected = 0_u64;
        let mut delivered = 0_u64;
        for offset in 0..count {
            let seed = first + u64::from(offset);
            let result = run(mode, seed, "scripted_fixture", deadline, |_, role, req| {
                hybrid_cases::scripted(role, req)
            })?;
            let trajectory = result["trajectory"]
                .as_array()
                .ok_or("missing simulated visits")?;
            let planned = schedule(mode, seed, LEARNER);
            fixed_attended += planned
                .iter()
                .filter(|s| trajectory[s.term as usize - 1]["attended"] == true)
                .count() as u64;
            let reports = result["insertions"]
                .as_array()
                .ok_or("missing insertion trace")?;
            evaluated += reports.iter().filter(|r| r["score"].is_object()).count() as u64;
            withheld += reports.iter().filter(|r| r["status"] == "withheld").count() as u64;
            rejected += reports.iter().filter(|r| r["status"] == "rejected").count() as u64;
            delivered += reports
                .iter()
                .filter(|r| r["status"] == "delivered")
                .count() as u64;
            reached_transfer += u64::from(
                reports
                    .iter()
                    .any(|r| r["score"]["focus"] == "transfer" && r["status"] == "delivered"),
            );
            changed_people += u64::from(
                trajectory
                    .iter()
                    .any(|r| r["response"] != r["baseline_response"]),
            );
            for visit in trajectory {
                match (
                    visit["independent_correct"].as_bool(),
                    visit["baseline_independent_correct"].as_bool(),
                ) {
                    (Some(true), Some(false)) => graded_better += 1,
                    (Some(false), Some(true)) => graded_worse += 1,
                    _ => {}
                }
            }
        }
        models.push(json!({"mode":mode.label(),"scheduled_opportunities":u64::from(count)*3,
            "attended_on_original_day":fixed_attended,"actually_evaluated":evaluated,"displayed":delivered,
            "withheld":withheld,"rejected":rejected,"learners_receiving_transfer":reached_transfer,
            "learners_with_changed_later_answer":changed_people,"later_independent_gains":graded_better,
            "later_independent_losses":graded_worse}));
    }
    Ok(
        json!({"mode":"compare","provenance":"scripted_fixture","provider_calls":0,"first_seed":first,
        "seeds":count,"terms_per_learner":TERMS,"results":models}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_responses_leave_the_paired_learner_trajectory_intact() {
        for mode in [Mode::Sparse, Mode::Finishing] {
            let result = run(
                mode,
                STUDY_SEED,
                "invalid_fixture",
                Instant::now() + Duration::from_secs(2),
                |_, _, _| Ok(json!({})),
            )
            .unwrap();
            assert_eq!(result["temporal_verification"], "passed");
            assert_eq!(result["passed"], false);
            assert_eq!(result["displayed_interventions"], 0);
            assert_eq!(result["observations"], 3);
            assert!(result["changed_answer_terms"]
                .as_array()
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn replay_uses_the_exact_public_request_at_the_original_point_in_time() {
        let mode = Mode::Finishing;
        let mut observations = Vec::new();
        let result = run(
            mode,
            STUDY_SEED,
            "recorded_fixture",
            Instant::now() + Duration::from_secs(2),
            |slot, role, request| {
                let reply = hybrid_cases::scripted(role, request)?;
                observations.push((slot.to_owned(), role, request.clone(), reply.clone()));
                Ok(reply)
            },
        )
        .unwrap();
        assert_eq!(observations.len(), 3);
        let replay = |rows: &[(String, Role, Value, Value)]| {
            run(
                mode,
                STUDY_SEED,
                "recorded_fixture",
                Instant::now() + Duration::from_secs(2),
                |slot, role, request| {
                    let record = rows
                        .iter()
                        .find(|(s, r, q, _)| s == slot && *r == role && q == request)
                        .ok_or("missing exact request")?;
                    Ok(record.3.clone())
                },
            )
        };
        assert_eq!(result, replay(&observations).unwrap());
        let inserts: Vec<_> = result["insertions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["status"] != "missed")
            .collect();
        let lengths: Vec<_> = inserts
            .iter()
            .map(|r| r["context"]["history"].as_array().unwrap().len())
            .collect();
        assert_eq!(lengths, [0, 1, 3]);
        for r in inserts {
            let day = r["day"].as_u64().unwrap();
            assert!(r["context"]["history"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["day"].as_u64().unwrap() < day));
        }
        observations[2].2["messages"][1]["content"] = json!("history from a different visit");
        assert!(replay(&observations).is_err());
    }

    #[test]
    fn a_settled_lost_observation_only_stops_the_dependent_history() {
        for unsettled in [false, true] {
            let mut finishing = 0;
            let histories = run_live_histories(
                Instant::now() + Duration::from_secs(3),
                |slot, role, req| {
                    if slot == "sparse-2" {
                        return Err(if unsettled {
                            CaptureError::Campaign("unknown charge".into())
                        } else {
                            CaptureError::LostObservation("lost paid response".into())
                        });
                    }
                    assert_ne!(slot, "sparse-3");
                    finishing += usize::from(slot.starts_with("finishing-"));
                    hybrid_cases::scripted(role, req).map_err(CaptureError::from)
                },
            );
            if unsettled {
                assert!(histories.is_err());
                assert_eq!(finishing, 0);
            } else {
                let results = histories.unwrap();
                assert_eq!(results[0]["status"], "paid_observation_unavailable");
                assert_eq!(results[1]["passed"], true);
                assert_eq!(finishing, 3);
            }
        }
    }

    #[test]
    fn elapsed_wall_time_aborts_before_any_model_opportunity() {
        assert!(run(
            Mode::Sparse,
            STUDY_SEED,
            "scripted_fixture",
            Instant::now(),
            |_, _, _| panic!("expired dispatch")
        )
        .is_err());
    }
}
