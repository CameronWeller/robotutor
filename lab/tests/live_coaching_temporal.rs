//! Bounded traces through the public two-provider workshop. "Visible" means the
//! returned report contains a note eligible for display; terminal rendering and HTTP are outside this test.
use serde_json::{json, Value};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA};
use tutor_lab::workshop::{
    draft_model::{ExportChoice, Suggestion},
    live::{coach, Outcome},
    Focus, Workshop,
};

const FIRST: &str = "The blue door opens onto the square.";
const EDITED: &str = "The service entrance faces the canal.";

#[derive(Clone, Copy, Debug)]
enum Path {
    Current,
    OldDecision,
    OldLanguage,
    Abstain,
    DecisionFailure,
    LanguageFailure,
    Local,
}
use Path::*;

#[derive(Clone, Debug)]
enum Step {
    Edit,
    Decision { stops: bool },
    Language,
    Finished(Option<(String, String)>),
}

#[derive(Clone, Debug)]
struct Frame {
    revision: u64,
    draft: String,
    decision_origin: Option<u64>,
    language_origin: Option<u64>,
    step: Step,
}

#[derive(Default)]
struct Packets {
    revision: u64,
    decision: Value,
    language: Value,
}

fn advice(revision: u64) -> String {
    format!("Compare the two possible directions in version {revision}.")
}

fn turn(work: &Workshop, path: Path, old: &Packets, frames: &mut Vec<Frame>) -> Packets {
    let revision = work.revision();
    let mut frame = Frame {
        revision,
        draft: work.draft().into(),
        decision_origin: None,
        language_origin: None,
        step: Step::Edit,
    };
    let mut captured = Packets {
        revision,
        ..Packets::default()
    };
    let share = if matches!(path, Local) {
        ExportChoice::KeepLocal
    } else {
        ExportChoice::ShareThisDraft
    };
    let report = coach(work, DEFAULT_JEV, DEFAULT_LUNA, share, |role, request| {
        assert!(
            !matches!(path, Local),
            "a local-only turn called a provider"
        );
        match role {
            Role::Jev => {
                let key = request["questions"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap();
                assert_eq!(request["state"]["revision"], revision);
                assert_eq!(request["state"]["draft"], work.draft());
                let choice = if matches!(path, Abstain) {
                    "abstain"
                } else {
                    "coach_0"
                };
                // Provenance comes from the injected transport's recorded replies, independent
                // of the implementation's own decision about whether to accept those bytes.
                frame.decision_origin = match path {
                    DecisionFailure | Abstain => None,
                    OldDecision => Some(old.revision),
                    _ => Some(revision),
                };
                frame.step = Step::Decision {
                    stops: frame.decision_origin != Some(revision),
                };
                frames.push(frame.clone());
                if matches!(path, DecisionFailure) {
                    return Err("simulated decision outage".into());
                }
                captured.decision = if matches!(path, OldDecision) {
                    old.decision.clone()
                } else {
                    json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":choice}}})
                };
                Ok(captured.decision.clone())
            }
            Role::Luna => {
                frame.language_origin = match path {
                    LanguageFailure => None,
                    OldLanguage => Some(old.revision),
                    _ => Some(revision),
                };
                frame.step = Step::Language;
                frames.push(frame.clone());
                if matches!(path, LanguageFailure) {
                    return Err("simulated language outage".into());
                }
                let job: Value =
                    serde_json::from_str(request["messages"][1]["content"].as_str().unwrap())
                        .unwrap();
                let note = json!({"job":job["job"],"critique":{"quote":job["pinned_quote"],
                    "suggestion":advice(revision),"question":"Which entrance should the reader use?"}});
                captured.language = if matches!(path, OldLanguage) {
                    old.language.clone()
                } else {
                    json!({"model":DEFAULT_LUNA,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":note.to_string()}}]})
                };
                Ok(captured.language.clone())
            }
        }
    });
    let visible = report.ok().and_then(|r| match r.outcome {
        Outcome::Suggestion(Suggestion {
            quote, suggestion, ..
        })
        | Outcome::Clarify(Suggestion {
            quote, suggestion, ..
        }) => Some((quote, suggestion)),
        Outcome::Abstain => None,
    });
    frame.step = Step::Finished(visible);
    frames.push(frame);
    captured
}

fn trace(restore_original_text: bool, path: Path) -> Vec<Frame> {
    let mut work = Workshop::new(FIRST, Focus::Clarity).unwrap();
    let mut frames = Vec::new();
    let saved = turn(&work, Current, &Packets::default(), &mut frames);
    for text in [EDITED]
        .into_iter()
        .chain(restore_original_text.then_some(FIRST))
    {
        work.revise(text).unwrap();
        frames.push(Frame {
            revision: work.revision(),
            draft: work.draft().into(),
            decision_origin: None,
            language_origin: None,
            step: Step::Edit,
        });
    }
    turn(&work, path, &saved, &mut frames);
    frames
}

fn monitor_factory(check_stop: bool) -> impl Fn() -> Monitor<Frame> {
    let symbols = move || {
        let mut p = Predicates::builder();
        let visible = p.new_predicate(ClosurePredicate::new(
            |f: &Frame| matches!(f.step, Step::Finished(Some(_))),
            "visible",
        ));
        let current = p.new_predicate(ClosurePredicate::new(
            |f: &Frame| {
                let Step::Finished(Some((quote, suggestion))) = &f.step else {
                    return false;
                };
                f.decision_origin == Some(f.revision)
                    && f.language_origin == Some(f.revision)
                    && f.draft.contains(quote)
                    && *suggestion == advice(f.revision)
            },
            "both receipts and the emitted note match this revision",
        ));
        let stop = p.new_predicate(ClosurePredicate::new(
            |f: &Frame| matches!(f.step, Step::Decision { stops: true }),
            "decision stopped the turn",
        ));
        let quiet = p.new_predicate(ClosurePredicate::new(
            |f: &Frame| matches!(f.step, Step::Finished(None)),
            "finished without a generated note",
        ));
        let (trigger, consequence) = if check_stop {
            (stop, Property::next(Property::atomic(quiet)))
        } else {
            (visible, Property::atomic(current))
        };
        let trigger = Property::atomic(trigger);
        // Disjoint guards are required for telo's deterministic monitor.
        let implication = Property::Or(Box::new(!trigger.clone()), Box::new(trigger & consequence));
        (p.build(), Property::always(implication))
    };
    let (predicates, property) = symbols();
    let automaton = property.to_monitoring_automaton(&predicates);
    move || Monitor::new(symbols().0, automaton.clone())
}

#[test]
fn only_current_receipts_can_release_a_note_and_a_stopped_decision_ends_the_turn() {
    let visible = monitor_factory(false);
    let stopped = monitor_factory(true);
    let (mut boundaries, mut shown, mut stops) = (0, 0, 0);
    for restore in [false, true] {
        for path in [
            Current,
            OldDecision,
            OldLanguage,
            Abstain,
            DecisionFailure,
            LanguageFailure,
            Local,
        ] {
            let mut released = visible();
            let mut halted = stopped();
            let trace = trace(restore, path);
            for f in &trace {
                assert!(
                    released.next_state(f) && halted.next_state(f),
                    "restore={restore} path={path:?} frame={f:?}"
                );
                boundaries += 1;
                shown += usize::from(matches!(f.step, Step::Finished(Some(_))));
                stops += usize::from(matches!(f.step, Step::Decision { stops: true }));
            }
            assert_eq!(
                matches!(trace.last().unwrap().step, Step::Finished(Some(_))),
                matches!(path, Current)
            );
        }
    }
    assert_eq!((boundaries, shown, stops), (95, 16, 6));
}

#[test]
fn replaying_the_first_note_after_editing_back_to_identical_words_fails_the_monitor() {
    let mut frames = trace(true, OldLanguage);
    let note = frames
        .iter()
        .find_map(|f| match &f.step {
            Step::Finished(Some(note)) => Some(note.clone()),
            _ => None,
        })
        .unwrap();
    let last = frames.last_mut().unwrap();
    assert_eq!(last.revision, 3);
    assert!(last.draft.contains(&note.0) && matches!(last.step, Step::Finished(None)));
    // Fault injection: a caller carries the last admitted note over a rejected stale response.
    last.step = Step::Finished(Some(note));
    let mut monitor = monitor_factory(false)();
    let (bad, good_prefix) = frames.split_last().unwrap();
    assert!(good_prefix.iter().all(|f| monitor.next_state(f)));
    assert!(!monitor.next_state(bad));
}
