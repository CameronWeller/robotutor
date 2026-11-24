//! Receipts and validation shared by course and private coaching turns.
use crate::openrouter::reported_model;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq)]
pub struct Attribution {
    pub requested: String,
    pub reported: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Configuration,
    Jev,
    Luna,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    pub stage: Stage,
    pub calls: u8,
    pub message: String,
}

pub(crate) fn failure(stage: Stage, message: impl Into<String>) -> Failure {
    Failure {
        stage,
        message: message.into(),
        calls: match stage {
            Stage::Configuration => 0,
            Stage::Jev => 1,
            Stage::Luna => 2,
        },
    }
}

pub(crate) fn attribution(
    model: &str,
    reply: &Value,
    stage: Stage,
) -> Result<Attribution, Failure> {
    let reported = reported_model(reply, model).map_err(|message| failure(stage, message))?;
    Ok(Attribution {
        requested: model.into(),
        reported: reported.into(),
    })
}

pub(crate) fn binding(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(crate) fn usable_line(text: &str, cap: usize) -> bool {
    !text.trim().is_empty()
        && text.len() <= cap
        && !text.chars().any(|c| {
            c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}
