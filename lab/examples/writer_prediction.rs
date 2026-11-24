//! A writer can commit to a reader prediction before opening the observed reading.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::protocol::Gate;
type Check<T> = Result<T, String>;
const OPTIONS: [&str; 5] = [
    "ask for clarification",
    "carry it away",
    "give it to the conductor",
    "leave it in the room",
    "other",
];
const INVITE:&str="Would you like to predict what one simulated new singer would do with each folder before you hear a reading? You can predict, ask for direct feedback, or stop.";
const INSTRUCTION:&str="Act as one new singer reading the notice. For each situation choose the one action you personally would take from action_options. Return owned and borrowed as the exact option strings. You may choose the same action twice.";
const A: &str = "0776a15ae7dafed260db7c3de7c32ab60f6d6e12f0496ee9ba8f3af8177ca595";
const B: &str = "9ea062d8339b0b091b44f5e834fd9ded51fab42eed9099fa10b2893a645b3ae1";
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn pin(file: &Path, expected: &str) -> Check<Value> {
    let data = fs::read(file).map_err(|e| e.to_string())?;
    if hash(&data) != expected {
        return Err("original trial receipt changed".into());
    }
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
struct Paths {
    owned: usize,
    borrowed: usize,
}
impl Paths {
    fn read(v: &Value) -> Check<Self> {
        if v.as_object().is_none_or(|o| o.len() != 2) {
            return Err("choose one action for each situation".into());
        }
        let index = |key: &str| {
            OPTIONS
                .iter()
                .position(|k| v[key] == *k)
                .ok_or("unoffered action".to_owned())
        };
        Ok(Self {
            owned: index("owned")?,
            borrowed: index("borrowed")?,
        })
    }
}
struct Study {
    draft: String,
    reader: Paths,
    original: [Value; 2],
}
fn study(directory: &Path) -> Check<Study> {
    let arms = [
        pin(&directory.join("A-return.json"), A)?,
        pin(&directory.join("B-return.json"), B)?,
    ];
    let course = include_str!("../fixtures/goals/writer.json").replace("\r\n", "\n");
    let pins: Value = serde_json::from_str(include_str!("../fixtures/goals/reviews.json")).unwrap();
    if pins["writer"] != hash(course.as_bytes()) {
        return Err("writer release changed".into());
    }
    let release: Value = serde_json::from_str(&course).unwrap();
    let work = arms[0]["packet"]["shared_work"]
        .as_str()
        .ok_or("no shared revision")?;
    for r in &arms {
        let p = &r["packet"];
        if p["shared_work"] != work
            || p["receipt"] != json!({})
            || p["new_task"] != release["return_pack"]["practice"]
            || p["lesson_route"]["unit"] != release["return_pack"]["unit"]
            || p["material"]["excerpt"] != release["return_pack"]["excerpt"]
        {
            return Err("release or current writing mismatch".into());
        }
    }
    let draft = work
        .split_once("\n\n")
        .ok_or("cannot separate the shared notice from its rationale")?
        .0
        .to_owned();
    let record: Value = serde_json::from_str(include_str!("../fixtures/writer-reader-031.json"))
        .map_err(|e| e.to_string())?;
    let expected = json!({"notice":draft,"situations":{"owned":"You brought your own folder.","borrowed":"You borrowed a folder."},"action_options":OPTIONS});
    let actual_prompt=format!("Use only the synthetic packet below. Use no tools, files or searches. Return one JSON object.\n{INSTRUCTION}\n{expected}");
    if record["packet"] != expected || record["request_sha256"] != hash(actual_prompt.as_bytes()) {
        return Err("reader packet or instructions differ from the commissioned reading".into());
    }
    Ok(Study {
        draft,
        reader: Paths::read(&record["response"])?,
        original: arms.map(|r| r["response"]["display"].clone()),
    })
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Stage {
    Offer,
    Predicting,
    Committed(Paths),
    InvitedEdit,
    Closed,
}
struct Visit {
    stage: Stage,
    draft: String,
}
struct Frame {
    was_committed: bool,
    commit_now: bool,
    direct: bool,
    revealed: bool,
    stopped: bool,
    changed: bool,
    writer_edit: bool,
}
fn monitor() -> Monitor<Frame> {
    let mut p = Predicates::builder();
    let good = p.new_predicate(ClosurePredicate::new(
        |f: &Frame| {
            (!f.revealed || (f.was_committed || f.direct) && !f.stopped)
                && (!f.changed || f.writer_edit)
        },
        "reveal only after a committed prediction or direct request; edits come from the writer",
    ));
    let previous = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |f: &Frame| f.was_committed,
        "prediction already committed",
    )));
    let now = Property::atomic(p.new_predicate(ClosurePredicate::new(
        |f: &Frame| f.commit_now,
        "writer committed two actions this turn",
    )));
    let follows = Property::Or(
        Box::new(now.clone() & Property::next(previous.clone())),
        Box::new(!now & Property::next(!previous.clone())),
    );
    let law = !previous & Property::always(Property::atomic(good) & follows);
    let p = p.build();
    let m = law.to_monitoring_automaton(&p);
    Monitor::new(p, m)
}
struct Turn {
    display: String,
    revealed: bool,
    closed: bool,
}
impl Visit {
    fn new(s: &Study) -> Self {
        Self {
            stage: Stage::Offer,
            draft: s.draft.clone(),
        }
    }
    fn act(
        &mut self,
        input: &Value,
        gate: Gate,
        reader: Paths,
        m: &mut Monitor<Frame>,
    ) -> Check<Turn> {
        let before = self.stage;
        let initial = self.draft.clone();
        let direct = input == "direct";
        let mut revealed = false;
        let mut writer_edit = false;
        let text = match (self.stage, input) {
            (Stage::Closed, _) => return Err("visit closed".into()),
            (_, v) if v == "stop" => {
                self.stage = Stage::Closed;
                "We'll leave your wording as you wrote it.".into()
            }
            (_, _) if gate != Gate::Supported => {
                self.stage = Stage::Closed;
                "The notice is no longer shared for this activity. We'll stop here.".into()
            }
            (Stage::Offer, v) if v == "predict" => {
                self.stage = Stage::Predicting;
                "Imagine first that you brought a folder, then that you borrowed one. What do you expect the singer to do in each situation?".into()
            }
            (Stage::Predicting, v) if v.is_object() => {
                self.stage = Stage::Committed(Paths::read(v)?);
                "Your prediction is recorded for this exercise. You can show the reading or stop here.".into()
            }
            (Stage::Offer, _) if direct => {
                self.stage = Stage::InvitedEdit;
                revealed = true;
                format!("You asked for direct feedback. One synthetic singer chose ‘{}’ for their own folder and ‘{}’ for a borrowed folder. Which words sent them down those two paths? Keep your version or try an edit of your own.",OPTIONS[reader.owned],OPTIONS[reader.borrowed])
            }
            (Stage::Committed(expected), v) if v == "show" => {
                self.stage = Stage::InvitedEdit;
                revealed = true;
                let mut differences = vec![];
                for (folder, predicted, read) in [
                    ("owned", expected.owned, reader.owned),
                    ("borrowed", expected.borrowed, reader.borrowed),
                ] {
                    if predicted != read {
                        differences.push(format!(
                            "For the {folder} folder you expected ‘{}’. The reader chose ‘{}’.",
                            OPTIONS[predicted], OPTIONS[read]
                        ));
                    }
                }
                if differences.is_empty() {
                    format!("This one synthetic reading matched both actions you predicted: ‘{}’ for an owned folder and ‘{}’ for a borrowed one. Which words do you want to keep so a new singer can make that distinction? You can leave the notice as it is.",OPTIONS[reader.owned],OPTIONS[reader.borrowed])
                } else {
                    format!("{} Read that part of your notice again. Which reading did you intend? You can keep your wording or try one change in your own voice.",differences.join(" "))
                }
            }
            (Stage::InvitedEdit, v) if v == "keep" => {
                self.stage = Stage::Closed;
                "Your original wording is kept.".into()
            }
            (Stage::InvitedEdit, v)
                if v.as_object()
                    .is_some_and(|o| o.len() == 1 && o.contains_key("revision")) =>
            {
                let revised = v["revision"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.chars().count() <= 650)
                    .ok_or("enter your own short notice")?;
                self.draft = revised.into();
                self.stage = Stage::Closed;
                writer_edit = true;
                "Your version is kept. This reader has not read the new wording.".into()
            }
            _ => return Err("that action is unavailable at this point".into()),
        };
        if !m.next_state(&Frame {
            was_committed: matches!(before, Stage::Committed(_)),
            commit_now: matches!(self.stage, Stage::Committed(_)),
            direct: direct && before == Stage::Offer,
            revealed,
            stopped: self.stage == Stage::Closed,
            changed: self.draft != initial,
            writer_edit,
        }) {
            return Err("writer ownership or feedback order failed".into());
        }
        Ok(Turn {
            display: text,
            revealed,
            closed: self.stage == Stage::Closed,
        })
    }
}
fn replay(s: &Study, events: &[Value]) -> Check<Value> {
    let mut visit = Visit::new(s);
    let mut law = monitor();
    let mut turns = vec![json!({"display":INVITE,"reader_shown":false})];
    for e in events {
        let turn = visit.act(e, Gate::Supported, s.reader, &mut law)?;
        turns.push(json!({"writer":e,"display":turn.display,"reader_shown":turn.revealed}));
    }
    Ok(json!({"turns":turns,"final_notice":visit.draft,"closed":visit.stage==Stage::Closed}))
}
fn main() -> Check<()> {
    let mut args = std::env::args().skip(1);
    let s = study(Path::new(
        &args.next().ok_or("supply extracted trial 031 directory")?,
    ))?;
    if args.next().as_deref() == Some("--interact") {
        let mut v = Visit::new(&s);
        let mut law = monitor();
        println!(
            "{}",
            json!({"notice":s.draft,"display":INVITE,"actions":["predict","direct","stop"],"reader_action_options":OPTIONS})
        );
        for line in io::stdin().lock().lines() {
            let e: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let t = v.act(&e, Gate::Supported, s.reader, &mut law)?;
            println!(
                "{}",
                json!({"display":t.display,"reader_shown":t.revealed,"notice":v.draft})
            );
            io::stdout().flush().map_err(|e| e.to_string())?;
            if t.closed {
                break;
            }
        }
        return Ok(());
    }
    let matching = json!({"owned":"carry it away","borrowed":"give it to the conductor"});
    let competing = json!({"owned":"carry it away","borrowed":"leave it in the room"});
    let edit = json!({"revision":"Keep the folder you brought. Hand a borrowed folder back to the conductor."});
    let result = json!({"archived_031":s.original,
        "predicted_both":replay(&s,&[json!("predict"),matching,json!("show"),json!("keep")])?,
        "predicted_room":replay(&s,&[json!("predict"),competing,json!("show"),edit])?,
        "direct":replay(&s,&[json!("direct"),json!("keep")])?,"decline":replay(&s,&[json!("stop")])?});
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn order_and_withdrawal_are_observable() {
        let reader = Paths {
            owned: 1,
            borrowed: 2,
        };
        let mut v = Visit {
            stage: Stage::Offer,
            draft: "original".into(),
        };
        let mut m = monitor();
        assert!(v
            .act(&json!("show"), Gate::Supported, reader, &mut m)
            .is_err());
        v.act(&json!("predict"), Gate::Supported, reader, &mut m)
            .unwrap();
        assert!(v
            .act(&json!("direct"), Gate::Supported, reader, &mut m)
            .is_err());
        assert!(v
            .act(
                &json!({"owned":"carry it away"}),
                Gate::Supported,
                reader,
                &mut m
            )
            .is_err());
        v.act(
            &json!({"owned":"carry it away","borrowed":"leave it in the room"}),
            Gate::Supported,
            reader,
            &mut m,
        )
        .unwrap();
        let turn = v
            .act(&json!("show"), Gate::Unavailable, reader, &mut m)
            .unwrap();
        assert!(turn.closed && !turn.revealed);
        assert!(v
            .act(&json!("show"), Gate::Supported, reader, &mut m)
            .is_err());
        assert!(!monitor().next_state(&Frame {
            was_committed: false,
            commit_now: false,
            direct: false,
            revealed: true,
            stopped: false,
            changed: false,
            writer_edit: false
        }));
        assert!(!monitor().next_state(&Frame {
            was_committed: false,
            commit_now: false,
            direct: false,
            revealed: false,
            stopped: false,
            changed: true,
            writer_edit: false
        }));
    }
}
