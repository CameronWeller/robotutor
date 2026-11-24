//! Recheck possible causes of an incorrect transfer response without overwriting it.
use serde_json::json;
use std::collections::BTreeMap;
use tutor_lab::curriculum::{
    goal_evidence::{Evidence, Presentation, Task},
    OutcomeId,
};
use tutor_lab::history::{Scope, Withdrawn};
use tutor_lab::material::document_id;
use tutor_lab::protocol::{transition, Assistance, Event, Gate, Question, State};
use tutor_lab::retention::{Cell, Moment};

fn check(
    id: u64,
    prompt: &str,
    answer: &str,
    key: i64,
    unit: &str,
    assistance: Assistance,
) -> Result<Evidence, String> {
    let cell = Cell {
        scope: Scope {
            learner: 42,
            revision: 1,
        },
        outcome: OutcomeId(10),
    };
    let pack = json!({"unit":id,"check":prompt,"criterion":prompt,"rubric":format!("Give a count in {unit}."),"quantity":{"value":key,"unit":unit}});
    let task = Task::from_pack(
        cell,
        document_id(pack.to_string().as_bytes()),
        "quantity",
        prompt,
        &pack,
    )?;
    let repeat = assistance == Assistance::Reported;
    let sequence = if repeat { 4 } else { id };
    Ok(task.unreviewed_text(
        Presentation {
            attempt: sequence,
            at: Moment { day: 58, sequence },
            fresh: !repeat,
            assistance,
        },
        answer,
    ))
}

pub(super) fn run() -> Result<(), String> {
    let original="A cup contained three quarters of a litre. After pouring, 2 eighths remained. How many eighths were poured?";
    let first = check(
        1,
        original,
        "1 eighths",
        4,
        "eighths",
        Assistance::NoneReported,
    )?;
    let mut state = State::default();
    for event in [
        Event::Ask(Question::RequestedProbe, Gate::Supported),
        Event::Answer(first.judgment(), Assistance::NoneReported),
        Event::Ask(Question::Clarification, Gate::Supported),
        Event::Feedback(Gate::Supported),
        Event::Ask(Question::Clarification, Gate::Supported),
    ] {
        let (next, effect) = transition(state, event);
        println!("{event:?} gives {effect:?}");
        state = next;
    }
    let conversion = "How many quarters of a strip are in one half of the strip?";
    let relation =
        "A shelf held nine books. Four remain after some were borrowed. How many were borrowed?";
    println!("Offered reflection: What did you do to get your number? You can show your working, say you guessed, or skip.");
    println!("Component probes: {conversion} | {relation}");
    let profiles = [
        ("both components established", "2 quarters", "5 books"),
        (
            "wrong operation with correct units",
            "2 quarters",
            "13 books",
        ),
        (
            "unconverted units with correct relation",
            "1 quarters",
            "5 books",
        ),
        ("both component errors", "1 quarters", "13 books"),
        ("conversion omitted", "", "5 books"),
        (
            "correct units, unrelated subtraction slip",
            "2 quarters",
            "6 books",
        ),
    ];
    let mut signatures: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (profile, c, r) in profiles {
        let units = check(2, conversion, c, 2, "quarters", Assistance::NoneReported)?;
        let relation_result = check(3, relation, r, 5, "books", Assistance::NoneReported)?;
        let code = format!("{:?}/{:?}", units.judgment(), relation_result.judgment());
        signatures.entry(code).or_default().push(profile);
    }
    println!("Independent component histories {signatures:?}");
    let repeated = check(1, original, "4 eighths", 4, "eighths", Assistance::Reported)?;
    let changed = check(
        5,
        "Half of a tray was full. One eighth of a whole tray remains. How many eighths were used?",
        "3 eighths",
        3,
        "eighths",
        Assistance::NoneReported,
    )?;
    let withdrawn = Withdrawn::new(Scope {
        learner: 42,
        revision: 1,
    });
    for (name, result) in [
        ("original", first),
        ("coached repetition", repeated),
        ("new item after instruction", changed),
    ] {
        println!(
            "{name}: {:?}, opens transfer={}",
            result.judgment(),
            result.opens_transfer(&withdrawn)
        );
    }
    Ok(())
}
