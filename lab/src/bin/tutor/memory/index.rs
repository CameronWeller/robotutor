use super::{session_name, short, Result, Workspace};
use crate::session::{Event, SourceRef};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};
use tutor_lab::platform::PrivateOpen;

impl Workspace {
    /// Read only newly appended, newline-committed events from each eligible visit.
    pub fn sync(&mut self) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut present = HashSet::new();
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(name) = path
                .file_stem()
                .and_then(|x| x.to_str())
                .filter(|x| session_name(x))
            else {
                continue;
            };
            let Ok(file) = fs::OpenOptions::new().read(true).open_private(&path) else {
                continue;
            };
            index_journal(&tx, name, &path, file)?;
            present.insert(name.to_owned());
        }
        let known = tx
            .prepare("SELECT name FROM visits")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for name in known {
            if !present.contains(&name) {
                tx.execute("DELETE FROM visits WHERE name=?", [name])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
fn legacy_day(name: &str) -> Option<&str> {
    let d = name.get(..10)?;
    (d.as_bytes().iter().enumerate().all(|(i, b)| {
        if i == 4 || i == 7 {
            *b == b'-'
        } else {
            b.is_ascii_digit()
        }
    }))
    .then_some(d)
}

fn index_journal(tx: &Transaction, name: &str, path: &Path, mut file: File) -> Result<()> {
    let meta = file.metadata()?;
    let (volume, index) = tutor_lab::platform::file_identity(&file)?;
    let identity = format!("{volume}:{index}");
    let mtime = format!("{:?}", meta.modified()?);
    let previous = tx
        .query_row(
            "SELECT identity,mtime,offset,ordinal,pending,prefix_hash FROM visits WHERE name=?",
            [name],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    u64::try_from(r.get::<_, i64>(2)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let mut digest = Sha256::new();
    let mut cursor = None;
    let mut project_plan = false;
    if let Some((id, stamp, off, n, p, previous_hash)) = previous {
        let indexed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM continuations WHERE visit=?)",
            [name],
            |r| r.get(0),
        )?;
        let checked_plan: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM plan_heads WHERE visit=?)",
            [name],
            |r| r.get(0),
        )?;
        if id == identity && off == meta.len() && stamp == mtime && checked_plan {
            if n > 0 && !indexed {
                super::study::backfill(tx, name, path, off)?;
            }
            return Ok(());
        }
        if id == identity && off <= meta.len() {
            // Check the old prefix even on growth: a learner can edit and append between visits.
            std::io::copy(&mut (&mut file).take(off), &mut digest)?;
            if format!("{:x}", digest.clone().finalize()) == previous_hash {
                if n > 0 && !indexed {
                    super::study::backfill(tx, name, path, off)?;
                }
                project_plan = !checked_plan && off > 0;
                cursor = Some((off, n, p));
            }
        }
    }
    let (mut offset, mut ordinal, mut pending) = match cursor {
        Some(state) => state,
        None => {
            digest = Sha256::new();
            file.seek(SeekFrom::Start(0))?;
            tx.execute("DELETE FROM visits WHERE name=?", [name])?;
            tx.execute(
                "INSERT INTO visits(name,identity,mtime,offset,ordinal) VALUES(?,?,?,0,0)",
                params![name, identity, mtime],
            )?;
            (0, 0, None)
        }
    };
    let mut cleared_plan = false;
    let mut verified_decisions = false;
    let mut input = BufReader::new(file.take(meta.len() - offset));
    let mut line = Vec::new();
    loop {
        line.clear();
        let count = input.read_until(b'\n', &mut line)?;
        if count == 0 || !line.ends_with(b"\n") {
            break;
        }
        let row: Value = serde_json::from_slice(&line)?;
        let stamp = row["recorded_at"].as_i64();
        let event: Event = serde_json::from_value(row)?;
        if (offset == 0) != matches!(event, Event::Open { version: 1, .. }) {
            return Err(format!("Invalid journal header in {name}.").into());
        }
        match event {
            Event::Open { version: 1, .. } => {}
            Event::Open { .. } => return Err("Unknown journal version.".into()),
            Event::RestoredGoal { .. } | Event::Planned { .. } => {}
            Event::SupervisorPlan { .. } => project_plan = true,
            Event::Continued { .. } => {}
            Event::PracticeAnswer { text, .. }
            | Event::Learner { text }
            | Event::ResumedInterrupted { text } => pending = Some(text),
            Event::Delivered {
                text,
                mut lesson,
                sources,
                judgment,
            } => {
                if let Some(r) = &judgment {
                    if r.inputs.source.rsplit_once('#').map(|(id, _)| id) != Some(name) {
                        return Err("Unexpected decision source.".into());
                    }
                    if !verified_decisions {
                        crate::session::replay_prefix(path, meta.len())?;
                        verified_decisions = true;
                    }
                    let serialized = crate::session::delivery_hash(r, &lesson, &text);
                    if let Some(prior) = tx
                        .query_row(
                            "SELECT receipt FROM teaching_decisions WHERE source=?",
                            [&r.inputs.source],
                            |q| q.get::<_, String>(0),
                        )
                        .optional()?
                    {
                        if prior != serialized
                            || tx.query_row(
                                "SELECT tutor FROM turns WHERE source=?",
                                [&r.inputs.source],
                                |q| q.get::<_, String>(0),
                            )? != text
                        {
                            return Err("Conflicting duplicate decision.".into());
                        }
                        digest.update(&line);
                        offset += count as u64;
                        continue;
                    }
                    if r.inputs.source != format!("{name}#{}", ordinal + 1) {
                        return Err("Unexpected decision source.".into());
                    }
                }
                project_plan = false;
                cleared_plan = true;
                ordinal += 1;
                index_turn(
                    tx,
                    name,
                    ordinal,
                    stamp,
                    pending.take().as_deref(),
                    &text,
                    &lesson.objective,
                    &sources,
                )?;
                if let Some(r) = judgment {
                    let findings = if r.inputs.shadow {
                        &[][..]
                    } else {
                        &r.decision.findings[..]
                    };
                    tx.execute(
                        "INSERT INTO teaching_decisions(source,findings,receipt) VALUES(?,?,?)",
                        params![
                            r.inputs.source,
                            serde_json::to_string(findings)?,
                            crate::session::delivery_hash(&r, &lesson, &text)
                        ],
                    )?;
                    lesson.rationale = r.decision.rationale();
                }
                super::study::record(tx, name, ordinal, &lesson)?;
            }
            Event::Recovered {
                exchanges,
                continuation,
            } => {
                if ordinal != 0 || pending.is_some() || exchanges.is_empty() {
                    return Err("Recovered turns must begin a visit.".into());
                }
                for (learner, tutor) in exchanges {
                    ordinal += 1;
                    index_turn(tx, name, ordinal, None, learner.as_deref(), &tutor, "", &[])?;
                }
                super::study::record(tx, name, ordinal, &continuation)?;
            }
            Event::SubmissionPreview { .. }
            | Event::Timing { .. }
            | Event::WorkspaceDraft { .. }
            | Event::DiscoveryFeedback { .. }
            | Event::Pilot { .. }
            | Event::TurnStep { .. }
            | Event::Rejected { .. }
            | Event::Failed { .. }
            | Event::Review { .. } => {}
        }
        digest.update(&line);
        offset += count as u64;
    }
    if cleared_plan {
        tx.execute("DELETE FROM plan_heads WHERE visit=?", [name])?;
    }
    if project_plan {
        let v = crate::session::replay_prefix(path, offset)?;
        if let Some(at) = v.last_plan_at {
            let anchor = if ordinal == 0 {
                v.from
            } else {
                let source = format!("{name}#{ordinal}");
                let digest =
                    tx.query_row("SELECT digest FROM turns WHERE source=?", [&source], |r| {
                        r.get(0)
                    })?;
                Some(SourceRef { source, digest })
            };
            if let Some(anchor) = anchor {
                tx.execute("INSERT OR REPLACE INTO plan_heads(visit,source,digest,state,recorded) VALUES(?,?,?,?,?)",
                    params![name, anchor.source, anchor.digest, serde_json::to_string(&v.lesson)?, i64::try_from(at)?])?;
            }
        }
    }
    tx.execute("INSERT OR IGNORE INTO plan_heads(visit) VALUES(?)", [name])?;
    tx.execute(
        "UPDATE visits SET mtime=?, offset=?, ordinal=?, pending=?,prefix_hash=? WHERE name=?",
        params![
            mtime,
            i64::try_from(offset)?,
            ordinal,
            pending,
            format!("{:x}", digest.finalize()),
            name
        ],
    )?;
    Ok(())
}
fn index_turn(
    tx: &Transaction,
    visit: &str,
    n: i64,
    ts: Option<i64>,
    learner: Option<&str>,
    tutor: &str,
    objective: &str,
    sources: &[SourceRef],
) -> Result<()> {
    let source = format!("{visit}#{n}");
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(learner, tutor))?)
    );
    tx.execute(
        "INSERT INTO turns(source,visit,ordinal,day,learner,tutor,digest,recorded)
        VALUES(?,?,?,coalesce(date(?,'unixepoch','localtime'),date(?)),?,?,?,?)",
        params![
            source,
            visit,
            n,
            ts,
            legacy_day(visit),
            learner,
            tutor,
            digest,
            ts
        ],
    )?;
    let mut left = learner.unwrap_or_default().chars().peekable();
    let mut right = tutor.chars().peekable();
    let mut part = 0;
    while left.peek().is_some() || right.peek().is_some() {
        let l: String = left.by_ref().take(850).collect();
        let r: String = right.by_ref().take(1200).collect();
        let passage = format!("{}\nLearner: {l}\nTutor: {r}", short(objective, 180));
        tx.execute("INSERT INTO passages(source,part,learner,tutor,body,vector)
             VALUES(?1,?2,?3,?4,?5,(SELECT vector FROM passages WHERE body=?5 AND vector IS NOT NULL LIMIT 1))",
             params![source,part,(!l.is_empty()).then_some(l),r,passage])?;
        part += 1;
    }
    for target in sources {
        tx.execute(
            "INSERT OR IGNORE INTO links(origin,target,digest) VALUES(?,?,?)",
            params![source, target.source, target.digest],
        )?;
    }
    Ok(())
}
