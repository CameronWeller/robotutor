//! Three different questions about the same synthetic multi-visit learner history.
//! A booked return, modeled need for refresh and a supported delayed performance
//! are separate targets. Dates, stability and help frequencies are experiment inputs.
use super::{Observation, Rng};
use crate::curriculum::goal_evidence::{
    self, Evidence, Observation as Response, Presentation, Spacing, Task,
};
use crate::curriculum::{Document, OutcomeId};
use crate::history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn};
use crate::material::document_id;
use crate::protocol::{Assistance, Question};
use crate::retention::{self, Cell, CheckResult, Exposure, Moment, Prior};
use serde_json::{json, Value};

const CELL: Cell = Cell {
    scope: Scope {
        learner: 42,
        revision: 7,
    },
    outcome: OutcomeId(9),
};
const RETURN_DAYS: [u32; 6] = [3, 5, 21, 24, 30, 730];

struct Form<'a> {
    task: Task<'a>,
    edition: Document,
    item: u64,
    key: i64,
}

fn pack(item: u64, check: &str, key: i64) -> Value {
    json!({"unit":item,"check":check,"criterion":"Count the tiles under both strips.",
        "rubric":"Accept the combined count and the unit tiles.","quantity":{"value":key,"unit":"tiles"}})
}

impl<'a> Form<'a> {
    fn new(pack: &'a Value) -> Result<Self, String> {
        let edition = document_id(pack.to_string().as_bytes());
        let task = Task::from_pack(
            CELL,
            edition,
            "quantity",
            "Combine two counts of equal tiles.",
            pack,
        )?;
        Ok(Self {
            task,
            edition,
            item: pack["unit"].as_u64().ok_or("missing form ID")?,
            key: pack["quantity"]["value"]
                .as_i64()
                .ok_or("missing tile counts")?,
        })
    }

    fn observe(
        &self,
        p: Presentation,
        response: Response<'_>,
        question: Question,
    ) -> (Evidence, Attempt) {
        let submission = self.task.capture(p, response);
        let evidence = self.task.assess(p, &submission, None);
        // The protocol records which move elicited this assessment. Both consumers
        // receive the same publication, assistance, observed judgment and event order.
        let attempt = Attempt {
            scope: CELL.scope,
            outcome: CELL.outcome,
            id: p.attempt,
            day: p.at.day,
            sequence: p.at.sequence,
            question,
            item: self.item,
            judgment: evidence.judgment(),
            assistance: p.assistance,
            fresh: p.fresh,
            sources: [self.edition].into(),
        };
        (evidence, attempt)
    }

    fn quantity(&self, matches: bool) -> Response<'static> {
        Response::Quantity {
            value: self.key + i64::from(!matches),
            unit: "tiles",
        }
    }
}

fn visit(day: u32, id: u64, help: Assistance, fresh: bool) -> Presentation {
    Presentation {
        attempt: id,
        at: Moment { day, sequence: id },
        fresh,
        assistance: help,
    }
}

fn finding(result: CheckResult) -> &'static str {
    match result {
        CheckResult::NoCheck => "no return performance",
        CheckResult::Ineligible => "ineligible evidence",
        CheckResult::InsufficientSpacing => "spacing too short",
        CheckResult::UnknownExposure => "exposure history unknown",
        CheckResult::ReviewFailure => "review early failure",
        CheckResult::SupportsDurability => "supports durability",
        CheckResult::OpposesDurability => "opposes durability",
    }
}

pub(super) fn cases(seed: u64) -> Result<Vec<Observation>, String> {
    let mut rng = Rng::new(seed);
    let shift = rng.below(6) as usize;
    let p0 = pack(
        1,
        "Two tiles and one more cover a strip. How many tiles?",
        3,
    );
    let p1 = pack(
        2,
        "Two strips each cover two tiles. How many tiles altogether?",
        4,
    );
    let p2 = pack(
        3,
        "Five tiles cover one strip and two cover the next. How many do both cover?",
        7,
    );
    // A new catalog number, but exactly the same normalized prompt and answer.
    let copy = pack(4, p1["check"].as_str().ok_or("missing prompt")?, 4);
    let (opening, confirmation, new_form, duplicate) = (
        Form::new(&p0)?,
        Form::new(&p1)?,
        Form::new(&p2)?,
        Form::new(&copy)?,
    );
    let mut rows = Vec::with_capacity(288);
    for i in 0..96 {
        let day = RETURN_DAYS[(i + shift) % RETURN_DAYS.len()];
        let due = if rng.below(2) == 0 { 21 } else { 30 };
        let stability = match rng.below(6) {
            0..=2 => 3.0,
            3..=4 => 14.0,
            _ => 60.0,
        };
        let annual = rng.below(2) == 0;
        let complete = rng.below(5) != 0;
        let intervening = rng.below(3) == 0;
        let attended = rng.below(8) != 0;
        let repeats = rng.below(7) == 0;
        let opening_assisted = rng.below(10) == 0;
        let confirm_help_unknown = rng.below(9) == 0;
        let confirm_matches = rng.below(9) != 0;
        let help = match rng.below(12) {
            0 => Assistance::Reported,
            1 => Assistance::Unknown,
            _ => Assistance::NoneReported,
        };
        let response = rng.below(8);
        let withdrawn_support = rng.below(16);
        let later = if repeats { &duplicate } else { &new_form };
        let mut withdrawals = Withdrawn::new(CELL.scope);
        if let Some(edition) = match withdrawn_support {
            0 => Some(opening.edition),
            1 => Some(confirmation.edition),
            2 => Some(later.edition),
            _ => None,
        } {
            withdrawals.sources.insert(edition);
        }

        let a = visit(
            0,
            1,
            if opening_assisted {
                Assistance::Reported
            } else {
                Assistance::NoneReported
            },
            true,
        );
        let b = visit(
            0,
            2,
            if confirm_help_unknown {
                Assistance::Unknown
            } else {
                Assistance::NoneReported
            },
            true,
        );
        let c = visit(day, 4, help, !repeats);
        let (first, first_attempt) = opening.observe(a, opening.quantity(true), Question::Practice);
        let (second, second_attempt) = confirmation.observe(
            b,
            confirmation.quantity(confirm_matches),
            Question::ChangedExample,
        );
        let mut attempts = vec![first_attempt, second_attempt];
        let last = attended.then(|| {
            let response = match response {
                0 => Response::SelfReport,
                1 | 2 => later.quantity(false),
                _ => later.quantity(true),
            };
            let (evidence, attempt) = later.observe(c, response, Question::DelayedReview);
            attempts.push(attempt);
            evidence
        });

        // The day-0 pair can book a date. A later eligible miss or withdrawal can
        // remove its support. Merely being absent leaves the previous record intact.
        let proposal = CheckpointProposal {
            scope: CELL.scope,
            outcome: CELL.outcome,
            first: a.attempt,
            confirmation: b.attempt,
            due,
        };
        let progress = reconcile(CELL.scope, day, &attempts, &[proposal], &withdrawals)
            .map_err(|e| format!("history read: {e:?}"))?;
        let booking = match progress.checkpoints.get(&CELL.outcome) {
            None => "no supported checkpoint",
            Some(check) if day >= check.due => "booked return due",
            Some(_) => "booked return ahead",
        };

        let exposures: Vec<_> = intervening
            .then_some(Exposure {
                cell: CELL,
                at: Moment {
                    day: day.saturating_sub(5),
                    sequence: 3,
                },
            })
            .into_iter()
            .collect();
        let ledger = complete.then_some(exposures.as_slice());
        let target = annual.then_some(day + 365);
        let prior = [(&second, b.at), (&first, a.at)]
            .into_iter()
            .find(|(e, _)| e.opens_transfer(&withdrawals))
            .map(|(_, verified)| Prior {
                cell: CELL,
                verified,
                stability_days: stability,
                threshold: 0.85,
                had_durable_success: false,
            });
        let (forecast, no_check) =
            retention::read(CELL, c.at, target, prior, ledger, None, &withdrawals)
                .map_err(|e| format!("refresh read: {e:?}"))?;
        let refresh = match forecast {
            None => "no supported forecast",
            Some(f) if f.refresh_due => "refresh indicated",
            Some(_) => "refresh not due",
        };
        let durable = if let Some(last) = last {
            goal_evidence::delayed(
                &second,
                &last,
                ledger,
                &withdrawals,
                Spacing {
                    stability_days: stability,
                    threshold: 0.85,
                    target_day: target,
                },
            )
            .map_err(|e| format!("delayed evidence: {e:?}"))?
        } else {
            no_check
        };

        for (readout, result) in [
            ("booking", booking),
            ("refresh", refresh),
            ("delayed", finding(durable)),
        ] {
            // The forecast is read at the start of the return opportunity. Neither
            // attendance nor the later submission is visible to that prediction.
            let has_return = attended && readout != "refresh";
            let facts = vec![
                ("query concerns a booked return", readout == "booking"),
                ("query concerns modeled refresh", readout == "refresh"),
                ("return at least five days later", day >= 5),
                ("return at least 21 days later", day >= 21),
                ("return at least 24 days later", day >= 24),
                ("return at least 30 days later", day >= 30),
                ("return after two years", day >= 730),
                ("booking policy uses 30 days", due == 30),
                ("stability snapshot is fourteen days", stability == 14.0),
                ("stability snapshot is sixty days", stability == 60.0),
                ("target is a year beyond return", annual),
                ("interim exposure ledger complete", complete),
                (
                    "intervening worked example recorded",
                    complete && intervening,
                ),
                ("learner attended return", has_return),
                ("return repeats earlier wording", has_return && repeats),
                ("opening used a hint", opening_assisted),
                ("confirmation help unknown", confirm_help_unknown),
                ("confirmation supplied four tiles", confirm_matches),
                (
                    "return has reported content help",
                    has_return && help == Assistance::Reported,
                ),
                (
                    "return help unknown",
                    has_return && help == Assistance::Unknown,
                ),
                (
                    "return observation is self-report only",
                    has_return && response == 0,
                ),
                (
                    "submitted return count differs from checked key",
                    has_return && matches!(response, 1 | 2),
                ),
                ("opening edition withdrawn", withdrawn_support == 0),
                ("confirmation edition withdrawn", withdrawn_support == 1),
                ("return edition withdrawn", withdrawn_support == 2),
            ];
            rows.push(Observation::new(
                "retention",
                format!("learner path {i}, day {day}, {readout}"),
                facts,
                result,
            ));
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::super::corpus;

    #[test]
    fn the_refresh_read_precedes_submission_and_absence_adds_no_durability_evidence() {
        let schema = corpus::schemas().unwrap().remove(0);
        let bit = |name: &str| 1_u64 << schema.iter().position(|feature| *feature == name).unwrap();
        let attended = bit("learner attended return");
        let long_absence = bit("return after two years");
        let current_response = attended
            | bit("return observation is self-report only")
            | bit("submitted return count differs from checked key")
            | bit("return has reported content help")
            | bit("return help unknown");
        let mut witnessed_long_absence = false;
        for seed in 0..4 {
            let rows = corpus::draw(0, seed, &schema).unwrap();
            assert_eq!(rows.len(), 288);
            for [booking, refresh, delayed] in rows.as_chunks::<3>().0 {
                assert_eq!(refresh.bits & current_response, 0);
                if delayed.bits & attended == 0 {
                    assert_eq!(delayed.outcome, "no return performance");
                    if delayed.bits & long_absence != 0 && booking.outcome == "booked return due" {
                        witnessed_long_absence = true;
                    }
                }
            }
        }
        assert!(witnessed_long_absence);
    }
}
