//! Forecast which released question would lead to a different lesson.
//! Run with question seconds and an answer in eighths, e.g. `10 2`.
//! The three answer-generating rules are experimental counterexamples.
use std::collections::BTreeSet;
use tutor_lab::{
    contrast::{select, Candidate},
    curriculum::Span,
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::Judgment,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Lesson {
    ChangeContext,
    UnequalCounts,
    LocateHalf,
}

const LESSONS: [Lesson; 3] = [
    Lesson::ChangeContext,
    Lesson::UnequalCounts,
    Lesson::LocateHalf,
];

fn form(first: u16, second: u16, seconds: u32) -> Candidate<FractionCard, u16> {
    let item = FractionCard::propose(FractionTask::Sum, first, second, 8)
        .expect("valid editorial example");
    Candidate {
        item,
        seconds,
        // Count both addends, double the first, or repeat the opening's half.
        predicts: vec![Some(first + second), Some(first * 2), Some(4)],
    }
}

fn run(seconds: u32, eighths: u16, source: &RuleSource<'_>) -> Result<(), &'static str> {
    let opening = form(2, 2, 10);
    let (prompt, _) = opening
        .item
        .question(source)
        .ok_or("original card unavailable")?;
    println!("First visit: {prompt} Answer: 1/2.");
    if opening.item.assess("1/2", source) != Some(Judgment::Correct) {
        return Err("opening unsupported");
    }
    // The first changed wording is cheaper. Its correct answer still equals half.
    let candidates = [form(1, 3, 8), form(1, 2, 10)];
    let (worked, _) = opening
        .item
        .worked(source)
        .ok_or("explanation unavailable")?;
    let Some(contrast) = select(&LESSONS, &candidates, seconds)? else {
        println!("Worked explanation: {worked}");
        return Ok(());
    };
    let card = &contrast.candidate.item;
    let (prompt, _) = card.question(source).ok_or("selected card unavailable")?;
    println!("Optional contrast: {prompt} Answer: {eighths}/8.");
    let answer = format!("{eighths}/8");
    let grade = card
        .assess(&answer, source)
        .ok_or("assessment unavailable")?;
    let (feedback, _) = card.worked(source).ok_or("feedback unavailable")?;
    println!("Quantity: {grade:?}. Feedback: {feedback}");
    let suggestion = match contrast.suggested_lesson(&eighths) {
        Some(Lesson::ChangeContext) => "Try finding a missing length in a ribbon?",
        Some(Lesson::UnequalCounts) => "Try counting two unequal groups of pieces?",
        Some(Lesson::LocateHalf) => "Try marking half a metre and comparing it with the sum?",
        None => "Would a picture or another worked example help?",
    };
    println!("Offer: {suggestion}");
    Ok(())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let seconds = args
        .next()
        .unwrap_or_else(|| "10".into())
        .parse::<u32>()
        .map_err(|_| "expected seconds available after reserving feedback time")?;
    let eighths = args
        .next()
        .unwrap_or_else(|| "3".into())
        .parse::<u16>()
        .map_err(|_| "expected a nonnegative answer in eighths")?;
    if args.next().is_some() {
        return Err("usage: teaching_contrast [seconds] [answer_in_eighths]".into());
    }
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin: Span {
            document: document_id(UNIT_RULE.as_bytes()),
            start: 0,
            end: UNIT_RULE.len() as u32,
        },
        revoked: &BTreeSet::new(),
    };
    run(seconds, eighths, &source).map_err(str::to_owned)
}

#[test]
fn only_a_changed_answer_partition_discriminates_the_three_interventions() {
    let cards = [form(1, 3, 8), form(1, 2, 10)];
    let cheaper = select(&LESSONS, &cards, 8).unwrap().unwrap();
    assert_eq!(cheaper.worst_remaining_lessons, 2);
    assert_eq!(cheaper.suggested_lesson(&4), None);
    let chosen = select(&LESSONS, &cards, 10).unwrap().unwrap();
    assert_eq!(chosen.worst_remaining_lessons, 1);
    for (answer, expected) in [
        (2, Lesson::UnequalCounts),
        (3, Lesson::ChangeContext),
        (4, Lesson::LocateHalf),
    ] {
        assert_eq!(chosen.suggested_lesson(&answer), Some(&expected));
    }
    assert_eq!(chosen.suggested_lesson(&7), None);
    assert!(select(&LESSONS, &cards, 7).unwrap().is_none());
}
