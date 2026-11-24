//! Deterministic fictional history, replayable through ordinary discovery and session code.

use crate::{discovery::Discovery, lesson::Lesson, Event, Journal, Visit};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Deserialize)]
struct Exchange {
    words: String,
    action: String,
    reply: String,
    reason: String,
    #[serde(default)]
    area: String,
    #[serde(default)]
    findings: Vec<Value>,
    start: Option<Value>,
}

fn visit(
    dir: &Path,
    name: &str,
    mut lesson: Lesson,
    turns: &[Exchange],
    accept: bool,
    at: u64,
) -> Result<Lesson, String> {
    let mut state = Visit::default();
    let mut journal = Journal::open(&dir.join(format!("{name}.jsonl")), &mut state)?;
    let mut clock = at;
    let mut save = |state: &mut Visit, event| {
        clock += 1;
        state.commit(event, clock, |e| journal.save_at(e, clock))
    };
    save(
        &mut state,
        Event::Open {
            version: 1,
            selected: None,
        },
    )?;
    let opening = lesson
        .discovery
        .as_ref()
        .ok_or("Missing demo learner account")?
        .return_prompt();
    let delivered = |lesson: &Lesson, text| Event::Delivered {
        lesson: Box::new(lesson.clone()),
        text,
        judgment: None,
        sources: vec![],
    };
    save(&mut state, delivered(&lesson, opening))?;
    let mut reply = |words: &str, decision: Value| -> Result<(), String> {
        let source = format!("{name}#{}", state.history.len() + 1);
        save(
            &mut state,
            Event::Learner {
                text: words.to_owned(),
            },
        )?;
        let discovery = lesson
            .discovery
            .clone()
            .ok_or("Missing demo learner account")?;
        let text = discovery.next(&mut lesson, words, &source, false, &mut |_, _| {
            Ok(decision.clone())
        })?;
        save(&mut state, delivered(&lesson, text))
    };
    for turn in turns {
        let mut decision = json!({"action":turn.action});
        if let Some(start) = &turn.start {
            decision["start"] = start.clone();
        }
        reply(
            &turn.words,
            json!({"decision":decision, "subject":"Algebra", "areas":["Rates","Graphs"],
            "area":turn.area,"findings":turn.findings,"reply":turn.reply,"reason":turn.reason}),
        )?;
    }
    if accept {
        reply("/start", Value::Null)?;
    }
    Ok(lesson)
}

pub fn seed(name: &str, dir: &Path) -> Result<(), String> {
    if !matches!(name, "new" | "returning" | "known") {
        return Err("Choose new, returning, or known.".into());
    }
    // Never seed or reset an existing directory. The caller publishes the new
    // generation only after these private journals have been successfully written.
    tutor_lab::platform::create_private_dir(dir, false).map_err(|e| e.to_string())?;
    if name == "new" {
        return Ok(());
    }
    let [intro, workshop]: [Vec<Exchange>; 2] =
        serde_json::from_str(include_str!("personas/visits.json")).map_err(|e| e.to_string())?;
    let today = crate::session_time()?.saturating_sub(60);
    let mut start = Lesson::default();
    start.discovery = Some(Discovery::new("Algebra".into(), Default::default()));
    let mut lesson = visit(
        dir,
        "first-visit",
        start,
        &intro,
        false,
        today.saturating_sub(7 * 86400),
    )?;
    if name == "known" {
        lesson = visit(
            dir,
            "workshop",
            lesson,
            &workshop,
            false,
            today.saturating_sub(2 * 86400),
        )?;
    }
    visit(dir, "current", lesson, &[], name == "known", today)?;
    Ok(())
}
