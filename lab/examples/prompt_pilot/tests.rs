use super::*;
use std::cell::Cell;
use uuid::Uuid;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(env::temp_dir().join(format!("robotutor-prompt-pilot-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn pilot() -> Frozen {
    freeze(
        serde_json::from_str(include_str!("../../fixtures/prompt-pilot/pilot.json")).unwrap(),
        "offline-test",
    )
    .unwrap()
}
fn capture(words: Value) -> Value {
    json!({"model":MODEL,"provider":"OpenAI","usage":{"total_tokens":225,"cost":0.00005},"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":words.to_string()}}]})
}
fn fixture_reply(r: &Value) -> Result<Value> {
    let sys = r["messages"][0]["content"].as_str().unwrap();
    let messages = r["messages"].as_array().unwrap();
    let data: Value = serde_json::from_str(messages.last().unwrap()["content"].as_str().unwrap())?;
    let answer = if sys == LEARNER || sys == REALIZE {
        assert!(data.get("criteria").is_none() && data.get("review").is_none());
        let history = data["own_experience"].as_array().unwrap();
        let seen_a = history.iter().any(|e| e["words"] == "Tutor alpha");
        let seen_b = history.iter().any(|e| e["words"] == "Tutor beta");
        assert!(!(seen_a && seen_b));
        json!({"event":if data["modeled_condition"]=="request help"{"help_requested"}else{"answer"},"words":"My written response"})
    } else if sys == QUALITY {
        let quote = data["exchange"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|e| e["speaker"] == "learner")
            .unwrap()["words"]
            .clone();
        assert!(data.get("version").is_none());
        json!({"uncertainty":"position or ownership","reply_quote":quote,"adaptation":"Reused the proposed rule.",
            "reply_changed_move":"unclear","mapping":"retrieval","mapping_reason":"Learner attempted the question."})
    } else if sys == SCORE {
        assert_eq!(data.as_object().unwrap().len(), 3);
        assert!(
            data["response"].is_string() && data["criteria"].is_array() && data["task"].is_string()
        );
        let facet =
            json!({"met":true,"evidence":data["response"],"reason":"Scripted scoring fixture."});
        json!({"first":facet,"second":facet})
    } else {
        assert_eq!(messages[1]["content"], FORMAT);
        assert_eq!(messages[2]["content"], ROUTE);
        json!({"words":if sys.contains("After a correct answer whose reasoning is ambiguous"){"Tutor beta"}else{"Tutor alpha"}})
    };
    Ok(capture(answer))
}

#[test]
fn frozen_text_is_the_source_baseline_and_single_instruction_candidate() {
    let f = pilot();
    let base = include_str!("../../src/bin/tutor/teacher.txt").replace("\r\n", "\n");
    assert_eq!(f.versions[0].1, base);
    assert_eq!(
        f.versions[0].0.sha256,
        "32510d1ff48f45edbdacef825993817306a2865b4f4d1403aab9e8dab683c370"
    );
    assert_eq!(
        f.versions[1].0.sha256,
        "a92a4876e20cede73ab4e271ebaeada29fe0ccb48288af371f8e7f17796d1a66"
    );
    assert!(f.versions[1].1.starts_with(&base));
    assert_eq!(estimate(&f)["max_inference_calls"], 540);
}

#[test]
fn paired_histories_blind_scoring_assistance_and_resume_replay_all_ten_cases() {
    let dir = Temp::new();
    let f = pilot();
    let dispatched = Cell::new(0);
    let mut send = |r: &Value| {
        dispatched.set(dispatched.get() + 1);
        fixture_reply(r)
    };
    let mut journal = Journal::open(&dir.0, &f.manifest, f.settings.clone(), false).unwrap();
    // Stop between completed calls, with no uncertain provider reservation.
    journal.settings_for_test_limit(7);
    assert!(study::run(&f, &mut journal, &mut send)
        .unwrap_err()
        .to_string()
        .contains("Hard pilot limit"));
    assert_eq!(dispatched.get(), 7);
    drop(journal);
    let mut journal = Journal::open(&dir.0, &f.manifest, f.settings.clone(), false).unwrap();
    let report = study::run(&f, &mut journal, &mut send).unwrap();
    assert_eq!(report["paired_cases"].as_array().unwrap().len(), 20);
    assert_eq!(journal.calls, dispatched.get());
    assert!(journal.calls <= 540);
    let mut assisted = 0;
    let mut missed = 0;
    for pair in report["paired_cases"].as_array().unwrap() {
        let arms = &pair["arms"];
        for visit in 0..2 {
            let base = &arms["baseline"]["outcomes"][visit];
            let cand = &arms["candidate"]["outcomes"][visit];
            if pair["simulation"] == "explicit" {
                assert_eq!(
                    base["model_draw"]["attended"],
                    cand["model_draw"]["attended"]
                );
                assert_eq!(base["model_draw"]["help"], cand["model_draw"]["help"]);
                if base["response"]["event"] == "assisted" {
                    assisted += 1;
                    assert!(base["score"].is_null() && base["worked_before_answer"].is_string());
                }
                if base["response"]["event"] == "missed" {
                    missed += 1;
                    assert!(base["score"].is_null());
                }
            } else {
                assert_eq!(pair["ceiling_case"], true);
            }
        }
    }
    let db = rusqlite::Connection::open(dir.0.join("receipts.sqlite")).unwrap();
    let mut q = db
        .prepare("SELECT request FROM calls WHERE id LIKE '%/agent/control/%/delayed'")
        .unwrap();
    let controls: Vec<String> = q
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(controls.len(), 20);
    for body in controls {
        let body: Value = serde_json::from_str(&body).unwrap();
        let input: Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(input["own_experience"], json!([]));
        assert_eq!(input.as_object().unwrap().len(), 4);
    }
    assert!(assisted > 0 && missed > 0);
    assert!(report["traces"]["candidate_advantage"].is_null());
    drop(journal);
    let mut replay = Journal::open(&dir.0, &f.manifest, f.settings.clone(), true).unwrap();
    assert_eq!(
        report,
        study::run(&f, &mut replay, &mut |_| panic!("duplicate call")).unwrap()
    );
}

#[test]
fn mismatched_prompt_model_uncertain_call_and_budget_block_dispatch() {
    let f = pilot();
    let body = request(
        &[LEARNER],
        json!({"task":"practice"}),
        json!({"type":"object"}),
        2,
    );
    let dir = Temp::new();
    let mut journal = Journal::open(&dir.0, &f.manifest, f.settings.clone(), false).unwrap();
    journal
        .call("one", body.clone(), &mut |_| Ok(capture(json!({}))))
        .unwrap();
    let mut changed = body.clone();
    changed["messages"][0]["content"] = json!("replacement");
    assert!(journal
        .call("one", changed, &mut |_| panic!("replayed different prompt"))
        .is_err());
    let mut wrong = capture(json!({}));
    wrong["model"] = json!("another-version");
    assert!(journal
        .call("two", body.clone(), &mut |_| Ok(wrong.clone()))
        .is_err());
    drop(journal);
    assert!(Journal::open(&dir.0, &f.manifest, f.settings.clone(), false).is_err());
    let dir = Temp::new();
    let mut journal = Journal::open(&dir.0, &f.manifest, f.settings.clone(), false).unwrap();
    assert!(journal
        .call("uncertain", body.clone(), &mut |_| Err("timeout".into()))
        .is_err());
    drop(journal);
    assert!(Journal::open(&dir.0, &f.manifest, f.settings.clone(), false).is_err());
    for (tokens, micros) in [(1, 1_000_000), (1_000_000, 1)] {
        let dir = Temp::new();
        let mut settings = f.settings.clone();
        settings.token_limit = tokens;
        settings.budget_microusd = micros;
        let mut journal = Journal::open(&dir.0, &f.manifest, settings, false).unwrap();
        assert!(journal
            .call("budget", body.clone(), &mut |_| panic!("exceeded budget"))
            .is_err());
    }
}

#[test]
fn promotion_requires_reviewed_live_report_and_preserves_the_previous_reference() {
    let root = Temp::new();
    fs::create_dir_all(&root.0).unwrap();
    let registry = root.0.join("promotions");
    let approval = root.0.join("approval.json");
    let mut study = pilot();
    let report = json!({"observations":"fictional promotion test"});
    let decision = json!({"previous":"luna-001","next":"luna-002","approved_by":"Fictional operator",
        "decision":"Adopt for the next comparison.","report_sha256":hash(&serde_json::to_vec_pretty(&report).unwrap())});
    fs::write(&approval, decision.to_string()).unwrap();
    assert!(promotion::record(&registry, &approval, &study, &report).is_err());
    study.manifest["mode"] = json!("live");
    assert!(promotion::record(&registry, &approval, &study, &json!({"changed":true})).is_err());
    promotion::record(&registry, &approval, &study, &report).unwrap();
    assert_eq!(promotion::canonical(&registry).unwrap(), "luna-002");
    assert_eq!(
        serde_json::from_str::<Value>(&read(&registry.join("000001.json")).unwrap()).unwrap(),
        decision
    );
    assert!(promotion::record(&registry, &approval, &study, &report).is_err());
}
