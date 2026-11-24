//! Private, retrospective supervision. A workspace revision orders learner activity,
//! teacher interventions and worker results across terminal processes.
use super::{session, Visit};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    error::Error,
    fs::OpenOptions,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tutor_lab::openrouter::{assistant_text, credentials};
use tutor_lab::platform::{BackgroundCommand, PrivateOpen};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub const VERSION: &str = "session-review/1";

pub struct Queue(pub(super) Connection);
#[derive(Deserialize, Serialize, Debug)]
pub struct Review {
    pub destination: String,
    pub accepted: String,
    pub uncertainty: String,
    pub next_task: String,
    pub sufficient: String,
    pub rationale: String,
    pub teacher_status: String,
    pub evidence: Vec<Evidence>,
    pub repeated_probes: Vec<usize>,
    pub missed_demonstrations: Vec<usize>,
}
#[derive(Deserialize, Serialize, Debug)]
pub struct Evidence {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    sufficient_for_skill: bool,
    turn: usize,
    quote: String,
    skill: String,
    facet: Facet,
    inferred: bool,
    rationale: String,
}
#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
enum Facet {
    Recognize,
    Choose,
    Execute,
}
impl Review {
    fn validate(&self, v: &Visit, linked: &Value) -> Result<()> {
        for s in [
            &self.destination,
            &self.accepted,
            &self.uncertainty,
            &self.next_task,
            &self.sufficient,
            &self.rationale,
            &self.teacher_status,
        ] {
            if s.trim().is_empty() || s.len() > 4000 {
                return Err("Invalid supervisor plan field".into());
            }
        }
        for e in &self.evidence {
            if e.skill.trim().is_empty()
                || e.rationale.trim().is_empty()
                || e.quote.trim().is_empty()
                || !match &e.source {
                    Some(id) => linked
                        .get(id)
                        .and_then(|h| h["learner"].as_str())
                        .filter(|_| {
                            id.rsplit_once('#')
                                .and_then(|(_, n)| n.parse::<usize>().ok())
                                == Some(e.turn)
                        }),
                    None => e
                        .turn
                        .checked_sub(1)
                        .and_then(|i| v.history.get(i))
                        .and_then(|p| p.0.as_deref()),
                }
                .is_some_and(|words| words.contains(&e.quote))
            {
                return Err("Unresolved learner evidence".into());
            }
        }
        if self
            .repeated_probes
            .iter()
            .chain(&self.missed_demonstrations)
            .any(|&n| n == 0 || n > v.history.len())
        {
            return Err("Invalid diagnostic turn".into());
        }
        Ok(())
    }
    pub fn plan(&self) -> String {
        format!("Destination · {}\nAccepted reasoning · {}\nStill uncertain · {}\nNext task · {}\nSufficient when · {}",self.destination,self.accepted,self.uncertainty,self.next_task,self.sufficient)
    }
}
impl Queue {
    pub fn open(dir: &Path) -> Result<Self> {
        if !tutor_lab::platform::private_directory(dir)? {
            return Err("Select a private workspace".into());
        }
        let path = dir.join("supervision.sqlite3");
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open_private(&path)?;
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(8))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS clock(id INTEGER PRIMARY KEY CHECK(id=1),rev INTEGER NOT NULL,visit TEXT,due INTEGER);
        INSERT OR IGNORE INTO clock VALUES(1,0,NULL,NULL);
        CREATE TABLE IF NOT EXISTS plan_boundary(id INTEGER PRIMARY KEY CHECK(id=1),first_eligible INTEGER NOT NULL);
        INSERT OR IGNORE INTO plan_boundary SELECT 1,rev FROM clock;
        CREATE TABLE IF NOT EXISTS reviews(rev INTEGER PRIMARY KEY,visit TEXT,at INTEGER,model TEXT,version TEXT,previous_json TEXT,report TEXT,turns INTEGER);
        CREATE TABLE IF NOT EXISTS openings(id INTEGER PRIMARY KEY,at INTEGER,visit TEXT);
        CREATE TABLE IF NOT EXISTS failures(rev INTEGER PRIMARY KEY, attempts INTEGER, message TEXT);
        CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY,at INTEGER,kind TEXT,body TEXT);")?;
        db.execute_batch(crate::preparation::SQL)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS away_work(rev INTEGER PRIMARY KEY,visit TEXT,mode TEXT,state TEXT,block TEXT);
            CREATE TABLE IF NOT EXISTS away_usage(id INTEGER PRIMARY KEY,rev INTEGER,at INTEGER DEFAULT(unixepoch()),tokens INTEGER,cost REAL);
            CREATE TABLE IF NOT EXISTS offline_windows(visit TEXT PRIMARY KEY,minutes INTEGER);")?;
        Ok(Self(db))
    }
    fn return_revision(&self, v: &Visit) -> Result<String> {
        use sha2::{Digest, Sha256};
        let note: i64 = self
            .0
            .query_row("SELECT coalesce(max(id),0) FROM notes", [], |r| r.get(0))?;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                v.history.len(),
                v.history.last(),
                v.supervisor_revision,
                note,
                &v.lesson
            ))?)
        ))
    }
    pub fn return_block(&self, name: &str, v: &Visit) -> Result<Vec<crate::practice::Released>> {
        let row = self
            .0
            .query_row(
                "SELECT state,block FROM away_work WHERE visit=? ORDER BY rev DESC LIMIT 1",
                [name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((state, block)) = row {
            let state: Value = serde_json::from_str(&state)?;
            if state["revision"] == self.return_revision(v)? {
                return Ok(serde_json::from_str(&block)?);
            }
        }
        Ok(vec![])
    }
    pub fn has_new_direction(&self, v: &Visit) -> Result<bool> {
        Ok(self.0.query_row("SELECT EXISTS(SELECT 1 FROM notes WHERE kind IN ('/correct','/assign','/redirect') AND at>=?)",[i64::try_from(v.last_delivery_at)?],|r|r.get(0))?)
    }
    pub fn offline(&self, visit: &str, minutes: u64) -> Result<()> {
        self.0.execute(
            "INSERT OR REPLACE INTO offline_windows VALUES(?,?)",
            params![visit, i64::try_from(minutes)?],
        )?;
        Ok(())
    }
    pub fn opened_visit(&self, session: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO openings(at,visit) VALUES(unixepoch(),?)",
            [session],
        )?;
        Ok(())
    }
    pub fn returned_since(&self, at: i64) -> Result<bool> {
        Ok(self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM openings WHERE at>?)",
            [at],
            |r| r.get(0),
        )?)
    }
    pub fn activity(&self, visit: &str) -> Result<i64> {
        Ok(self.0.query_row(
            "UPDATE clock SET rev=rev+1,visit=?,due=NULL WHERE id=1 RETURNING rev",
            [visit],
            |r| r.get(0),
        )?)
    }
    /// Record a newer learner contribution or teacher decision before accepting it.
    pub fn supersede(&self, revision: i64) -> Result<()> {
        self.0.execute(
            "UPDATE plan_boundary SET first_eligible=max(first_eligible,?)",
            [revision],
        )?;
        Ok(())
    }

    /// Serialize application with other terminal activity. The recovered journal revision
    /// makes retry after a journal append and before the database commit idempotent.
    pub fn apply_ready(
        &self,
        revision: i64,
        session: &str,
        turns: usize,
        continued: Option<&str>,
        already: i64,
        apply: impl FnOnce(i64, Review) -> Result<()>,
    ) -> Result<bool> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.0,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let row = tx
            .query_row(
                "SELECT r.rev,r.report FROM reviews r,clock c,plan_boundary b
          WHERE c.rev=?1 AND r.rev>=b.first_eligible AND r.rev>?2
          AND ((r.visit=?3 AND r.turns=?4) OR (?4=0 AND r.visit||'#'||r.turns=?5))
          ORDER BY r.rev DESC LIMIT 1",
                params![revision, already, session, i64::try_from(turns)?, continued],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((n, json)) = row else {
            return Ok(false);
        };
        apply(n, serde_json::from_str(&json)?)?;
        tx.execute("UPDATE plan_boundary SET first_eligible=?", [n + 1])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn schedule(&self, rev: i64, due: u64) -> Result<()> {
        self.0.execute(
            "UPDATE clock SET due=? WHERE rev=? AND NOT EXISTS (SELECT 1 FROM reviews WHERE reviews.rev=clock.rev) AND NOT EXISTS (SELECT 1 FROM failures WHERE failures.rev=clock.rev AND attempts>=3)",
            params![i64::try_from(due)?, rev],
        )?;
        Ok(())
    }
    fn failed(&mut self, rev: i64, message: &str, now: u64) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM clock WHERE rev=? AND due IS NOT NULL)",
            [rev],
            |r| r.get(0),
        )?;
        if !current {
            return Ok(());
        }
        tx.execute("INSERT INTO failures VALUES(?,1,?) ON CONFLICT(rev) DO UPDATE SET attempts=attempts+1,message=excluded.message",params![rev,message])?;
        tx.execute("UPDATE clock SET due=CASE WHEN (SELECT attempts FROM failures WHERE rev=?1)<3 THEN ?2 END WHERE rev=?1",params![rev,i64::try_from(now+600)?])?;
        tx.commit()?;
        Ok(())
    }
    pub fn last_failure(&self) -> Result<Option<String>> {
        Ok(self.0.query_row("SELECT message FROM failures WHERE rev>(SELECT coalesce(max(rev),0) FROM reviews) ORDER BY rev DESC LIMIT 1",[],|r|r.get(0)).optional()?)
    }
    fn job(&self) -> Result<Option<(i64, String, u64)>> {
        Ok(self
            .0
            .query_row(
                "SELECT rev,visit,due FROM clock WHERE due IS NOT NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64)),
            )
            .optional()?)
    }
    fn finish(
        &mut self,
        rev: i64,
        model: &str,
        previous: &Value,
        review: &Review,
        turns: usize,
    ) -> Result<bool> {
        let tx = self.0.transaction()?;
        if tx.execute(
            "UPDATE clock SET due=NULL WHERE rev=? AND due IS NOT NULL",
            [rev],
        )? == 0
        {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO reviews SELECT rev,visit,unixepoch(),?,?,?,?,? FROM clock",
            params![
                model,
                VERSION,
                previous.to_string(),
                serde_json::to_string(review)?,
                i64::try_from(turns)?
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn latest(&self) -> Result<Option<(i64, Review)>> {
        let row = self
            .0
            .query_row(
                "SELECT rev,report FROM reviews ORDER BY rev DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        row.map(|(n, s)| Ok((n, serde_json::from_str(&s)?)))
            .transpose()
    }
    pub fn note(&self, kind: &str, body: &str, revision: i64) -> Result<i64> {
        if body.trim().is_empty() || body.len() > 4000 {
            return Err("Supply a short observation or direction".into());
        }
        let f: Vec<_> = body.split('|').map(str::trim).collect();
        match kind {
            "/experiment" if f.len() != 2 || f.iter().any(|s| s.is_empty()) => {
                return Err("Use /experiment VERSION | hypothesis".into())
            }
            "/measure" => {
                if f.len() != 4
                    || f[3].is_empty()
                    || !matches!(
                        (f[1], f[2]),
                        ("return", "observed")
                            | ("enjoyment", "positive" | "neutral" | "negative")
                            | ("delayed-unaided", "success" | "failure")
                    )
                {
                    return Err(
                        "Use /measure ID | enjoyment/return/delayed-unaided | result | evidence"
                            .into(),
                    );
                }
                let at: i64 = self.0.query_row(
                    "SELECT at FROM notes WHERE id=? AND kind='/experiment'",
                    [f[0].parse::<i64>()?],
                    |r| r.get(0),
                )?;
                if f[1] == "delayed-unaided" && super::session_time()? < at as u64 + 86400 {
                    return Err("Delayed evidence requires a later day (24 hours)".into());
                }
            }
            "/review-result" => {
                if f.len() != 3 || !matches!(f[1], "supported" | "contradicted") || f[2].is_empty()
                {
                    return Err("Use /review-result REV | supported/contradicted | subsequent independent evidence".into());
                }
                self.0.query_row(
                    "SELECT rev FROM reviews WHERE rev=?",
                    [f[0].parse::<i64>()?],
                    |r| r.get::<_, i64>(0),
                )?;
            }
            _ => {}
        }
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO notes(at,kind,body) VALUES(unixepoch(),?,?)",
            params![kind, body],
        )?;
        let id = tx.last_insert_rowid();
        if matches!(kind, "/correct" | "/assign" | "/redirect") {
            self.supersede(revision)?;
        }
        tx.commit()?;
        Ok(id)
    }
    pub fn all_notes(&self) -> Result<Vec<Value>> {
        let mut q = self
            .0
            .prepare("SELECT id,kind,body,at FROM notes ORDER BY id")?;
        let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"kind":r.get::<_,String>(1)?,"body":r.get::<_,String>(2)?,"at":r.get::<_,i64>(3)?})))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn notes(&self) -> Result<Vec<Value>> {
        let mut q=self.0.prepare("SELECT kind,body,at FROM notes WHERE kind IN ('/correct','/redirect','/assign') ORDER BY id DESC LIMIT 32")?;
        let rows=q.query_map([],|r|Ok(json!({"source":"workspace owner","kind":r.get::<_,String>(0)?,"body":r.get::<_,String>(1)?,"at":r.get::<_,i64>(2)?})))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn history(&self) -> Result<Vec<Value>> {
        let mut q=self.0.prepare("SELECT visit,rev,at,model,version,previous_json,report,turns FROM reviews ORDER BY rev")?;
        let mut out = vec![];
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            out.push(json!({"visit":r.get::<_,String>(0)?,"rev":r.get::<_,i64>(1)?,"at":r.get::<_,i64>(2)?,"model":r.get::<_,String>(3)?,"version":r.get::<_,String>(4)?,"previous":serde_json::from_str::<Value>(&r.get::<_,String>(5)?)?,"review":serde_json::from_str::<Value>(&r.get::<_,String>(6)?)?,"turns":r.get::<_,i64>(7)?}));
        }
        Ok(out)
    }
}
pub fn assess(v: &Visit, previous: &Value, notes: &[Value]) -> Result<(String, Review)> {
    assess_metered(v, previous, notes, |_| Ok(()))
}
fn assess_metered(
    v: &Visit,
    previous: &Value,
    notes: &[Value],
    usage: impl FnOnce(&Value) -> Result<()>,
) -> Result<(String, Review)> {
    let turns: Vec<_> = v
        .history
        .iter()
        .enumerate()
        .skip(v.history.len().saturating_sub(12))
        .map(|(i, (u, t))| json!({"turn":i+1,"learner":u,"tutor":t}))
        .collect();
    let input = json!({"turns":turns,"previous":previous,"teacher_observations":notes,"course":v.lesson.course});
    if input.to_string().len() > 24000 {
        return Err("Visit exceeds the bounded supervisor input.".into());
    }
    let request = json!({"messages":[{"role":"system","content":include_str!("supervise.txt")},{"role":"user","content":input.to_string()}],"max_tokens":2400,"response_format":{"type":"json_object"}});
    let (model, result) = if let Some(worker) = std::env::var_os("ROBOTUTOR_ASTRA_WORKER") {
        use std::io::Write;
        let mut process = Command::new(tutor_lab::platform::python())
            .arg(worker)
            .arg("retrospective")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        process
            .stdin
            .take()
            .ok_or("missing supervisor input")?
            .write_all(&serde_json::to_vec(&request)?)?;
        let output = process.wait_with_output()?;
        if !output.status.success() {
            return Err("Codex subscription review unavailable".into());
        }
        let content = String::from_utf8(output.stdout)?;
        (
            "gpt-6.2-astra".into(),
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":content}}]}),
        )
    } else {
        credentials::connect()?.supervise(&request)?
    };
    usage(&result["usage"])?;
    let report: Review = serde_json::from_str(assistant_text(&result)?)?;
    report.validate(v, &previous["linked_turns"])?;
    Ok((model, report))
}
pub fn idle_window(task: &str) -> u64 {
    let task = task.to_lowercase();
    if [
        "on paper",
        "offline",
        "write an essay",
        "work through the full",
        "draft a report",
    ]
    .iter()
    .any(|phrase| task.contains(phrase))
    {
        7200
    } else {
        1800
    }
}
/// Enable subscription review for this tutor and its private background workers.
pub fn enable_comparison(dir: &Path, session: &str) -> Result<()> {
    let sibling = std::env::current_exe()?.with_file_name("astra.py");
    let worker = std::env::var_os("ROBOTUTOR_ASTRA_WORKER")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            if sibling.is_file() {
                sibling
            } else {
                tutor_lab::platform::project_root().join("desktop/astra.py")
            }
        })
        .canonicalize()?;
    let reviews = dir.canonicalize()?.join(".astra");
    std::env::set_var("ROBOTUTOR_ASTRA_WORKER", &worker);
    std::env::set_var("ROBOTUTOR_ASTRA_DIR", &reviews);
    std::env::set_var("ROBOTUTOR_ASTRA_SESSION", session);
    if reviews.join("reviews.sqlite3").is_file() {
        tutor_lab::openrouter::wake_astra(&worker, &reviews)?;
    }
    Ok(())
}

pub fn spawn(dir: &Path) -> Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--supervisor-worker")
        .arg(dir)
        .background()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
pub fn worker(dir: &Path) -> Result<()> {
    let mut queue = Queue::open(dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open_private(dir.join("supervisor.lock"))?;
    if let Err(e) = lock.try_lock() {
        return match e {
            std::fs::TryLockError::WouldBlock => Ok(()),
            _ => Err(e.into()),
        };
    }
    loop {
        if !dir.is_dir() {
            return Ok(());
        }
        let Some((rev, name, due)) = queue.job()? else {
            lock.unlock()?;
            if queue.job()?.is_some() {
                spawn(dir)?;
            }
            return Ok(());
        };
        let now = super::session_time()?;
        if due > now {
            std::thread::sleep(Duration::from_secs((due - now).min(5)));
            continue;
        }
        run_due(&mut queue, dir, rev, &name, now)?;
    }
}

/// Account for preparation failures under the same revision fence and retry
/// budget as provider failures. A later visit owns its own job.
fn run_due(queue: &mut Queue, dir: &Path, rev: i64, name: &str, now: u64) -> Result<()> {
    let result = (|| -> Result<_> {
        let mut workspace = super::memory::Workspace::open(dir)?;
        workspace.sync()?;
        let path = workspace.journal_path(Some(Path::new(&format!("{name}.jsonl"))))?;
        let v = session::replay(&path)?;
        let compact = crate::preparation::consolidated(&v);
        let latest_correction = queue.has_new_direction(&v)?;
        let uncertain = v.pending.is_some()
            || crate::discovery::active(&v)
            || v.lesson.declared_goal.as_deref().is_none_or(str::is_empty)
            || v.lesson.practice_correction.is_some()
            || latest_correction;
        let long_window: bool = queue.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM offline_windows WHERE visit=? AND minutes>=30)",
            [name],
            |r| r.get(0),
        )?;
        let established =
            v.lesson.course.is_some() || path.with_extension("assignment.json").is_file();
        let limit = if long_window && established { 3 } else { 1 };
        let block = if uncertain {
            vec![]
        } else {
            crate::practice::Prepared::return_block(&v, limit)?
        };
        let mode = if uncertain {
            "consolidate"
        } else if block.len() > 1 {
            "study_block"
        } else {
            "return"
        };
        let stored=queue.0.execute("INSERT OR REPLACE INTO away_work SELECT rev,visit,?,?,? FROM clock WHERE rev=? AND due IS NOT NULL",params![mode,json!({"revision":queue.return_revision(&v)?,"evidence":compact}).to_string(),serde_json::to_string(&block)?,rev])?;
        if stored == 0 {
            return Ok(None);
        }
        let count: i64 = queue.0.query_row(
            "SELECT count(*) FROM away_usage WHERE at>unixepoch()-86400",
            [],
            |r| r.get(0),
        )?;
        if uncertain || !v.draft.empty() || !block.is_empty() || count >= 3 {
            queue
                .0
                .execute("UPDATE clock SET due=NULL WHERE rev=?", [rev])?;
            return Ok(None);
        }
        let previous = json!({"objective":v.lesson.objective,"active_step":v.lesson.active_step(),"assessments":v.assessments,"linked_turns":workspace.review_sources(name)?,"consolidated":compact,"last_review":queue.latest()?.map(|(_,r)|r)});
        queue
            .0
            .execute("INSERT INTO away_usage(rev) VALUES(?)", [rev])?;
        let call_id = queue.0.last_insert_rowid();
        let (model, review) = assess_metered(&v, &previous, &queue.notes()?, |usage| {
            queue.0.execute(
                "UPDATE away_usage SET tokens=?,cost=? WHERE id=?",
                params![
                    usage["total_tokens"].as_i64(),
                    usage["cost"].as_f64(),
                    call_id
                ],
            )?;
            Ok(())
        })?;
        Ok(Some((model, review, previous, v.history.len())))
    })();
    match result {
        Ok(Some((model, review, previous, turns))) => {
            queue.finish(rev, &model, &previous, &review, turns)?;
        }
        Ok(None) => {}
        Err(error) => {
            queue.failed(rev, &error.to_string(), now)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn planned_offline_block_is_private_and_a_correction_invalidates_its_revision() {
        let dir =
            std::env::temp_dir().join(format!("robotutor-return-block-{}", std::process::id()));
        tutor_lab::platform::create_private_dir(&dir, false).unwrap();
        let path = dir.join("lesson.jsonl");
        fs::write(
            path.with_extension("assignment.json"),
            r#"{"id":7,"version":1,"activity":{"objective":"Choose experimental controls"}}"#,
        )
        .unwrap();
        let mut visit = Visit::default();
        let mut journal = Some(session::Journal::open(&path, &mut visit).unwrap());
        crate::append(
            &mut journal,
            &mut visit,
            session::Event::Open {
                version: 1,
                selected: None,
            },
        )
        .unwrap();
        crate::append(
            &mut journal,
            &mut visit,
            session::Event::Learner {
                text: "/activity order".into(),
            },
        )
        .unwrap();
        crate::practice::respond(
            &mut visit,
            &mut journal,
            "lesson",
            &Default::default(),
            &mut None,
        )
        .unwrap();
        let original_len = visit.history.len();
        let mut q = Queue::open(&dir).unwrap();
        let rev = q.activity("lesson").unwrap();
        q.offline("lesson", 60).unwrap();
        q.schedule(rev, 100).unwrap();
        run_due(&mut q, &dir, rev, "lesson", 100).unwrap();
        assert_eq!(q.return_block("lesson", &visit).unwrap().len(), 3);
        assert_eq!(session::replay(&path).unwrap().history.len(), original_len);
        assert!(q.latest().unwrap().is_none());
        q.note("/correct", "Revisit the controlled variable first.", rev)
            .unwrap();
        assert!(q.return_block("lesson", &visit).unwrap().is_empty());
        drop(journal);
        drop(q);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn preparation_failure_retries_after_restart_and_a_cancelled_job_stays_cancelled() {
        let dir =
            std::env::temp_dir().join(format!("robotutor-preparation-{}", std::process::id()));
        tutor_lab::platform::create_private_dir(&dir, false).unwrap();
        let mut q = Queue::open(&dir).unwrap();
        let rev = q.activity("missing").unwrap();
        q.schedule(rev, 100).unwrap();
        for now in [100, 700, 1300] {
            run_due(&mut q, &dir, rev, "missing", now).unwrap();
            drop(q);
            q = Queue::open(&dir).unwrap();
            assert_eq!(
                q.job().unwrap().map(|j| j.2),
                (now < 1300).then_some(now + 600)
            );
        }
        q.schedule(rev, 2400).unwrap();
        assert!(q.job().unwrap().is_none());
        let old = q.activity("cancelled").unwrap();
        q.schedule(old, 5000).unwrap();
        let current = q.activity("current").unwrap();
        q.schedule(current, 6000).unwrap();
        q.failed(old, "stale worker", 5500).unwrap();
        assert_eq!(q.job().unwrap(), Some((current, "current".into(), 6000)));
        assert_ne!(q.last_failure().unwrap().as_deref(), Some("stale worker"));
        drop(q);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn restart_return_and_late_completion() {
        let dir = std::env::temp_dir().join(format!("robotutor-queue-{}", std::process::id()));
        tutor_lab::platform::create_private_dir(&dir, false).unwrap();
        let q = Queue::open(&dir).unwrap();
        let old = q.activity("first").unwrap();
        q.schedule(old, 600).unwrap();
        drop(q);
        let mut q = Queue::open(&dir).unwrap();
        assert_eq!(q.job().unwrap(), Some((old, "first".into(), 600)));
        let current = q.activity("second").unwrap();
        assert!(q.job().unwrap().is_none());
        let r:Review=serde_json::from_value(json!({"destination":"growth","accepted":"recognition","uncertainty":"selection","next_task":"Choose a method","sufficient":"justify the choice","rationale":"new decision","teacher_status":"progress","evidence":[],"repeated_probes":[],"missed_demonstrations":[]})).unwrap();
        q.schedule(current, 1000).unwrap();
        assert!(!q.finish(old, "strong", &Value::Null, &r, 4).unwrap());
        assert!(q.latest().unwrap().is_none());
        assert!(q
            .finish(current, "strong", &json!({"objective":"earlier"}), &r, 5)
            .unwrap());
        assert!(!q.finish(current, "strong", &Value::Null, &r, 5).unwrap());
        q.schedule(current, 2000).unwrap();
        assert!(q.job().unwrap().is_none());
        assert_eq!(q.history().unwrap().len(), 1);
        assert_eq!(q.latest().unwrap().unwrap().1.next_task, "Choose a method");
        let inspected = q.activity("second").unwrap();
        // Reading /plan and /teacher advances the activity clock, but contributes no evidence.
        let other = q.activity("other").unwrap();
        assert!(!q
            .apply_ready(other, "other", 5, None, 0, |_, _| panic!("crossed visits"))
            .unwrap());
        assert!(!q
            .apply_ready(inspected, "second", 5, None, 0, |_, _| panic!(
                "stale terminal"
            ))
            .unwrap());
        let continued = q.activity("third").unwrap();
        assert!(!q
            .apply_ready(continued, "third", 0, Some("second#4"), 0, |_, _| panic!(
                "stale parent"
            ))
            .unwrap());
        assert!(q
            .apply_ready(continued, "third", 0, Some("second#5"), 0, |_, r| {
                assert_eq!(r.next_task, "Choose a method");
                Ok(())
            })
            .unwrap());
        assert!(!q
            .apply_ready(continued, "third", 0, Some("second#5"), 0, |_, _| panic!(
                "applied twice"
            ))
            .unwrap());
        q.schedule(continued, 2000).unwrap();
        q.finish(continued, "strong", &Value::Null, &r, 7).unwrap();
        let changed = q.activity("third").unwrap();
        q.supersede(changed).unwrap();
        assert!(!q
            .apply_ready(changed, "third", 7, None, 0, |_, _| panic!(
                "outdated assessment"
            ))
            .unwrap());
        drop(q);
        fs::remove_dir_all(dir).unwrap();
    }
}
