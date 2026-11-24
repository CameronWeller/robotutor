//! Rebuildable learner projections. Journals remain authoritative.
use super::{Result, Workspace};
use crate::lesson::Lesson;
use crate::session::{Event, SourceRef};
use crate::study::{Observation, Skill, Target, Work};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

pub(super) fn record(
    tx: &Transaction<'_>,
    visit: &str,
    ordinal: i64,
    lesson: &Lesson,
) -> Result<()> {
    let source = format!("{visit}#{ordinal}");
    let mut lesson = lesson.clone();
    if lesson.return_draft.take().is_none()
        && (lesson.declared_goal.is_none()
            || lesson.observation.is_none()
            || lesson.prior_attempts.is_empty())
    {
        let previous: Option<String> = tx
            .query_row(
                "SELECT state FROM continuations WHERE visit=?",
                [visit],
                |r| r.get(0),
            )
            .optional()?;
        let previous = previous
            .map(|s| serde_json::from_str::<Lesson>(&s))
            .transpose()?;
        let learner: Option<String> =
            tx.query_row("SELECT learner FROM turns WHERE source=?", [&source], |r| {
                r.get(0)
            })?;
        lesson.recover_metadata(previous.as_ref(), learner.as_deref());
    }
    tx.execute("INSERT INTO continuations(visit,source,state) VALUES(?,?,?) ON CONFLICT(visit) DO UPDATE SET source=excluded.source,state=excluded.state",
        params![visit,source,serde_json::to_string(&lesson)?])?;
    // Discovery retains a legacy exercise for resumption. Talking about it does
    // not constitute another attempt at that exercise.
    if lesson
        .discovery
        .as_ref()
        .is_some_and(crate::discovery::Discovery::active)
    {
        return Ok(());
    }
    let target = Target::from_lesson(&lesson);
    tx.execute(
        "INSERT INTO study_turns(source,target,exercise,observation,taught) VALUES(?,?,?,?,?)",
        params![
            source,
            serde_json::to_string(&target)?,
            lesson.exercise,
            lesson
                .observation
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            lesson.taught()
        ],
    )?;
    Ok(())
}

/// Existing vector indexes already have the turns. Replay only the projection missing from their prefix.
pub(super) fn backfill(tx: &Transaction<'_>, name: &str, path: &Path, offset: u64) -> Result<()> {
    let mut ordinal = 0;
    let mut seen_decisions = std::collections::BTreeSet::new();
    for line in BufReader::new(File::open(path)?.take(offset)).lines() {
        let row: serde_json::Value = serde_json::from_str(&line?)?;
        let stamp = row["recorded_at"].as_i64();
        match serde_json::from_value::<Event>(row)? {
            Event::Delivered {
                mut lesson,
                judgment,
                ..
            } => {
                if let Some(r) = judgment {
                    if !seen_decisions.insert(r.inputs.source) {
                        continue;
                    }
                    lesson.rationale = r.decision.rationale();
                }
                ordinal += 1;
                tx.execute(
                    "UPDATE turns SET recorded=? WHERE visit=? AND ordinal=?",
                    params![stamp, name, ordinal],
                )?;
                record(tx, name, ordinal, &lesson)?;
            }
            Event::Recovered {
                exchanges,
                continuation,
            } => {
                ordinal += exchanges.len() as i64;
                record(tx, name, ordinal, &continuation)?;
            }

            _ => {}
        }
    }
    Ok(())
}
impl Workspace {
    /// Follow a continued visit back to its learner-declared intake, at the actual turn.
    pub fn goal_at(&self, anchor: &SourceRef) -> Result<Option<String>> {
        let mut anchor = anchor.clone();
        let mut seen = std::collections::BTreeSet::new();
        while seen.insert(anchor.source.clone()) {
            if self
                .lookup(&anchor.source)?
                .is_none_or(|h| h.digest != anchor.digest)
            {
                return Ok(None);
            }
            let Some((visit, n)) = anchor.source.rsplit_once('#') else {
                return Ok(None);
            };
            let n = n.parse::<usize>()?;
            let path = self.journal_path(Some(Path::new(&format!("{visit}.jsonl"))))?;
            let mut pending = None;
            let mut parent = None;
            let mut goal = None;
            let mut ordinal = 0;
            let mut seen_decisions = std::collections::BTreeSet::new();
            for line in BufReader::new(File::open(path)?).lines() {
                match serde_json::from_str::<Event>(&line?)? {
                    Event::Open { selected, .. } => goal = selected,
                    Event::Continued { from, .. } => parent = Some(from),
                    Event::RestoredGoal { goal: recovered } => goal = Some(recovered),
                    Event::Learner { text } | Event::ResumedInterrupted { text } => {
                        pending = Some(text)
                    }
                    Event::Delivered {
                        lesson, judgment, ..
                    } => {
                        if judgment.is_some_and(|r| !seen_decisions.insert(r.inputs.source)) {
                            continue;
                        }
                        if let Some(g) = lesson.goal_request(pending.take().as_deref()) {
                            goal = Some(g);
                        }
                        ordinal += 1;
                    }
                    Event::Recovered {
                        exchanges,
                        continuation,
                    } => {
                        ordinal += exchanges.len();
                        goal = continuation.selected.clone();
                    }
                    _ => {}
                }
                if ordinal >= n {
                    break;
                }
            }
            if goal.is_some() {
                return Ok(goal);
            }
            let Some(from) = parent else {
                return Ok(None);
            };
            anchor = from;
        }
        Ok(None)
    }
    pub fn continuity(&self, from: &SourceRef) -> Result<Vec<super::Hit>> {
        let mut stmt=self.db.prepare("SELECT t.source FROM turns t JOIN turns anchor ON anchor.visit=t.visit
            WHERE anchor.source=? AND anchor.digest=? AND t.ordinal<=anchor.ordinal ORDER BY t.ordinal DESC LIMIT 4")?;
        let mut keys = stmt
            .query_map(params![from.source, from.digest], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if keys.is_empty() {
            return Ok(vec![]);
        }
        keys.extend(self.links(&from.source)?.into_iter().take(2));
        let mut hits = Vec::new();
        for key in keys {
            if let Some(mut h) = self.lookup(&key)? {
                h.learner = h.learner.map(|s| super::short(&s, 800));
                h.tutor = super::short(&h.tutor, 1200);
                hits.push(h);
            }
        }
        Ok(hits)
    }
    pub fn continuation(&self, session: &str) -> Result<Option<(SourceRef, Lesson)>> {
        let row:Option<(String,String,String)>=self.db.query_row(
            "SELECT source,digest,state FROM (
                 SELECT t.source,t.digest,c.state,t.day,coalesce(t.recorded,0) AS at,v.mtime,t.ordinal,0 AS directed
                 FROM continuations c JOIN turns t ON t.source=c.source JOIN visits v ON v.name=c.visit WHERE c.visit<>?1
                 UNION ALL
                 SELECT t.source,t.digest,p.state,coalesce(date(nullif(p.recorded,0),'unixepoch','localtime'),t.day),
                        coalesce(nullif(p.recorded,0),t.recorded,0),v.mtime,t.ordinal,1
                 FROM plan_heads p JOIN turns t ON t.source=p.source AND t.digest=p.digest
                 JOIN visits v ON v.name=p.visit WHERE p.visit<>?1
             ) ORDER BY day DESC,at DESC,mtime DESC,ordinal DESC,directed DESC LIMIT 1",
            [session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        row.map(|(source, digest, state)| {
            Ok((SourceRef { source, digest }, serde_json::from_str(&state)?))
        })
        .transpose()
    }
    pub fn skills(&self) -> Result<Vec<Skill>> {
        let today: u32 = self.db.query_row(
            "SELECT CAST(julianday(date('now','localtime'))-2440587.5 AS INTEGER)",
            [],
            |r| r.get(0),
        )?;
        let mut stmt=self.db.prepare("SELECT t.source, CAST(julianday(t.day)-2440587.5 AS INTEGER),
            s.target,s.exercise,s.observation,s.taught FROM study_turns s JOIN turns t ON t.source=s.source
            ORDER BY t.day,coalesce(t.recorded,0),t.visit,t.ordinal")?;
        let mut rows = stmt.query([])?;
        let mut skills = BTreeMap::new();
        let mut seq = 0u64;
        while let Some(r) = rows.next()? {
            let work = Work {
                source: r.get(0)?,
                day: r
                    .get::<_, Option<i64>>(1)?
                    .and_then(|n| u32::try_from(n).ok()),
                target: serde_json::from_str(&r.get::<_, String>(2)?)?,
                exercise: r.get(3)?,
                observation: r
                    .get::<_, Option<String>>(4)?
                    .map(|s| serde_json::from_str::<Observation>(&s))
                    .transpose()?,
                offered: r.get(5)?,
            };
            seq += 2;
            if let Some(obs) = &work.observation {
                skills
                    .entry(obs.target.key())
                    .or_insert_with(|| Skill::new(obs.target.clone()))
                    .encounter(&work, seq, true);
            }
            if !work.target.objective.is_empty() && (work.offered || !work.exercise.is_empty()) {
                let skill = skills
                    .entry(work.target.key())
                    .or_insert_with(|| Skill::new(work.target.clone()));
                if work.offered {
                    skill.encounter(&work, seq + 1, false);
                } else if !work.exercise.is_empty() {
                    skill.last_task = work.exercise;
                    if skill.last_source.is_empty() {
                        skill.last_source = work.source;
                        skill.latest_day = work.day.unwrap_or(0);
                    }
                }
            }
        }
        let mut skills: Vec<_> = skills.into_values().collect();
        let next_day_guided_review = std::env::var("ROBOTUTOR_NEXT_DAY_GUIDED_REVIEW").as_deref() == Ok("1");
        for s in &mut skills {
            s.schedule(today, next_day_guided_review);
        }
        skills.sort_by(|a, b| {
            b.due
                .cmp(&a.due)
                .then_with(|| {
                    if a.due {
                        a.latest_day.cmp(&b.latest_day)
                    } else {
                        b.latest_day.cmp(&a.latest_day)
                    }
                })
                .then_with(|| a.target.key().cmp(&b.target.key()))
        });
        Ok(skills)
    }
}
