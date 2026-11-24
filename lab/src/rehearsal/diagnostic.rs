//! Voluntary, changed component work after an interpretable independent error.
//! The enclosing session supplies the learner, original attempt and eligibility.
use crate::{
    curriculum::{goal_evidence::check_quantity_text, OutcomeId},
    history::Scope,
    protocol::{Assistance, Judgment},
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
struct Form {
    id: &'static str,
    prompt: &'static str,
    hint: &'static str,
    key: i64,
}

fn component(target: OutcomeId) -> Result<Form, String> {
    match target.0 {
        1 => Ok(Form {
            id: "combined-books/1",
            prompt: "Three books are on the top shelf and four more are on the bottom shelf. How many books are on both shelves altogether? Give your count followed by ‘books’.",
            hint: "The two shelves hold separate groups. You can count from the books on one shelf through the books on the other.",
            key: 7,
        }),
        2 => Ok(Form {
            id: "borrowed-books/1",
            prompt: "A shelf held nine books. Four remain after some were borrowed. How many books were borrowed? Give your count followed by ‘books’.",
            hint: "Start from the four books that stayed. How many more would take the count back to nine?",
            key: 5,
        }),
        _ => Err("no reviewed component exercise for this objective".into()),
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    scope: Scope,
    original_attempt: u64,
    target: OutcomeId,
    form: Form,
    shown_before: bool,
    used_hint: bool,
}

#[derive(Default, Clone, Debug)]
pub struct Diagnostic {
    pending: Option<Pending>,
    seen: BTreeSet<&'static str>,
}

impl Diagnostic {
    pub fn offer(
        &mut self,
        scope: Scope,
        target: OutcomeId,
        original: Judgment,
        original_attempt: u64,
    ) -> Result<Value, String> {
        if original != Judgment::Incorrect {
            return Err("a changed diagnostic requires a checked incorrect original".into());
        }
        if self.pending.is_some() {
            return Err("a component exercise is already open".into());
        }
        let form = component(target)?;
        let pending = Pending {
            scope,
            original_attempt,
            target,
            form,
            shown_before: !self.seen.insert(form.id),
            used_hint: false,
        };
        self.pending = Some(pending);
        let display = format!(
            "You can try this separate exercise, ask for help, or skip it. {}",
            form.prompt
        );
        Ok(pending.receipt(
            &display,
            true,
            "changed_component_prompt",
            "optional_component",
            None,
        ))
    }

    pub fn help(&mut self) -> Result<Value, String> {
        let pending = self
            .pending
            .as_mut()
            .ok_or("no component exercise is open")?;
        pending.used_hint = true;
        Ok(pending.receipt(
            pending.form.hint,
            false,
            "component_hint",
            "guided_component_practice",
            None,
        ))
    }

    pub fn answer(&mut self, response: &str, reported: Assistance) -> Result<Value, String> {
        let pending = self.pending.take().ok_or("no component exercise is open")?;
        let assistance = if pending.used_hint {
            Assistance::Reported
        } else {
            reported
        };
        let judgment = check_quantity_text(pending.form.key, "books", response);
        let display = match judgment {
            Judgment::Correct => "That count matches the changed book exercise.",
            Judgment::Incorrect => {
                "That count or unit differs from the one requested in the book exercise."
            }
            Judgment::Missing | Judgment::Ambiguous => {
                "I could not check a count with its unit in that response."
            }
        };
        Ok(pending.receipt(display, false, "component_response", "component_observation",
            Some(json!({"judgment":format!("{judgment:?}"),"assistance":format!("{assistance:?}"),"reported_assistance":format!("{reported:?}")}))))
    }

    pub fn skip(&mut self) -> Result<Value, String> {
        let pending = self.pending.take().ok_or("no component exercise is open")?;
        Ok(pending.receipt(
            "You skipped the optional book exercise.",
            false,
            "component_skipped",
            "no_performance",
            None,
        ))
    }

    /// Describe the pending exercise for a returning learner. A status read
    /// never allocates another assessment or records another contact.
    pub fn view(&self) -> Value {
        self.pending.map_or(Value::Null, |p| {
            json!({"original_attempt":p.original_attempt,"objective":p.target.0,"form":p.form.id,"prompt":p.form.prompt,
                "hint_shown":p.used_hint, "previously_shown":p.shown_before})
        })
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

impl Pending {
    fn receipt(
        self,
        display: &str,
        opened: bool,
        contact: &str,
        contribution: &str,
        observation: Option<Value>,
    ) -> Value {
        json!({"display":display,"opened":opened,"contact":contact,"contribution":contribution,"credit":null,
            "original_attempt":self.original_attempt,"original":{"learner":self.scope.learner,"revision":self.scope.revision,"objective":self.target.0,"judgment":"Incorrect"},
            "diagnostic":{"form":self.form.id,"previously_shown":self.shown_before,"hint_shown":self.used_hint,"observation":observation}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_optional_count_keeps_observed_help_and_original_error_even_after_a_correct_answer() {
        let mut work = Diagnostic::default();
        let b = OutcomeId(2);
        let scope = Scope {
            learner: 11,
            revision: 3,
        };
        assert!(work.offer(scope, b, Judgment::Ambiguous, 17).is_err());
        let open = work.offer(scope, b, Judgment::Incorrect, 17).unwrap();
        assert!(open["display"].as_str().unwrap().contains("nine books"));
        work.help().unwrap();
        let result = work.answer("5 books", Assistance::NoneReported).unwrap();
        assert_eq!(result["diagnostic"]["observation"]["judgment"], "Correct");
        assert_eq!(
            result["diagnostic"]["observation"]["assistance"],
            "Reported"
        );
        assert_eq!(result["original"]["judgment"], "Incorrect");
        assert!(result["credit"].is_null());
        assert!(work.answer("5 books", Assistance::Reported).is_err());
        assert_eq!(
            work.offer(scope, b, Judgment::Incorrect, 17).unwrap()["diagnostic"]
                ["previously_shown"],
            true
        );
        work.cancel();
        assert!(work.help().is_err());
    }
}
