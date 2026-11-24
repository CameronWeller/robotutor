//! A low-stakes check of a previously taught meaning, independent of the case grade.
use crate::{
    judgment::{mentions, Inputs},
    lesson::{object, Generate, Lesson},
    method::NameRecall,
    session::Visit,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Waiting,
    Due,
    Asking,
    Recalled,
    Partial,
    Reminded,
    Skipped,
    Compared,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub term: String,
    pub reference: String,
    pub source: String,
    pub state: State,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    resume: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    taught_objective: String,
}

impl Item {
    fn relevant(&self, objective: &str, context: &[&str]) -> bool {
        (!self.taught_objective.is_empty() && self.taught_objective == objective)
            || context.iter().any(|text| mentions(text, &self.term))
    }

    pub fn redirect(&mut self, step: &str) {
        if self.state == State::Asking {
            self.resume = step.into();
        }
    }
    pub fn prompt(&self) -> String {
        if self.term.len() >= 2 && self.term.chars().all(|c| c.is_uppercase()) {
            format!("What does {} stand for? You can explain what its parts mean in your own words, or say ‘skip’.",self.term)
        } else {
            format!(
                "What does “{}” mean here? Explain it in your own words, or say ‘skip’.",
                self.term
            )
        }
    }
}

fn reference_source(visit: &Visit, input: &Inputs, ordinal: usize) -> String {
    visit
        .decisions
        .get(ordinal - 1)
        .and_then(|r| r.as_ref())
        .map(|r| r.inputs.source.clone())
        .unwrap_or_else(|| {
            format!(
                "{}#{ordinal}",
                input.source.rsplit_once('#').map_or("visit", |(id, _)| id)
            )
        })
}

fn irac_source(visit: &Visit) -> Option<(usize, &str)> {
    if !visit.lesson.studying_law() || visit.lesson.name_recall.is_none() {
        return None;
    }
    visit
        .history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, (_, text))| {
            if !crate::method::supplies_name(text) {
                return None;
            }
            text.lines()
                .find(|line| crate::method::supplies_name(line))
                .map(|line| (i + 1, line))
        })
}

pub fn legacy_due(visit: &Visit) -> bool {
    !visit.lesson.explanation_only
        && !visit.lesson.recall_items.iter().any(|i| i.term == "IRAC")
        && matches!(
            visit.lesson.name_recall,
            Some(NameRecall::Due | NameRecall::Recalled | NameRecall::Reminded)
        )
        && irac_source(visit).is_some()
}

/// Catch explicit definitions even when the optional observer is unavailable.
/// More varied definitions are nominated by the observer and checked against the visible text.
fn explicit_definition(text: &str) -> Option<(String, String)> {
    for line in text.lines() {
        for sentence in line.split_terminator(['.', '!', '?']) {
            let raw = sentence.trim();
            let plain = raw.replace("**", "").replace('`', "");
            let lower = plain.to_ascii_lowercase();
            for sep in [" stands for ", " means ", " refers to ", " is "] {
                let Some(offset) = lower.find(sep) else {
                    continue;
                };
                let term = plain[..offset].trim();
                let meaning = plain[offset + sep.len()..].trim();
                if term.is_empty()
                    || term.len() > 64
                    || meaning.len() < 5
                    || meaning.len() > 450
                    || term.split_whitespace().count() > 5
                    || !term
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '’'))
                    || [
                        "that",
                        "this",
                        "it",
                        "which",
                        "what",
                        "the answer",
                        "the question",
                        "the result",
                        "you",
                        "i",
                        "he",
                        "she",
                        "there",
                    ]
                    .contains(&term.to_ascii_lowercase().as_str())
                    || (sep == " is " && !raw.starts_with("**"))
                {
                    continue;
                }
                return Some((term.into(), meaning.into()));
            }
        }
    }
    None
}

fn contains_definition(text: &str, item: &Item) -> bool {
    text.to_lowercase()
        .contains(&item.reference.trim_end_matches('.').to_lowercase())
        || explicit_definition(text).is_some_and(|(term, _)| term.eq_ignore_ascii_case(&item.term))
}

impl Lesson {
    pub fn defer_open_recall(&mut self) {
        for item in &mut self.recall_items {
            if matches!(item.state, State::Asking | State::Due) {
                item.state = State::Waiting;
                item.resume.clear();
            }
        }
        if self.asking_name() || self.recall_due() {
            self.name_recall = Some(NameRecall::Supplied);
        }
    }
    pub fn asking_meaning(&self) -> Option<&Item> {
        self.recall_items.iter().find(|i| i.state == State::Asking)
    }
    pub fn label_check_due(&self) -> bool {
        self.recall_due()
            && !self
                .recall_items
                .iter()
                .any(|i| i.term.eq_ignore_ascii_case("IRAC"))
    }
    pub fn meaning_due(&self) -> bool {
        self.recall_items.iter().any(|i| i.state == State::Due)
    }
    fn remember_meaning(
        &mut self,
        term: &str,
        reference: &str,
        source: String,
        state: State,
        scope_to_objective: bool,
    ) {
        let term = term.trim();
        if term.is_empty()
            || term.chars().count() > 72
            || reference.trim().is_empty()
            || self
                .recall_items
                .iter()
                .any(|i| i.term.eq_ignore_ascii_case(term))
        {
            return;
        }
        if self.recall_items.len() >= 6 {
            if let Some(i) = self
                .recall_items
                .iter()
                .position(|i| !matches!(i.state, State::Asking | State::Due | State::Waiting))
            {
                self.recall_items.remove(i);
            } else {
                return;
            }
        }
        self.recall_items.push(Item {
            term: term.into(),
            reference: reference.trim().into(),
            source,
            state,
            resume: String::new(),
            taught_objective: if scope_to_objective {
                self.objective.clone()
            } else {
                String::new()
            },
        });
    }

    pub fn capture_meaning(&mut self, visit: &Visit, input: &Inputs) {
        // A supervisor may have replaced the exercise since the quoted lesson.
        let same_lesson = matches!(
            input.version.as_str(),
            crate::judgment::ELEVENTH_VERSION | crate::judgment::VERSION
        ) && visit.last_plan_at.is_none();
        if let Some((n, _)) = irac_source(visit) {
            let state = if legacy_due(visit) {
                State::Due
            } else {
                State::Waiting
            };
            self.remember_meaning(
                "IRAC",
                "Issue, Rule, Application, Conclusion.",
                reference_source(visit, input, n),
                state,
                same_lesson && visit.teaching_history().len() == n,
            );
        }
        let Some((_, last)) = visit.teaching_history().last() else {
            return;
        };
        if visit
            .decisions
            .last()
            .and_then(|d| d.as_ref())
            .is_some_and(|d| d.decision.delivered_action.starts_with("meaning_"))
        {
            return;
        }
        let candidate = &input.proposals["recall_candidate"];
        if let (Some(term), Some(quote)) = (
            candidate["term"].as_str(),
            candidate["definition_quote"].as_str(),
        ) {
            if quote.len() <= 600
                && quote.len() > term.len()
                && last
                    .split("\n\n")
                    .any(|paragraph| paragraph.contains(quote) && mentions(paragraph, term))
                && !quote.trim_end().ends_with('?')
            {
                self.remember_meaning(
                    term,
                    quote,
                    reference_source(visit, input, visit.teaching_history().len()),
                    State::Waiting,
                    same_lesson,
                );
            }
        }
    }

    pub fn finish_meaning_turn(
        &mut self,
        prior: &Lesson,
        words: Option<&str>,
        source: &str,
        text: &mut String,
        scoped: bool,
    ) {
        if self.declared_goal != prior.declared_goal {
            self.recall_items.clear();
        }
        if self.explanation_only || self.action() == "pause" {
            return;
        }
        let worked = self
            .observation
            .as_ref()
            .is_some_and(|o| !o.quote.is_empty() && !o.exercise.is_empty())
            || (matches!(self.action(), "discussion" | "discuss_guess")
                && words.is_some_and(|s| s.split_whitespace().count() >= 4));
        if worked {
            let context = [words.unwrap_or_default(), &self.exercise, text.as_str()];
            for i in &mut self.recall_items {
                if scoped && !i.relevant(&self.objective, &context) {
                    if i.state == State::Due {
                        i.state = State::Waiting;
                    }
                    continue;
                }
                if i.state == State::Waiting {
                    if words.is_some_and(|s| {
                        s.to_lowercase()
                            .contains(&i.reference.trim_end_matches('.').to_lowercase())
                    }) {
                        i.state = State::Recalled;
                    } else if !contains_definition(text, i) {
                        i.state = State::Due;
                    }
                }
            }
        }
        // A clarification, request to redirect, or expression of difficulty takes precedence.
        if worked
            && matches!(
                self.action(),
                "complete" | "partial" | "repair" | "discussion" | "discuss_guess"
            )
        {
            let active = self.active_step().to_owned();
            if let Some(i) = self
                .recall_items
                .iter_mut()
                .find(|i| i.state == State::Due && !contains_definition(text, i))
            {
                // Return feedback on the submitted work now. Hold the complete next
                // instruction if its setup can't be separated from the question.
                let parts = text
                    .strip_suffix(&active)
                    .filter(|_| !active.is_empty())
                    .map(|prefix| (prefix, active.as_str()))
                    .or_else(|| {
                        if scoped {
                            None
                        } else {
                            text.rsplit_once("\n\n")
                        }
                    });
                if let Some((prefix, next)) =
                    parts.filter(|(p, n)| !p.trim().is_empty() && !n.trim().is_empty())
                {
                    i.resume = next.trim().into();
                    *text = format!("{}\n\n{}", prefix.trim_end(), i.prompt());
                    i.state = State::Asking;
                } else if !scoped {
                    i.resume = std::mem::take(text);
                    *text = i.prompt();
                    i.state = State::Asking;
                }
            }
        }
        if let Some((term, definition)) = explicit_definition(text) {
            self.remember_meaning(&term, &definition, source.into(), State::Waiting, scoped);
        }
        if crate::method::supplies_name(text) && self.studying_law() {
            self.remember_meaning(
                "IRAC",
                "Issue, Rule, Application, Conclusion.",
                source.into(),
                State::Waiting,
                scoped,
            );
        }
    }

    pub fn meaning_turn(
        &mut self,
        words: Option<&str>,
        scoped: bool,
        generate: &mut Generate,
    ) -> Result<Option<(&'static str, &'static str, String)>, String> {
        let returning = matches!(words, None | Some("/continue"));
        if self.explanation_only {
            return Ok(None);
        }
        if scoped && returning {
            let step = self.active_step().to_owned();
            for item in &mut self.recall_items {
                if item.state == State::Due
                    && !item.relevant(&self.objective, &[&self.exercise, &step])
                {
                    item.state = State::Waiting;
                }
            }
        }
        if returning && self.asking_meaning().is_none() && self.meaning_due() {
            let step = self.active_step().to_owned();
            let item = self
                .recall_items
                .iter_mut()
                .find(|i| i.state == State::Due)
                .unwrap();
            item.state = State::Asking;
            item.resume = step;
        }
        let Some(item) = self
            .recall_items
            .iter_mut()
            .find(|i| i.state == State::Asking)
        else {
            return Ok(None);
        };
        if returning {
            return Ok(Some((
                "meaning_check",
                "A previously taught concept is ready for a recall check.",
                item.prompt(),
            )));
        }
        let answer = words.unwrap_or_default().trim();
        let lower = answer.trim_end_matches(['.', '!', '?']).to_lowercase();
        let normalized = |s: &str| {
            s.split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (state, feedback) = if matches!(lower.as_str(), "skip" | "/skip" | "pass") {
            (State::Skipped, "We’ll leave that check here.".into())
        } else if matches!(
            lower.as_str(),
            "hint"
                | "tell me"
                | "i forgot"
                | "i don't know"
                | "i don’t know"
                | "i dont know"
                | "i can't remember"
                | "i can’t remember"
                | "i cant remember"
                | "no idea"
        ) {
            (
                State::Reminded,
                format!("Here’s the reminder: {}", item.reference),
            )
        } else if lower.starts_with('/') {
            item.state = State::Skipped;
            item.resume.clear();
            return Ok(None);
        } else if normalized(answer) == normalized(&item.reference) {
            (
                State::Recalled,
                format!("Yes, that’s what {} means.", item.term),
            )
        } else if lower == item.term.to_lowercase() {
            return Ok(Some((
                "meaning_check",
                "The learner supplied the name. Its meaning is still open.",
                format!("Yes, that’s the name. {}", item.prompt()),
            )));
        } else {
            let packet = json!({"task":"check_meaning","term":item.term,"reference":item.reference,"learner_answer":answer});
            let schema = object(
                json!({"result":{"type":"string","enum":["recalled","partial","remind","other"]},"quote":{"type":"string"}}),
            );
            let result = generate(&packet, &schema)?;
            let cited = result["quote"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && answer.contains(s));
            match result["result"].as_str() {
                Some("other")=>{item.state=State::Skipped;item.resume.clear();return Ok(None);}
                Some("recalled") if cited=>(State::Recalled,format!("Yes, that captures the meaning of {}.",item.term)),
                Some("partial") if cited=>(State::Partial,format!("You have part of it. Compare your answer with the original explanation: {}",item.reference)),
                Some("remind")=>(State::Reminded,format!("Here’s the reminder: {}",item.reference)),
                _=>(State::Compared,format!("Compare your answer with the original explanation: {}",item.reference)),
            }
        };
        item.state = state;
        let resume = std::mem::take(&mut item.resume);
        let remainder = if resume.is_empty() {
            "Use /continue to resume the lesson."
        } else {
            &resume
        };
        Ok(Some((
            "meaning_answer",
            "This response updates recall of the concept’s meaning separately from application.",
            format!("{feedback}\n\n{remainder}"),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_explicit_definitions_without_an_observer_or_subject_dictionary() {
        for (sentence, term, meaning) in [
            (
                "**Osmosis** is net water movement across a selectively permeable membrane.",
                "Osmosis",
                "net water movement across a selectively permeable membrane",
            ),
            (
                "RMS stands for root mean square.",
                "RMS",
                "root mean square",
            ),
            (
                "Syncopation means placing emphasis on a normally weak beat.",
                "Syncopation",
                "placing emphasis on a normally weak beat",
            ),
        ] {
            assert_eq!(
                explicit_definition(sentence),
                Some((term.into(), meaning.into()))
            );
        }
        assert!(explicit_definition("This is where to start.").is_none());
    }
}
