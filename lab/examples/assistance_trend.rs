//! Compare two curriculum decisions on the same synthetic learner history.
use std::collections::BTreeSet;
use telo::monitor::Monitor;
use telo::predicate::{ClosurePredicate, Predicates};
use telo::Property;
use tutor_lab::context::{self, Purpose};
use tutor_lab::curriculum::{Decision, Outcome, OutcomeId, ReviewedUnit, Slot, Span};
use tutor_lab::history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn};
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::{Assistance, Judgment, Question};

// The legacy measure groups assessed criteria by learner/capability and time.
// Here all four distinct records refer to one such cell in chronological order.
fn legacy_trend(attempts: &[Attempt]) -> Option<f64> {
    let counted: Vec<_> = attempts
        .iter()
        .filter(|a| {
            a.assistance != Assistance::Unknown
                && matches!(a.judgment, Judgment::Correct | Judgment::Incorrect)
        })
        .collect();
    let middle = counted.len() / 2;
    if middle < 2 {
        return None;
    }
    let half = |rows: &[&Attempt]| {
        let unaided: Vec<_> = rows
            .iter()
            .filter(|a| a.assistance == Assistance::NoneReported)
            .collect();
        let share = (rows.len() - unaided.len()) as f64 / rows.len() as f64;
        let success = (!unaided.is_empty()).then(|| {
            unaided
                .iter()
                .filter(|a| a.judgment == Judgment::Correct)
                .count() as f64
                / unaided.len() as f64
        });
        (share, success)
    };
    let (early_share, early_success) = half(&counted[..middle]);
    let (late_share, late_success) = half(&counted[middle..]);
    Some(
        late_success
            .zip(early_success)
            .map_or(0.0, |(late, early)| late - early)
            - (late_share - early_share),
    )
}

struct Course {
    scope: Scope,
    adding: OutcomeId,
    remaining: OutcomeId,
    unit: ReviewedUnit,
}

impl Course {
    fn new(pin: Span) -> Self {
        let (brief, _, _) = context::example(Purpose::Teach);
        let scope = brief.snapshot.scope;
        let adding = brief.snapshot.outcome;
        let remaining = OutcomeId(adding.0 + 1);
        let support = BTreeSet::from([pin]);
        let unit = ReviewedUnit::build(
            scope.revision,
            vec![
                Outcome {
                    id: adding,
                    requires: BTreeSet::new(),
                    support: support.clone(),
                },
                Outcome {
                    id: remaining,
                    requires: BTreeSet::from([adding]),
                    support,
                },
            ],
        )
        .unwrap();
        Self {
            scope,
            adding,
            remaining,
            unit,
        }
    }

    fn next(&self, attempts: &[Attempt], day: u32, withdrawn: &Withdrawn) -> Decision {
        // Enumerate the possible support pairs in this small frozen history.
        // The existing reconciler checks validity, ordering, eligibility and withdrawals.
        let pairs: Vec<_> = attempts
            .iter()
            .enumerate()
            .flat_map(|(i, a)| {
                attempts[i + 1..].iter().map(move |b| CheckpointProposal {
                    scope: self.scope,
                    outcome: self.adding,
                    first: a.id,
                    confirmation: b.id,
                    due: b.day + 30,
                })
            })
            .collect();
        let progress = reconcile(self.scope, day, attempts, &pairs, withdrawn).unwrap();
        self.unit
            .next(
                self.remaining,
                &progress,
                day,
                &withdrawn.sources,
                Slot::Forward,
            )
            .unwrap()
    }

    fn record(
        &self,
        seq: u64,
        day: u32,
        judgment: Judgment,
        help: Assistance,
        pin: Span,
    ) -> Attempt {
        Attempt {
            scope: self.scope,
            outcome: self.adding,
            id: seq,
            sequence: seq,
            item: seq,
            day,
            question: if seq == 1 {
                Question::Practice
            } else {
                Question::ChangedExample
            },
            judgment,
            assistance: help,
            fresh: true,
            sources: BTreeSet::from([pin.document]),
        }
    }
}

fn promotion_monitor() -> impl Fn() -> Monitor<(bool, bool)> {
    let predicates = || {
        let mut builder = Predicates::builder();
        let promote = builder.new_predicate(ClosurePredicate::new(
            |s: &(bool, bool)| s.0,
            "next objective",
        ));
        let evidence = builder.new_predicate(ClosurePredicate::new(
            |s: &(bool, bool)| s.1,
            "supported pair",
        ));
        (
            builder.build(),
            Property::atomic(promote),
            Property::atomic(evidence),
        )
    };
    let (p, promote, evidence) = predicates();
    let automaton = Property::always(Property::Or(
        Box::new(!promote.clone()),
        Box::new(promote & evidence),
    ))
    .to_monitoring_automaton(&p);
    move || Monitor::new(predicates().0, automaton.clone())
}

fn audit(course: &Course, pin: Span, withdrawn: &Withdrawn) {
    let monitor = promotion_monitor();
    let mut trend_failures = 0;
    let mut supported_routes = 0;
    for history in 0..6usize.pow(4) {
        let mut digits = history;
        let mut attempts = vec![];
        let mut supported_successes = 0;
        let mut evidence_monitor = monitor();
        for seq in 1..=4 {
            let digit = digits % 6;
            digits /= 6;
            let help = [
                Assistance::NoneReported,
                Assistance::Reported,
                Assistance::Unknown,
            ][digit / 2];
            let judgment = [Judgment::Incorrect, Judgment::Correct][digit % 2];
            attempts.push(course.record(seq, seq as u32, judgment, help, pin));
            // Independent oracle for this bounded, fully fresh alphabet.
            if help == Assistance::NoneReported {
                if judgment == Judgment::Correct {
                    supported_successes += 1;
                } else {
                    supported_successes = 0;
                }
            }
            let advance =
                course.next(&attempts, seq as u32, withdrawn) == Decision::Teach(course.remaining);
            assert!(evidence_monitor.next_state(&(advance, supported_successes >= 2)));
            assert_eq!(advance, supported_successes >= 2);
        }
        supported_routes += usize::from(supported_successes >= 2);
        let advance = legacy_trend(&attempts).is_some_and(|s| s >= 0.15);
        trend_failures += usize::from(!monitor().next_state(&(advance, supported_successes >= 2)));
    }
    println!("Four-turn audit: 1296 histories, {supported_routes} eligible end states, {trend_failures} unsupported trend promotions, zero unsupported evidence-routed promotions.");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let choice = args.next().unwrap_or_else(|| "practice".into());
    let supplied = args.next();
    if !matches!(choice.as_str(), "practice" | "check" | "pause" | "teacher")
        || args.next().is_some()
        || (supplied.is_some() && matches!(choice.as_str(), "pause" | "teacher"))
    {
        eprintln!("Usage: assistance_trend [practice|check [numeric-response] | pause | teacher]");
        std::process::exit(2);
    }
    let bytes = UNIT_RULE.as_bytes();
    let pin = Span {
        document: document_id(bytes),
        start: 0,
        end: bytes.len() as u32,
    };
    let course = Course::new(pin);
    let withdrawn = Withdrawn::new(course.scope);
    let source = RuleSource {
        pin,
        bytes: Some(bytes),
        revoked: &withdrawn.sources,
    };
    let sum = |a, b, d| FractionCard::propose(FractionTask::Sum, a, b, d).unwrap();
    let mut history = vec![];
    for (day, card, answer, help) in [
        (1, sum(1, 2, 8), "3/8", Assistance::Reported),
        (3, sum(2, 2, 5), "4/5", Assistance::Reported),
        (8, sum(2, 3, 8), "5/16", Assistance::NoneReported),
        (11, sum(1, 3, 6), "4/12", Assistance::NoneReported),
    ] {
        let (question, _) = card.question(&source).unwrap();
        let judgment = card.assess(answer, &source).unwrap();
        println!("Day {day} | {question} | {answer} | {judgment:?} | {help:?}");
        history.push(course.record(history.len() as u64 + 1, day, judgment, help, pin));
    }
    let trend = legacy_trend(&history).unwrap();
    assert_eq!(trend, 1.0);
    assert_eq!(
        course.next(&history, 12, &withdrawn),
        Decision::Teach(course.adding)
    );
    let advanced = FractionCard::propose(FractionTask::MissingPart, 2, 4, 9).unwrap();
    println!("Trend-driven comparison (+{trend:.2}): Let's apply fraction arithmetic to a missing length. You can see an example, try a fresh sum, ask your teacher, or stop here.");
    println!("Evidence-routed lesson: The last two sums with no help reported came out too small. What does the bottom number tell you about each piece? You can see an example, try a fresh sum, ask your teacher, or stop here.");
    if choice == "practice" {
        println!(
            "Trend-driven day's example. {} {}",
            advanced.question(&source).unwrap().0,
            advanced.worked(&source).unwrap().0
        );
        println!(
            "Evidence-routed day's example. {}",
            sum(2, 4, 10).worked(&source).unwrap().0
        );
    }
    if matches!(choice.as_str(), "practice" | "check") {
        let check = sum(2, 3, 7);
        let answer = supplied.as_deref().unwrap_or("5/7");
        let judgment = check.assess(answer, &source).unwrap();
        println!(
            "Day 14 shared follow-up (response held fixed in both replays) | {} | Learner: {answer}",
            check.question(&source).unwrap().0
        );
        println!("{}", match judgment {
            Judgment::Correct => "Five sevenths is correct on this example. We can check another form on a later visit.",
            Judgment::Incorrect => "The pieces are sevenths. Would you like to draw the two groups and count them together?",
            Judgment::Ambiguous => "I can compare a number or fraction here. You can explain in words to your teacher or enter a numerical result.",
            Judgment::Missing => "We can leave the check open and come back when you want.",
        });
        history.push(course.record(5, 14, judgment, Assistance::NoneReported, pin));
        assert_eq!(
            course.next(&history, 14, &withdrawn),
            Decision::Teach(course.adding)
        );
        println!("Next plan after this response: trend {:.2}, evidence route stays with equal-parts sums.", legacy_trend(&history).unwrap());
    } else if choice == "teacher" {
        println!("You chose a teacher conversation. Bring one of the two recent sums and explain how you chose the bottom number. The independent check stays open.");
    } else {
        println!("We can stop here. When you return, you can choose an example or a fresh sum.");
    }
    println!(
        "Teacher conversations requested: {}. Estimated minutes at 3 per conversation: {}.",
        usize::from(choice == "teacher"),
        3 * usize::from(choice == "teacher")
    );
    audit(&course, pin, &withdrawn);
}
