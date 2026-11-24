//! Derived index for one learner's explicitly selected journal directory.
mod index;
mod search;
mod study;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    error::Error,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};
use tutor_lab::openrouter::{Config, EMBEDDING_DIM};
use tutor_lab::platform::PrivateOpen;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const SCHEMA: &str = include_str!("memory.sql");
const BATCH: i64 = 12;

#[derive(Clone, Serialize, Debug)]
pub struct Hit {
    pub source: String,
    #[serde(skip)]
    pub digest: String,
    pub day: Option<String>,
    pub learner: Option<String>,
    pub tutor: String,
}
pub struct Workspace {
    dir: PathBuf,
    db: Connection,
}

fn short(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c))
        && !name.starts_with('.')
}
fn journal_path(dir: &Path, requested: Option<&Path>, db: &Connection) -> Result<PathBuf> {
    let name = match requested {
        Some(p) => p
            .to_str()
            .and_then(|s| s.strip_suffix(".jsonl"))
            .filter(|s| session_name(s))
            .ok_or("In workspace mode use a journal filename such as 2026-03-05.jsonl.")?
            .to_owned(),
        None => db.query_row("SELECT date('now','localtime')", [], |r| {
            r.get::<_, String>(0)
        })?,
    };
    let path = dir.join(format!("{name}.jsonl"));
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if !meta.is_file() || OpenOptions::new().read(true).open_private(&path).is_err() {
            return Err("Select a regular workspace journal.".into());
        }
    }
    Ok(path)
}

/// Resolve the same journal when the derived index is unavailable. Validate the
/// private directory and filename before opening anything independently of it.
pub fn local_journal(dir: &Path, requested: Option<&Path>) -> Result<PathBuf> {
    if !tutor_lab::platform::private_directory(dir)? {
        return Err("Select a private learner directory (mode 0700).".into());
    }
    journal_path(
        &dir.canonicalize()?,
        requested,
        &Connection::open_in_memory()?,
    )
}
impl Workspace {
    pub fn open(dir: &Path) -> Result<Self> {
        tutor_lab::platform::create_private_dir(dir, true)?;
        let dir = dir.canonicalize()?;
        let path = dir.join("recall.sqlite3");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open_private(&path)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        OpenOptions::new().read(true).open_private(&path)?;
        let mut db = Connection::open(&path)?;
        db.busy_timeout(Duration::from_secs(8))?;
        db.pragma_update(None, "foreign_keys", true)?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        const APP_ID: i64 = 0x52545554;
        let empty = app == 0
            && version == 0
            && tx.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })? == 0;
        match (app,version){
            _ if empty => tx.execute_batch(SCHEMA)?,
            (APP_ID,1)=>{
                tx.execute_batch("ALTER TABLE turns ADD COLUMN recorded INTEGER")?;
                tx.execute_batch(SCHEMA)?;
            }
            (APP_ID,2|3)=>{
                // Replay the revised learner projection without disturbing stored passages or vectors.
                tx.execute_batch("DELETE FROM study_turns; DELETE FROM continuations; PRAGMA user_version=4;")?;
            }
            (APP_ID,4)=>{}
            _=>return Err("Unsupported recall index. Choose a fresh learner directory or rebuild the derived database from the original journals.".into()),
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS teaching_decisions(source TEXT PRIMARY KEY REFERENCES turns(source) ON DELETE CASCADE, findings TEXT NOT NULL, receipt TEXT NOT NULL)")?;
        // An empty row marks a visit whose existing prefix has already been checked.
        tx.execute_batch("CREATE TABLE IF NOT EXISTS plan_heads(visit TEXT PRIMARY KEY REFERENCES visits(name) ON DELETE CASCADE, source TEXT, digest TEXT, state TEXT, recorded INTEGER)")?;
        tx.commit()?;
        Ok(Self { dir, db })
    }
    pub fn supervisor_guidance(&self) -> Result<serde_json::Value> {
        let q = crate::supervisor::Queue::open(&self.dir)?;
        Ok(serde_json::json!({"last_review":q.latest()?.map(|(_,r)|r),"teacher_notes":q.notes()?}))
    }
    pub fn journal_path(&self, requested: Option<&Path>) -> Result<PathBuf> {
        journal_path(&self.dir, requested, &self.db)
    }

    pub fn external_findings(&self, visit: &str) -> Result<Vec<crate::judgment::Finding>> {
        let mut stmt=self.db.prepare("SELECT j.findings FROM teaching_decisions j JOIN turns t ON t.source=j.source WHERE t.visit<>? ORDER BY j.source")?;
        let mut all = Vec::new();
        for row in stmt.query_map([visit], |r| r.get::<_, String>(0))? {
            all.extend(serde_json::from_str::<Vec<crate::judgment::Finding>>(
                &row?,
            )?);
        }
        Ok(all)
    }

    pub fn trace_at(&mut self, source: &str, full: bool) -> Result<String> {
        let (name, ordinal) = source.rsplit_once('#').ok_or("Use VISIT#TURN.")?;
        let n = ordinal.parse::<usize>()?;
        let path = self.journal_path(Some(Path::new(&format!("{name}.jsonl"))))?;
        self.sync()?;
        let visit = crate::session::replay(&path)?;
        Ok(crate::judgment::trace(
            &visit,
            n,
            &self.external_findings(name)?,
            full,
        ))
    }
    pub fn method_cue_before(
        &self,
        anchor: &crate::session::SourceRef,
    ) -> Result<Option<(String, String)>> {
        Ok(self.db.query_row("SELECT t.source,t.tutor FROM turns t JOIN turns a ON a.visit=t.visit
            WHERE a.source=? AND a.digest=? AND t.ordinal<=a.ordinal AND
            (instr(lower(t.tutor),'irac') OR instr(lower(t.tutor),'creac') OR instr(lower(t.tutor),'crac'))
            ORDER BY t.ordinal DESC LIMIT 1", params![anchor.source, anchor.digest],
            |row| Ok((row.get(0)?,row.get(1)?))).optional()?)
    }
    pub fn earlier_cue(&self, visit: &str, method: &str) -> Result<Option<(String, String)>> {
        let mut stmt=self.db.prepare("SELECT source,tutor FROM turns WHERE visit<>? AND instr(lower(tutor),lower(?))>0 ORDER BY source")?;
        for pair in stmt.query_map(params![visit, method], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, text) = pair?;
            if crate::judgment::mentions(&text, method) {
                return Ok(Some((id, text)));
            }
        }
        Ok(None)
    }
    pub fn lookup(&self, source: &str) -> Result<Option<Hit>> {
        Ok(self
            .db
            .query_row(
                "SELECT source,day,learner,tutor,digest FROM turns WHERE source=?",
                [source],
                hit,
            )
            .optional()?)
    }
    pub fn review_sources(&self, visit: &str) -> Result<serde_json::Value> {
        let mut stmt=self.db.prepare("SELECT DISTINCT t.source,t.learner,t.tutor FROM links l JOIN turns o ON o.source=l.origin JOIN turns t ON t.source=l.target AND t.digest=l.digest WHERE o.visit=? AND t.visit!=o.visit ORDER BY t.recorded DESC, t.ordinal DESC LIMIT 24")?;
        let mut refs = serde_json::Map::new();
        let mut size = 0;
        for row in stmt.query_map([visit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (source, learner, tutor) = row?;
            let entry = serde_json::json!({"learner":learner,"tutor":tutor});
            size += entry.to_string().len();
            if size > 24000 {
                break;
            }
            refs.insert(source, entry);
        }
        Ok(refs.into())
    }
    pub fn links(&self, source: &str) -> Result<Vec<String>> {
        let mut stmt=self.db.prepare("SELECT target FROM links JOIN turns ON turns.source=links.target AND turns.digest=links.digest WHERE origin=? ORDER BY target")?;
        let items = stmt
            .query_map([source], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }

    // Commit each completed batch. Unavailable embeddings stay queued for the next turn.
    fn fill_vectors(
        &self,
        embed: &mut impl FnMut(&[String]) -> std::result::Result<Vec<Vec<f32>>, String>,
        batches: usize,
    ) -> Result<()> {
        for _ in 0..batches {
            let mut stmt = self.db.prepare(
                "SELECT body FROM passages WHERE vector IS NULL GROUP BY body ORDER BY min(rowid) LIMIT ?",
            )?;
            let inputs = stmt
                .query_map([BATCH], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if inputs.is_empty() {
                break;
            }
            let vectors = embed(&inputs)?;
            if vectors.len() != inputs.len() {
                return Err("Incomplete embedding batch.".into());
            }
            let tx = self.db.unchecked_transaction()?;
            for (input, v) in inputs.iter().zip(vectors) {
                if v.len() != EMBEDDING_DIM {
                    return Err("Embedding dimension changed.".into());
                }
                let bytes: Vec<u8> = v.iter().flat_map(|n| n.to_le_bytes()).collect();
                tx.execute(
                    "UPDATE passages SET vector=? WHERE body=? AND vector IS NULL",
                    params![bytes, input],
                )?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    pub fn recall(
        &mut self,
        connection: &Config,
        query: &str,
        session: &str,
        recent_from: usize,
    ) -> Result<Vec<Hit>> {
        self.sync()?;
        let any: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM turns WHERE visit!=? OR ordinal<?)",
            params![session, recent_from as i64],
            |r| r.get(0),
        )?;
        if !any || query.trim().is_empty() {
            return Ok(vec![]);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        let mut embed = |inputs: &[String]| connection.embed_by(inputs, Some(deadline));
        let result = self
            .fill_vectors(&mut embed, 1)
            .and_then(|_| Ok(embed(&[short(query, 1600)])?.pop()));
        let vector = match result {
            Ok(v) => v,
            Err(error) => {
                eprintln!("Semantic recall unavailable ({error}). Searching dated text matches.");
                None
            }
        };
        self.search(query, vector.as_deref(), session, recent_from)
    }
    pub fn build_vectors(&mut self, connection: &Config) -> Result<i64> {
        self.sync()?;
        self.fill_vectors(&mut |inputs| connection.embed(inputs), usize::MAX)?;
        Ok(self
            .db
            .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))?)
    }
}

fn hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    Ok(Hit {
        source: row.get(0)?,
        day: row.get(1)?,
        learner: row.get(2)?,
        tutor: row.get(3)?,
        digest: row.get(4)?,
    })
}
#[cfg(test)]
mod tests;
