//! Two consecutive live-coaching reads. A native decision can select an already released
//! resource, but never author its text or carry that selection into a different turn.
use serde_json::{json, Value};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    coaching::Stage,
    curriculum::{
        goal_rehearsal::Options,
        live_turn::{coach, ReleasedMaterial},
    },
    material::document_id,
    openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA},
};

const WRITER: &str = include_str!("../fixtures/goals/writer.json");
const CHEM: &str = include_str!("../fixtures/goals/chemical-engineering.json");
const ANSWER: &str = "Can I see the source for this part?";

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResultOfTurn {
    Worked(String),
    Source(String),
    Local,
    RejectedAtDecision,
    Unexpected,
}
#[derive(Clone, Debug)]
enum Step {
    Decision,
    Language,
    End {
        expected: ResultOfTurn,
        observed: ResultOfTurn,
    },
}

struct Edition {
    raw: String,
    pin: String,
    first: Value,
}
impl Edition {
    fn from(raw: &str, pin: String) -> Self {
        let data: Value = serde_json::from_str(raw).unwrap();
        Self {
            raw: raw.to_owned(),
            pin,
            first: data["first_pack"].clone(),
        }
    }

    fn result(&self, label: &str) -> ResultOfTurn {
        let text = |key: &str| self.first[key].as_str().unwrap();
        match label {
            "worked_example" => ResultOfTurn::Worked(text("worked").into()),
            "source_passage" => ResultOfTurn::Source(format!(
                "“{}”\n{}\n{}",
                text("excerpt"),
                text("locator"),
                text("source_url")
            )),
            "local_only" => ResultOfTurn::Local,
            _ => unreachable!(),
        }
    }
}

fn turn(
    edition: &Edition,
    answer: &str,
    guided: bool,
    label: &str,
    replay: Option<&Value>,
    expected: ResultOfTurn,
    trace: &mut Vec<Step>,
) -> Value {
    let mut packet = Value::Null;
    let result = coach(
        &edition.raw,
        &edition.pin,
        &Options::default(),
        answer,
        guided,
        (DEFAULT_JEV, DEFAULT_LUNA),
        |role, request| match role {
            Role::Jev => {
                trace.push(Step::Decision);
                let questions = request["questions"].as_object().unwrap();
                let (key, question) = questions.iter().next().unwrap();
                assert_eq!(questions.len(), 1);
                assert!(question["criteria"].get(label).is_some());
                packet=replay.cloned().unwrap_or_else(||json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":label}}}));
                Ok(packet.clone())
            }
            Role::Luna => {
                trace.push(Step::Language);
                Err("unexpected language call in a direct-material trace".into())
            }
        },
    );
    let observed = match result {
        Ok(r) if r.calls == 1 && r.luna.is_none() && r.coaching.is_none() => match r.material {
            Some(ReleasedMaterial::WorkedExample(s)) => ResultOfTurn::Worked(s),
            Some(ReleasedMaterial::SourcePassage(s)) => ResultOfTurn::Source(s),
            None => ResultOfTurn::Local,
        },
        Err(e) if e.stage == Stage::Jev && e.calls == 1 => ResultOfTurn::RejectedAtDecision,
        _ => ResultOfTurn::Unexpected,
    };
    trace.push(Step::End { expected, observed });
    packet
}

// G(decision -> X(exact terminal disposition)). A language call inserted before a
// seemingly correct result violates this property at the call boundary.
fn monitor() -> Monitor<Step> {
    let mut p = Predicates::builder();
    let decision = p.new_predicate(ClosurePredicate::new(
        |s: &Step| matches!(s, Step::Decision),
        "native decision",
    ));
    let end = p.new_predicate(ClosurePredicate::new(
        |s: &Step| matches!(s,Step::End {expected,observed} if expected==observed),
        "correct direct release or hold",
    ));
    let guard = Property::atomic(decision);
    let property = Property::always(Property::Or(
        Box::new(!guard.clone()),
        Box::new(guard & Property::next(Property::atomic(end))),
    ));
    let predicates = p.build();
    let automaton = property.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

#[test]
fn a_direct_decision_terminates_at_its_own_released_card_or_is_held_on_replay() {
    let pins: Value = serde_json::from_str(include_str!("../fixtures/goals/reviews.json")).unwrap();
    let writer = Edition::from(WRITER, pins["writer"].as_str().unwrap().into());
    let chem = Edition::from(CHEM, pins["chemical-engineering"].as_str().unwrap().into());
    // Independent synthetic edition: identical answer and unit ID, newly pinned worked example.
    let mut data: Value = serde_json::from_str(WRITER).unwrap();
    data["revision"] = json!(data["revision"].as_u64().unwrap() + 1);
    data["first_pack"]["worked"] =
        json!("For a first-time visitor, replace 'the usual place' with the named door.");
    let raw = data.to_string();
    let pin = document_id(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let revised = Edition::from(&raw, pin);
    let mut histories = 0;
    let mut boundaries = 0;
    let mut held = 0;
    let mut faults_rejected = 0;
    for initial in ["worked_example", "source_passage"] {
        for (next, answer, guided, same_context) in [
            (&writer, ANSWER, false, true),
            (&writer, "I need the example again.", false, false),
            (&writer, ANSWER, true, false),
            (&revised, ANSWER, false, false),
            (&chem, ANSWER, false, false),
        ] {
            for action in ["worked_example", "source_passage", "local_only", "replay"] {
                let mut trace = Vec::new();
                let prior = turn(
                    &writer,
                    ANSWER,
                    false,
                    initial,
                    None,
                    writer.result(initial),
                    &mut trace,
                );
                let replay = action == "replay";
                let label = if replay { initial } else { action };
                let expected = if replay && !same_context {
                    ResultOfTurn::RejectedAtDecision
                } else {
                    next.result(label)
                };
                turn(
                    next,
                    answer,
                    guided,
                    label,
                    replay.then_some(&prior),
                    expected,
                    &mut trace,
                );
                let mut m = monitor();
                assert!(
                    trace.iter().all(|s| m.next_state(s)),
                    "{initial}, {action}, same={same_context}: {trace:?}"
                );
                boundaries += trace.len();
                histories += 1;
                held += usize::from(matches!(
                    trace.last(),
                    Some(Step::End {
                        observed: ResultOfTurn::RejectedAtDecision,
                        ..
                    })
                ));
                if replay && next.pin == revised.pin && initial == "worked_example" {
                    let (last, prefix) = trace.split_last().unwrap();
                    let Step::End { expected, .. } = last else {
                        unreachable!()
                    };
                    // Faulty renderer reuses the earlier edition's teaching card after rejection.
                    let mut bad = monitor();
                    assert!(prefix.iter().all(|s| bad.next_state(s)));
                    assert!(!bad.next_state(&Step::End {
                        expected: expected.clone(),
                        observed: writer.result(initial)
                    }));
                    // Faulty router invokes Luna after the first valid direct selection.
                    let mut extra = monitor();
                    assert!(extra.next_state(&trace[0]));
                    assert!(!extra.next_state(&Step::Language));
                    faults_rejected += 2;
                }
            }
        }
    }
    assert_eq!(
        (histories, boundaries, held, faults_rejected),
        (40, 160, 8, 2)
    );
}
