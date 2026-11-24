//! With the incoming histories fixed, change the selected return task, check the
//! submitted fraction, then ask the planner again on the resulting history.
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tutor_lab::{
    curriculum::{Decision, OutcomeId, Progress, ReviewedUnit, Slot, Span},
    history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{Assistance, Question},
};

use super::stress::release;

struct History {
    scope: Scope,
    attempts: Vec<Attempt>,
    bookings: Vec<CheckpointProposal>,
}

#[derive(Clone, Copy)]
struct Performance {
    correct: bool,
    assistance: Assistance,
}

impl History {
    fn new(learner: u64, due: [u32; 2]) -> Result<Self, String> {
        let mut h = Self {
            scope: Scope {
                learner,
                revision: 1,
            },
            attempts: vec![],
            bookings: vec![],
        };
        for (index, due) in due.into_iter().enumerate() {
            let id = OutcomeId(index as u32 + 1);
            let response = Performance {
                correct: true,
                assistance: Assistance::NoneReported,
            };
            let first = h.answer(id, 0, Question::Practice, response)?;
            let confirmation = h.answer(id, 1, Question::ChangedExample, response)?;
            h.bookings.push(CheckpointProposal {
                scope: h.scope,
                outcome: id,
                first,
                confirmation,
                due,
            });
        }
        Ok(h)
    }

    fn answer(
        &mut self,
        target: OutcomeId,
        day: u32,
        question: Question,
        p: Performance,
    ) -> Result<u64, String> {
        let id = self.attempts.len() as u64 + 1;
        let pieces = u16::try_from(id).map_err(|_| "too many performances in scenario")?;
        let task = match target.0 {
            1 => FractionTask::Sum,
            2 => FractionTask::MissingPart,
            _ => return Err("no reviewed rehearsal form for this target".into()),
        };
        let card =
            FractionCard::propose(task, pieces, 2, 16).ok_or("invalid rehearsal parameters")?;
        let pin = Span {
            document: document_id(UNIT_RULE.as_bytes()),
            start: 0,
            end: UNIT_RULE.len() as u32,
        };
        let revoked = BTreeSet::new();
        let source = RuleSource {
            pin,
            bytes: Some(UNIT_RULE.as_bytes()),
            revoked: &revoked,
        };
        let (prompt, _) = card.question(&source).ok_or("unreleased question")?;
        let key = match task {
            FractionTask::Sum => pieces + 2,
            FractionTask::MissingPart => 2,
        };
        let submitted = format!("{}/16", if p.correct { key } else { 0 });
        let judgment = card
            .assess(&submitted, &source)
            .ok_or("unreleased assessment")?;
        self.attempts.push(Attempt {
            scope: self.scope,
            id,
            sequence: id,
            outcome: target,
            item: id,
            day,
            question,
            judgment,
            assistance: p.assistance,
            fresh: true,
            sources: [pin.document, document_id(prompt.as_bytes())].into(),
        });
        Ok(id)
    }

    fn project(&self, day: u32) -> Result<Progress, String> {
        reconcile(
            self.scope,
            day,
            &self.attempts,
            &self.bookings,
            &Withdrawn::new(self.scope),
        )
        .map_err(|e| format!("invalid paired history: {e:?}"))
    }
}

fn pair(
    unit: &ReviewedUnit,
    learner: u64,
    due: [u32; 2],
    arm: &str,
    profile: &str,
) -> Result<Value, String> {
    let mut history = History::new(learner, due)?;
    let before = history.project(30)?;
    let today = unit.next(OutcomeId(3), &before, 30, &BTreeSet::new(), Slot::Review);
    let Ok(Decision::Recheck(reference)) = today else {
        return Err(format!("expected two overdue components, got {today:?}"));
    };
    let selected = match arm {
        "carry_target" => Some(reference),
        "guess_first" => Some(OutcomeId(1)),
        "guess_second" => Some(OutcomeId(2)),
        "abstain" => None,
        _ => return Err("unrecognized routing arm".into()),
    };
    let mut answered = false;
    if let Some(id) = selected {
        if profile != "absent" {
            // The response mechanism is the same across arms and dates. A/B
            // change the work actually offered; the answer depends on its skill.
            let correct = match profile {
                "miss_sum" => id.0 != 1,
                "miss_missing_part" => id.0 != 2,
                _ => true,
            };
            let help = if profile == "with_help" {
                Assistance::Reported
            } else {
                Assistance::NoneReported
            };
            let first = history
                .attempts
                .iter()
                .find(|a| a.outcome == id)
                .ok_or("missing opening work")?
                .id;
            let confirmation = history.answer(
                id,
                30,
                Question::DelayedReview,
                Performance {
                    correct,
                    assistance: help,
                },
            )?;
            history.bookings.push(CheckpointProposal {
                scope: history.scope,
                outcome: id,
                first,
                confirmation,
                due: 60,
            });
            answered = true;
        }
    }
    let after = history.project(31)?;
    let dates = [OutcomeId(1), OutcomeId(2)].map(|id| after.checkpoints.get(&id).map(|c| c.due));
    let tomorrow = unit
        .next(OutcomeId(3), &after, 31, &BTreeSet::new(), Slot::Forward)
        .map_err(|e| format!("next meeting: {e:?}"))?;
    Ok(
        json!({"learner":learner,"initial_due":due,"profile":profile,"arm":arm,"reference_target":reference.0,"selected_target":selected.map(|id|id.0),
        "return_performances":u32::from(answered),"next_due":dates,"next_meeting":format!("{tomorrow:?}")}),
    )
}

pub(super) fn compare() -> Result<Value, String> {
    let unit = release(false)?;
    let mut pairs = Vec::new();
    for (learner, due) in [(1, [10, 20]), (2, [20, 10])] {
        for profile in [
            "independent_correct",
            "with_help",
            "absent",
            "miss_sum",
            "miss_missing_part",
        ] {
            for arm in ["carry_target", "guess_first", "guess_second", "abstain"] {
                pairs.push(pair(&unit, learner, due, arm, profile)?);
            }
        }
    }
    Ok(Value::Array(pairs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_independent_miss_changes_the_next_lesson_only_for_the_task_presented() {
        let unit = release(false).unwrap();
        let run = |arm| pair(&unit, 2, [20, 10], arm, "miss_missing_part").unwrap();
        assert_eq!(run("carry_target")["next_due"], json!([20, null]));
        assert_eq!(run("guess_first")["next_due"], json!([60, 10]));
        assert_eq!(run("abstain")["next_due"], json!([20, 10]));
        for profile in ["with_help", "absent"] {
            assert_eq!(
                pair(&unit, 2, [20, 10], "carry_target", profile).unwrap()["next_due"],
                json!([20, 10])
            );
        }
    }
}
