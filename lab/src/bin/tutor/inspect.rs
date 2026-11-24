//! Read-only, allowlisted projections of committed lesson journal records.
//! A missing historical field stays absent. A checkpoint is not a delivery.
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
    path::Path,
};
use tutor_lab::platform::PrivateOpen;

#[derive(Serialize, Default)]
struct Object {
    name: String,
    fields: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    micros: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    links: Vec<(String, String)>,
}
impl Object {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
    fn field(&mut self, label: &str, value: &Value) {
        let text = match value {
            Value::String(s) if !s.is_empty() => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(v) => if *v { "Yes" } else { "No" }.into(),
            _ => return,
        };
        self.fields.push((label.into(), text));
    }
    fn add(&mut self, label: &str, text: &str) {
        self.fields.push((label.into(), text.into()));
    }
    fn read(&mut self, value: &Value, fields: &[(&str, &str)]) {
        for (label, key) in fields {
            self.field(label, &value[*key]);
        }
    }
}

#[derive(Serialize)]
struct Record {
    event: String,
    title: String,
    recorded_at: Option<u64>,
    objects: Vec<Object>,
}
impl Record {
    fn push(&mut self, o: Object) {
        if !o.fields.is_empty() || o.micros.is_some() || !o.links.is_empty() {
            self.objects.push(o);
        }
    }
    fn fields(&mut self, name: &str, v: &Value, fields: &[(&str, &str)]) {
        let mut o = Object::new(name);
        o.read(v, fields);
        self.push(o);
    }
}

fn items(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

fn lesson(r: &mut Record, data: &Value, saved: &Value) {
    let discovery = &saved["discovery"];
    let decision = &data["judgment"]["decision"];
    let mut move_record = Object::new(if decision["shadow"] == true {
        "Shadow decision"
    } else if decision.is_object() {
        "Teaching decision"
    } else if discovery["reason"].is_string() {
        "Discovery decision"
    } else {
        "Saved lesson"
    });
    if data["event"] == "delivered" && !decision.is_object() && !discovery["reason"].is_string() {
        move_record.add(
            "Provenance",
            "No decision receipt was saved with this reply.",
        );
    }
    move_record.read(
        decision,
        &[
            ("Policy", "policy"),
            ("Source", "source"),
            ("Shadow comparison", "shadow"),
            ("Selected action", "selected"),
            ("Delivered action", "delivered_action"),
            ("Reason", "why"),
            ("What would change this step", "next_check"),
        ],
    );
    move_record.read(
        saved,
        &[
            ("Learning goal", "declared_goal"),
            ("Objective", "objective"),
        ],
    );
    move_record.read(
        discovery,
        &[
            ("Discovery phase", "phase"),
            ("Discovery rationale", "reason"),
        ],
    );
    if let Some(cue) = decision["tutor_cue"].as_array().filter(|c| c.len() == 2) {
        if let (Some(source), Some(excerpt)) = (cue[0].as_str(), cue[1].as_str()) {
            move_record.add("What Luna supplied", &format!("{excerpt} ({source})"));
        }
    }
    let applied = discovery["phase"] == "instruction";
    let sources: Vec<_> = items(&discovery["proposal"]["sources"])
        .filter_map(Value::as_str)
        .filter(|_| applied)
        .collect();
    for id in &sources {
        move_record
            .links
            .push(("Evidence cited for this exercise".into(), id.to_string()));
    }
    for id in items(&decision["guidance"]).filter_map(|f| f["id"].as_str()) {
        move_record
            .links
            .push(("Guidance on this decision".into(), id.into()));
    }
    r.push(move_record);
    let facts = discovery["profile"]["facts"].as_object();
    if let Some(facts) = facts {
        // Put the evidence referenced by this exercise before the rest of the profile.
        let mut ordered: Vec<_> = facts.iter().collect();
        ordered.sort_by_key(|(id, _)| !sources.contains(&id.as_str()));
        for (id, fact) in ordered {
            let mut o = Object::new(if sources.contains(&id.as_str()) {
                "Cited learner evidence"
            } else {
                "Learner profile evidence"
            });
            o.id = Some(id.clone());
            o.add("Source", id);
            o.read(
                fact,
                &[
                    ("Evidence type", "kind"),
                    ("Learner’s words", "quote"),
                    ("Tutor’s interpretation", "reading"),
                    ("Subject", "subject"),
                    ("Area", "area"),
                    ("Task", "task"),
                ],
            );
            for link in items(&fact["replaces"]).filter_map(Value::as_str) {
                o.links
                    .push(("Corrects earlier evidence".into(), link.to_owned()));
            }
            r.push(o);
        }
    }
    let mut seen = std::collections::HashSet::new();
    for finding in items(&decision["guidance"]).chain(items(&decision["findings"])) {
        let id = finding["id"].as_str();
        if id.is_some_and(|s| !seen.insert(s)) {
            continue;
        }
        let mut o = Object::new("Teaching finding");
        o.id = id.map(str::to_owned);
        o.read(
            finding,
            &[
                ("Evidence type", "kind"),
                ("Source exchange", "source"),
                ("Speaker", "speaker"),
                ("Quotation", "quote"),
                ("Interpretation", "detail"),
            ],
        );
        if let Some(target) = finding["target"].as_str().filter(|t| !t.is_empty()) {
            o.links.push(("Refers to".into(), target.into()));
        }
        r.push(o);
    }
    r.fields(
        "Progression",
        &decision["progression"],
        &[
            ("Source", "source"),
            ("Reasoning", "operation"),
            ("Learner’s words", "quote"),
            ("Assisted", "assisted"),
            ("Decision", "choice"),
            ("Reason", "reason"),
            ("Next skill", "next_skill"),
        ],
    );
    r.fields(
        "Assessment",
        &data["judgment"]["assessment"],
        &[
            ("Action", "action"),
            ("Verification method", "method"),
            ("Task family", "family"),
        ],
    );
    r.fields(
        "Observed work",
        &saved["observation"],
        &[
            ("Action", "action"),
            ("Quotation", "quote"),
            ("Assisted", "assisted"),
            ("Validation", "validation"),
        ],
    );
    r.fields(
        "Practice result",
        &saved["practice_evidence"],
        &[
            ("Source", "source"),
            ("Activity", "activity"),
            ("Feedback", "feedback"),
            ("Decision", "decision"),
            ("Earlier help", "earlier_help"),
            ("Moves prepared", "prepared"),
            ("Moves used", "used"),
        ],
    );
    for source in items(&data["sources"]) {
        r.fields(
            "Earlier exchange",
            source,
            &[("Source", "source"), ("Digest", "digest")],
        );
    }
    r.fields("Delivered reply", data, &[("Luna’s words", "text")]);
}

fn timing(r: &mut Record, sample: &Value) {
    let mut o = Object::new("Turn measurement");
    o.read(
        sample,
        &[
            ("Kind", "kind"),
            ("Delivery recorded", "delivered"),
            ("Entry", "entry"),
        ],
    );
    for (key, label) in [
        ("feedback_us", "Time to feedback"),
        ("actionable_us", "Time to exercise"),
    ] {
        if let Some(us) = sample[key].as_u64() {
            o.add(label, &format!("{:.1} ms", us as f64 / 1000.));
        }
    }
    r.push(o);
    for stage in items(&sample["stages"]) {
        let mut o = Object::new(stage["name"].as_str().unwrap_or("Measured stage"));
        o.micros = stage["micros"].as_u64();
        if o.micros.is_none() {
            o.add("Duration", "Not recorded");
        }
        o.read(
            &stage["usage"],
            &[
                ("Input tokens", "prompt_tokens"),
                ("Output tokens", "completion_tokens"),
                ("Total tokens", "total_tokens"),
                ("Reported cost (USD)", "cost"),
            ],
        );
        r.push(o);
    }
}

fn project(data: &Value) -> Record {
    let kind = data["event"].as_str().unwrap_or("unknown");
    let title = match kind {
        "open" => "Journal opened",
        "delivered" => "Tutor reply delivered",
        "learner" => "Learner message",
        "practice_answer" => "Practice submitted",
        "workspace_draft" => "Draft saved",
        "timing" => "Turn timing",
        "turn_step" => "Processing checkpoint",
        "review" => "Draft reviewed",
        "rejected" => "Draft rejected",
        "failed" => "Turn failed",
        "discovery_feedback" => "Learner feedback",
        "continued" => "Earlier lesson resumed",
        "supervisor_plan" => "Supervisor plan",
        "planned" => "Lesson planned",
        "recovered" => "History recovered",
        "restored_goal" => "Goal restored",
        "resumed_interrupted" => "Interrupted turn resumed",
        "submission_preview" => "Submission preview updated",
        "pilot" => "Pilot observation",
        _ => "Unrecognized record",
    };
    let mut r = Record {
        event: kind.into(),
        title: title.into(),
        recorded_at: data["recorded_at"].as_u64(),
        objects: vec![],
    };
    match kind {
        "delivered" | "continued" | "recovered" => {
            let saved = if kind == "recovered" {
                &data["continuation"]
            } else {
                &data["lesson"]
            };
            if kind == "recovered" {
                let mut o = Object::new("Recovered history");
                o.add("Exchanges", &items(&data["exchanges"]).count().to_string());
                o.add("Provenance", "Earlier routing decisions were not recorded.");
                r.push(o);
            }
            r.fields(
                "Continued from",
                &data["from"],
                &[("Source", "source"), ("Digest", "digest")],
            );
            lesson(&mut r, data, saved);
        }
        "turn_step" => {
            let step = &data["step"];
            let key = step["key"].as_str().unwrap_or("");
            let mut o = Object::new("Model processing");
            if let Some(stage) = key.strip_prefix("debug/") {
                o.add("Stage", stage);
                if step["output"]["state"] == "reused" {
                    r.title = format!("{stage} · checkpoint reused");
                    o.add("Recorded state", "Recovered saved output");
                    o.field("Checkpoint", &step["output"]["checkpoint"]);
                } else {
                    r.title = format!("{stage} · model attempt");
                    o.field("Attempt", &step["output"]["attempt"]);
                    o.field("Requested model", &step["output"]["model"]);
                    o.add("Recorded state", "Request starting");
                }
            } else if let Some((stage, _)) = key.split_once('/') {
                r.title = format!("{stage} · checkpoint saved");
                o.add("Stage", stage);
                o.add("Recorded state", "Output saved for recovery");
                o.add("Checkpoint", key);
            }
            r.push(o);
        }
        "timing" => timing(&mut r, &data["sample"]),
        "review" => r.fields(
            "Content review",
            &data["verdict"],
            &[("Verdict", "verdict")],
        ),
        "rejected" => {
            let mut o = Object::new("Content review");
            o.add("State", "Draft rejected before delivery");
            r.push(o);
        }
        "learner" | "discovery_feedback" | "resumed_interrupted" => {
            r.fields("Learner input", data, &[("Recorded text", "text")])
        }
        "practice_answer" | "workspace_draft" => {
            let submitted = kind == "practice_answer";
            let draft = &data[if submitted { "work" } else { "draft" }];
            let mut o = Object::new(if submitted {
                "Submitted work"
            } else {
                "Saved draft"
            });
            o.field("Journal turn index", &data["turn"]);
            o.read(draft, &[("Learner text", "text"), ("Help used", "helped")]);
            if let Some(fields) = draft["fields"].as_object() {
                for (k, v) in fields {
                    o.field(k, v);
                }
            }
            for ms in items(&draft["intervals"]) {
                o.field("Recorded interval (ms)", ms);
            }
            r.push(o);
            for pair in items(&draft["measurements"]) {
                let mut o = Object::new("Simulation measurement");
                o.field("Length (cm)", &pair[0]);
                o.field("Distance (m)", &pair[1]);
                r.push(o);
            }
            if submitted {
                r.fields("Submission", data, &[("Rendered work", "text")]);
            }
        }
        "open" => r.fields(
            "Journal",
            data,
            &[
                ("Journal version", "version"),
                ("Selected lesson", "selected"),
            ],
        ),
        "failed" => r.fields("Failure", data, &[("Message", "message")]),
        "planned" => {
            r.fields("Lesson", data, &[("Topic", "topic")]);
            let mut o = Object::new("Outline");
            for point in items(&data["outline"]) {
                o.field("Step", point);
            }
            r.push(o);
        }
        "supervisor_plan" => r.fields(
            "Supervisor decision",
            data,
            &[
                ("Revision", "revision"),
                ("Objective", "objective"),
                ("Task", "task"),
                ("Replace current prompt", "replace_prompt"),
            ],
        ),
        "restored_goal" => r.fields("Lesson", data, &[("Restored goal", "goal")]),
        "submission_preview" => {
            let mut o = Object::new("Submission preview");
            o.add(
                "State",
                if data["preview"].is_null() {
                    "Cleared"
                } else {
                    "Saved"
                },
            );
            o.field("Assignment", &data["preview"]["id"]);
            r.push(o);
        }
        "pilot" => r.fields(
            "Pilot note",
            &data["note"],
            &[
                ("Kind", "kind"),
                ("ID", "id"),
                ("Source", "source"),
                ("Category", "category"),
                ("Result", "result"),
            ],
        ),
        _ => {}
    }
    r
}

/// Resume at a byte boundary previously emitted by this reader. Never acquire the writer lock.
pub fn read_range(
    path: &Path,
    offset: u64,
    limit: usize,
    out: impl Write,
    lesson: bool,
) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .open_private(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    let size = meta.len();
    if offset > size {
        return Err("Journal changed since the previous read.".into());
    }
    use std::io::Read;
    let mut file = file;
    if offset != 0 {
        file.seek(SeekFrom::Start(offset - 1))
            .map_err(|e| e.to_string())?;
        let mut byte = [0];
        file.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte[0] != b'\n' {
            return Err("Offset must follow a complete journal record.".into());
        }
    }
    let mut reader = BufReader::new(file.take(size - offset));
    let mut pos = offset;
    let mut line = Vec::new();
    let mut out = std::io::BufWriter::new(out);
    for _ in 0..limit {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 || !line.ends_with(b"\n") {
            break;
        }
        let v: serde_json::Value = serde_json::from_slice(&line)
            .map_err(|_| format!("Invalid journal record at byte {pos}"))?;
        pos += n as u64;
        let record = if lesson {
            // Only the fields consumed by the lesson renderer leave this process.
            let mut row = serde_json::Map::new();
            for key in ["event", "text", "task", "replace_prompt", "exchanges"] {
                if let Some(value) = v.get(key) {
                    row.insert(key.into(), value.clone());
                }
            }
            if v.get("judgment").is_some_and(|j| !j.is_null()) {
                row.insert("judgment".into(), true.into());
            }
            for key in ["lesson", "continuation"] {
                if let Some(state) = v.get(key) {
                    let mut fields = serde_json::Map::new();
                    for field in [
                        "exercise",
                        "objective",
                        "current_step",
                        "name_recall",
                        "recall_items",
                        "practice_evidence",
                    ] {
                        if let Some(val) = state.get(field) {
                            fields.insert(field.into(), val.clone());
                        }
                    }
                    if let Some(discovery) = state.get("discovery").filter(|d| d.is_object()) {
                        fields.insert("discovery".into(),serde_json::json!({"question":discovery["question"],"phase":discovery["phase"],"reason":discovery["reason"]}));
                    }
                    row.insert(key.into(), fields.into());
                }
            }
            serde_json::Value::Object(row)
        } else {
            serde_json::to_value(project(&v)).map_err(|e| e.to_string())?
        };
        serde_json::to_writer(
            &mut out,
            &serde_json::json!({"next_offset":pos,"record":record}),
        )
        .map_err(|e| e.to_string())?;
        out.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn hidden_model_output_and_pending_start_stay_out_of_projection() {
        for row in [
            json!({"event":"turn_step","step":{"key":"route/digest","output":{"text":"SECRET"}}}),
            json!({"event":"review","verdict":{"verdict":"revise","criterion":"SECRET"},"draft":"SECRET"}),
            json!({"event":"delivered","lesson":{"discovery":{"phase":"review","proposal":{"sources":["SECRET"]}},"rubric":"SECRET"},"judgment":{"calls":["SECRET"]}}),
            json!({"event":"unknown","anything":"SECRET"}),
        ] {
            assert!(!serde_json::to_string(&project(&row))
                .unwrap()
                .contains("SECRET"));
        }
    }
    #[test]
    fn distinguish_missing_timing_from_zero_and_keep_correction_links() {
        let r = project(
            &json!({"event":"timing","sample":{"stages":[{"name":"routing","micros":0},{"name":"review"}]}}),
        );
        assert_eq!(
            r.objects
                .iter()
                .find(|o| o.name == "routing")
                .unwrap()
                .micros,
            Some(0)
        );
        assert_eq!(
            r.objects
                .iter()
                .find(|o| o.name == "review")
                .unwrap()
                .micros,
            None
        );
        let r = project(
            &json!({"event":"delivered","lesson":{"discovery":{"phase":"instruction","proposal":{"sources":["visit#2"]},"profile":{"facts":{"visit#2":{"kind":"correction","replaces":["visit#1"]}}}}}}),
        );
        assert!(r.objects.iter().any(|o| o.name == "Cited learner evidence"
            && o.links
                .contains(&("Corrects earlier evidence".into(), "visit#1".into()))));
    }
}
