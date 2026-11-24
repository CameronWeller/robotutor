//! Audit a complete paired teaching_trial directory. Reads stage packets, not printed summaries.
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    protocol::Gate,
    response_loop::{assessment_route, AssessmentChoice, AssessmentRoute},
};

type Check<T> = Result<T, String>;
const FOCUS: &[&str] = &[
    "condition",
    "reasoning",
    "source",
    "observation",
    "choice",
    "unknown",
];
const MOVES: &[&str] = &["explain", "practice", "probe", "clarify", "defer", "stop"];
const MEMORY: &[&str] = &["focus", "move", "goal", "help", "family"];
const TOPIC_MEMORY: &[&str] = &["focus", "move", "goal", "help", "family", "topic"];
const MATERIAL: &[&str] = &["lesson", "worked", "practice", "excerpt"];
const ASSESSMENT_CHOICES: &[&str] = &["submit", "private_practice", "skip"];
const NEW_GOAL_POLICY:&str="Help the learner scope the newly chosen goal. Use their current request to ask one useful intake question, offer to receive a sample, or stop. Let the learner decide the objective before prescribing an exercise.";
const NEW_GOAL_LIMIT: &str =
    "No exercise or assessment criterion for this new goal has been released for this visit.";
const PROFILES: &[&str] = &[
    "available",
    "withdrawal",
    "familiar_form",
    "stop",
    "private_answer",
    "example_only",
    "teacher_queue",
    "goal_switch",
    "due_choice",
    "observation_gap",
    "forget_topic",
    "assisted_history",
    "long_gap",
];

fn condition_for(trial: u64, domain: &str, evidence_mode: &Value) -> Check<Value> {
    require((1..=150).contains(&trial), "trial outside study")?;
    let block = (trial - 1) / PROFILES.len() as u64;
    let slot = (trial - 1) % PROFILES.len() as u64;
    let id = PROFILES[slot as usize];
    let queue = id == "teacher_queue";
    let setting = if queue || (block + slot) % 2 == 1 {
        "classroom"
    } else {
        "one-to-one"
    };
    let days = if id == "long_gap" {
        [730, 1825][(block % 2) as usize]
    } else {
        [7, 30, 180, 730, 1825][((block + slot) % 5) as usize]
    };
    Ok(
        json!({"version":1,"id":id,"domain":domain,"evidence_mode":evidence_mode,"setting":setting,"days":days,
        "queue_ahead":if queue { if block%2==0 {3} else {1} } else {0},
        "teacher_minutes":if queue {10} else {0},"review_minutes":if queue {3} else {0}}),
    )
}

fn local(r: &mut Value, display: &str, movement: &str, focus: &str) {
    r["local"] = json!({"display":display,"move":movement,"focus":focus});
    r["task"] = json!(false);
    r["answer"] = json!(false);
    r["learner_attempt"] = json!(false);
    r["actions"] = json!([movement]);
    r["material"] = json!([]);
}
fn unassessed(r: &mut Value, material: &[&str], actions: &[&str]) {
    r["material"] = json!(material);
    r["actions"] = json!(actions);
    r["learner_attempt"] = json!(false);
    r["task"] = json!(false);
}

fn rules_for(condition: &Value, returning: bool) -> Check<Value> {
    let id = string(&condition["id"], "condition")?;
    let day = if returning {
        condition["days"].clone()
    } else {
        json!(0)
    };
    let mut r = json!({"context":{"setting":condition["setting"],"day":day,"evidence_mode":condition["evidence_mode"]},
        "task":true,"answer":true,"learner_attempt":true,"material":MATERIAL,"actions":MOVES,"memory":MEMORY,"local":null});
    let guided = &["explain", "clarify", "defer", "stop"];
    let clarify = &["clarify", "defer", "stop"];
    let observed = match (id, returning) {
        ("private_answer", false) => {
            r["memory"] = json!([]);
            local(
                &mut r,
                "Your response can stay private. We can return to a shared exercise another time.",
                "defer",
                "choice",
            );
            "The learner kept the first response and clarification private.".to_owned()
        }
        ("private_answer", true) => {
            r["memory"] = json!([]);
            "The learner permits this new shared exercise. No prior private answer is available."
                .into()
        }
        ("assisted_history", false) => {
            r["actions"] = json!(["explain", "practice", "clarify", "defer", "stop"]);
            "A worked demonstration was supplied before the learner submitted this response.".into()
        }
        ("assisted_history", true) => {
            r["material"] = json!(["lesson", "excerpt"]);
            r["actions"] = json!(["practice", "probe", "clarify", "defer", "stop"]);
            "The earlier response followed supplied help. Offer the fresh task before advancing."
                .into()
        }
        ("forget_topic", true) => {
            r["memory"] = json!([]);
            "The learner withdrew permission to use earlier teaching metadata.".into()
        }
        ("withdrawal", true) => {
            local(
                &mut r,
                "The source for this exercise is on hold. We can leave the review open.",
                "defer",
                "source",
            );
            "Permission for the return exercise's source was withdrawn.".into()
        }
        ("familiar_form", true) => {
            unassessed(&mut r, &["lesson", "worked", "excerpt"], guided);
            "Every released check for this target has already been seen. Guided review remains available.".into()
        }
        ("stop", true) => {
            r["memory"] = json!([]);
            local(&mut r, "We can stop here.", "stop", "choice");
            "The learner explicitly ended today's meeting before any activity or model dispatch."
                .into()
        }
        ("example_only", true) => {
            unassessed(
                &mut r,
                &["worked", "excerpt"],
                &["explain", "defer", "stop"],
            );
            "The learner requested one reviewed example and no further questions today.".into()
        }
        ("teacher_queue", true) => {
            let ahead = condition["queue_ahead"].as_u64().ok_or("queue length")?;
            let minutes = condition["teacher_minutes"]
                .as_u64()
                .ok_or("remaining minutes")?;
            let duration = condition["review_minutes"]
                .as_u64()
                .ok_or("review duration")?;
            if (ahead + 1) * duration <= minutes {
                local(&mut r,"A teacher review fits this meeting. The exercise can wait for that conversation.","defer","choice");
            } else {
                unassessed(
                    &mut r,
                    &["lesson", "worked", "practice"],
                    &["explain", "practice", "defer", "stop"],
                );
            }
            format!("The learner asked for an individual review. {ahead} requests are ahead, with {minutes} minutes left and {duration} minutes per review.")
        }
        ("goal_switch", true) => {
            r["memory"] = json!([]);
            unassessed(&mut r, &[], clarify);
            "The learner replaced the previous aim. The old booking has been released; the new aim has no reviewed exercise yet.".into()
        }
        ("due_choice", true) => {
            unassessed(&mut r, &[], clarify);
            "An earlier review is due. The learner first wants to choose between that review and today's other goal.".into()
        }
        ("observation_gap", true) => {
            unassessed(&mut r, &["lesson", "worked", "excerpt"], guided);
            let need = match condition["evidence_mode"].as_str() {
                Some("written_artifact") => "the actual written artifact",
                Some("quantity") => "a numerical submission",
                Some("witnessed_performance") => "a witnessed performance",
                _ => return Err("unknown evidence mode".into()),
            };
            format!("The learner described practice. The record lacks {need}; only self-report is observable.")
        }
        ("long_gap", true) => {
            r["memory"] = json!(["goal"]);
            unassessed(
                &mut r,
                &["lesson", "excerpt"],
                &["clarify", "explain", "defer", "stop"],
            );
            "Intervening study and exposure are unknown. Confirm the current goal before scheduling an independent check.".into()
        }
        _ => {
            "The reviewed exercise and source are available. Only the submitted text is observable."
                .into()
        }
    };
    r["context"]["observed"] = json!(observed);
    Ok(r)
}

fn observed_case(case: &Value, rules: &Value) -> Value {
    let mut seen = case.clone();
    let fields = seen.as_object_mut().expect("validated case");
    if rules["opening"]["answer"] == false {
        fields.remove("first_answer");
    }
    if rules["feedback"]["answer"] == false {
        fields.remove("surprise");
        fields.remove("topic_selection");
        fields.remove("memory_choices");
        fields.remove("selected_focus");
    }
    seen
}

fn read(file: &Path) -> Check<Value> {
    let bytes = fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))
}
fn require(ok: bool, message: &str) -> Check<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn string<'a>(v: &'a Value, label: &str) -> Check<&'a str> {
    v.as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{label} requires nonempty text"))
}
fn selected<'a>(v: &'a Value, legal: &[&str]) -> Check<Vec<&'a str>> {
    let fields = v.as_array().ok_or("expected array of field names")?;
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    for field in fields {
        let name = string(field, "field")?;
        require(
            legal.contains(&name) && seen.insert(name),
            "unknown or duplicate field",
        )?;
        names.push(name);
    }
    Ok(names)
}
fn pick(from: &Value, keys: &[&str]) -> Check<Value> {
    let mut data = Map::new();
    for key in keys {
        data.insert(
            (*key).into(),
            from.get(*key)
                .ok_or_else(|| format!("missing {key}"))?
                .clone(),
        );
    }
    Ok(Value::Object(data))
}

const OPTIONAL_MEMORY: [&str; 4] = ["goal", "move", "help", "family"];

fn memory_options() -> Value {
    json!({
        "goal":"Keep the broad course goal shown for this visit.",
        "move":"Keep the kind of teaching response I received.",
        "help":"Keep whether this activity recorded supplied help or left it unknown.",
        "family":"Keep the identifier of the kind of exercise I tried."
    })
}

fn authorized_booking(
    case: &Value,
    available: &Value,
    requested: &[&str],
    rules: &Value,
    private: bool,
) -> Check<Value> {
    let choices = selected(&case["memory_choices"], &OPTIONAL_MEMORY)?;
    let canonical: Vec<_> = OPTIONAL_MEMORY
        .into_iter()
        .filter(|k| choices.contains(k))
        .collect();
    require(
        canonical == choices,
        "saved memory choices differ from validated option order",
    )?;
    let focus = case.get("selected_focus").ok_or("missing focus choice")?;
    require(
        focus.is_null() || focus.as_str().is_some_and(|s| FOCUS.contains(&s)),
        "unknown focus choice",
    )?;
    require(
        !private || (choices.is_empty() && focus.is_null() && case["topic_selection"] == "none"),
        "private encounter retained an explicit grant",
    )?;
    let permitted = selected(&rules["feedback"]["memory"], TOPIC_MEMORY)?;
    let mut record = Map::new();
    for &key in requested.iter().filter(|k| permitted.contains(k)) {
        let value = match key {
            "focus" if !focus.is_null() => focus,
            "topic" if case["topic_selection"] != "none" => {
                available.get(key).ok_or("unbound selected topic")?
            }
            k if choices.contains(&k) => {
                let value = &available[k];
                let valid = match k {
                    "goal" => value.as_str().is_some_and(|s| {
                        !s.trim().is_empty()
                            && s.chars().count() <= 200
                            && !s.chars().any(char::is_control)
                    }),
                    "move" => value.as_str().is_some_and(|s| MOVES.contains(&s)),
                    "help" => value == "unknown" || value == "supplied",
                    "family" => value
                        .as_u64()
                        .is_some_and(|id| (1..=u64::from(u32::MAX)).contains(&id)),
                    _ => false,
                };
                require(valid, "invalid value for authorized memory field")?;
                value
            }
            _ => continue,
        };
        record.insert(key.to_owned(), value.clone());
    }
    Ok(Value::Object(record))
}
fn exact(v: &Value, keys: &[&str]) -> Check<()> {
    require(*v == pick(v, keys)?, "missing or additional object fields")
}

const PAIRED_STAGES: [&str; 4] = ["opening", "feedback", "return", "resume"];
const SELECTION: &[&str] = &["first_unit", "return_unit", "contrast_material"];

fn present(receipt: &Value, channel: &str, feature: &str, stage: &str) -> Check<bool> {
    require(receipt.is_object(), "missing actual teaching receipt")?;
    if channel == "move" {
        return Ok(string(&receipt["response"]["move"], "executed move")? == feature);
    }
    if receipt["origin"] == "local"
        || (channel == "memory" && !matches!(stage, "return" | "resume"))
    {
        return Ok(false);
    }
    if channel == "selection" {
        require(
            matches!(receipt["origin"].as_str(), Some("model" | "fallback")),
            "unknown lesson dispatch origin",
        )?;
        let packet = &receipt["packet"];
        if feature == "contrast_material" {
            return match packet.get("comparison_material") {
                None => Ok(false),
                Some(fields) => fields
                    .as_object()
                    .map(|m| !m.is_empty())
                    .ok_or("malformed comparison material".into()),
            };
        }
        if let Some(route) = packet.get("lesson_route") {
            require(
                matches!(route["slot"].as_str(), Some("first" | "return")),
                "malformed lesson selection",
            )?;
        }
        return Ok(match feature {
            "first_unit" => packet["lesson_route"]["slot"] == "first",
            "return_unit" => packet["lesson_route"]["slot"] == "return",
            _ => return Err("unknown lesson selection feature".into()),
        });
    }
    let slot = if channel == "material" {
        "material"
    } else {
        "receipt"
    };
    let sent = receipt["packet"][slot]
        .as_object()
        .ok_or("missing dispatched fields")?;
    if let Some(value) = sent.get(feature) {
        require(
            !value.is_null() && value != "",
            "dispatched field has no value",
        )?;
        return Ok(true);
    }
    Ok(false)
}

fn witness(hypothesis: &Value, candidate: &str, receipts: &Value) -> Check<Value> {
    exact(hypothesis, &["stage", "channel", "feature", "direction"])?;
    let stage = string(&hypothesis["stage"], "witness stage")?;
    let channel = string(&hypothesis["channel"], "witness channel")?;
    let feature = string(&hypothesis["feature"], "witness feature")?;
    require(PAIRED_STAGES.contains(&stage), "unsupported witness stage")?;
    let fields = match channel {
        "memory" if matches!(stage, "return" | "resume") => TOPIC_MEMORY,
        "selection" if matches!(stage, "return" | "resume") => SELECTION,
        "material" => MATERIAL,
        "move" => MOVES,
        _ => return Err("invalid channel or stage".into()),
    };
    require(
        fields.contains(&feature),
        "witness references unknown feature",
    )?;
    let candidate_only = match hypothesis["direction"].as_str() {
        Some("candidate_only") => true,
        Some("incumbent_only") => false,
        _ => return Err("witness direction".into()),
    };
    let has = |name: &str| -> Check<bool> {
        let receipt = receipts[name].get(stage).ok_or("missing witness receipt")?;
        present(receipt, channel, feature, stage)
    };
    let observed =
        json!({"candidate":has(candidate)?,"incumbent":has(if candidate=="A" {"B"} else {"A"})?});
    let expected = json!({"candidate":candidate_only,"incumbent":!candidate_only});
    Ok(
        json!({"hypothesis":hypothesis,"observed":observed,"expected":expected,"met":observed==expected}),
    )
}

struct PairedVisit {
    selected_stage: bool,
    candidate_only: bool,
    candidate_present: bool,
    incumbent_present: bool,
}

fn prediction_monitor(offset: usize) -> Monitor<PairedVisit> {
    let mut p = Predicates::builder();
    let stage = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |v: &PairedVisit| v.selected_stage,
        "preregistered stage",
    )));
    let direction = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |v: &PairedVisit| v.candidate_only,
        "candidate-only direction",
    )));
    let candidate = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |v: &PairedVisit| v.candidate_present,
        "candidate executed feature",
    )));
    let incumbent = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |v: &PairedVisit| v.incumbent_present,
        "incumbent executed feature",
    )));
    let directed = Property::Or(
        Box::new(direction.clone() & candidate.clone() & !incumbent.clone()),
        Box::new(!direction & !candidate & incumbent),
    );
    let mut law = stage & directed;
    for _ in 0..offset {
        law = Property::next(law);
    }
    let predicates = p.build();
    let automaton = law.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

/// Uses the preregistration and every executed teaching receipt in the paired word.
fn temporal_witness(h: &Value, candidate: &str, receipts: &Value) -> Check<bool> {
    let target = string(&h["stage"], "preregistered stage")?;
    let offset = PAIRED_STAGES
        .iter()
        .position(|s| *s == target)
        .ok_or("unknown stage")?;
    let channel = string(&h["channel"], "preregistered channel")?;
    let feature = string(&h["feature"], "preregistered feature")?;
    let candidate_only = h["direction"] == "candidate_only";
    let incumbent = if candidate == "A" { "B" } else { "A" };
    let mut law = prediction_monitor(offset);
    let mut met = true;
    let resumed = receipts["A"].get("resume").is_some() || receipts["B"].get("resume").is_some();
    require(
        target != "resume" || resumed,
        "absent predicted later visit",
    )?;
    for stage in PAIRED_STAGES.into_iter().take(if resumed { 4 } else { 3 }) {
        let observed = |arm: &str| {
            let receipt = receipts[arm].get(stage).ok_or("incomplete paired word")?;
            present(receipt, channel, feature, stage)
        };
        let visit = PairedVisit {
            selected_stage: stage == target,
            candidate_only,
            candidate_present: observed(candidate)?,
            incumbent_present: observed(incumbent)?,
        };
        met = met && law.next_state(&visit);
    }
    // The absorbing end has no dispatch. X^offset has resolved by the final visit, so
    // every further repetition has the same verdict.
    let end = PairedVisit {
        selected_stage: false,
        candidate_only,
        candidate_present: false,
        incumbent_present: false,
    };
    Ok(met && law.next_state(&end))
}

fn course(domain: &str) -> Check<(Value, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/goals");
    let reviews = read(&dir.join("reviews.json"))?;
    let pin = string(&reviews[domain], "reviewed domain")?;
    let raw = fs::read_to_string(dir.join(format!("{domain}.json"))).map_err(|e| e.to_string())?;
    let normalized = raw.replace("\r\n", "\n");
    require(
        format!("{:x}", Sha256::digest(normalized.as_bytes())) == pin,
        "teaching pack differs from reviewed edition",
    )?;
    let pack = serde_json::from_str(&normalized).map_err(|e| e.to_string())?;
    Ok((pack, pin.to_owned()))
}

fn topic_menu(course: &Value, domain: &str, edition: &str) -> Check<Value> {
    let units = course["proposal"]["units"]
        .as_array()
        .ok_or("missing reviewed units")?;
    let mut menu = json!({"none":null});
    for (name, pack) in [("first", "first_pack"), ("return", "return_pack")] {
        let unit = course[pack]["unit"]
            .as_u64()
            .filter(|u| *u > 0 && *u <= u64::from(u32::MAX))
            .ok_or("invalid topic unit")?;
        let mut found = units.iter().filter(|entry| entry["id"] == unit);
        let title = string(
            &found.next().ok_or("topic was not released")?["title"],
            "released title",
        )?;
        require(
            found.next().is_none() && title.trim().chars().count() <= 200,
            "ambiguous or oversized topic title",
        )?;
        menu[name] = json!({"domain":domain,"unit":unit,"edition":edition,"title":title});
    }
    require(
        menu["first"]["unit"] != menu["return"]["unit"],
        "duplicate topic options",
    )?;
    Ok(menu)
}

fn topic_rules(mut rule: Value, allow: bool) -> Value {
    let memory = rule["memory"].as_array_mut().expect("authoritative rules");
    if allow && memory.contains(&json!("focus")) {
        memory.push(json!("topic"));
    }
    rule
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum Stage {
    Opening,
    Feedback,
    Return,
    Learner,
    Resume,
}

struct Frame {
    stage: Stage,
    matches_packet: bool,
    legal_response: bool,
}

fn monitor(resumed: bool) -> Monitor<Frame> {
    let mut p = Predicates::builder();
    let good = p.new_predicate(ClosurePredicate::new(
        |f: &Frame| f.matches_packet && f.legal_response,
        "exact reviewed packet and typed response",
    ));
    let feedback = p.new_predicate(ClosurePredicate::new(
        |f: &Frame| f.stage == Stage::Feedback,
        "feedback",
    ));
    let returned = p.new_predicate(ClosurePredicate::new(
        |f: &Frame| f.stage == Stage::Return,
        "return with checked receipt",
    ));
    let feedback = Property::atomic(feedback);
    let follows = Property::Or(
        Box::new(!feedback.clone()),
        Box::new(feedback & Property::next(Property::atomic(returned))),
    );
    let mut invariant = Property::atomic(good) & follows;
    if resumed {
        let learner = Property::atomic(p.new_predicate(ClosurePredicate::new(
            |f: &Frame| f.stage == Stage::Learner,
            "second-visit assessment decision",
        )));
        let resume = Property::atomic(p.new_predicate(ClosurePredicate::new(
            |f: &Frame| f.stage == Stage::Resume,
            "later learner-initiated visit",
        )));
        invariant = invariant
            & Property::Or(
                Box::new(!learner.clone()),
                Box::new(learner & Property::next(resume)),
            );
    }
    let law = Property::always(invariant);
    let predicates = p.build();
    let automaton = law.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

fn legal_turn(stage: Stage, v: &Value) -> bool {
    if stage == Stage::Learner {
        return exact(v, &["response"]).is_ok() && string(&v["response"], "modeled answer").is_ok();
    }
    exact(v, &["move", "focus", "display"]).is_ok()
        && string(&v["display"], "display").is_ok()
        && v["move"].as_str().is_some_and(|s| MOVES.contains(&s))
        && v["focus"].as_str().is_some_and(|s| FOCUS.contains(&s))
}

fn filter_fields(keys: &[&str], legal: &Value, from: &Value, names: &[&str]) -> Check<Value> {
    let allowed = selected(legal, names)?;
    let admitted: Vec<_> = keys
        .iter()
        .copied()
        .filter(|k| allowed.contains(k))
        .collect();
    pick(from, &admitted)
}

fn restrict(packet: &mut Value, rule: &Value, material: &Value, retrieve: &[&str]) -> Check<()> {
    let data = packet.as_object_mut().ok_or("missing packet")?;
    if rule["task"] != true {
        data.remove("task");
        data.remove("new_task");
    }
    if rule["answer"] != true {
        data.remove("answer");
        if data.contains_key("displayed") {
            data.remove("learner");
        }
    }
    data.insert(
        "material".into(),
        filter_fields(retrieve, &rule["material"], material, MATERIAL)?,
    );
    data.insert("condition".into(), rule["context"].clone());
    data.insert("allowed_moves".into(), rule["actions"].clone());
    Ok(())
}

#[cfg(test)]
fn arm(
    course: &Value,
    case: &Value,
    variant: &Value,
    stages: &[Value; 4],
    condition: Option<&Value>,
) -> Check<Value> {
    arm_with_topics(course, case, variant, stages, condition, None, None, None)
}

fn carry_memory(
    stored: &Value,
    condition: &Value,
    snapshot: Option<&Value>,
    vocabulary: &[&str],
) -> Check<Value> {
    let retained = if let Some(saved) = snapshot {
        let id = string(&condition["condition"]["id"], "visit condition")?;
        let erase = matches!(id, "forget_topic" | "goal_switch");
        let expected = json!({"format":"paired-return-memory/1","stored":if erase{json!({})}else{stored.clone()},"reason":if erase{id}else{"carry"}});
        require(
            *saved == expected,
            "persisted return memory differs from current authorization",
        )?;
        &saved["stored"]
    } else {
        stored
    };
    project_memory(
        retained,
        &condition["stages"]["return"]["memory"],
        vocabulary,
    )
}

fn project_memory(retained: &Value, permitted: &Value, vocabulary: &[&str]) -> Check<Value> {
    let mut result = json!({});
    for key in selected(permitted, vocabulary)? {
        if let Some(value) = retained.get(key) {
            result[key] = value.clone();
        }
    }
    Ok(result)
}

const INTENTS: &[&str] = &["keep", "cancel_topic", "forget_all", "change_goal"];

fn return_state(intent: &Value, profile: &Value, original: &Value) -> Check<(Value, &'static str)> {
    let intent = string(intent, "learner return intent")?;
    let allowed = match profile.as_str() {
        Some("withdrawal") => intent == "cancel_topic",
        Some("forget_topic") => intent == "forget_all",
        Some("goal_switch") => intent == "change_goal",
        _ => matches!(intent, "keep" | "cancel_topic"),
    };
    require(allowed, "learner intent does not match the assigned visit")?;
    let mut carried = original.clone();
    let (reason, confirmation) = match intent {
        "keep" => ("carry", "Your earlier sharing choices still apply."),
        "cancel_topic" => {
            carried
                .as_object_mut()
                .ok_or("invalid original booking")?
                .remove("topic");
            ("cancel_topic", "No lesson topic is booked now.")
        }
        "forget_all" | "change_goal" => {
            carried = json!({});
            (
                if intent == "forget_all" {
                    "forget_topic"
                } else {
                    "goal_switch"
                },
                "No earlier teaching context is carried into this visit.",
            )
        }
        _ => unreachable!(),
    };
    Ok((
        json!({"format":"paired-return-memory/1","reason":reason,"stored":carried}),
        confirmation,
    ))
}

fn visit_choices(case: &Value, stages: &Value) -> Check<()> {
    let mut previous = selected(&case["memory_choices"], &OPTIONAL_MEMORY)?;
    if !case["selected_focus"].is_null() {
        previous.push("focus");
    }
    if case["topic_selection"] != "none" && case["return_intent"] != "cancel_topic" {
        previous.push("topic");
    }
    if matches!(
        case["return_intent"].as_str(),
        Some("forget_all" | "change_goal")
    ) {
        previous.clear();
    }
    for (stage, key) in [("return", "return_fields"), ("resume", "resume_fields")] {
        let allowed = selected(&case[key], TOPIC_MEMORY)?;
        require(
            allowed
                == TOPIC_MEMORY
                    .iter()
                    .copied()
                    .filter(|k| allowed.contains(k))
                    .collect::<Vec<_>>(),
            "saved grants differ from validated option order",
        )?;
        require(
            allowed.iter().all(|field| previous.contains(field)),
            "visit reauthorizes an ungranted, cancelled or erased field",
        )?;
        if stage == "resume" {
            require(
                case["resume_keep"] == !allowed.is_empty(),
                "later keep flag differs from chosen fields",
            )?;
        }
        require(
            allowed.is_empty() || stages.get(stage).is_some_and(|s| s["local"].is_null()),
            "fields authorized for absent or local teaching",
        )?;
    }
    Ok(())
}

fn visit_use_note(projected: &Value) -> String {
    let labels = [
        "chosen focus",
        "recorded teaching move",
        "course goal",
        "recorded assistance",
        "exercise family",
        "booked lesson",
    ];
    let names: Vec<_> = TOPIC_MEMORY
        .iter()
        .zip(labels)
        .filter_map(|(&key, label)| projected.get(key).map(|_| label))
        .collect();
    if names.is_empty() {
        "No earlier personal context was made available for this activity.".into()
    } else {
        format!(
            "Earlier context made available for this activity includes {}.",
            names.join(", ")
        )
    }
}

fn assessment_form(
    packet: &mut Value,
    choice: &Value,
    rule: &Value,
    release: &Value,
) -> Check<(AssessmentRoute, Value)> {
    let option = match choice.as_str() {
        Some("submit") => AssessmentChoice::Submit,
        Some("private_practice") => AssessmentChoice::PrivatePractice,
        Some("skip") => AssessmentChoice::Skip,
        _ => return Err("unknown assessment choice".into()),
    };
    let eligible = rule["learner_attempt"]
        .as_bool()
        .ok_or("missing assessment permission")?;
    require(
        eligible || option == AssessmentChoice::Skip,
        "collection selected for unavailable assessment",
    )?;
    let route = assessment_route(
        if eligible {
            Gate::Supported
        } else {
            Gate::Unavailable
        },
        option,
    );
    let mut reference = Value::Null;
    match route {
        AssessmentRoute::Submitted => {
            if release["show_source_during_check"] == true {
                reference = pick(release, &["unit", "locator", "excerpt"])?;
                packet["required_reference"] = reference.clone();
            }
        }
        AssessmentRoute::NotOffered => {
            *packet = json!({"eligible":false,"condition":rule["context"]})
        }
        AssessmentRoute::PrivatePractice | AssessmentRoute::Skipped => {
            *packet = json!({"eligible":true,"choice":choice,"condition":rule["context"]});
        }
    }
    Ok((route, reference))
}

fn assessment_status(route: AssessmentRoute) -> (&'static str, Option<&'static str>) {
    match route {
        AssessmentRoute::Submitted => ("submitted", None),
        AssessmentRoute::NotOffered => ("not_offered", Some("No assessment offered.")),
        AssessmentRoute::PrivatePractice => (
            "private_practice",
            Some("Private practice chosen. No answer was collected."),
        ),
        AssessmentRoute::Skipped => (
            "skipped",
            Some("Assessment skipped. The check remains open."),
        ),
    }
}

fn past_designs(history: &Value, trial: u64) -> Check<()> {
    let history = history.as_array().ok_or("no same-domain history")?;
    let count = ((trial - 1) / 5).min(4);
    require(
        history.len() == count as usize,
        "incomplete same-domain history",
    )?;
    for (i, item) in history.iter().enumerate() {
        exact(
            item,
            &[
                "trial",
                "change",
                "memory",
                "retrieve",
                "resume_mode",
                "measured",
                "adopted",
                "verdict",
            ],
        )?;
        require(
            item["trial"] == trial - 5 * (count - i as u64),
            "nonconsecutive same-domain history",
        )?;
        require(
            string(&item["change"], "earlier policy change")?
                .chars()
                .count()
                <= 650,
            "oversized earlier policy change",
        )?;
        selected(&item["memory"], TOPIC_MEMORY)?;
        selected(&item["retrieve"], MATERIAL)?;
        require(
            matches!(
                item["resume_mode"].as_str(),
                Some("booked" | "compare" | "current")
            ) && matches!(
                item["verdict"].as_str(),
                Some("supported" | "unobserved" | "refuted" | "unscored")
            ) && ["measured", "adopted"]
                .iter()
                .all(|k| item[*k].is_boolean() || item[*k].is_null()),
            "invalid earlier policy summary",
        )?;
    }
    Ok(())
}

fn resumed_memory(
    keep: bool,
    retained: &Value,
    rule: &Value,
    returned: &Value,
    chosen: Option<&Value>,
) -> Check<Value> {
    let mut stored = if keep {
        project_memory(retained, &rule["memory"], TOPIC_MEMORY)?
    } else {
        json!({})
    };
    if let Some(chosen) = chosen {
        stored = project_memory(&stored, chosen, TOPIC_MEMORY)?;
    }
    if stored.get("move").is_some() {
        stored["move"] = returned["move"].clone();
    }
    Ok(json!({"format":"paired-resume-memory/1","stored":stored}))
}

fn later_rule(c: &Value) -> Value {
    let memory = if matches!(c["id"].as_str(), Some("goal_switch" | "forget_topic")) {
        json!([])
    } else {
        json!(TOPIC_MEMORY)
    };
    json!({"context":{"setting":c["setting"],"evidence_mode":c["evidence_mode"],
        "day":c["days"].as_u64().expect("canonical condition")+30,
        "observed":"The learner returned to the original goal on their own initiative. A new grant permits both reviewed practice releases and the learner's newly shared submission for this visit."},
        "material":MATERIAL,"actions":MOVES,"memory":memory,"task":true,"answer":true,"learner_attempt":false,"local":null})
}

fn resolved_pack<'a>(
    packet: &mut Value,
    course: &'a Value,
    menu: &Value,
    rule: &Value,
    retrieve: &[&str],
    mode: Option<&str>,
) -> Check<&'a Value> {
    let mut slot = "return";
    if let Some(topic) = packet["receipt"].get("topic") {
        let mut matches = ["first", "return"].into_iter().filter(|s| {
            pick(&menu[*s], &["domain", "unit", "edition"]).is_ok_and(|id| &id == topic)
        });
        slot = matches
            .next()
            .ok_or("retained topic has no released lesson")?;
        require(matches.next().is_none(), "ambiguous released lesson")?;
        packet["booked_lesson"] = menu[slot].clone();
    }
    if let Some(mode) = mode {
        require(
            ["current", "booked", "compare"].contains(&mode),
            "unknown lesson routing mode",
        )?;
        if mode != "booked" {
            slot = "return";
        }
        let compare = mode == "compare";
        packet["lesson_route"] = json!({"mode":mode,"slot":slot,"unit":menu[slot]["unit"],
            "comparison_unit":if compare {menu["first"]["unit"].clone()} else {Value::Null}});
        packet["comparison_material"] = if compare {
            filter_fields(retrieve, &rule["material"], &course["first_pack"], MATERIAL)?
        } else {
            json!({})
        };
    }
    Ok(&course[if slot == "first" {
        "first_pack"
    } else {
        "return_pack"
    }])
}

fn has_text(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(values) => values.iter().any(|v| has_text(v, needle)),
        Value::Object(fields) => fields.values().any(|v| has_text(v, needle)),
        _ => false,
    }
}

fn planned_visit(condition: &Value) -> Check<Value> {
    let mut stages = json!({});
    for name in PAIRED_STAGES
        .into_iter()
        .filter(|s| condition["stages"].get(*s).is_some())
    {
        let rule = &condition["stages"][name];
        stages[name] = pick(rule, &["actions", "memory", "material"])?;
        stages[name]["model"] = json!(rule["local"].is_null());
    }
    Ok(
        json!({"condition":condition["condition"]["id"],"shared_work_on_return":condition["stages"]["return"]["learner_attempt"],"stages":stages}),
    )
}

// Immutable V5 registry snapshot. Pin the UTF-8 wording so a future case cannot ride in a
// nominally allowed agenda field. Changing these definitions requires a new study edition.
fn verify_research_focus(focus: &Value, trial: u64, domain: &str) -> Check<()> {
    const PINS: [&str; 9] = [
        "47eace073cb07904f015f34bf2a33a84e73de73ed4d766cd5a48557ee18654d4",
        "400eb3450accf58e2b974a058e4fa795e3420360e74f13d15fb75d33ef1da840",
        "c7216f44272d00cc2c758b03ddabc3ede69dd248dba386a898b7325553e71c74",
        "d76a412c8f3ca935464f8a91060dc478c9fc1e047f4ee0ca5fcc3d131d554848",
        "a59b50f57a6cc0eef446038e5f0e163b0c6b6b9cbc4a467739f84352e920ce6f",
        "e8891fb5728b084e1156cc7786896c41c51baa5328de6f48fb0fd3890c855f18",
        "d9d788dc3bb7294227b1076fd68f01498ff463f2d3adbf102edbda6a8ae297fb",
        "7bf358325b6927731fc7bedeb73b519b3cace6baf078ae74fb9f1000399bd8ba",
        "da695e1d5e2897e58bafd5e7c05a6890c00a128f10c547ab792dd69ca9ab914c",
    ];
    let domain_pin = match domain {
        "chemical-engineering" => {
            "630cada42b327643a6437c9bf6f692ebdf1b7d8230f269a278d2a6ecc45733ca"
        }
        "writer" => "a8264d0a25a2fffa5a1350e934f85b901314bf4801e46674a48e8a464cad33c6",
        "violin" => "46ab61f5452f58dbeac76ae7706d83c4a16ef3bbec71c3f4bc33adb64adba7d9",
        "better-person" => "a991f420d2659c1f3fb25a0698b582ce8dad5950a5b5b9e749435f91deac87a6",
        "worship" => "33685a20cb062a8bdb9e9c1532cccbdb08a79ff61d0d1ec6325d7c166ac383d5",
        _ => return Err("unknown research domain".into()),
    };
    let fields = [
        "id",
        "research_question",
        "compare",
        "inspect",
        "domain_probe",
    ];
    exact(focus, &fields)?;
    let mut words = Vec::new();
    for field in &fields[..4] {
        words.push(string(&focus[*field], field)?);
    }
    let digest = |text: &str| format!("{:x}", Sha256::digest(text.as_bytes()));
    let slot = trial.checked_sub(1).ok_or("invalid trial")? % 9;
    require(
        digest(&words.join("\0")) == PINS[slot as usize]
            && digest(string(&focus["domain_probe"], "domain probe")?) == domain_pin,
        "research agenda differs from the preregistered slot or domain",
    )
}

fn arm_with_topics(
    course: &Value,
    case: &Value,
    variant: &Value,
    stages: &[Value; 4],
    condition: Option<&Value>,
    menu: Option<&Value>,
    return_snapshot: Option<&Value>,
    resume: Option<(&Value, &Value)>,
) -> Check<Value> {
    let conditions = condition.map(|c| &c["stages"]);
    let v8 = case.get("resume_keep").is_some();
    let mut keys = vec!["teaching", "returning", "memory", "retrieve"];
    let mut mode = "booked";
    if v8 && variant.get("resume_mode").is_some() {
        keys.push("resume_mode");
        mode = string(&variant["resume_mode"], "lesson routing policy")?;
        require(
            ["current", "booked", "compare"].contains(&mode),
            "invalid lesson routing policy",
        )?;
    }
    exact(variant, &keys)?;
    string(&variant["teaching"], "teaching policy")?;
    string(&variant["returning"], "return policy")?;
    let retrieve = selected(&variant["retrieve"], MATERIAL)?;
    let vocabulary = if menu.is_some() { TOPIC_MEMORY } else { MEMORY };
    let memory_keys = selected(&variant["memory"], vocabulary)?;
    let (first, fresh, goal) = (
        &course["first_pack"],
        &course["return_pack"],
        &course["scope"]["goal"],
    );
    require(
        string(&first["check"], "first task")? != string(&fresh["check"], "return task")?,
        "reviewed return repeats the first task",
    )?;
    let opening = &stages[0]["response"];
    let feedback = &stages[1]["response"];
    let returned = &stages[2]["response"];
    let learner = &stages[3]["response"];

    let mut first_packet = json!({"goal":goal,"evidence_limit":course["limit"],"policy":variant["teaching"],
        "task":first["check"],"answer":case["first_answer"],"material":pick(first, &retrieve)?});
    let mut feedback_packet = first_packet.clone();
    feedback_packet["displayed"] = opening["display"].clone();
    feedback_packet["learner"] = case["surprise"].clone();
    let help = if condition.is_some_and(|c| c["condition"]["id"] == "assisted_history") {
        "supplied"
    } else {
        "unknown"
    };
    let mut values = json!({"focus":feedback["focus"],"move":feedback["move"],"goal":goal,
        "help":help,"family":first["unit"]});
    let mut title = None;
    if let Some(offers) = menu {
        let slot = string(&case["topic_selection"], "explicit topic selection")?;
        let offer = offers.get(slot).ok_or("topic was not offered")?;
        let private = condition.is_some_and(|c| c["condition"]["id"] == "private_answer");
        require(
            case["save_topic"] == !offer.is_null() && (!private || offer.is_null()),
            "invalid consent for selection",
        )?;
        let booking = if offer.is_null() {
            Value::Null
        } else {
            title = Some(string(&offer["title"], "topic title")?);
            let id = pick(offer, &["domain", "edition", "unit"])?;
            values["topic"] = id.clone();
            id
        };
        if conditions.is_some_and(|r| r["feedback"]["answer"] == true) {
            feedback_packet["approved_topic"] = booking;
        }
    }
    let stored = if case.get("memory_choices").is_some() {
        authorized_booking(
            case,
            &values,
            &memory_keys,
            conditions.ok_or("missing consent rules")?,
            condition.is_some_and(|c| c["condition"]["id"] == "private_answer"),
        )?
    } else {
        match case["save_topic"].as_bool() {
            Some(true) => match conditions {
                Some(rules) => {
                    let eligible = selected(&rules["feedback"]["memory"], vocabulary)?;
                    let carried: Vec<_> = memory_keys
                        .iter()
                        .copied()
                        .filter(|key| eligible.contains(key) && values.get(*key).is_some())
                        .collect();
                    pick(&values, &carried)?
                }
                None => pick(&values, &memory_keys)?,
            },
            Some(false) => json!({}),
            None => return Err("case requires an explicit sharing choice".into()),
        }
    };
    let mut return_confirmation = None;
    let mut memory = if v8 {
        let c = condition.ok_or("missing visit condition")?;
        let (state, text) = return_state(&case["return_intent"], &c["condition"]["id"], &stored)?;
        let saved = return_snapshot.ok_or("missing persisted return state")?;
        require(
            *saved == state,
            "return intent did not update durable state",
        )?;
        return_confirmation = Some(text);
        project_memory(
            &saved["stored"],
            &c["stages"]["return"]["memory"],
            vocabulary,
        )?
    } else if let Some(c) = condition {
        carry_memory(&stored, c, return_snapshot, vocabulary)?
    } else {
        stored.clone()
    };
    if let Some(chosen) = case.get("return_fields") {
        memory = project_memory(&memory, chosen, TOPIC_MEMORY)?;
    }
    let mut return_packet = json!({"goal":goal,"evidence_limit":course["limit"],"policy":variant["returning"],
        "days":case["days"],"learner":case["return_request"],"receipt":memory,
        "new_task":fresh["check"],"material":pick(fresh, &retrieve)?});
    let mut participant_packet = json!({"learner_history":case,"goal":goal,
        "earlier_instruction":[opening["display"],feedback["display"]],"new_task":fresh["check"],
        "current_instruction":returned["display"]});
    let confirmation = if stored.get("topic").is_some() {
        format!(
            "Saved lesson topic: {}",
            title.ok_or("missing stored topic title")?
        )
    } else {
        "No lesson topic was saved.".to_owned()
    };
    if menu.is_some() {
        participant_packet["earlier_instruction"]
            .as_array_mut()
            .unwrap()
            .push(json!(confirmation));
    }
    if let Some(rules) = conditions {
        restrict(&mut first_packet, &rules["opening"], first, &retrieve)?;
        restrict(&mut feedback_packet, &rules["feedback"], first, &retrieve)?;
        let teaching_pack = if let Some(options) = menu {
            resolved_pack(
                &mut return_packet,
                course,
                options,
                &rules["return"],
                &retrieve,
                (v8 && resume.is_none()).then_some(mode),
            )?
        } else {
            fresh
        };
        restrict(
            &mut return_packet,
            &rules["return"],
            teaching_pack,
            &retrieve,
        )?;
        if return_snapshot.is_some() {
            if rules["return"]["task"] == true {
                let practice = string(&fresh["practice"], "reviewed return practice")?;
                require(
                    practice != fresh["check"],
                    "practice repeats the withheld assessment",
                )?;
                return_packet["new_task"] = json!(practice);
            }
            if rules["return"]["learner_attempt"] == true {
                return_packet["shared_work"] = case["return_work"].clone();
            }
        }
        if condition.is_some_and(|c| c["condition"]["id"] == "goal_switch") {
            return_packet.as_object_mut().unwrap().remove("goal");
            if case.get("assessment_choice").is_some() {
                return_packet["policy"] = json!(NEW_GOAL_POLICY);
                return_packet["evidence_limit"] = json!(NEW_GOAL_LIMIT);
            }
        }
        participant_packet["learner_history"] = observed_case(case, rules);
        if rules["return"]["learner_attempt"] == false {
            participant_packet = json!({"eligible":false,"condition":rules["return"]["context"]});
        }
    }
    let assessment = if let Some(choice) = case.get("assessment_choice") {
        Some(assessment_form(
            &mut participant_packet,
            choice,
            &conditions.ok_or("missing assessment condition")?["return"],
            fresh,
        )?)
    } else {
        None
    };
    let mut visits = vec![
        (Stage::Opening, first_packet, &stages[0]),
        (Stage::Feedback, feedback_packet, &stages[1]),
        (Stage::Return, return_packet, &stages[2]),
        (Stage::Learner, participant_packet, &stages[3]),
    ];
    if let Some((state, receipt)) = resume {
        let rule = &conditions.ok_or("missing later conditions")?["resume"];
        let retained = &return_snapshot.ok_or("missing durable return memory")?["stored"];
        let expected = resumed_memory(
            case["resume_keep"].as_bool().ok_or("missing later grant")?,
            retained,
            rule,
            returned,
            case.get("resume_fields"),
        )?;
        require(
            *state == expected,
            "resumed memory repopulated or incorrectly updated",
        )?;
        let mut packet = json!({"goal":goal,"evidence_limit":course["limit"],"policy":variant["returning"],
            "days":rule["context"]["day"],"learner":case["resume_request"],"receipt":state["stored"],
            "new_task":fresh["practice"],"shared_work":case["resume_work"]});
        let pack = resolved_pack(
            &mut packet,
            course,
            menu.ok_or("missing later menu")?,
            rule,
            &retrieve,
            Some(mode),
        )?;
        restrict(&mut packet, rule, pack, &retrieve)?;
        visits.push((Stage::Resume, packet, receipt));
    }
    let mut safety = monitor(resume.is_some());
    for (stage, expected, receipt) in visits {
        let response = &receipt["response"];
        let admitted = match (stage, conditions) {
            (Stage::Learner, Some(_)) if assessment.is_some() => {
                match assessment_status(assessment.as_ref().unwrap().0).1 {
                    Some(text) => local_receipt(receipt) && *response == json!({"response":text}),
                    None => {
                        live_receipt(receipt)
                            && (receipt.get("origin").is_none() || receipt["origin"] == "model")
                    }
                }
            }
            (Stage::Learner, Some(rules)) if rules["return"]["learner_attempt"] == false => {
                local_receipt(receipt) && *response == json!({"response":"No assessment offered."})
            }
            (Stage::Learner, Some(_)) => {
                live_receipt(receipt)
                    && (receipt.get("origin").is_none() || receipt["origin"] == "model")
            }
            (_, None) => true,
            (visit, Some(rules)) => {
                let name = match visit {
                    Stage::Opening => "opening",
                    Stage::Feedback => "feedback",
                    Stage::Return => "return",
                    Stage::Resume => "resume",
                    _ => unreachable!(),
                };
                let rule = &rules[name];
                let menu = selected(&rule["actions"], MOVES)?;
                let allowed = response["move"].as_str().is_some_and(|m| menu.contains(&m));
                allowed
                    && if rule["local"].is_null() {
                        match receipt["origin"].as_str() {
                            Some("model") => {
                                live_receipt(receipt) && receipt.get("proposed_response").is_none()
                            }
                            Some("fallback") => {
                                let draft = &receipt["proposed_response"];
                                live_receipt(receipt)
                                    && legal_turn(visit, draft)
                                    && draft["move"].as_str().is_some_and(|m| !menu.contains(&m))
                                    && *response
                                        == json!({"display":"We can leave this activity open for another visit.","move":"defer","focus":"choice"})
                            }
                            _ => false,
                        }
                    } else {
                        local_receipt(receipt) && response == &rule["local"]
                    }
            }
        };
        let prior_visible = return_snapshot.is_some() && stage != Stage::Learner;
        let held_check = fresh["check"].as_str().ok_or("missing assessment")?;
        let sealed = !prior_visible
            || (!has_text(&receipt["packet"], held_check)
                && !has_text(&receipt["response"], held_check));
        let frame = Frame {
            stage,
            matches_packet: sealed && receipt.get("packet") == Some(&expected),
            legal_response: admitted && legal_turn(stage, response),
        };
        if !safety.next_state(&frame) {
            return Err(format!(
                "{stage:?}: packet binding, response type or temporal order failed"
            ));
        }
    }
    let mut trace = json!({"opening":opening,"feedback":feedback,"return":returned,"memory":memory,"modeled_attempt":learner["response"]});
    if let Some((route, reference)) = assessment {
        trace["assessment_route"] = json!(assessment_status(route).0);
        trace["assessment_reference"] = reference;
    }
    if menu.is_some() {
        trace["stored"] = stored;
        trace["confirmation"] = json!(confirmation);
    }
    if let Some(text) = return_confirmation {
        trace["return_confirmation"] = json!(text);
    }
    if case.get("return_fields").is_some() {
        trace["return_use_note"] = json!(visit_use_note(&memory));
    }
    if let Some((state, receipt)) = resume {
        trace["resume_memory"] = state["stored"].clone();
        trace["resume"] = receipt["response"].clone();
        if case.get("resume_fields").is_some() {
            trace["resume_use_note"] = json!(visit_use_note(&state["stored"]));
        }
    }
    Ok(trace)
}

fn local_receipt(receipt: &Value) -> bool {
    exact(
        receipt,
        &[
            "origin",
            "seconds",
            "model_requested",
            "tokens",
            "packet",
            "response",
        ],
    )
    .is_ok()
        && receipt["origin"] == "local"
        && receipt["seconds"].as_f64() == Some(0.0)
        && receipt.get("model_requested") == Some(&Value::Null)
        && receipt.get("tokens") == Some(&Value::Null)
        && receipt.get("proposed_response").is_none()
}

fn live_receipt(receipt: &Value) -> bool {
    string(&receipt["model_requested"], "provider").is_ok()
        && receipt["seconds"].as_f64().is_some_and(|t| t >= 0.0)
        && receipt["request_hash"]
            .as_str()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn scoped_incumbent(source: &Value, policy: &Value, domain: &str, trial: u64) -> Check<()> {
    exact(source, &["scope", "origin", "trial", "sha256"])?;
    require(
        source["scope"] == domain,
        "incumbent belongs to another domain",
    )?;
    let digest = source["sha256"]
        .as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    let valid = match source["origin"].as_str() {
        Some("baseline") => {
            let baseline = json!({"teaching":"Help with the current task and then offer practice.",
                "returning":"Resume the goal on a different task using the available history.",
                "memory":["goal"],"retrieve":["excerpt","lesson"]});
            source["trial"].is_null() && source["sha256"].is_null() && *policy == baseline
        }
        Some("domain-adoption") => {
            digest
                && source["trial"]
                    .as_u64()
                    .is_some_and(|prior| prior > 0 && prior < trial && (trial - prior) % 5 == 0)
        }
        Some("external-seed") => digest && source["trial"].is_null(),
        _ => false,
    };
    require(valid, "invalid domain-scoped incumbent provenance")
}

fn audit(dir: &Path) -> Check<()> {
    let stage = |name: &str| read(&dir.join(format!("{name}.json")));
    let decision = stage("decision")?;
    let design = stage("design")?;
    let case_receipt = stage("case")?;
    let version = match decision.get("protocol") {
        None => 1,
        Some(v) => v.as_u64().ok_or("invalid protocol field")?,
    };
    require((1..=10).contains(&version), "unknown teaching protocol")?;
    let grounded = version >= 2;
    let review = stage(if grounded {
        "review-grounded"
    } else {
        "review"
    })?;
    let domain = string(&decision["domain"], "decision domain")?;
    let (course, edition) = course(domain)?;
    let topics = if version >= 4 {
        Some(topic_menu(&course, domain, &edition)?)
    } else {
        None
    };
    let condition = if version >= 3 {
        let trial = decision["trial"].as_u64().ok_or("missing trial ID")?;
        let expected = condition_for(trial, domain, &course["evidence_mode"])?;
        let domains = [
            "writer",
            "chemical-engineering",
            "violin",
            "better-person",
            "worship",
        ];
        require(
            domain == domains[((trial - 1) % 5) as usize]
                && decision["candidate"] == ["A", "B"][(trial % 2) as usize],
            "domain or candidate arm differs from trial assignment",
        )?;
        let opening = topic_rules(rules_for(&expected, false)?, version >= 4);
        let returning = topic_rules(
            rules_for(&expected, true)?,
            version >= 4 && expected["id"] != "withdrawal",
        );
        let mut stages = json!({"opening":opening,"feedback":opening,"return":returning});
        if version >= 10 && expected["id"] == "goal_switch" {
            stages["return"]["context"]["evidence_mode"] = json!("unscoped");
        }
        if version >= 8 && returning["learner_attempt"] == false {
            stages["resume"] = later_rule(&expected);
        }
        let expected = json!({"condition":expected,"stages":stages});
        require(
            stage("condition")? == expected,
            "saved condition or stage boundaries differ from the trial assignment",
        )?;
        Some(expected)
    } else {
        None
    };
    let resumed = condition
        .as_ref()
        .is_some_and(|c| c["stages"].get("resume").is_some());
    let effective_stage = if resumed { "resume" } else { "return" };
    let design_packet = &design["packet"];
    exact(
        design_packet,
        &[
            "domain",
            "goal",
            "incumbent",
            "last_finding",
            "evidence_mode",
        ],
    )?;
    if version >= 3 {
        let prior = &design_packet["last_finding"];
        let mut keys = vec![
            "last_review",
            "recent_questions",
            "possible_operating_conditions",
        ];
        if version >= 5 {
            keys.extend(["planned_visit", "research_focus"]);
            require(
                prior["planned_visit"]
                    == planned_visit(condition.as_ref().ok_or("missing condition")?)?,
                "prospective permissions differ from stage rules",
            )?;
            verify_research_focus(
                &prior["research_focus"],
                decision["trial"].as_u64().ok_or("missing trial")?,
                domain,
            )?;
        }
        if version >= 6 {
            keys.extend(["previous_contrast", "domain_predecessor"]);
            let trial = decision["trial"].as_u64().ok_or("missing trial")?;
            let previous = &prior["domain_predecessor"];
            if trial > 5 {
                exact(previous, &["trial", "next_question", "witness"])?;
                require(
                    previous["trial"] == trial - 5,
                    "invalid preceding domain trial",
                )?;
                string(&previous["next_question"], "preceding research question")?;
            } else {
                require(previous.is_null(), "unexpected preceding domain trial")?;
            }
            // The study reporter binds the contents to earlier audited directories.
            for contrast in [&prior["previous_contrast"], &previous["witness"]] {
                if !contrast.is_null() {
                    exact(contrast, &["hypothesis", "expected", "observed", "met"])?;
                    exact(
                        &contrast["hypothesis"],
                        &["stage", "channel", "feature", "direction"],
                    )?;
                    for slot in ["expected", "observed"] {
                        exact(&contrast[slot], &["candidate", "incumbent"])?;
                        require(
                            contrast[slot]["candidate"].is_boolean()
                                && contrast[slot]["incumbent"].is_boolean(),
                            "invalid previous contrast operands",
                        )?;
                    }
                    require(
                        contrast["met"] == (contrast["observed"] == contrast["expected"]),
                        "inconsistent previous contrast",
                    )?;
                }
            }
        }
        if version >= 7 {
            keys.push("incumbent_source");
            scoped_incumbent(
                &prior["incumbent_source"],
                &design_packet["incumbent"],
                domain,
                decision["trial"].as_u64().ok_or("missing trial")?,
            )?;
            require(
                !dir.join("parent-correction.json").exists(),
                "global correction on a domain-scoped trial",
            )?;
        }
        if version >= 8 {
            keys.push("effective_stage");
            require(
                prior["effective_stage"] == effective_stage,
                "wrong final teaching stage",
            )?;
        }
        if version >= 9 {
            keys.push("visit_use_options");
            require(
                prior["visit_use_options"] == json!(TOPIC_MEMORY),
                "incorrect designer visit use options",
            )?;
        }
        if version >= 10 {
            keys.extend(["assessment_choices", "recent_domain_designs"]);
            require(
                prior["assessment_choices"] == json!(ASSESSMENT_CHOICES),
                "designer assessment menu differs",
            )?;
            past_designs(
                &prior["recent_domain_designs"],
                decision["trial"].as_u64().ok_or("missing trial number")?,
            )?;
        }
        exact(prior, &keys)?;
        require(
            prior["possible_operating_conditions"] == json!(PROFILES)
                && prior["recent_questions"].as_array().is_some_and(|arr| {
                    arr.len() <= 6 && arr.iter().all(|q| string(q, "prior question").is_ok())
                }),
            "design history contains an invalid condition catalog or question list",
        )?;
    }
    require(
        design_packet["domain"] == domain
            && design_packet["goal"] == course["scope"]["goal"]
            && design_packet["evidence_mode"] == course["evidence_mode"],
        "design domain or edition drift",
    )?;
    let proposal = &design["response"];
    if version >= 8 {
        let policy = &proposal["policy"];
        let incumbent = &design_packet["incumbent"];
        let requested = |p: &Value, key: &str, legal: &[&str]| -> Check<BTreeSet<String>> {
            Ok(selected(&p[key], legal)?
                .into_iter()
                .map(str::to_owned)
                .collect())
        };
        let changed = policy["resume_mode"]
            .as_str()
            .ok_or("candidate requires explicit lesson routing")?
            != incumbent["resume_mode"].as_str().unwrap_or("booked")
            || requested(policy, "retrieve", MATERIAL)?
                != requested(incumbent, "retrieve", MATERIAL)?
            || requested(policy, "memory", TOPIC_MEMORY)?
                != requested(incumbent, "memory", TOPIC_MEMORY)?;
        require(
            changed,
            "proposal made no routing, retrieval or memory change",
        )?;
        require(
            proposal["hypothesis"]["stage"] == effective_stage,
            "prediction targets the wrong visit",
        )?;
    } else {
        require(
            proposal["hypothesis"]["channel"] != "selection"
                && proposal["hypothesis"]["stage"] != "resume",
            "new prediction vocabulary in an older protocol",
        )?;
    }
    let case_packet = &case_receipt["packet"];
    let mut blind_case = json!({"goal": course["scope"]["goal"],"first_task": course["first_pack"]["check"],"evidence_mode":course["evidence_mode"]});
    if let Some(v) = &condition {
        blind_case["condition"] = v["condition"].clone();
    }
    if let Some(menu) = &topics {
        blind_case["offered_topics"] = menu.clone();
    }
    if version >= 6 {
        blind_case["memory_options"] = memory_options();
        blind_case["focus_options"] = json!(FOCUS);
    }
    if version >= 7 {
        blind_case["study_protocol"] = json!(version);
    }
    if version >= 10 {
        blind_case["assessment_choices"] = json!(ASSESSMENT_CHOICES);
    }
    if version >= 9 {
        blind_case["visit_use_options"] = json!(TOPIC_MEMORY);
    }
    if version >= 8 {
        blind_case["later_practice"] = if resumed {
            course["return_pack"]["practice"].clone()
        } else {
            Value::Null
        };
        blind_case["resume_day"] = if resumed {
            condition.as_ref().unwrap()["stages"]["resume"]["context"]["day"].clone()
        } else {
            Value::Null
        };
        blind_case["return_choices"] = json!(INTENTS);
    }
    if version >= 5 {
        let eligible = condition
            .as_ref()
            .is_some_and(|c| c["stages"]["return"]["learner_attempt"] == true);
        blind_case["return_practice"] = if eligible {
            course["return_pack"]["practice"].clone()
        } else {
            Value::Null
        };
    }
    let mut coupled_case = blind_case.clone();
    coupled_case["research_question"] = proposal["question"].clone();
    let coupled = version < 3 && case_packet == &coupled_case;
    require(
        coupled || case_packet == &blind_case,
        "case author did not receive the reviewed task",
    )?;
    let case = &case_receipt["response"];
    let mut case_fields = vec![
        "first_answer",
        "surprise",
        "return_request",
        "days",
        "save_topic",
    ];
    if version >= 4 {
        case_fields.push("topic_selection");
    }
    if version >= 6 {
        case_fields.extend(["memory_choices", "selected_focus"]);
    }
    if version >= 10 {
        case_fields.push("assessment_choice");
    }
    if version >= 9 {
        case_fields.extend(["return_fields", "resume_fields"]);
        visit_choices(
            case,
            &condition.as_ref().ok_or("missing visit rules")?["stages"],
        )?;
    }
    if version >= 8 {
        case_fields.extend([
            "resume_request",
            "resume_work",
            "resume_keep",
            "return_intent",
        ]);
        require(
            case["resume_keep"].is_boolean(),
            "later sharing choice must be explicit",
        )?;
        for key in ["resume_request", "resume_work"] {
            if resumed {
                require(
                    string(&case[key], key)?.chars().count() <= 650,
                    "oversized later submission",
                )?;
            } else {
                require(
                    case[key].is_null() && case["resume_keep"] == false,
                    "unexpected later submission or grant",
                )?;
            }
        }
    }
    if version >= 5 {
        case_fields.push("return_work");
        if blind_case["return_practice"].is_null() {
            require(
                case["return_work"].is_null(),
                "unexpected new learner work on ineligible visit",
            )?;
        } else {
            require(
                string(&case["return_work"], "shared return work")?
                    .chars()
                    .count()
                    <= 650,
                "oversized shared work",
            )?;
        }
    }
    exact(case, &case_fields)?;
    for key in ["first_answer", "surprise", "return_request"] {
        string(&case[key], key)?;
    }
    if let Some(v) = &condition {
        require(
            case["days"] == v["condition"]["days"],
            "case interval differs from assigned return day",
        )?;
    }
    require(
        case["days"]
            .as_u64()
            .is_some_and(|d| (1..=1825).contains(&d)),
        "invalid visit interval",
    )?;
    let candidate = string(&decision["candidate"], "candidate arm")?;
    require(["A", "B"].contains(&candidate), "invalid candidate arm")?;
    let variants = |name| {
        if name == candidate {
            &proposal["policy"]
        } else {
            &design_packet["incumbent"]
        }
    };
    let mut reviewed_traces = json!({});
    let mut retrieved = json!({});
    let mut receipts_by_arm = json!({});
    let mut tasks_at_dispatch = json!({});
    let mut shared_work = json!({});
    let mut later_work = json!({});
    let mut effective_material = json!({});
    for name in ["A", "B"] {
        let load = |suffix| stage(&format!("{name}-{suffix}"));
        let receipts = [
            load("opening")?,
            load("feedback")?,
            load("return")?,
            load("learner")?,
        ];
        let saved_return = if version >= 5 {
            Some(stage(&format!("{name}-return-memory"))?)
        } else {
            None
        };
        let later = if resumed {
            Some((load("resume-memory")?, load("resume")?))
        } else {
            None
        };
        if version >= 8 {
            require(
                !dir.join(format!("{name}-resume-learner.json")).exists(),
                "unexpected third-visit assessor",
            )?;
            if !resumed {
                require(
                    !dir.join(format!("{name}-resume.json")).exists()
                        && !dir.join(format!("{name}-resume-memory.json")).exists(),
                    "unplanned additional visit",
                )?;
            }
        }
        reviewed_traces[name] = arm_with_topics(
            &course,
            case,
            variants(name),
            &receipts,
            condition.as_ref(),
            topics.as_ref(),
            saved_return.as_ref(),
            later.as_ref().map(|(s, r)| (s, r)),
        )
        .map_err(|e| format!("arm {name}, {e}"))?;
        if version >= 4 {
            let booking = stage(&format!("{name}-booking"))?;
            require(
                booking
                    == json!({"format":"paired-booking/1","stored":reviewed_traces[name]["stored"]}),
                "persisted booking differs from first-visit storage",
            )?;
        }
        if version >= 5 {
            shared_work[name] = if receipts[2]["origin"] == "local" {
                Value::Null
            } else {
                receipts[2]["packet"]["shared_work"].clone()
            };
        }
        for (label, r) in ["opening", "feedback", "return"].into_iter().zip(&receipts) {
            receipts_by_arm[name][label] = r.clone();
            tasks_at_dispatch[name][label] = if r["origin"] == "local" {
                Value::Null
            } else {
                r["packet"]
                    .get("new_task")
                    .or_else(|| r["packet"].get("task"))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
        }
        retrieved[name] = json!({"opening": receipts[0]["packet"]["material"],
                                  "feedback": receipts[1]["packet"]["material"],
                                  "return": receipts[2]["packet"]["material"]});
        if version >= 8 {
            let packet = if let Some((_, r)) = &later {
                receipts_by_arm[name]["resume"] = r.clone();
                let p = &r["packet"];
                retrieved[name]["resume"] = p["material"].clone();
                tasks_at_dispatch[name]["resume"] = p["new_task"].clone();
                later_work[name] = p["shared_work"].clone();
                p
            } else {
                later_work[name] = Value::Null;
                &receipts[2]["packet"]
            };
            effective_material[name] = pick(packet, &["lesson_route", "comparison_material"])?;
        }
    }
    let mut reviewer_packet = json!({"claims":pick(proposal, &["question","prediction","falsifier"])?,
        "learner":case,"first_item":pick(&course["first_pack"], &["excerpt","check","rubric"])?,
        "return_item":pick(&course["return_pack"], &["excerpt","check","rubric"])?,"traces":reviewed_traces});
    if grounded {
        reviewer_packet["material_actually_retrieved"] = retrieved.clone();
    }
    if let Some(v) = &condition {
        reviewer_packet["condition"] = v["condition"].clone();
        reviewer_packet["learner"] = observed_case(case, &v["stages"]);
    }
    if version >= 8 {
        reviewer_packet["shared_work_at_resume"] = later_work;
        reviewer_packet["effective_lesson_material"] = effective_material;
    }
    let mut met = true;
    if version >= 4 {
        let h = &proposal["hypothesis"];
        let result = witness(h, candidate, &receipts_by_arm)?;
        let temporal = temporal_witness(h, candidate, &receipts_by_arm)?;
        require(
            result["met"] == temporal,
            "paired temporal verdict disagrees with receipt witness",
        )?;
        if version >= 5 {
            let stage = string(&h["stage"], "hypothesis stage")?;
            let partition = &design_packet["last_finding"]["planned_visit"]["stages"][stage];
            let channel = match h["channel"].as_str() {
                Some("move") => "actions",
                Some("material") => "material",
                Some("memory") => "memory",
                Some("selection") if version >= 8 => "selection",
                _ => return Err("unknown predicted channel".into()),
            };
            require(
                partition["model"] == true
                    && (channel == "selection"
                        || partition[channel]
                            .as_array()
                            .is_some_and(|items| items.contains(&h["feature"]))),
                "witness cannot dispatch under the prospective conditions",
            )?;
            reviewer_packet["shared_work_at_return"] = shared_work;
        }
        if version >= 8 && h["channel"] == "selection" && h["feature"] == "contrast_material" {
            let predicted = json!({"candidate":proposal["policy"]["resume_mode"]=="compare","incumbent":design_packet["incumbent"]["resume_mode"]=="compare"});
            require(
                result["expected"] == predicted,
                "comparison-material prediction differs from routing policies",
            )?;
        }
        if let Some(field) = match h["channel"].as_str() {
            Some("memory") => Some("memory"),
            Some("material") => Some("retrieve"),
            _ => None,
        } {
            let selected = |v: &Value| {
                v[field]
                    .as_array()
                    .is_some_and(|labels| labels.contains(&h["feature"]))
            };
            let predicted = json!({"candidate":selected(&proposal["policy"]),"incumbent":selected(&design_packet["incumbent"])});
            require(
                result["expected"] == predicted,
                "witness differs from designed policy contrasts",
            )?;
        }
        require(
            decision["witness"] == result,
            "decision witness differs from executed stage receipts",
        )?;
        met = temporal;
        if version == 4 {
            reviewer_packet["witness"] = result;
        }
        reviewer_packet["task_at_dispatch"] = tasks_at_dispatch;
    }
    require(
        review["packet"] == reviewer_packet && review["response"] == decision["review"],
        "review or decision differs from the observed arms",
    )?;
    let preferred = string(&decision["review"]["preferred"], "review choice")?;
    require(
        ["A", "B", "tie", "neither"].contains(&preferred),
        "invalid review choice",
    )?;
    let adopt = met
        && preferred == candidate
        && (!grounded || decision["review"]["prediction"] == "supported");
    if grounded {
        require(decision["adopted"] == adopt, "incorrect adoption receipt")?;
    }
    let next = if adopt {
        &proposal["policy"]
    } else {
        &design_packet["incumbent"]
    };
    require(
        decision["incumbent_after"] == *next,
        "incumbent diverged from review decision",
    )?;
    if dir.join("reassessment.json").exists() {
        let second = stage("review-grounded")?;
        let finding = stage("reassessment")?;
        reviewer_packet["material_actually_retrieved"] = retrieved;
        let supported = second["response"]["preferred"] == candidate
            && second["response"]["prediction"] == "supported";
        require(
            second["packet"] == reviewer_packet
                && finding["review"] == second["response"]
                && finding["adopted"] == supported
                && finding["incumbent_after"]
                    == *if supported {
                        &proposal["policy"]
                    } else {
                        &design_packet["incumbent"]
                    },
            "reassessment does not match observed teaching material, review or disposition",
        )?;
    }
    let boundaries = if resumed { 10 } else { 8 };
    let later_obligations = if resumed { 2 } else { 0 };
    println!("{boundaries} checked boundaries, 2 feedback-to-return obligations, {later_obligations} later-visit obligations. Case author received candidate question: {coupled}. Topic selected: {}.",case["save_topic"]);
    Ok(())
}

fn main() -> Result<(), String> {
    let directory = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: teaching_trace DIRECTORY (one completed numbered trial)")?;
    audit(&directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifth_protocol_checks_the_persisted_withdrawal_before_any_return_projection() {
        let previous =
            json!({"topic":{"domain":"writer","unit":101,"edition":"first"},"focus":"reasoning"});
        for profile in [
            "forget_topic",
            "goal_switch",
            "stop",
            "withdrawal",
            "available",
        ] {
            let erase = matches!(profile, "forget_topic" | "goal_switch");
            let data = if erase { json!({}) } else { previous.clone() };
            let snapshot = json!({"format":"paired-return-memory/1","stored":data,"reason":if erase{profile}else{"carry"}});
            let condition = json!({"condition":{"id":profile},"stages":{"return":{"memory":if profile=="available"{json!(TOPIC_MEMORY)}else{json!([])}}}});
            let projection =
                carry_memory(&previous, &condition, Some(&snapshot), TOPIC_MEMORY).unwrap();
            assert_eq!(
                projection,
                if profile == "available" {
                    previous.clone()
                } else {
                    json!({})
                }
            );
            let wrong = json!({"format":"paired-return-memory/1","stored":if erase{previous.clone()}else{json!({})},"reason":snapshot["reason"]});
            assert!(carry_memory(&previous, &condition, Some(&wrong), TOPIC_MEMORY).is_err());
        }
    }
    #[test]
    fn fifth_protocol_keeps_shared_practice_and_final_assessment_separate() {
        let (mut course, mut case, policy, mut stages) = fixture(false);
        let c = condition_for(1, "writer", &json!("written_artifact")).unwrap();
        let opening = topic_rules(rules_for(&c, false).unwrap(), true);
        let ret = topic_rules(rules_for(&c, true).unwrap(), true);
        let condition =
            json!({"condition":c,"stages":{"opening":opening,"feedback":opening,"return":ret}});
        course["return_pack"]["practice"] = json!("shared practice");
        case["return_work"] = json!("my current draft");
        case["topic_selection"] = json!("none");
        case["days"] = c["days"].clone();
        for s in &mut stages {
            s["origin"] = json!("model");
            s["model_requested"] = json!("fixture");
            s["request_hash"] = json!("a".repeat(64));
            s["seconds"] = json!(1);
        }
        for s in &mut stages[..2] {
            s["packet"]["allowed_moves"] = json!(MOVES);
            s["packet"]["condition"] = opening["context"].clone();
        }
        stages[1]["packet"]["approved_topic"] = Value::Null;
        stages[2]["packet"]["allowed_moves"] = json!(MOVES);
        stages[2]["packet"]["condition"] = ret["context"].clone();
        stages[2]["packet"]["days"] = c["days"].clone();
        stages[2]["packet"]["new_task"] = json!("shared practice");
        stages[2]["packet"]["shared_work"] = case["return_work"].clone();
        stages[3]["packet"]["learner_history"] = case.clone();
        stages[3]["packet"]["earlier_instruction"]
            .as_array_mut()
            .unwrap()
            .push(json!("No lesson topic was saved."));
        let menu = json!({"none":null});
        let saved = json!({"format":"paired-return-memory/1","stored":{},"reason":"carry"});
        let audit = |receipts: &[Value; 4]| {
            arm_with_topics(
                &course,
                &case,
                &policy,
                receipts,
                Some(&condition),
                Some(&menu),
                Some(&saved),
                None,
            )
        };
        assert!(audit(&stages).is_ok());
        for (index, field, value) in [
            (2, "new_task", "fresh task"),
            (2, "shared_work", "first answer"),
            (3, "new_task", "shared practice"),
        ] {
            let mut changed = stages.clone();
            changed[index]["packet"][field] = json!(value);
            assert!(audit(&changed).is_err());
        }
        let mut leaked = stages.clone();
        leaked[2]["response"]["display"] = json!("Here is the withheld fresh task today.");
        leaked[3]["packet"]["current_instruction"] = leaked[2]["response"]["display"].clone();
        assert!(audit(&leaked).is_err());
    }
    #[test]
    fn stopped_return_cannot_carry_a_task_prior_memory_private_material_or_model_origin() {
        let (course, case, policy, mut stages) = fixture(true);
        let c = condition_for(4, "better-person", &json!("written_artifact")).unwrap();
        let open = rules_for(&c, false).unwrap();
        let ret = rules_for(&c, true).unwrap();
        let assigned =
            json!({"condition":c,"stages":{"opening":open,"feedback":open,"return":ret}});
        let mut case = case;
        case["days"] = c["days"].clone();
        for stage in &mut stages[..2] {
            stage["origin"] = json!("model");
            stage["model_requested"] = json!("fixture");
            stage["request_hash"] = json!("a".repeat(64));
            stage["seconds"] = json!(1);
            stage["packet"]["allowed_moves"] = json!(MOVES);
            stage["packet"]["condition"] = open["context"].clone();
        }
        let deterministic = |packet: Value, response: Value| json!({"origin":"local","tokens":null,"seconds":0,"model_requested":null,"packet":packet,"response":response});
        stages[2] = deterministic(
            json!({"goal":"goal","evidence_limit":"limit","policy":"resume",
            "days":c["days"],"learner":case["return_request"],"receipt":{},"material":{},
            "condition":ret["context"],"allowed_moves":["stop"]}),
            json!({"display":"We can stop here.","move":"stop","focus":"choice"}),
        );
        stages[3] = deterministic(
            json!({"eligible":false,"condition":ret["context"]}),
            json!({"response":"No assessment offered."}),
        );
        assert!(arm(&course, &case, &policy, &stages, Some(&assigned)).is_ok());
        for (stage, pointer, value) in [
            (2, "/packet/new_task", json!("fresh task")),
            (2, "/packet/receipt", json!({"focus":"choice"})),
            (2, "/packet/material", json!({"excerpt":"new passage"})),
            (2, "/response/display", json!("Another question?")),
            (2, "/origin", json!("model")),
            (3, "/packet/learner_history", case.clone()),
        ] {
            let mut changed = stages.clone();
            let mut keys = pointer.trim_start_matches('/').split('/');
            let first = keys.next().unwrap();
            if let Some(second) = keys.next() {
                changed[stage][first][second] = value;
            } else {
                changed[stage][first] = value;
            }
            assert!(arm(&course, &case, &policy, &changed, Some(&assigned)).is_err());
        }
        // The current request survives a goal switch. An old goal in the shared header leaks the withdrawn aim.
        let c = condition_for(8, "violin", &json!("witnessed_performance")).unwrap();
        let open = rules_for(&c, false).unwrap();
        let ret = rules_for(&c, true).unwrap();
        let assigned =
            json!({"condition":c,"stages":{"opening":open,"feedback":open,"return":ret}});
        case["days"] = c["days"].clone();
        for s in &mut stages[..2] {
            s["packet"]["condition"] = open["context"].clone();
        }
        let packet = json!({"evidence_limit":"limit","policy":"resume","days":c["days"],"learner":case["return_request"],"receipt":{},"material":{},
            "condition":ret["context"],"allowed_moves":["clarify","defer","stop"]});
        stages[2] = json!({"origin":"model","model_requested":"fixture","seconds":1,"request_hash":"a".repeat(64),
            "packet":packet,"response":{"move":"clarify","focus":"choice","display":"What would you like to do?"}});
        stages[3]["packet"]["condition"] = ret["context"].clone();
        assert!(arm(&course, &case, &policy, &stages, Some(&assigned)).is_ok());
        stages[2]["packet"]["goal"] = json!("goal");
        assert!(arm(&course, &case, &policy, &stages, Some(&assigned)).is_err());
        stages[2]["packet"].as_object_mut().unwrap().remove("goal");
        stages[2]["origin"] = json!("fallback");
        stages[2]["proposed_response"] =
            json!({"move":"probe","focus":"reasoning","display":"Try the old question."});
        stages[2]["response"] = json!({"display":"We can leave this activity open for another visit.","move":"defer","focus":"choice"});
        assert!(arm(&course, &case, &policy, &stages, Some(&assigned)).is_ok());
        stages[2]["proposed_response"]["move"] = json!("clarify");
        assert!(arm(&course, &case, &policy, &stages, Some(&assigned)).is_err());
    }
    #[test]
    fn privacy_crosses_actor_boundaries_and_teacher_reviews_obey_capacity() {
        let (_, case, _, _) = fixture(true);
        let c = condition_for(5, "worship", &json!("written_artifact")).unwrap();
        let opening = rules_for(&c, false).unwrap();
        let rules =
            json!({"opening":opening,"feedback":opening,"return":rules_for(&c,true).unwrap()});
        let safe = observed_case(&case, &rules);
        assert_eq!(
            safe,
            json!({"save_topic":true,"days":30,"return_request":"Help me with the new task."})
        );
        assert_eq!(rules["return"]["memory"], json!([]));
        for (trial, admits) in [(7, false), (20, true)] {
            let c = condition_for(trial, "violin", &json!("witnessed_performance")).unwrap();
            let r = rules_for(&c, true).unwrap();
            assert_eq!(r["local"].is_object(), admits);
            assert_eq!(r["task"], false);
            assert_eq!(r["learner_attempt"], false);
        }
    }
    #[test]
    fn a_witness_uses_dispatched_fields_and_the_displayed_fallback() {
        let arms = json!({"A":{"return":{"origin":"local","packet":{"material":{"worked":"example"}},"response":{"move":"stop"}}},
            "B":{"return":{"origin":"fallback","packet":{"material":{"worked":"example"}},"proposed_response":{"move":"probe"},"response":{"move":"defer"}}}});
        let mut h = json!({"stage":"return","channel":"material","feature":"worked","direction":"candidate_only"});
        assert_eq!(witness(&h, "B", &arms).unwrap()["met"], true);
        h["channel"] = json!("move");
        h["feature"] = json!("probe");
        assert_eq!(witness(&h, "B", &arms).unwrap()["met"], false);
        h["feature"] = json!("defer");
        assert_eq!(witness(&h, "B", &arms).unwrap()["met"], true);
        h["channel"] = json!("memory");
        h["feature"] = json!("topic");
        h["stage"] = json!("feedback");
        assert!(witness(&h, "B", &arms).is_err());
    }

    #[test]
    fn assessment_needs_a_separate_grant_and_only_the_released_reference() {
        let (worship, _) = course("worship").unwrap();
        let (engineer, _) = course("chemical-engineering").unwrap();
        for source in [&worship, &engineer] {
            let release = &source["return_pack"];
            let rule = json!({"learner_attempt":true,"context":{"day":30}});
            for (choice, expected) in [
                ("submit", AssessmentRoute::Submitted),
                ("private_practice", AssessmentRoute::PrivatePractice),
                ("skip", AssessmentRoute::Skipped),
            ] {
                let mut form = json!({"new_task":release["check"],"learner_history":{"answer":"private writing"}});
                let (actual, reference) =
                    assessment_form(&mut form, &json!(choice), &rule, release).unwrap();
                assert_eq!(actual, expected);
                if choice == "submit" {
                    assert_eq!(form["new_task"], release["check"]);
                    assert_eq!(
                        form.get("required_reference").is_some(),
                        release["show_source_during_check"] == true
                    );
                    if !reference.is_null() {
                        assert_eq!(
                            reference,
                            pick(release, &["unit", "locator", "excerpt"]).unwrap()
                        );
                    }
                } else {
                    assert_eq!(
                        form,
                        json!({"eligible":true,"choice":choice,"condition":rule["context"]})
                    );
                    assert!(reference.is_null());
                }
            }
            let mut form = json!({"new_task":release["check"]});
            let closed = json!({"learner_attempt":false,"context":{"day":30}});
            assert!(assessment_form(&mut form, &json!("submit"), &closed, release).is_err());
            assert_eq!(
                assessment_form(&mut form, &json!("skip"), &closed, release).unwrap(),
                (AssessmentRoute::NotOffered, Value::Null)
            );
            assert_eq!(
                form,
                json!({"eligible":false,"condition":closed["context"]})
            );
        }
        let prior = json!({"trial":5,"change":"Offer a released practice item.","memory":[],"retrieve":["lesson"],"resume_mode":"booked","measured":null,"adopted":false,"verdict":"unobserved"});
        past_designs(&json!([prior]), 10).unwrap();
        assert!(past_designs(&json!([prior]), 11).is_err());
    }

    #[test]
    fn a_retained_focus_needs_a_new_grant_to_reach_a_later_lesson() {
        let mut case = json!({"memory_choices":["goal","family"],"selected_focus":"reasoning","topic_selection":"first",
            "return_intent":"keep","return_fields":[],"resume_fields":["goal","family"],"resume_keep":true});
        let stages = json!({"return":{"local":{"move":"defer"}},"resume":{"local":null,"memory":TOPIC_MEMORY}});
        visit_choices(&case, &stages).unwrap();
        let retained =
            json!({"goal":"read the source","family":3,"focus":"reasoning","move":"clarify"});
        let snapshot = resumed_memory(
            true,
            &retained,
            &stages["resume"],
            &json!({"move":"defer"}),
            Some(&case["resume_fields"]),
        )
        .unwrap();
        assert_eq!(
            snapshot["stored"],
            json!({"goal":"read the source","family":3})
        );
        assert_eq!(visit_use_note(&snapshot["stored"]),"Earlier context made available for this activity includes course goal, exercise family.");
        assert_eq!(
            visit_use_note(&json!({})),
            "No earlier personal context was made available for this activity."
        );
        let mut corrupt = snapshot.clone();
        corrupt["stored"]["focus"] = retained["focus"].clone();
        assert_ne!(snapshot, corrupt);
        case["return_fields"] = json!(["focus"]);
        assert!(visit_choices(&case, &stages).is_err());
        case["return_fields"] = json!([]);
        case["resume_fields"] = json!(["help"]);
        assert!(visit_choices(&case, &stages).is_err());
        case["return_intent"] = json!("cancel_topic");
        case["resume_fields"] = json!(["topic"]);
        assert!(visit_choices(&case, &stages).is_err());
        case["resume_fields"] = json!([]);
        assert!(visit_choices(&case, &stages).is_err());
        case["resume_keep"] = json!(false);
        visit_choices(&case, &stages).unwrap();
    }

    #[test]
    fn a_cancelled_or_erased_booking_cannot_reappear_in_later_teaching() {
        let topic = json!({"domain":"violin","unit":1,"edition":"pinned"});
        let booking = json!({"topic":topic,"goal":"play","move":"practice"});
        let (cancelled, message) =
            return_state(&json!("cancel_topic"), &json!("withdrawal"), &booking).unwrap();
        assert_eq!(message, "No lesson topic is booked now.");
        assert_eq!(
            cancelled["stored"],
            json!({"goal":"play","move":"practice"})
        );
        let rule = later_rule(
            &json!({"id":"withdrawal","days":14,"setting":"individual","evidence_mode":"written"}),
        );
        let executed = json!({"move":"defer"});
        let resumed = resumed_memory(true, &cancelled["stored"], &rule, &executed, None).unwrap();
        assert_eq!(resumed["stored"], json!({"goal":"play","move":"defer"}));
        assert_ne!(
            resumed["stored"],
            json!({"goal":"play","move":"defer","topic":topic})
        );
        assert_eq!(
            resumed_memory(false, &cancelled["stored"], &rule, &executed, None).unwrap()["stored"],
            json!({})
        );
        let (forgot, _) =
            return_state(&json!("forget_all"), &json!("forget_topic"), &booking).unwrap();
        assert_eq!(
            resumed_memory(true, &forgot["stored"], &rule, &executed, None).unwrap()["stored"],
            json!({})
        );
        assert!(return_state(&json!("keep"), &json!("withdrawal"), &booking).is_err());
    }

    #[test]
    fn lesson_selection_is_bound_to_the_actual_release_and_effective_visit() {
        let first = json!({"domain":"violin","unit":1,"edition":"pinned","title":"Before"});
        let fresh = json!({"domain":"violin","unit":2,"edition":"pinned","title":"After"});
        let menu = json!({"first":first,"return":fresh});
        let course = json!({"first_pack":{"excerpt":"first reviewed excerpt"},"return_pack":{"excerpt":"new reviewed excerpt"}});
        let rule = json!({"material":["excerpt"]});
        let mut arms = json!({"A":{},"B":{}});
        let dispatched = json!({"origin":"model","packet":{}});
        for stage in ["opening", "feedback", "return"] {
            arms["A"][stage] = dispatched.clone();
            arms["B"][stage] = dispatched.clone();
        }
        let topic = pick(&first, &["domain", "unit", "edition"]).unwrap();
        for (name, mode, slot, unit) in [("A", "compare", "return", 2), ("B", "booked", "first", 1)]
        {
            let mut p = json!({"receipt":{"topic":topic}});
            let pack =
                resolved_pack(&mut p, &course, &menu, &rule, &["excerpt"], Some(mode)).unwrap();
            assert_eq!(p["booked_lesson"], first);
            assert_eq!(p["lesson_route"]["unit"], unit);
            assert_eq!(p["lesson_route"]["slot"], slot);
            assert_eq!(pack, &course[format!("{slot}_pack")]);
            arms[name]["resume"] = json!({"origin":"model","packet":p});
        }
        let h = json!({"stage":"resume","channel":"selection","feature":"contrast_material","direction":"candidate_only"});
        assert_eq!(temporal_witness(&h, "A", &arms), Ok(true));
        assert_eq!(witness(&h, "A", &arms).unwrap()["met"], true);
        arms["A"]["resume"]["packet"]["comparison_material"] = json!({});
        assert_eq!(temporal_witness(&h, "A", &arms), Ok(false));
        let h = json!({"stage":"resume","channel":"selection","feature":"first_unit","direction":"incumbent_only"});
        assert_eq!(temporal_witness(&h, "A", &arms), Ok(true));
        arms["B"]["resume"]["packet"]["lesson_route"]["slot"] = json!("return");
        assert_eq!(temporal_witness(&h, "A", &arms), Ok(false));
        arms["B"].as_object_mut().unwrap().remove("resume");
        assert!(temporal_witness(&h, "A", &arms).is_err());
        let mut law = monitor(true);
        let frame = |stage| Frame {
            stage,
            matches_packet: true,
            legal_response: true,
        };
        for s in [
            Stage::Opening,
            Stage::Feedback,
            Stage::Return,
            Stage::Learner,
        ] {
            assert!(law.next_state(&frame(s)));
        }
        assert!(!law.next_state(&frame(Stage::Return)));
    }

    #[test]
    fn a_v7_incumbent_requires_the_tested_domain_and_a_valid_source() {
        let baseline = json!({"teaching":"Help with the current task and then offer practice.",
            "returning":"Resume the goal on a different task using the available history.",
            "memory":["goal"],"retrieve":["excerpt","lesson"]});
        let mut source = json!({"scope":"violin","origin":"baseline","trial":null,"sha256":null});
        assert!(scoped_incumbent(&source, &baseline, "violin", 23).is_ok());
        let mut adopted = baseline;
        adopted["teaching"] = json!("Explore the musical relationship.");
        assert!(scoped_incumbent(&source, &adopted, "violin", 23).is_err());
        source["origin"] = json!("domain-adoption");
        source["trial"] = json!(18);
        source["sha256"] = json!("af".repeat(32));
        assert!(scoped_incumbent(&source, &adopted, "violin", 23).is_ok());
        assert!(scoped_incumbent(&source, &adopted, "better-person", 19).is_err());
        source["scope"] = json!("better-person");
        assert!(scoped_incumbent(&source, &adopted, "better-person", 19).is_err());
        source["scope"] = json!("violin");
        for other in [0, 17, 23, 28] {
            source["trial"] = json!(other);
            assert!(scoped_incumbent(&source, &adopted, "violin", 23).is_err());
        }
        source["origin"] = json!("external-seed");
        assert!(scoped_incumbent(&source, &adopted, "violin", 23).is_err());
        source["trial"] = Value::Null;
        assert!(scoped_incumbent(&source, &adopted, "violin", 23).is_ok());
        source["sha256"] = json!("no parent decision");
        assert!(scoped_incumbent(&source, &adopted, "violin", 23).is_err());
    }

    #[test]
    fn granular_booking_requires_a_distinct_learner_grant_for_every_field() {
        let rules = json!({"feedback":{"memory":["goal","focus","topic","help","move","family"]}});
        let available = json!({"topic":{"domain":"writer","edition":"pinned","unit":31},
            "goal":"Write a scene","focus":"reasoning","move":"probe","help":"unknown","family":31});
        let mut case = json!({"memory_choices":[],"selected_focus":null,"topic_selection":"first"});
        let all = TOPIC_MEMORY;
        assert_eq!(
            authorized_booking(&case, &available, all, &rules, false).unwrap(),
            json!({"topic":available["topic"]})
        );
        case["topic_selection"] = json!("none");
        case["memory_choices"] = json!(["goal", "family"]);
        case["selected_focus"] = json!("source");
        assert_eq!(
            authorized_booking(&case, &available, all, &rules, false).unwrap(),
            json!({"goal":"Write a scene","family":31,"focus":"source"})
        );
        assert_eq!(
            authorized_booking(&case, &available, &["move", "focus"], &rules, false).unwrap(),
            json!({"focus":"source"})
        );
        let restricted = json!({"feedback":{"memory":["help","move","family"]}});
        assert_eq!(
            authorized_booking(&case, &available, all, &restricted, false).unwrap(),
            json!({"family":31})
        );
        assert!(authorized_booking(&case, &available, all, &rules, true).is_err());
        case["memory_choices"] = json!(["goal", "goal"]);
        assert!(authorized_booking(&case, &available, all, &rules, false).is_err());
        case["memory_choices"] = json!([]);
        case["selected_focus"] = Value::Null;
        assert_eq!(
            authorized_booking(&case, &available, all, &rules, true).unwrap(),
            json!({})
        );
        let view = observed_case(
            &case,
            &json!({"opening":{"answer":false},"feedback":{"answer":false}}),
        );
        assert_eq!(view, json!({}));
    }

    #[test]
    fn paired_ltl_checks_the_selected_visit_direction_and_executed_feature() {
        let sent = json!({"origin":"model","packet":{"material":{"worked":"shown"},"receipt":{"focus":"choice"}},"response":{"move":"probe"}});
        let hold = json!({"origin":"local","packet":{"material":{"worked":"planned"},"receipt":{"focus":"choice"}},"response":{"move":"stop"}});
        let fallback = json!({"origin":"fallback","packet":{"material":{},"receipt":{}},"response":{"move":"defer"},"proposed_response":{"move":"probe"}});
        let mut arms = json!({"A":{"opening":sent,"feedback":fallback,"return":hold},
                              "B":{"opening":hold,"feedback":sent,"return":fallback}});
        for (stage, channel, feature, dir, candidate, expected) in [
            ("opening", "material", "worked", "candidate_only", "A", true),
            (
                "feedback",
                "material",
                "worked",
                "candidate_only",
                "A",
                false,
            ),
            (
                "feedback",
                "material",
                "worked",
                "incumbent_only",
                "A",
                true,
            ),
            (
                "feedback",
                "material",
                "worked",
                "candidate_only",
                "B",
                true,
            ),
            ("return", "move", "defer", "candidate_only", "B", true),
            ("return", "move", "probe", "candidate_only", "B", false),
            ("return", "move", "stop", "incumbent_only", "B", true),
            ("return", "memory", "focus", "candidate_only", "A", false),
        ] {
            let h = json!({"stage":stage,"channel":channel,"feature":feature,"direction":dir});
            assert_eq!(temporal_witness(&h, candidate, &arms), Ok(expected));
            assert_eq!(witness(&h, candidate, &arms).unwrap()["met"], expected);
        }
        arms["A"].as_object_mut().unwrap().remove("return");
        let early = json!({"stage":"opening","channel":"move","feature":"defer","direction":"candidate_only"});
        assert!(temporal_witness(&early, "A", &arms).is_err());
    }
    fn fixture(save: bool) -> (Value, Value, Value, [Value; 4]) {
        let course = json!({"scope":{"goal":"goal"},"limit":"limit",
            "first_pack":{"check":"first task","unit":"unit","excerpt":"first passage"},
            "return_pack":{"check":"fresh task","excerpt":"new passage"}});
        let case = json!({"first_answer":"first answer","surprise":"private clarification",
            "save_topic":save,"days":30,"return_request":"Help me with the new task."});
        let policy = json!({"teaching":"teach","returning":"resume","retrieve":["excerpt"],"memory":["focus","help"]});
        let opening = json!({"packet":{"goal":"goal","evidence_limit":"limit","policy":"teach","task":"first task",
            "answer":"first answer","material":{"excerpt":"first passage"}},
            "response":{"move":"explain","focus":"source","display":"Explanation."}});
        let mut feedback = opening["packet"].clone();
        feedback["displayed"] = json!("Explanation.");
        feedback["learner"] = json!("private clarification");
        let feedback = json!({"packet":feedback,"response":{"move":"probe","focus":"choice","display":"A question."}});
        let memory = if save {
            json!({"focus":"choice","help":"unknown"})
        } else {
            json!({})
        };
        let returned = json!({"packet":{"goal":"goal","evidence_limit":"limit","policy":"resume","days":30,
            "learner":"Help me with the new task.","receipt":memory,"new_task":"fresh task","material":{"excerpt":"new passage"}},
            "response":{"move":"practice","focus":"condition","display":"Practice."}});
        let learner = json!({"packet":{"learner_history":case,"goal":"goal","earlier_instruction":["Explanation.","A question."],
            "new_task":"fresh task","current_instruction":"Practice."},"response":{"response":"Tried it."}});
        (course, case, policy, [opening, feedback, returned, learner])
    }
    #[test]
    fn stale_label_extra_private_field_and_declined_memory_fail_at_return() {
        for save in [true, false] {
            let (course, case, policy, stages) = fixture(save);
            assert!(arm(&course, &case, &policy, &stages, None).is_ok());
            for mutation in [
                json!({"focus":"source","help":"unknown"}),
                json!({"focus":"choice","help":"unknown","answer":"private clarification"}),
            ] {
                let mut changed = stages.clone();
                changed[2]["packet"]["receipt"] = mutation;
                assert!(arm(&course, &case, &policy, &changed, None)
                    .unwrap_err()
                    .contains("Return"));
            }
            let mut wrong_edition = stages.clone();
            wrong_edition[2]["packet"]["new_task"] = stages[0]["packet"]["task"].clone();
            assert!(arm(&course, &case, &policy, &wrong_edition, None)
                .unwrap_err()
                .contains("Return"));
        }
    }
    #[test]
    fn arms_must_use_the_authored_answer_and_feedback_must_precede_return() {
        let (course, case, policy, mut stages) = fixture(true);
        stages[0]["packet"]["answer"] = json!("other arm's answer");
        assert!(arm(&course, &case, &policy, &stages, None)
            .unwrap_err()
            .contains("Opening"));
        let mut law = monitor(false);
        let edge = |stage| Frame {
            stage,
            matches_packet: true,
            legal_response: true,
        };
        assert!(law.next_state(&edge(Stage::Feedback)));
        assert!(!law.next_state(&edge(Stage::Learner)));
    }
}
