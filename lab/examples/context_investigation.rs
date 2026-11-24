//! Offline experiments with the real writing-coach requests and publication checks.
#[path = "context_investigation/ambiguity.rs"]
mod ambiguity;
#[path = "context_investigation/classroom.rs"]
mod classroom;
#[path = "context_investigation/selection.rs"]
mod selection;

use selection::{Grant, Selection, Turn};
use serde_json::{json, Value};
use std::cell::RefCell;
use tutor_lab::context::{self, Authority, Count, Kind, Purpose, RecordRef};
use tutor_lab::openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA};
use tutor_lab::workshop::{draft_model::ExportChoice, live, Focus, Workshop};

const INTRO: &str = "Élodie will host the meeting.";
const LINE: &str = "They will open the hall at seven.";
const PRIVATE: &str = "Private organizer note: contact the blue team.";

#[derive(Default)]
struct Recorder {
    requests: Vec<Value>,
}

impl Recorder {
    // Scripted answers exercise both provider adapters. They are not model observations.
    fn answer(&mut self, role: Role, request: &Value, action: &str) -> Result<Value, String> {
        self.requests.push(request.clone());
        match role {
            Role::Jev => {
                let questions = request["questions"]
                    .as_object()
                    .ok_or("missing decisions")?;
                let key = questions.keys().next().ok_or("missing decision")?;
                Ok(
                    json!({"model":DEFAULT_JEV, "answers":{(key):{"type":"choice","choice":action}}}),
                )
            }
            Role::Luna => {
                let data: Value = serde_json::from_str(
                    request["messages"][1]["content"]
                        .as_str()
                        .ok_or("missing chat data")?,
                )
                .map_err(|e| e.to_string())?;
                let reply = json!({"job":data["job"], "critique":{
                    "quote":data["pinned_quote"], "suggestion":"Read this line with a name in place of the pronoun.",
                    "question":"Who opens the hall?"
                }});
                Ok(
                    json!({"model":DEFAULT_LUNA, "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":reply.to_string()}}]}),
                )
            }
        }
    }
    fn describe(&self) -> Value {
        let calls: Vec<_> = self.requests.iter().map(|r| {
            let wire = r.to_string();
            json!({"model":r["model"], "bytes":wire.len(),"includes_private_note":wire.contains(PRIVATE)})
        }).collect();
        json!(calls)
    }
}

fn author_packet(private_text: &str) -> Result<String, String> {
    let (mut request, _, mut records) = context::example(Purpose::Author);
    request.input_limit = 4096;
    let mut episode = records[0].clone();
    episode.key = RecordRef {
        id: 88,
        revision: 1,
    };
    episode.learner = Some(request.snapshot.scope.learner);
    episode.kind = Kind::Episode;
    episode.text = private_text.into();
    records.push(episode);
    let authority = Authority::new(&request, &records).map_err(|e| format!("{e:?}"))?;
    let packet = context::compile(&request, &authority, &records, &|messages| {
        Count::Measured(messages.wire().len() as u32)
    })
    .map_err(|e| format!("{e:?}"))?;
    packet
        .dispatch(&authority)
        .map(|messages| messages.wire())
        .map_err(|e| format!("{e:?}"))
}

fn main() -> Result<(), String> {
    let draft = format!("{INTRO}\n{LINE}\n{PRIVATE}");
    let source = Workshop::new(&draft, Focus::Clarity)?;
    for choice in [ExportChoice::KeepLocal, ExportChoice::ShareThisDraft] {
        let mut trace = Recorder::default();
        let result = live::coach(
            &source,
            DEFAULT_JEV,
            DEFAULT_LUNA,
            choice,
            |role, request| trace.answer(role, request, "coach_0"),
        );
        assert_eq!(result.is_ok(), choice == ExportChoice::ShareThisDraft);
        println!("original {choice:?}: {}", trace.describe());
    }

    // The same line appears twice after the author prepends it. The approved
    // occurrence is the second one, after a multibyte name.
    let repeated = format!("{LINE}\n{draft}");
    let work = Workshop::new(&repeated, Focus::Clarity)?;
    let start = repeated.rfind(LINE).ok_or("missing chosen line")?;
    let grant = Grant {
        artifact: (2, 14),
        epoch: 1,
        revision: work.revision(),
        focus: work.focus(),
        selected: start..start + LINE.len(),
    };
    let mut selection = Selection::new((2, 14), &work, grant.clone())?;
    let mut selected = Recorder::default();
    let Turn::Advice(note) = selection.coach(
        || Some(grant.clone()),
        None,
        |role, r| selected.answer(role, r, "coach_0"),
    )?
    else {
        return Err("missing coaching note".into());
    };
    assert_eq!(note.span, grant.selected);
    assert_eq!(
        work.draft().get(note.span.clone()),
        Some(note.quote.as_str())
    );
    println!(
        "selected: {}; naive start {}, local range {:?}",
        selected.describe(),
        repeated.find(&note.quote).unwrap(),
        note.span
    );

    // A different intended actor in text outside the selection creates the
    // same inputs to both models. The trainer can ask rather than guess.
    let other_document = format!("The two ushers will host the meeting.\n{LINE}");
    let other = Workshop::new(&other_document, Focus::Clarity)?;
    let other_start = other_document.find(LINE).ok_or("missing second case")?;
    let other_grant = Grant {
        artifact: (3, 14),
        epoch: 7,
        selected: other_start..other_start + LINE.len(),
        ..grant.clone()
    };
    assert!(Selection::new((3, 14), &other, grant.clone()).is_err());
    let mut other_trace = Recorder::default();
    let mut other_selection = Selection::new((3, 14), &other, other_grant.clone())?;
    other_selection.coach(
        || Some(other_grant.clone()),
        None,
        |role, r| other_trace.answer(role, r, "coach_0"),
    )?;
    assert_eq!(other_trace.requests, selected.requests);
    println!("two distinct source contexts give identical Jev and Luna requests");

    let mut ask_trace = Recorder::default();
    let Turn::Ask(note, question) = selection.coach(
        || Some(grant.clone()),
        None,
        |role, r| ask_trace.answer(role, r, "clarify_0"),
    )?
    else {
        return Err("expected clarification".into());
    };
    assert_eq!(note.question, "Who opens the hall?");
    assert_eq!(note.span, grant.selected);
    let answer = "The two ushers.";
    let mut clarification = Recorder::default();
    selection.coach(
        || Some(grant.clone()),
        Some((&question, answer)),
        |role, r| clarification.answer(role, r, "coach_0"),
    )?;
    assert!(clarification
        .requests
        .iter()
        .all(|r| r.to_string().contains(answer) && !r.to_string().contains(PRIVATE)));
    println!("with writer's clarification: {}", clarification.describe());

    let mut attempted = Recorder::default();
    let consumed = selection.coach(
        || Some(grant.clone()),
        Some((&question, answer)),
        |role, r| attempted.answer(role, r, "coach_0"),
    );
    let crossed = other_selection.coach(
        || Some(other_grant.clone()),
        Some((&question, PRIVATE)),
        |role, r| attempted.answer(role, r, "coach_0"),
    );
    assert!(consumed.is_err() && crossed.is_err() && attempted.requests.is_empty());
    println!("consumed and different-project answers: zero requests");

    for (label, at, next) in [
        ("withdraw before routing", None, None),
        ("withdraw before coaching", Some(Role::Jev), None),
        (
            "switch selection before display",
            Some(Role::Luna),
            Some(Grant {
                selected: 0..LINE.len(),
                ..grant.clone()
            }),
        ),
        (
            "revise before display",
            Some(Role::Luna),
            Some(Grant {
                revision: 2,
                ..grant.clone()
            }),
        ),
        (
            "switch writer's project before display",
            Some(Role::Luna),
            Some(other_grant),
        ),
        (
            "regrant same range before display",
            Some(Role::Luna),
            Some(Grant {
                epoch: 2,
                ..grant.clone()
            }),
        ),
    ] {
        let current = RefCell::new(at.map(|_| grant.clone()));
        let mut trace = Recorder::default();
        let result = selection.coach(
            || current.borrow().clone(),
            None,
            |role, r| {
                let response = trace.answer(role, r, "coach_0")?;
                if Some(role) == at {
                    current.replace(next.clone());
                }
                Ok(response)
            },
        );
        assert!(result.is_err());
        println!(
            "{label}: {} request(s), 0 notes displayed",
            trace.requests.len()
        );
    }

    // Negative control retaining the prior iteration's raw (question, answer)
    // interface. An excerpt alone cannot fence a mistakenly attached exchange.
    let mut stale_reply = Recorder::default();
    let raw_excerpt = Workshop::new(LINE, Focus::Clarity)?;
    live::coach_with_reply(
        &raw_excerpt,
        DEFAULT_JEV,
        DEFAULT_LUNA,
        ExportChoice::ShareThisDraft,
        Some(("What will the organizer do?", PRIVATE)),
        |role, r| stale_reply.answer(role, r, "coach_0"),
    )
    .map_err(|failure| failure.message)?;
    assert!(stale_reply
        .requests
        .iter()
        .all(|r| r.to_string().contains(PRIVATE)));
    println!("unbound old answer: {}", stale_reply.describe());

    ambiguity::compare()?;
    classroom::project();
    let first = author_packet(&draft)?;
    let second = author_packet("A different writer brought a different project.")?;
    assert_eq!(first, second);
    assert!(!first.contains(LINE) && !first.contains(PRIVATE));
    println!(
        "teacher's shared-authoring request: {} bytes, identical across private documents",
        first.len()
    );
    Ok(())
}
