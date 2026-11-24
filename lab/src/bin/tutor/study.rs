//! Learner-facing projections of delivered work. Each judgment stays attached to the task it concerns.
use crate::lesson::Lesson;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tutor_lab::curriculum::OutcomeId;
use tutor_lab::history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn};
use tutor_lab::material::document_id;
use tutor_lab::protocol::{Assistance, Judgment, Question};
use tutor_lab::retention::{self, Cell, CheckResult, Exposure, Moment, Prior};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Target {
    pub topic: String,
    pub objective: String,
    pub card: Option<String>,
    pub stage: usize,
    #[serde(default)]
    pub claim: Option<String>,
}
impl Target {
    pub fn from_lesson(lesson: &Lesson) -> Self {
        Self {
            topic: lesson.topic.clone(),
            objective: lesson.objective.clone(),
            card: lesson.selected.clone(),
            stage: lesson.stage,
            claim: lesson.claim.clone(),
        }
    }
    pub fn key(&self) -> String {
        let scope = self.card.as_deref().unwrap_or(&self.topic);
        // The reviewed objective is stable even if a tutor paraphrases the lesson title.
        if let Some(claim) = &self.claim {
            return format!("{}\nclaim:{}", scope.to_lowercase(), claim);
        }
        if let Some(card) = self.card.as_deref() {
            let count = if card == "washington-civil" { 3 } else { 2 };
            if self.stage < count {
                return format!("card:{card}/{}", self.stage);
            }
        }
        format!(
            "{}\n{}",
            scope.trim().to_lowercase(),
            self.objective.trim().to_lowercase()
        )
    }
    pub fn label(&self) -> &str {
        self.card.as_deref().unwrap_or(&self.topic)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Observation {
    pub target: Target,
    pub exercise: String,
    pub action: String,
    pub quote: String,
    pub assisted: bool,
    #[serde(default)]
    pub validation: String,
    #[serde(default)]
    pub family: String,
}

impl Observation {
    pub fn verified(&self) -> bool {
        matches!(
            self.validation.as_str(),
            "exact_key" | "independent_agreement"
        )
    }
    pub fn demonstrated(&self) -> bool {
        self.action == "complete" && self.verified()
    }
}

#[derive(Debug)]
pub struct Work {
    pub source: String,
    pub day: Option<u32>,
    pub target: Target,
    pub exercise: String,
    pub observation: Option<Observation>,
    pub offered: bool,
}

#[derive(Debug)]
pub struct Skill {
    pub target: Target,
    pub latest_assessment: Option<Observation>,
    pub last_source: String,
    pub last_task: String,
    pub history: Vec<String>,
    pub last_action: String,
    pub attempts: usize,
    pub checked: usize,
    pub durable: bool,
    pub changed_form: bool,
    pub due: bool,
    pub due_in: Option<u32>,
    pub latest_day: u32,
    prior: Option<Prior>,
    exposures: Vec<Exposure>,
    seen_tasks: BTreeSet<String>,
    supported_families: BTreeSet<String>,
    checks: Vec<Attempt>,
    proposals: Vec<CheckpointProposal>,
}
impl Skill {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            latest_assessment: None,
            last_source: String::new(),
            last_task: String::new(),
            history: vec![],
            last_action: String::new(),
            attempts: 0,
            checked: 0,
            durable: false,
            changed_form: false,
            due: false,
            due_in: None,
            latest_day: 0,
            prior: None,
            exposures: vec![],
            seen_tasks: BTreeSet::new(),
            supported_families: BTreeSet::new(),
            checks: vec![],
            proposals: vec![],
        }
    }
    fn cell(&self) -> Cell {
        let digest = document_id(self.target.key().as_bytes());
        Cell {
            scope: Scope {
                learner: 1,
                revision: u64::from_be_bytes(digest[..8].try_into().unwrap()),
            },
            outcome: OutcomeId(1),
        }
    }
    pub fn encounter(&mut self, work: &Work, sequence: u64, answer: bool) {
        let cell = self.cell();
        self.last_source = work.source.clone();
        let day = work.day;
        self.latest_day = day.unwrap_or(self.latest_day);
        let at = Moment {
            day: self.latest_day,
            sequence,
        };
        if answer {
            let obs = work.observation.as_ref().expect("answer row");
            if obs.verified() {
                self.latest_assessment = Some(obs.clone());
            }
            self.attempts += 1;
            self.last_action = obs.action.clone();
            self.last_task = obs.exercise.clone();
            let task_key = obs
                .exercise
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            let fresh = self.seen_tasks.insert(task_key);
            let validated = obs.verified();
            let judge = match obs.action.as_str() {
                "complete" => Judgment::Correct,
                "repair" => Judgment::Incorrect,
                _ => Judgment::Ambiguous,
            };
            let mut label = format!("{} · {}", work.source, obs.action);
            if obs.assisted {
                label.push_str(" · guided");
            }
            if !obs.validation.is_empty() {
                label.push_str(&format!(" · {}", obs.validation));
            }
            if !obs.quote.is_empty() {
                label.push_str(&format!(" · “{}”", obs.quote));
            }
            self.history.push(label);
            if validated
                && fresh
                && !obs.assisted
                && day.is_some()
                && matches!(judge, Judgment::Correct | Judgment::Incorrect)
            {
                let mut attempt = Attempt {
                    scope: cell.scope,
                    id: sequence,
                    sequence,
                    outcome: cell.outcome,
                    item: sequence,
                    day: at.day,
                    question: Question::IndependentCheck,
                    judgment: judge,
                    assistance: Assistance::NoneReported,
                    fresh,
                    sources: [document_id(obs.exercise.as_bytes())].into(),
                };
                let withdrawn = Withdrawn::new(cell.scope);
                if let Ok((_, result)) = retention::read(
                    cell,
                    at,
                    Some(at.day.saturating_add(30)),
                    self.prior,
                    Some(&self.exposures),
                    Some(&attempt),
                    &withdrawn,
                ) {
                    match result {
                        CheckResult::SupportsDurability => self.durable = true,
                        CheckResult::OpposesDurability | CheckResult::ReviewFailure => {
                            self.durable = false
                        }
                        _ => {}
                    }
                }
                if !obs.family.is_empty()
                    && judge == Judgment::Correct
                    && !self.supported_families.is_empty()
                    && !self.supported_families.contains(&obs.family)
                {
                    if let Some(first) = self
                        .checks
                        .iter()
                        .rev()
                        .find(|a| a.judgment == Judgment::Correct)
                    {
                        attempt.question = Question::ChangedExample;
                        self.proposals.push(CheckpointProposal {
                            scope: cell.scope,
                            outcome: cell.outcome,
                            first: first.id,
                            confirmation: attempt.id,
                            due: at.day.saturating_add(21),
                        });
                    }
                }
                self.checks.push(attempt);
                self.checked += 1;
                if judge == Judgment::Correct {
                    if !obs.family.is_empty() {
                        self.supported_families.insert(obs.family.clone());
                    }
                    self.changed_form = self.supported_families.len() >= 2;
                    self.prior = Some(Prior {
                        cell,
                        verified: at,
                        stability_days: 14.,
                        threshold: 0.85,
                        had_durable_success: self.durable,
                    });
                } else {
                    self.supported_families.clear();
                    self.changed_form = false;
                    self.prior = None;
                }
            }
        } else if !work.exercise.is_empty() {
            self.last_task = work.exercise.clone();
        }
        if day.is_some() {
            self.exposures.push(Exposure { cell, at });
        }
    }
    pub fn schedule(&mut self, today: u32, next_day_guided_review: bool) {
        let interval = if self.prior.is_none() {
            let guided_success = self.latest_assessment.as_ref().is_some_and(|observation| {
                observation.assisted && observation.demonstrated()
            });
            if (next_day_guided_review && guided_success) || matches!(self.last_action.as_str(), "repair" | "partial") {
                1
            } else {
                7
            }
        } else {
            let cell = self.cell();
            let at = Moment {
                day: today,
                sequence: u64::MAX,
            };
            let forecast = retention::read(
                cell,
                at,
                Some(today.saturating_add(30)),
                self.prior,
                Some(&self.exposures),
                None,
                &Withdrawn::new(cell.scope),
            );
            match forecast {
                Ok((Some(f), _)) => f.required_gap_days.ceil() as u32,
                _ => 7,
            }
        };
        if self.latest_day > 0 {
            let cell = self.cell();
            let checkpoint = reconcile(
                cell.scope,
                today,
                &self.checks,
                &self.proposals,
                &Withdrawn::new(cell.scope),
            )
            .ok()
            .and_then(|p| p.checkpoints.get(&cell.outcome).copied());
            let next = self.latest_day.saturating_add(interval);
            let remaining = checkpoint
                .map_or(next, |c| c.due.max(next))
                .saturating_sub(today);
            self.due = remaining == 0;
            self.due_in = Some(remaining);
        }
    }
    pub fn prior_tasks(&self) -> Vec<String> {
        self.seen_tasks.iter().cloned().collect()
    }
    pub fn status(&self) -> &'static str {
        if self.durable && self.changed_form {
            "delayed and changed-form evidence"
        } else if self.durable {
            "delayed evidence"
        } else if self.changed_form {
            "two assessed forms"
        } else if self.prior.is_some() {
            "demonstrated"
        } else if self.checked > 0 {
            "needs repair"
        } else if self.attempts > 0 {
            "attempt recorded"
        } else {
            "introduced"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn work(day: u32, family: &str, assisted: bool) -> Work {
        let target = Target {
            topic: "Tank inventory".into(),
            objective: "Net change".into(),
            ..Default::default()
        };
        let exercise = format!("Measure net change, form {family}");
        Work {
            source: format!("day-{day}#1"),
            day: Some(day),
            target: target.clone(),
            exercise: exercise.clone(),
            offered: false,
            observation: Some(Observation {
                target,
                exercise,
                action: "complete".into(),
                quote: "10 kg".into(),
                assisted,
                validation: "exact_key".into(),
                family: family.into(),
            }),
        }
    }
    #[test]
    fn guided_probability_success_schedules_a_fresh_independent_check() {
        let case: serde_json::Value = serde_json::from_str(include_str!("../../../fixtures/conditional-probability-v1.json")).unwrap();
        let mut guided = work(100, "counts", true);
        guided.target.objective = case["capability"].as_str().unwrap().into();
        let mut skill = Skill::new(guided.target.clone());
        let mut later = work(101, "branch-diagram", false);
        later.target = guided.target.clone();
        for (row, name) in [(&mut guided, "probe"), (&mut later, "return")] {
            row.exercise = case[name]["prompt"].as_str().unwrap().into();
            let observation = row.observation.as_mut().unwrap();
            observation.target = row.target.clone();
            observation.exercise.clone_from(&row.exercise);
            observation.quote = case[name]["answer"].as_str().unwrap().into();
            observation.validation = "independent_agreement".into();
        }
        skill.encounter(&guided, 1, true);
        skill.schedule(101, false);
        assert!(!skill.due);
        skill.schedule(101, true);
        assert!(skill.due);
        assert_eq!(skill.checked, 0);
        skill.encounter(&later, 2, true);
        assert_eq!(skill.checked, 1);
        assert_eq!(skill.status(), "demonstrated");
        skill.encounter(&later, 3, true);
        assert_eq!(skill.checked, 1);
    }

    #[test]
    fn guided_exposure_shortens_the_gap_and_changed_cases_require_verified_work() {
        for (intervening, expected) in [(false, true), (true, false)] {
            let first = work(100, "a", false);
            let mut skill = Skill::new(first.target.clone());
            skill.encounter(&first, 1, true);
            if intervening {
                skill.encounter(&work(125, "guided", true), 2, true);
            }
            skill.encounter(&work(130, "b", false), 3, true);
            assert_eq!(skill.durable, expected);
            assert!(skill.changed_form);
            assert_eq!(skill.checked, 2);
            skill.schedule(130, false);
            assert!(!skill.due);
            let mut miss = work(131, "c", false);
            miss.observation.as_mut().unwrap().action = "repair".into();
            skill.encounter(&miss, 4, true);
            assert_eq!(skill.status(), "needs repair");
            assert!(!skill.durable);
        }
    }
}
