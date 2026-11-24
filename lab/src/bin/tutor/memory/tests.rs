use super::*;
use crate::lesson::Lesson;
use serde_json::{json, Value};
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

static SEQ: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "robotutor-memory-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        tutor_lab::platform::create_private_dir(&dir, false).unwrap();
        Self(dir)
    }
    fn write(&self, visit: &str, events: &[Value]) {
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open_private(self.0.join(format!("{visit}.jsonl")))
            .unwrap();
        for e in events {
            writeln!(f, "{e}").unwrap();
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn open() -> Value {
    json!({"event":"open","version":1,"selected":null})
}
fn exchange(learner: &str, tutor: &str) -> Vec<Value> {
    vec![
        open(),
        json!({"event":"learner","text":learner}),
        json!({"event":"delivered","text":tutor,"lesson":Lesson::default()}),
    ]
}

#[test]
fn december_semantic_recall_reaches_a_late_passage_in_march_after_restart() {
    let root = Temp::new();
    let mut w = Workspace::open(&root.0).unwrap();
    let long =
        "A short exercise. ".repeat(200) + "The learner called the bottle wrap Pocket Glacier.";
    for n in 0..305 {
        let day: String =
            w.db.query_row(
                "SELECT date('2026-03-01', ?)",
                [format!("+{n} days")],
                |r| r.get(0),
            )
            .unwrap();
        root.write(
            &day,
            &exchange(
                "Here is my submission.",
                if n == 6 {
                    &long
                } else {
                    "Review musical timing."
                },
            ),
        );
    }
    w.sync().unwrap();
    let example_vectors = |inputs: &[String]| {
        Ok(inputs
            .iter()
            .map(|s| {
                let mut v = vec![0.; EMBEDDING_DIM];
                v[usize::from(!s.contains("Pocket Glacier"))] = 1.;
                v
            })
            .collect())
    };
    w.fill_vectors(&mut { example_vectors }, usize::MAX)
        .unwrap();
    drop(w);
    let w = Workspace::open(&root.0).unwrap();
    let mut query = vec![0.; EMBEDDING_DIM];
    query[0] = 1.;
    let hits = w
        .search(
            "thermal insulation in March 2026",
            Some(&query),
            "2026-12-31",
            1,
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, "2026-03-07#1");
    assert_eq!(hits[0].day.as_deref(), Some("2026-03-07"));
    assert!(hits[0].tutor.contains("Pocket Glacier"));
    assert_eq!(w.lookup(&hits[0].source).unwrap().unwrap().tutor, long);
    assert!(w
        .search("thermal insulation", Some(&query), "2026-03-07", 1)
        .unwrap()
        .is_empty());
    assert!(w
        .search("thermal insulation in March 2025", Some(&query), "today", 1)
        .unwrap()
        .is_empty());
}

#[test]
fn recorded_edges_survive_rebuild_but_cannot_rebind_to_an_edited_turn() {
    let root = Temp::new();
    let march = "2026-03-07";
    let fall = "2026-10-08";
    root.write(
        march,
        &exchange("An earlier answer", "Initial explanation."),
    );
    let mut w = Workspace::open(&root.0).unwrap();
    w.sync().unwrap();
    let src = w.lookup("2026-03-07#1").unwrap().unwrap();
    let mut events = exchange("Portfolio feedback", "Examine the portfolio.");
    events[2]["sources"] = json!([{"source":src.source,"digest":src.digest}]);
    root.write(fall, &events);
    w.sync().unwrap();
    drop(w);
    fs::remove_file(root.0.join("recall.sqlite3")).unwrap();
    let mut w = Workspace::open(&root.0).unwrap();
    w.sync().unwrap();
    let sources = || {
        w.search("portfolio", None, "december", 1)
            .unwrap()
            .into_iter()
            .map(|h| h.source)
            .collect::<Vec<_>>()
    };
    assert_eq!(sources(), ["2026-10-08#1", "2026-03-07#1"]);
    // Same inode, previously valid prefix replaced and length increased.
    let updated = exchange(
        "A different, longer answer after editing.",
        "Replacement explanation.",
    );
    root.write(march, &updated);
    w.sync().unwrap();
    assert!(w.links("2026-10-08#1").unwrap().is_empty());
    assert_eq!(
        w.lookup("2026-03-07#1")
            .unwrap()
            .unwrap()
            .learner
            .as_deref(),
        Some("A different, longer answer after editing.")
    );
    fs::remove_file(root.0.join(format!("{march}.jsonl"))).unwrap();
    w.sync().unwrap();
    assert!(w.lookup("2026-03-07#1").unwrap().is_none());
}

#[test]
fn learner_scope_incomplete_events_roles_and_permissions_gate_candidates() {
    let alice = Temp::new();
    let bob = Temp::new();
    bob.write(
        "2026-03-07",
        &exchange("Bob's unrelated secret", "Bob's reply"),
    );
    let mut e = exchange("Retained learner statement", "Shown reply");
    e.insert(
        2,
        json!({"event":"review","draft":"unshown secret","verdict":{}}),
    );
    e.push(json!({"event":"learner","text":"unanswered secret"}));
    e.push(json!({"event":"failed","message":"provider secret"}));
    alice.write("2026-03-07", &e);
    #[cfg(unix)]
    std::os::unix::fs::symlink(bob.0.join("2026-03-07.jsonl"), alice.0.join("other.jsonl"))
        .unwrap();
    #[cfg(windows)]
    fs::hard_link(bob.0.join("2026-03-07.jsonl"), alice.0.join("other.jsonl")).unwrap();
    let path = alice.0.join("2026-03-07.jsonl");
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    let row = json!({"event":"delivered","text":"Second visible reply","lesson":Lesson::default(),"recorded_at":1797775200});
    write!(f, "{row}").unwrap();
    let mut w = Workspace::open(&alice.0).unwrap();
    w.sync().unwrap();
    assert!(w.search("secret", None, "today", 1).unwrap().is_empty());
    assert!(w.lookup("other#1").unwrap().is_none());
    assert!(w.lookup("2026-03-07#2").unwrap().is_none());
    writeln!(f).unwrap();
    w.sync().unwrap();
    let reply = w.lookup("2026-03-07#2").unwrap().unwrap();
    assert_eq!(reply.learner.as_deref(), Some("unanswered secret"));
    assert!(reply.day.unwrap().starts_with("2026-12-"));
    assert!(w.journal_path(Some(Path::new("../other.jsonl"))).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(windows)]
    fs::hard_link(&path, alice.0.join("aliased-file")).unwrap();
    w.sync().unwrap();
    assert!(w.lookup("2026-03-07#1").unwrap().is_none());
}

#[test]
fn archived_skill_restores_the_actual_task_and_credits_only_the_student_submission() {
    use crate::study::{Observation, Target};
    let root = Temp::new();
    let mut lesson = Lesson::default();
    lesson.topic = "measurement".into();
    lesson.declared_goal = Some(lesson.topic.clone());
    lesson.objective = "Compare two readings".into();
    lesson.exercise = "Which reading is more precise?".into();
    let first = json!({"event":"delivered","text":"Compare the readings.","lesson":lesson});
    let mut coached = lesson.clone();
    coached.assisted = true;
    coached.exercise = "What does the last digit tell you?".into();
    coached.observation = Some(Observation {
        target: Target::from_lesson(&lesson),
        exercise: lesson.exercise.clone(),
        action: "partial".into(),
        quote: "extra digit".into(),
        assisted: false,
        validation: "luna_only".into(),
        family: String::new(),
    });
    root.write(
        "2026-03-07",
        &[
            open(),
            first,
            json!({"event":"learner","text":"The extra digit"}),
            json!({"event":"delivered","text":"Look more closely.","lesson":coached}),
        ],
    );
    {
        let mut w = Workspace::open(&root.0).unwrap();
        w.sync().unwrap();
        let skills = w.skills().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].attempts, 1);
        assert_eq!(skills[0].checked, 0);
        assert!(skills[0].history[0].contains("2026-03-07#2"));
    }
    let mut w = Workspace::open(&root.0).unwrap();
    w.sync().unwrap();
    let (from, carried) = w.continuation("new-day").unwrap().unwrap();
    assert_eq!(from.source, "2026-03-07#2");
    assert_eq!(carried.exercise, coached.exercise);
    std::fs::remove_file(root.0.join("2026-03-07.jsonl")).unwrap();
    w.sync().unwrap();
    assert!(w.skills().unwrap().is_empty());
    assert!(w.continuation("new-day").unwrap().is_none());
}

#[test]
fn previous_context_keeps_speakers_and_rejects_changed_anchors() {
    let root = Temp::new();
    root.write(
        "prior",
        &exchange("I identified delivery.", "The tutor supplied the rest."),
    );
    let mut w = Workspace::open(&root.0).unwrap();
    w.sync().unwrap();
    let (mut anchor, _) = w.continuation("next").unwrap().unwrap();
    let history = w.continuity(&anchor).unwrap();
    assert_eq!(
        history[0].learner.as_deref(),
        Some("I identified delivery.")
    );
    assert_eq!(history[0].tutor, "The tutor supplied the rest.");
    anchor.digest.push('0');
    assert!(w.continuity(&anchor).unwrap().is_empty());
}

#[test]
fn upgrading_the_existing_index_preserves_vectors_and_replays_legacy_turns() {
    let root = Temp::new();
    let mut l = Lesson::default();
    l.objective = "Inspect evidence".into();
    l.exercise = "What does it show?".into();
    let mut l1 = json!(l);
    l1["last_decision"] = json!({"action":"start","evidence":"","target_stage":-1,"note":""});
    root.write("2026-03-07",&[open(),json!({"event":"learner","text":"Study law"}),json!({"event":"delivered","lesson":l1,"text":"Try this example.","recorded_at":1772841600})]);
    {
        let mut w = Workspace::open(&root.0).unwrap();
        w.sync().unwrap();
        w.db.execute_batch("UPDATE passages SET vector=x'01020304'; DROP TABLE continuations; DROP TABLE study_turns; ALTER TABLE turns DROP COLUMN recorded; PRAGMA user_version=1;").unwrap();
    }
    let mut w = Workspace::open(&root.0).unwrap();
    w.sync().unwrap();
    w.sync().unwrap();
    assert_eq!(
        w.db.query_row("SELECT hex(vector) FROM passages", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "01020304"
    );
    let (_, l) = w.continuation("new-day").unwrap().unwrap();
    assert_eq!(l.topic, "Study law");
    assert_eq!(w.skills().unwrap().len(), 1);
}

#[test]
fn indexing_a_torn_unicode_tail_waits_for_the_complete_record() {
    let root = Temp::new();
    root.write("one", &exchange("first", "received"));
    let mut w = Workspace::open(&root.0).unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .open(root.0.join("one.jsonl"))
        .unwrap();
    file.write_all(b"{\"event\":\"learner\",\"text\":\"caf\xc3")
        .unwrap();
    w.sync().unwrap();
    assert_eq!(w.lookup("one#1").unwrap().unwrap().tutor, "received");
    file.write_all(b"\xa9\"}\n").unwrap();
    writeln!(
        file,
        "{}",
        json!({"event":"delivered","text":"next","lesson":Lesson::default()})
    )
    .unwrap();
    w.sync().unwrap();
    assert_eq!(
        w.lookup("one#2").unwrap().unwrap().learner.as_deref(),
        Some("café")
    );
}

#[test]
fn a_redirect_after_the_last_delivery_carries_across_visits_and_index_upgrade() {
    let root = Temp::new();
    let mut w = Workspace::open(&root.0).unwrap();
    root.write("source", &exchange("first", "received"));
    w.sync().unwrap();
    let skill_count = w.skills().unwrap().len();
    let (anchor, mut lesson) = w.continuation("later").unwrap().unwrap();
    lesson.recall_items = serde_json::from_value(json!([{"term":"term","reference":"meaning","source":"source#1","state":"asking","resume":"Old task"}])).unwrap();
    lesson.name_recall = Some(crate::method::NameRecall::Asking);
    let redirect = json!({"event":"supervisor_plan","replace_prompt":true,"revision":4,"objective":"Next application","task":"Explain the new case","criterion":"Justify the choice","recorded_at":1800000000});
    root.write(
        "returned",
        &[
            open(),
            json!({"event":"continued","from":anchor,"lesson":lesson}),
            redirect,
        ],
    );
    for pass in 0..2 {
        if pass == 1 {
            w.db.execute("DELETE FROM plan_heads", []).unwrap();
        }
        w.sync().unwrap();
        let (link, carried) = w.continuation("third").unwrap().unwrap();
        assert_eq!(link.source, anchor.source);
        assert_eq!(carried.active_step(), "Explain the new case");
        assert!(carried.asking_meaning().is_none() && !carried.asking_name());
        assert_eq!(carried.recall_items[0].state, crate::recall::State::Waiting);
        assert_eq!(w.skills().unwrap().len(), skill_count);
    }
    root.write("source", &exchange("edited", "received"));
    w.sync().unwrap();
    assert_ne!(
        w.continuation("third").unwrap().unwrap().1.active_step(),
        "Explain the new case"
    );
}

#[test]
fn duplicate_detection_cannot_rebind_a_receipt_to_a_different_visit() {
    let root = Temp::new();
    let events: Vec<Value> =
        include_str!("../../../../fixtures/teaching-judgment/volunteered/case.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let mut w = Workspace::open(&root.0).unwrap();
    root.write("case", &events);
    w.sync().unwrap();
    root.write("another", &events);
    assert!(w
        .sync()
        .unwrap_err()
        .to_string()
        .contains("Unexpected decision source"));
    assert!(w.lookup("another#1").unwrap().is_none());
    assert!(w.lookup("case#2").unwrap().is_some());
}
