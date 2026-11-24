//! An investigation adapter. The caller resolves current authority before each
//! callback and display. A new grant, even over identical text, gets a new epoch.
use serde_json::Value;
use std::ops::Range;
use tutor_lab::openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA};
use tutor_lab::workshop::{
    draft_model::{ExportChoice, Suggestion},
    live, Focus, Workshop,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Grant {
    pub artifact: (u64, u64), // Learner and project, resolved by the local store.
    pub epoch: u64,
    pub revision: u64,
    pub focus: Focus,
    pub selected: Range<usize>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Asked {
    grant: Grant,
    serial: u64,
    question: String,
}

pub(super) enum Turn {
    Advice(Suggestion),
    Ask(Suggestion, Asked),
    Abstain,
}

pub(super) struct Selection {
    grant: Grant,
    excerpt: Workshop,
    serial: u64,
    pending: Option<Asked>,
}

impl Selection {
    pub fn new(artifact: (u64, u64), work: &Workshop, grant: Grant) -> Result<Self, String> {
        if artifact != grant.artifact
            || work.focus() != grant.focus
            || work.revision() != grant.revision
        {
            return Err("stale source".into());
        }
        let text = work
            .draft()
            .get(grant.selected.clone())
            .ok_or("invalid UTF-8 selection")?;
        let excerpt = Workshop::new(text, work.focus())?;
        Ok(Self {
            grant,
            excerpt,
            serial: 0,
            pending: None,
        })
    }

    pub fn coach(
        &mut self,
        current: impl Fn() -> Option<Grant>,
        reply: Option<(&Asked, &str)>,
        mut send: impl FnMut(Role, &Value) -> Result<Value, String>,
    ) -> Result<Turn, String> {
        let authorize = || {
            if current().as_ref() == Some(&self.grant) {
                Ok(())
            } else {
                Err("the selected passage is no longer shared".to_owned())
            }
        };
        authorize()?;
        let exchange = match reply {
            Some((asked, answer)) if self.pending.as_ref() == Some(asked) => {
                Some((asked.question.as_str(), answer))
            }
            Some(_) => return Err("no matching open clarification for this selection".into()),
            None => None,
        };
        let report = live::coach_with_reply(
            &self.excerpt,
            DEFAULT_JEV,
            DEFAULT_LUNA,
            ExportChoice::ShareThisDraft,
            exchange,
            |role, request| {
                authorize()?;
                send(role, request)
            },
        )
        .map_err(|failure| failure.message)?;
        authorize()?;
        self.pending = None;
        let locate = |mut note: Suggestion| {
            note.span = self.grant.selected.start + note.span.start
                ..self.grant.selected.start + note.span.end;
            note
        };
        match report.outcome {
            live::Outcome::Suggestion(note) => Ok(Turn::Advice(locate(note))),
            live::Outcome::Clarify(note) => {
                self.serial = self
                    .serial
                    .checked_add(1)
                    .ok_or("question serial exhausted")?;
                let asked = Asked {
                    grant: self.grant.clone(),
                    serial: self.serial,
                    question: note.question.clone(),
                };
                self.pending = Some(asked.clone());
                Ok(Turn::Ask(locate(note), asked))
            }
            live::Outcome::Abstain => Ok(Turn::Abstain),
        }
    }
}
