//! The samples are synthetic live-model deliveries. All checks below run offline.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
use tutor_lab::platform::PrivateOpen;

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "tutor-judgment-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        tutor_lab::platform::create_private_dir(&path, false).unwrap();
        Self(path)
    }
    fn install(&self, sample: &str, name: &str) -> PathBuf {
        let path = self.0.join(name);
        let bytes = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/teaching-judgment")
                .join(sample),
        )
        .unwrap();
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open_private(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn binary() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tutor"));
    // Reject credentials before accessing a native keyring and keep offline fixtures
    // free of background jobs that would hold their SQLite files open on Windows.
    cmd.env_clear()
        .env("PATH", "/does-not-exist")
        .env("OPENROUTER_API_KEY", "offline fixture")
        .env("ROBOTUTOR_REPLAY_POLICY", "history");
    cmd
}
fn checked(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn replay(path: &Path) -> String {
    checked(binary().arg("--replay").arg(path).output().unwrap())
}
fn rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn terminal(dir: &Path, session: &str, commands: &str) -> String {
    let mut child = binary()
        .arg("--workspace")
        .arg(dir)
        .args(["--session", session])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(commands.as_bytes())
        .unwrap();
    checked(child.wait_with_output().unwrap())
}
#[test]
fn speaker_and_paraphrase_change_only_the_supported_treatment() {
    for (case, selected, kind, actual) in [
        ("volunteered", "use_method", "method", "wheel was wobbling"),
        (
            "cued",
            "use_prompted_method",
            "prompted_method",
            "What fact about how the bicycle was left",
        ),
        (
            "paraphrase",
            "use_method",
            "method",
            "neighbor saw the borrower",
        ),
    ] {
        let w = Workspace::new();
        let path = w.install(&format!("{case}/case.jsonl"), "case.jsonl");
        let rendered = replay(&path);
        let rows = rows(&path);
        let event = rows.last().unwrap();
        let decision = &event["judgment"]["decision"];
        assert_eq!(decision["selected"], selected);
        assert_eq!(decision["findings"][0]["kind"], kind);
        assert!(rendered.contains(actual));
        assert!(rendered.contains("Trace unavailable"));
        let reply = event["text"].as_str().unwrap();
        assert!(reply.contains(actual) && rendered.contains(reply));
        assert!(!reply.to_lowercase().contains("stands for"));
        assert!(
            event["lesson"]["observation"].is_null(),
            "recognition cannot become a grade"
        );
        assert!(rendered.contains("Verified 1 recorded decisions across 2 delivered turns"));
    }
}
#[test]
fn cross_visit_correction_rebuilds_the_learner_record_and_changes_the_next_reply() {
    let w = Workspace::new();
    for name in ["case", "correction", "return"] {
        w.install(
            &format!("volunteered/{name}.jsonl"),
            &format!("{name}.jsonl"),
        );
    }
    let text = terminal(
        &w.0,
        "return.jsonl",
        "/trace case#2\n/show correction#1\n/trace\n/quit\n",
    );
    assert!(text.contains("Correct · /show correction#1"), "{text}");
    assert!(text.contains("Actually, start with the concrete facts"));
    let path = w.0.join("return.jsonl");
    let rows = rows(&path);
    let row = rows.last().unwrap();
    assert!(row["text"]
        .as_str()
        .unwrap()
        .starts_with("Maya borrowed a violin"));
    let guidance = row["judgment"]["decision"]["guidance"].as_array().unwrap();
    assert!(guidance.iter().any(|f| f["id"] == "correction#1/1"));
    assert!(!guidance.iter().any(|f| f["id"] == "case#2/3"));
    assert_eq!(replay(&path), replay(&path));
    assert!(replay(&w.0).contains("Current workspace findings"));
    fs::rename(w.0.join("case.jsonl"), w.0.join("withdrawn")).unwrap();
    assert!(!binary()
        .arg("--replay")
        .arg(&w.0)
        .output()
        .unwrap()
        .status
        .success());
}
#[test]
fn duplicate_receipts_are_idempotent_and_conflicting_copies_fail_closed() {
    let w = Workspace::new();
    let path = w.install("volunteered/case.jsonl", "case.jsonl");
    let baseline = replay(&path);
    let mut copy = rows(&path).pop().unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        "{}",
        json!({"event":"rejected","draft":{"turn":"discard this"},"reason":"repeated question"})
    )
    .unwrap();
    writeln!(file, "{copy}").unwrap();
    drop(file);
    assert_eq!(baseline, replay(&path));
    let t = terminal(&w.0, "case.jsonl", "/trace\n/skills\n/quit\n");
    assert!(t.contains("Resumed 2 delivered exchanges"));
    copy["lesson"]["objective"] = json!("an altered task");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{copy}").unwrap();
    assert!(!binary()
        .arg("--replay")
        .arg(path)
        .output()
        .unwrap()
        .status
        .success());
}
#[test]
fn replay_detects_tampered_decisions_replies_proposals_and_policy_versions() {
    for field in ["text", "quote", "policy", "decision", "call"] {
        let w = Workspace::new();
        let path = w.install("volunteered/case.jsonl", "case.jsonl");
        let mut events = rows(&path);
        let row = events.last_mut().unwrap();
        match field {
            "text" => row["text"] = json!("A repeated question."),
            "quote" => {
                row["judgment"]["inputs"]["proposals"]["findings"][0]["quote"] =
                    json!("words the learner never used")
            }
            "policy" => row["judgment"]["decision"]["policy"] = json!("future-policy/999"),
            "decision" => row["judgment"]["decision"]["selected"] = json!("repair"),
            _ => row["judgment"]["calls"][0]["output"]["action"] = json!("pause"),
        }
        fs::write(
            &path,
            events.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        assert!(
            !binary()
                .arg("--replay")
                .arg(path)
                .output()
                .unwrap()
                .status
                .success(),
            "{field}"
        );
    }
}

#[cfg(unix)] // The scripted provider fixture is a POSIX shell executable.
#[test]
fn interrupted_provider_review_resumes_its_completed_steps_across_a_real_terminal_restart() {
    use std::os::unix::fs::PermissionsExt;
    let w = Workspace::new();
    let path = w.install("cued/case.jsonl", "case.jsonl");
    let before = rows(&path).len();
    // Only this synthetic curl is on PATH. It consumes the request and emits
    // numbered envelopes. An unexpected repeated call shifts the whole sequence.
    let curl = w.0.join("curl");
    fs::write(
        &curl,
        r#"#!/bin/sh
while IFS= read -r line; do :; done
dir=${0%/*}
n=0
if [ -f "$dir/count" ]; then read -r n < "$dir/count"; fi
n=$((n+1))
echo "$n" > "$dir/count"
if [ -f "$dir/$n" ]; then /bin/cat "$dir/$n"; else exit 70; fi
"#,
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    let good = |v: Value| {
        format!(
            "{}\n200",
            json!({"model":"openai/gpt-6-luna","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":v.to_string()}}]})
        )
    };
    let step = "Which detail would you compare first?";
    let wire = [
        good(json!({"findings":[]})),
        good(json!({"action":"question","evidence":"","note":"","target_stage":-1})),
        good(
            json!({"destination":"Explain evidence","milestones":(["Compare","Check","Argue","Recommend"].map(|s|json!({"skill":s,"application":s}))),"focus":0}),
        ),
        good(json!({"turn":{"move":"reply","response":"Consider the timing.","next_step":step}})),
        "\n401".into(),
        good(
            json!({"verdict":"revise","reason":"Define the term in plain language.","criterion":"","mode":"unassessable"}),
        ),
        good(json!({"turn":{"move":"reply","response":"Compare their timing.","next_step":step}})),
        good(
            json!({"verdict":"revise","reason":"Give a different example.","criterion":"","mode":"unassessable"}),
        ),
        good(
            json!({"turn":{"move":"reply","response":"Two clock readings can help order events.","next_step":step}}),
        ),
        "\n503".into(),
        good(json!({"verdict":"accept","reason":"","criterion":"","mode":"unassessable"})),
    ];
    for (i, contents) in wire.into_iter().enumerate() {
        fs::write(w.0.join((i + 1).to_string()), contents).unwrap();
    }
    let terminal = |commands: &str| {
        let mut child = binary()
            .env("PATH", &w.0)
            .env("OPENROUTER_API_KEY", "synthetic-test-token")
            .arg("--session")
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(commands.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        (checked(output), stderr)
    };
    let (ui, err) = terminal("What do you mean by the timing?\n/plan\n/quit\n");
    assert!(
        err.contains("HTTP 401") && ui.contains("Open task"),
        "{err}\n{ui}"
    );
    assert_eq!(fs::read_to_string(w.0.join("count")).unwrap().trim(), "5");
    let (ui, err) = terminal("/retry\n/quit\n");
    assert!(
        ui.contains("4 completed steps") && ui.contains("Two clock readings"),
        "{err}\n{ui}"
    );
    let events = rows(&path);
    let new = &events[before..];
    assert_eq!(new.iter().filter(|e| e["event"] == "learner").count(), 1);
    assert_eq!(new.iter().filter(|e| e["event"] == "delivered").count(), 1);
    let last = new
        .iter()
        .rev()
        .find(|event| event["event"] == "delivered")
        .unwrap();
    assert!(new
        .iter()
        .any(|event| event["event"] == "timing" && event["sample"]["delivered"] == true));
    assert!(last["lesson"]["observation"].is_null());
    assert_eq!(last["lesson"]["current_step"], step);
    assert!(last["judgment"]["calls"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["task"] == "edit_turn"));
    assert!(replay(&path).contains("Two clock readings"));
    assert_eq!(fs::read_to_string(w.0.join("count")).unwrap().trim(), "11");
}

#[test]
fn terminal_keeps_local_controls_and_the_pending_message_when_auxiliary_stores_and_credentials_fail(
) {
    let w = Workspace::new();
    let path = w.install("cued/case.jsonl", "case.jsonl");
    for name in ["recall.sqlite3", "supervision.sqlite3"] {
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open_private(w.0.join(name))
            .unwrap();
        f.write_all(b"synthetic broken database").unwrap();
    }
    let ui = terminal(
        &w.0,
        "case.jsonl",
        "A saved question\n/plan\n/trace\n/teacher\n/quit\n",
    );
    assert!(ui.contains("Open task") && ui.contains("Supervision is unavailable"));
    let reopened = terminal(&w.0, "case.jsonl", "/plan\n/quit\n");
    assert!(reopened.contains("last message is awaiting a reply"));
    assert!(replay(&path).contains("A saved question"));
}
