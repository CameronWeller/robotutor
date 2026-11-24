//! Separate-process exercise for a saved course obligation and its current authority.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, io::Write, path::Path};
use tutor_lab::{
    context::{self, index::MemoryIndex, Authority, Count, Kind, Purpose, Record, RecordRef},
    curriculum::{OutcomeId, Span},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    supply::{select, Contact, Enrollment, ExposureBook, Form, Requirement},
};
const PRIVATE: &str = "PRIVATE-DRAFT-fundraiser at the repair café";
const RETIRED: &str = "retired worked example";
const OBLIGATION: &str = "Independent check B is still owed.";
const USAGE: &str = "return init DIR | renew DIR PRIOR_HEAD | read DIR MANIFEST CURRENT_HEAD resolved|blob|derived|old-brief|missing-contact|new-assignment|stale-body";

fn save(dir: &Path, file: &str, value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut handle = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(dir.join(file))
        .map_err(|e| e.to_string())?;
    handle
        .write_all(&bytes)
        .and_then(|()| handle.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
fn load(path: &Path, current: &str) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != current {
        return Err("current authority head does not match recovered manifest".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn source_form(task: u64, source: Span) -> Form {
    Form {
        class: 7,
        release: 1,
        outcome: OutcomeId(10),
        owner: None,
        revision: task + 1,
        task,
        family: task,
        source,
        card: FractionCard::propose(FractionTask::MissingPart, 3, task as u16 + 1, 16).unwrap(),
    }
}
fn read(dir: &Path, manifest: &Value, mode: &str) -> Result<(), String> {
    let pointer: Value =
        serde_json::from_slice(&fs::read(dir.join("return.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let (mut req, _, mut base) = context::example(Purpose::Probe);
    if mode == "new-assignment" {
        req.snapshot.class += 1;
    }
    if pointer["assignment"]
        != json!([
            req.snapshot.scope.learner,
            req.snapshot.class,
            req.snapshot.scope.revision,
            req.snapshot.outcome.0
        ])
        || pointer["obligation"] != manifest["obligation"]
        || manifest["assignment"] != pointer["assignment"]
    {
        return Err(
            "saved return does not resolve against the current assignment and obligation".into(),
        );
    }
    let edition = manifest["source_revision"]
        .as_u64()
        .ok_or("missing source revision")?;
    let mut index = MemoryIndex::new(req.snapshot.scope, req.snapshot.class);
    // Recover current authority first. Restoring stale archived bodies afterward cannot admit them.
    base[1].text = format!(
        "{} · {UNIT_RULE}",
        if edition == 1 {
            RETIRED
        } else {
            "current B material"
        }
    );
    base[1].key.revision = edition;
    for row in &mut base {
        row.carry = true;
        index
            .put(index.epoch(), row.clone(), BTreeSet::new())
            .map_err(|e| format!("{e:?}"))?;
    }
    let draft = Record {
        key: RecordRef { id: 8, revision: 1 },
        kind: Kind::Episode,
        text: PRIVATE.into(),
        ..base[0].clone()
    };
    index
        .put(index.epoch(), draft.clone(), BTreeSet::new())
        .map_err(|e| format!("{e:?}"))?;
    let saved_text = if matches!(mode, "blob" | "derived") {
        format!("{OBLIGATION} Reuse {RETIRED}. My draft: {PRIVATE}.")
    } else {
        OBLIGATION.to_string()
    };
    let saved = Record {
        key: RecordRef {
            id: 10,
            revision: 1,
        },
        kind: Kind::SavedReturn,
        text: saved_text,
        ..base[0].clone()
    };
    index
        .put(
            index.epoch(),
            saved.clone(),
            if mode == "derived" {
                [draft.key].into()
            } else {
                BTreeSet::new()
            },
        )
        .map_err(|e| format!("{e:?}"))?;
    if manifest["draft_forgotten"] == true {
        index
            .forget(index.epoch(), draft.key)
            .map_err(|e| format!("{e:?}"))?;
    }
    let known_contact =
        mode != "missing-contact" && manifest.get("exposure").and_then(Value::as_u64).is_some();
    let help = Record {
        key: RecordRef {
            id: 11,
            revision: 1,
        },
        kind: Kind::Help,
        text: "Previously presented task 1; coaching was delivered.".into(),
        ..base[0].clone()
    };
    if known_contact {
        index
            .put(index.epoch(), help.clone(), BTreeSet::new())
            .map_err(|e| format!("{e:?}"))?;
    }
    if mode != "old-brief" {
        base[0].term += 1;
        base[0].key.revision += 1;
        base[0].text = "Prepare an independent B check from the current source and authorized exposure information.".into();
        index
            .put(index.epoch(), base[0].clone(), BTreeSet::new())
            .map_err(|e| format!("{e:?}"))?;
    }
    if mode == "stale-body" {
        let mut archive = base.clone();
        archive[1].key.revision = 1;
        archive[1].text = format!("{RETIRED} · {UNIT_RULE}");
        archive.extend([saved.clone(), help.clone(), draft]);
        index
            .restore(index.epoch(), &archive)
            .map_err(|e| format!("{e:?}"))?;
    }
    req.memory = [known_contact.then_some(help.key), None, Some(saved.key)];
    req.required = base.iter().map(|r| r.key).collect();
    req.input_limit = 8192;
    req.snapshot.term += 1;
    req.snapshot.epoch = index.epoch();
    let rows = index.page(&req).map_err(|e| format!("{e:?}"))?.records;
    let authority = Authority::new(&req, &rows).map_err(|e| format!("{e:?}"))?;
    let pkt = context::compile(&req, &authority, &rows, &|msg| {
        Count::Measured(msg.wire().len() as u32)
    })
    .map_err(|e| format!("packet held: {e:?}"))?;
    let body = &pkt.dispatch(&authority).map_err(|e| format!("{e:?}"))?.user;
    let seat = Enrollment {
        scope: req.snapshot.scope,
        class: req.snapshot.class,
    };
    let bytes = base[1].text.as_bytes();
    let pin = Span {
        document: document_id(bytes),
        start: (bytes.len() - UNIT_RULE.len()) as u32,
        end: bytes.len() as u32,
    };
    let source = RuleSource {
        bytes: Some(bytes),
        pin,
        revoked: &BTreeSet::new(),
    };
    let mut book = ExposureBook::new(seat, known_contact);
    let forms = [source_form(1, pin), source_form(2, pin)];
    if known_contact {
        let contacted = manifest["exposure"].as_u64().ok_or("bad contact")?;
        book.record(seat, &source_form(contacted, pin), Contact::Delivered)
            .map_err(|e| format!("{e:?}"))?;
    }
    let fresh = select(
        seat,
        req.snapshot.outcome,
        Requirement::FreshTask,
        &book,
        &forms,
        &source,
    )
    .ok()
    .map(|r| r.task);
    println!(
        "{}",
        json!({"saved_return":body.contains(OBLIGATION),"private_bytes_in_packet":body.contains(PRIVATE),"retired_instruction_in_packet":body.contains(RETIRED),
        "source_revision":edition,"known_exposure":known_contact,"eligible_fresh_task":fresh,"obligation_completed":false})
    );
    Ok(())
}
pub(super) fn main(args: &[String]) -> Result<(), String> {
    match args {
        [op, dir] if op == "init" => {
            let p = Path::new(dir);
            let pointer = json!({"assignment":[42,7,1,10],"obligation":"B/independent/8"});
            save(p, "return.json", &pointer)?;
            let mut live = pointer;
            live["source_revision"] = json!(1);
            live["draft_forgotten"] = json!(false);
            live["exposure"] = json!(1);
            println!("{}", save(p, "authority-1.json", &live)?);
            Ok(())
        }
        [op, dir, head] if op == "renew" => {
            let p = Path::new(dir);
            let mut live = load(&p.join("authority-1.json"), head)?;
            live["source_revision"] = json!(2);
            live["draft_forgotten"] = json!(true);
            println!("{}", save(p, "authority-2.json", &live)?);
            Ok(())
        }
        [op, dir, name, head, mode]
            if op == "read"
                && matches!(
                    mode.as_str(),
                    "resolved"
                        | "old-brief"
                        | "blob"
                        | "derived"
                        | "missing-contact"
                        | "new-assignment"
                        | "stale-body"
                ) =>
        {
            let p = Path::new(dir);
            read(p, &load(&p.join(name), head)?, mode)
        }
        _ => Err(USAGE.into()),
    }
}
