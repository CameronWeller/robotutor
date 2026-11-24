//! Compare review policies on paired synthetic learner histories.
#[path = "retention_investigation/reschedule.rs"]
mod reschedule;

use tutor_lab::curriculum::OutcomeId;
use tutor_lab::history::{Attempt, Scope, Withdrawn};
use tutor_lab::protocol::{Assistance, Judgment, Question};
use tutor_lab::retention::{self, Cell, CheckResult, Exposure, Moment, Prior};

const ENDPOINT: u32 = 90;
const TARGET: u32 = 365;
const LEARNERS: u64 = 1200;

#[derive(Clone, Copy)]
enum Draw {
    Attendance,
    Response,
    InitialStability,
    HelpRequest,
    OutsideInvited,
    OutsideAttended,
    RosterAvailable,
}

/// A draw belongs to a learner, calendar day and event, independent of the policy's actions.
fn draw(seed: u64, learner: u64, day: u32, event: Draw) -> f64 {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(seed.to_le_bytes());
    hash.update(learner.to_le_bytes());
    hash.update(day.to_le_bytes());
    hash.update([event as u8]);
    let bytes = hash.finalize();
    let bits = u64::from_le_bytes(bytes[..8].try_into().unwrap()) >> 11;
    bits as f64 / (1u64 << 53) as f64
}

#[derive(Clone, Copy)]
struct Policy {
    label: &'static str,
    interval: u32,
    protect_endpoint: bool,
    review_limit: Option<u32>,
}

const POLICIES: [Policy; 4] = [
    Policy {
        label: "every-21",
        interval: 21,
        protect_endpoint: false,
        review_limit: None,
    },
    Policy {
        label: "every-30",
        interval: 30,
        protect_endpoint: false,
        review_limit: None,
    },
    Policy {
        label: "protect-21",
        interval: 21,
        protect_endpoint: true,
        review_limit: None,
    },
    Policy {
        label: "every-21-two",
        interval: 21,
        protect_endpoint: false,
        review_limit: Some(2),
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encounter {
    Retrieved,
    Corrected,
    RequestedHelp,
}

struct Visit {
    day: u32,
    encounter: Encounter,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scenario {
    Ordinary,
    LearnerRequests,
    UntrackedOutsideLesson,
    NoLearningEffect,
}

struct History {
    interventions: Vec<Visit>,
    final_response: Option<bool>,
    evidence: CheckResult,
    tutor_only: CheckResult,
    coverage_aware: CheckResult,
    outside_attended: bool,
    outside_invited: bool,
    receipt_needed: bool,
    resolved_absence: bool,
}

fn moment(day: u32) -> Moment {
    Moment {
        day,
        sequence: u64::from(day),
    }
}

fn simulate(seed: u64, learner: u64, policy: Policy, scenario: Scenario) -> History {
    let cell = Cell {
        scope: Scope {
            learner,
            revision: 1,
        },
        outcome: OutcomeId(1),
    };
    let mut stability =
        [3.0, 14.0, 60.0][(draw(seed, learner, 0, Draw::InitialStability) * 3.0) as usize];
    let mut last_exposure = 0;
    let mut encoded_day = 0;
    let mut last_verified = 0;
    let mut interventions = Vec::new();
    let mut scheduled_reviews = 0;
    let mut exposures = Vec::new();
    let mut final_response = None;
    let estimate = Prior {
        cell,
        verified: moment(0),
        stability_days: 14.0,
        threshold: 0.85,
        had_durable_success: false,
    };
    let withdrawn = Withdrawn::new(cell.scope);
    let endpoint_forecast = retention::read(
        cell,
        moment(ENDPOINT),
        Some(TARGET),
        Some(estimate),
        Some(&[]),
        None,
        &withdrawn,
    )
    .expect("valid endpoint")
    .0
    .expect("verified opening");
    // A learner may ask for one explanation late in the term. Attendance still governs delivery.
    let requested = draw(seed, learner, 0, Draw::HelpRequest);
    let request_day = (scenario != Scenario::Ordinary && requested < 0.5)
        .then_some(76 + (requested * 20.0) as u32);

    let learns = scenario != Scenario::NoLearningEffect;
    let outside_invited = matches!(
        scenario,
        Scenario::UntrackedOutsideLesson | Scenario::NoLearningEffect
    ) && draw(seed, learner, 80, Draw::OutsideInvited) < 0.5;
    let outside_attended = outside_invited && draw(seed, learner, 80, Draw::OutsideAttended) < 0.5;
    for day in 1..=ENDPOINT {
        // Outside the tutor. Invitation scope is known; actual attendance is hidden from policy.
        if day == 80 && outside_attended && learns {
            encoded_day = day;
            stability *= 1.05;
        }
        let endpoint = day == ENDPOINT;
        let attendance = if endpoint { 0.85 } else { 3.0 / 7.0 };
        if draw(seed, learner, day, Draw::Attendance) >= attendance {
            continue;
        }
        if Some(day) == request_day {
            interventions.push(Visit {
                day,
                encounter: Encounter::RequestedHelp,
            });
            exposures.push(Exposure {
                cell,
                at: moment(day),
            });
            last_exposure = day;
            if learns {
                encoded_day = day;
                stability *= 1.05;
            }
            continue;
        }
        let invades_gap = policy.protect_endpoint
            && f64::from(ENDPOINT - day) < endpoint_forecast.required_gap_days;
        if !endpoint
            && (day - last_exposure < policy.interval
                || invades_gap
                || policy
                    .review_limit
                    .is_some_and(|limit| scheduled_reviews >= limit))
        {
            continue;
        }
        let elapsed = f64::from(day - encoded_day);
        let recall = 1.0 / (1.0 + elapsed / (9.0 * stability));
        let correct = draw(seed, learner, day, Draw::Response) < recall;
        if endpoint {
            final_response = Some(correct);
            break;
        }
        scheduled_reviews += 1;
        interventions.push(Visit {
            day,
            encounter: if correct {
                Encounter::Retrieved
            } else {
                Encounter::Corrected
            },
        });
        exposures.push(Exposure {
            cell,
            at: moment(day),
        });
        // Retrieval strengthens a successful trace. A miss gets one worked correction.
        if correct {
            last_verified = day;
        }
        if learns {
            encoded_day = day;
            stability *= if correct {
                (1.2 * (1.0 - recall)).exp()
            } else {
                1.05
            };
        }
        last_exposure = day;
    }
    let check = final_response.map(|correct| Attempt {
        scope: cell.scope,
        outcome: cell.outcome,
        day: ENDPOINT,
        id: u64::from(ENDPOINT),
        sequence: u64::from(ENDPOINT),
        item: u64::from(ENDPOINT),
        question: Question::DelayedReview,
        judgment: if correct {
            Judgment::Correct
        } else {
            Judgment::Incorrect
        },
        assistance: Assistance::NoneReported,
        fresh: true,
        sources: [[1; 32]].into(),
    });
    let prior = Prior {
        verified: moment(last_verified),
        ..estimate
    };
    let assess = |ledger: Option<&[Exposure]>| {
        retention::read(
            cell,
            moment(ENDPOINT),
            Some(TARGET),
            Some(prior),
            ledger,
            check.as_ref(),
            &withdrawn,
        )
        .expect("constructed history")
        .1
    };
    let tutor_only = assess(Some(&exposures));
    let coverage_aware = if outside_invited {
        assess(None)
    } else {
        tutor_only
    };
    if outside_invited {
        exposures.push(Exposure {
            cell,
            at: moment(80),
        });
    }
    let with_lesson = assess(Some(&exposures));
    let evidence = if outside_attended {
        with_lesson
    } else {
        tutor_only
    };
    // Query a roster only when the possible exposure changes a positive durability finding.
    // A returned roster attests both presence and absence. Silence supplies neither.
    let receipt_needed = outside_invited
        && tutor_only == CheckResult::SupportsDurability
        && with_lesson != CheckResult::SupportsDurability;
    let roster_available = draw(seed, learner, 80, Draw::RosterAvailable) < 0.60;
    let resolved_absence = receipt_needed && roster_available && !outside_attended;
    History {
        interventions,
        final_response,
        evidence,
        tutor_only,
        coverage_aware,
        outside_attended,
        outside_invited,
        receipt_needed,
        resolved_absence,
    }
}

#[derive(Default)]
struct Totals {
    retrievals: usize,
    worked_corrections: usize,
    requests_met: usize,
    seen: usize,
    correct: usize,
    supported: usize,
    false_support_without_outside_ledger: usize,
    support_lost_when_coverage_unknown: usize,
    outside_exposures: usize,
    invitations: usize,
    targeted_queries: usize,
    resolved_absences: usize,
}

impl Totals {
    fn add(&mut self, h: &History) {
        self.retrievals += h
            .interventions
            .iter()
            .filter(|v| !matches!(v.encounter, Encounter::RequestedHelp))
            .count();
        self.worked_corrections += h
            .interventions
            .iter()
            .filter(|v| matches!(v.encounter, Encounter::Corrected))
            .count();
        self.requests_met += h
            .interventions
            .iter()
            .filter(|v| matches!(v.encounter, Encounter::RequestedHelp))
            .count();
        self.seen += usize::from(h.final_response.is_some());
        self.correct += usize::from(h.final_response == Some(true));
        self.invitations += usize::from(h.outside_invited);
        self.targeted_queries += usize::from(h.receipt_needed);
        self.resolved_absences += usize::from(h.resolved_absence);
        self.outside_exposures += usize::from(h.outside_attended);
        self.false_support_without_outside_ledger += usize::from(
            h.tutor_only == CheckResult::SupportsDurability
                && h.evidence != CheckResult::SupportsDurability,
        );
        self.support_lost_when_coverage_unknown += usize::from(
            h.coverage_aware != CheckResult::SupportsDurability
                && h.evidence == CheckResult::SupportsDurability,
        );
        self.supported += usize::from(h.evidence == CheckResult::SupportsDurability);
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_some_and(|s| s == "reschedule") {
        args.next();
        if args.next().is_some() {
            return Err("reschedule takes no arguments".into());
        }
        return reschedule::run();
    }
    if args.peek().is_some_and(|s| s == "transfer") {
        args.next();
        if args.next().is_some() {
            return Err("transfer takes no arguments".into());
        }
        return transfer_comparison();
    }
    let seed = args
        .next()
        .map_or(Ok(20261118), |s| s.parse::<u64>())
        .map_err(|_| "invalid seed")?;
    let trace_id = args
        .next()
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|_| "invalid learner ID")?;
    if args.next().is_some() {
        return Err("expected [seed [learner ID]]".into());
    }
    if trace_id.is_some_and(|id| id >= LEARNERS) {
        return Err(format!("learner ID must be below {LEARNERS}"));
    }
    for scenario in [
        Scenario::Ordinary,
        Scenario::LearnerRequests,
        Scenario::UntrackedOutsideLesson,
        Scenario::NoLearningEffect,
    ] {
        let mut totals = POLICIES.map(|_| Totals::default());
        let mut paired = POLICIES.map(|_| [0; 2]);
        for learner in 0..LEARNERS {
            let arms = POLICIES.map(|p| simulate(seed, learner, p, scenario));
            for (i, h) in arms.iter().enumerate() {
                totals[i].add(h);
                if trace_id == Some(learner) {
                    let visits: Vec<_> = h
                        .interventions
                        .iter()
                        .map(|v| {
                            format!(
                                "{}={}",
                                v.day,
                                match v.encounter {
                                    Encounter::Retrieved => "unaided retrieval",
                                    Encounter::Corrected => "miss then worked correction",
                                    Encounter::RequestedHelp => "requested example",
                                }
                            )
                        })
                        .collect();
                    println!("{scenario:?} learner {learner}, {}: {}; outside on 80: {}; day {ENDPOINT}: {:?}, {:?}", POLICIES[i].label,visits.join(", "),h.outside_attended,h.final_response,h.evidence);
                }
                match (h.final_response, arms[1].final_response) {
                    (Some(true), Some(false)) => paired[i][0] += 1,
                    (Some(false), Some(true)) => paired[i][1] += 1,
                    (Some(_), None) | (None, Some(_)) => {
                        return Err("endpoint attendance differs across paired learners".into())
                    }
                    _ => {}
                }
            }
        }
        println!("seed {seed}, {LEARNERS} learners per policy, common endpoint day {ENDPOINT}, scenario {scenario:?}");
        println!("policy\tretrievals\tworked corrections\trequested examples\tendpoint responses\tcorrect\tsupports durability\tcorrect only here vs 30d\tcorrect only at 30d\toutside exposures\tunsupported credits with missing ledger\tsupport foregone with unknown coverage");
        for ((p, t), differences) in POLICIES.iter().zip(totals).zip(paired) {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                p.label,
                t.retrievals,
                t.worked_corrections,
                t.requests_met,
                t.seen,
                t.correct,
                t.supported,
                differences[0],
                differences[1],
                t.outside_exposures,
                t.false_support_without_outside_ledger,
                t.support_lost_when_coverage_unknown
            );
            if t.invitations > 0 {
                println!("{} roster: {} invited; {} targeted lookups; {} justified positive findings recovered", p.label, t.invitations, t.targeted_queries, t.resolved_absences);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_null_keeps_each_endpoint_identical_despite_different_interventions() {
        let mut differing_schedules = 0;
        for learner in 0..48 {
            let arms = POLICIES.map(|p| simulate(99, learner, p, Scenario::NoLearningEffect));
            assert_eq!(arms[0].final_response, arms[1].final_response);
            assert_eq!(arms[1].final_response, arms[2].final_response);
            assert_eq!(arms[1].final_response, arms[3].final_response);
            if arms[0].interventions.len() != arms[1].interventions.len() {
                differing_schedules += 1;
            }
        }
        assert!(differing_schedules > 0);
    }

    #[test]
    fn incomplete_coverage_and_targeted_receipts_do_not_grant_false_positive_support() {
        let mut restored = 0;
        let mut incorrect_if_assumed_complete = 0;
        for learner in 0..100 {
            let h = simulate(99, learner, POLICIES[2], Scenario::UntrackedOutsideLesson);
            let before = h.coverage_aware == CheckResult::SupportsDurability;
            let truth = h.evidence == CheckResult::SupportsDurability;
            if before || h.resolved_absence {
                assert!(truth);
            }
            if h.resolved_absence {
                assert!(h.receipt_needed && !h.outside_attended && !before);
                restored += 1;
            }
            incorrect_if_assumed_complete +=
                usize::from(!truth && h.tutor_only == CheckResult::SupportsDurability);
        }
        assert!(restored > 0 && incorrect_if_assumed_complete > 0);
    }
}

/// Four independent delayed forms show when correct arithmetic can still arise from repeating a cue.
fn transfer_comparison() -> Result<(), String> {
    use serde_json::json;
    use tutor_lab::curriculum::goal_evidence::{Presentation, Task};
    use tutor_lab::material::document_id;

    let cell = Cell {
        scope: Scope {
            learner: 42,
            revision: 1,
        },
        outcome: OutcomeId(2),
    };
    let arms = [("A-worked", 3), ("B-worked", 1), ("B-explain", 1)];
    use tutor_lab::curriculum::Span;
    use tutor_lab::material::{FractionCard, FractionTask, RuleSource, UNIT_RULE};
    let active = Default::default();
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin: Span {
            document: document_id(UNIT_RULE.as_bytes()),
            start: 0,
            end: UNIT_RULE.len() as u32,
        },
        revoked: &active,
    };
    for (arm, _) in arms {
        let lesson = if arm == "A-worked" {
            FractionCard::propose(FractionTask::Sum, 3, 3, 16)
        } else {
            FractionCard::propose(FractionTask::MissingPart, 3, 2, 16)
        }
        .ok_or("invalid teaching card")?;
        let (worked, _) = lesson.worked(&source).ok_or("unavailable lesson")?;
        println!("Day 30, eight minutes, {arm}: {worked}");
        if arm == "B-explain" {
            println!("Why does adding the two pieces let you check the cut? Use your own words or point to the pieces.");
        }
    }
    let strategies = [
        "repeat worked answer",
        "use inverse relation",
        "add the amounts",
        "treat quarters as eighths",
    ];
    let mut counts = [[0; 4]; 3];
    // Every row is an alternative first delayed observation. Separate learners receive
    // only one form each, avoiding feedback or practice from testing a prior form.
    for remaining in [5, 4, 3, 2] {
        let eighths_at_start = 6;
        let key = eighths_at_start - remaining;
        let pack = json!({"unit":u64::try_from(remaining).unwrap(),"check":format!("A cup contained three quarters of a litre. After pouring, {remaining} eighths of a litre remained. How many eighths were poured?"),
            "criterion":"Find the missing part of one whole after converting units.",
            "rubric":"Give the amount poured in eighths.","quantity":{"value":key,"unit":"eighths"}});
        let task = Task::from_pack(
            cell,
            document_id(pack.to_string().as_bytes()),
            "quantity",
            "Find a missing part in common units.",
            &pack,
        )?;
        let shown = Presentation {
            attempt: 1,
            at: moment(58),
            fresh: true,
            assistance: Assistance::NoneReported,
        };
        for (arm, (policy, cue)) in arms.iter().enumerate() {
            let answers = [
                *cue,
                eighths_at_start - remaining,
                eighths_at_start + remaining,
                3 - remaining,
            ];
            for (method, answer) in answers.into_iter().enumerate() {
                let observed = task.unreviewed_text(shown, &format!("{answer} eighths"));
                counts[arm][method] += u32::from(observed.judgment() == Judgment::Correct);
                println!(
                    "Pouring task, {remaining} eighths left: {policy}, {}, {answer} eighths: {:?}",
                    strategies[method],
                    observed.judgment()
                );
            }
        }
    }
    for ((policy, _), count) in arms.into_iter().zip(counts) {
        println!("{policy}, correct of 4: {count:?} for {strategies:?}");
    }
    // Eight example-only learners and eight prompted learners, each strategy represented
    // equally across forms. Vary uptake by one balanced set of four, with no change
    // to teaching minutes, endpoint delay, or scoring. These are sensitivity weights.
    for rule_in_prompted_arm in [0, 1, 2] {
        let worked = counts[1][0] + counts[1][1];
        let prompted =
            (2 - rule_in_prompted_arm) * counts[2][0] + rule_in_prompted_arm * counts[2][1];
        println!("Conditional matched groups of 8: worked {worked}, explanation {prompted}; prompted inverse-rule learners {}/8",rule_in_prompted_arm*4);
    }
    Ok(())
}
