//! Instructional pacing is separate from the assessment of a submitted exercise.
use crate::{judgment::Inputs, lesson::Lesson};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reading {
    pub source: String,
    pub operation: String,
    pub quote: String,
    pub assisted: bool,
    pub choice: String,
    pub reason: String,
    pub next_skill: String,
    pub application: String,
}

pub fn schema() -> Value {
    let text = json!({"type":"string"});
    crate::lesson::object(
        json!({"operation":text,"quote":text,"choice":{"type":"string","enum":["advance","distinguish","stay","requested_practice"]},"reason":text,"next_skill":text,"application":text}),
    )
}

pub fn read(input: &Inputs, lesson: &Lesson, words: &str) -> Option<Reading> {
    let candidate = &input.proposals["progression"];
    let field = |key: &str| {
        candidate[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 800)
            .map(str::to_owned)
    };
    let quote = field("quote")?;
    if !words.contains(&quote) {
        return None;
    }
    let operation = field("operation")?;
    let mut choice = field("choice")?;
    let next_skill = field("next_skill")?;
    let reason = field("reason");
    let repeated = lesson
        .reasoning
        .last()
        .is_some_and(|r| r.operation.eq_ignore_ascii_case(&operation));
    match choice.as_str() {
        "stay" if repeated && reason.is_none() => choice = "distinguish".into(),
        "advance" | "distinguish" | "stay" | "requested_practice" => {}
        _ => return None,
    }
    if choice == "advance"
        && (next_skill.eq_ignore_ascii_case(&operation)
            || next_skill.eq_ignore_ascii_case(&lesson.objective))
    {
        return None;
    }
    Some(Reading {
        source: input.source.clone(),
        operation,
        quote,
        assisted: lesson.assisted,
        choice,
        reason: reason
            .unwrap_or_else(|| "Change the conditions to identify what is still uncertain.".into()),
        next_skill,
        application: field("application")?,
    })
}
