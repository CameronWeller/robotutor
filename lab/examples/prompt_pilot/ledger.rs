use super::{Result, Settings, MODEL};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
use tutor_lab::openrouter::assistant_text;

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub struct Journal {
    db: Connection,
    _lock: File,
    settings: Settings,
    replay: bool,
    pub calls: u64,
    pub tokens: u64,
    pub micros: u64,
}

impl Journal {
    #[cfg(test)]
    pub(super) fn settings_for_test_limit(&mut self, calls: u64) {
        self.settings.call_limit = calls;
    }
    pub fn open(dir: &Path, manifest: &Value, settings: Settings, replay: bool) -> Result<Self> {
        if !replay {
            std::fs::create_dir_all(dir)?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(!replay)
            .truncate(false)
            .open(dir.join("run.lock"))?;
        lock.try_lock()
            .map_err(|_| "Pilot is already running in this directory")?;
        let path = dir.join("receipts.sqlite");
        if replay && !path.is_file() {
            return Err("Missing pilot receipts".into());
        }
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS manifest(id INTEGER PRIMARY KEY CHECK(id=1), body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS calls(id TEXT PRIMARY KEY, request TEXT NOT NULL, receipt TEXT, digest TEXT);")?;
        let frozen = manifest.to_string();
        if !replay {
            db.execute("INSERT OR IGNORE INTO manifest VALUES(1,?1)", [&frozen])?;
        }
        let saved: String =
            db.query_row("SELECT body FROM manifest WHERE id=1", [], |r| r.get(0))?;
        if frozen != saved {
            return Err("Pilot pins differ from the frozen manifest".into());
        }
        let mut journal = Self {
            db,
            _lock: lock,
            settings,
            replay,
            calls: 0,
            tokens: 0,
            micros: 0,
        };
        let rows: Vec<(String, Option<String>, Option<String>)> = {
            let mut q = journal
                .db
                .prepare("SELECT request,receipt,digest FROM calls ORDER BY rowid")?;
            let values = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            values
        };
        for (request, receipt, digest) in rows {
            let receipt = receipt
                .ok_or("Unresolved provider dispatch. Reconcile its receipt before resuming.")?;
            if digest.as_deref() != Some(&record_hash(&request, &receipt)) {
                return Err("Receipt integrity check failed".into());
            }
            journal.account(&serde_json::from_str(&receipt)?)?;
        }
        Ok(journal)
    }

    fn account(&mut self, raw: &Value) -> Result<()> {
        if raw["model"] != MODEL || raw["provider"] != "OpenAI" {
            return Err("Invalid comparison: model or provider mismatch".into());
        }
        let tokens = raw["usage"]["total_tokens"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("Missing token usage")?;
        let cost = raw["usage"]["cost"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or("Missing charge")?;
        self.calls += 1;
        self.tokens = self
            .tokens
            .checked_add(tokens)
            .ok_or("Token usage overflow")?;
        self.micros = self
            .micros
            .checked_add((cost * 1_000_000.0).ceil() as u64)
            .ok_or("Charge overflow")?;
        if self.tokens > self.settings.token_limit
            || self.micros > self.settings.budget_microusd
            || self.calls > self.settings.call_limit
        {
            return Err("Pilot limit exceeded by provider receipt".into());
        }
        Ok(())
    }

    pub fn call(
        &mut self,
        id: &str,
        request: Value,
        send: &mut impl FnMut(&Value) -> Result<Value>,
    ) -> Result<Value> {
        let body = request.to_string();
        let cached: Option<(String, String)> = self
            .db
            .query_row("SELECT request,receipt FROM calls WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let raw = if let Some((original, receipt)) = cached {
            if original != body {
                return Err(
                    format!("Invalid comparison: effective request changed at {id}").into(),
                );
            }
            serde_json::from_str(&receipt)?
        } else {
            if self.replay {
                return Err(format!("Missing receipt at {id}").into());
            }
            // Reserve a byte per input token plus framing, and the entire completion allowance.
            // Price filters bound prompt and completion to $1/M and $3/M respectively.
            let input = body.len() as u64 + 512;
            let output = request["max_tokens"].as_u64().ok_or("Missing output cap")?;
            if self.calls >= self.settings.call_limit
                || input + output > self.settings.token_limit - self.tokens
                || input + 3 * output > self.settings.budget_microusd - self.micros
            {
                return Err(format!("Hard pilot limit reached before {id}").into());
            }
            self.db.execute(
                "INSERT INTO calls(id,request) VALUES(?1,?2)",
                params![id, body],
            )?;
            let raw = send(&request)?;
            let receipt = raw.to_string();
            self.db.execute(
                "UPDATE calls SET receipt=?1,digest=?2 WHERE id=?3",
                params![receipt, record_hash(&body, &receipt), id],
            )?;
            self.account(&raw)?;
            raw
        };
        Ok(serde_json::from_str(assistant_text(&raw)?)?)
    }

    pub fn usage(&self) -> Value {
        json!({"calls":self.calls,"tokens":self.tokens,"charged_usd_upper_rounded":self.micros as f64/1_000_000.0})
    }

    pub fn arm_usage(&self, case: &str, simulation: &str, arm: &str) -> Result<Value> {
        let prefix = format!("{case}/{simulation}/{arm}/");
        Ok(self.db.query_row(
            "SELECT count(*), coalesce(sum(json_extract(receipt,'$.usage.total_tokens')),0),
                    coalesce(sum(json_extract(receipt,'$.usage.cost')),0.0)
             FROM calls WHERE substr(id,1,length(?1))=?1",
            [prefix],
            |row| {
                Ok(json!({"calls":row.get::<_,i64>(0)?,
                "tokens":row.get::<_,i64>(1)?,"reported_cost_usd":row.get::<_,f64>(2)?}))
            },
        )?)
    }
}

fn record_hash(request: &str, receipt: &str) -> String {
    hash(&serde_json::to_vec(&(request, receipt)).expect("string pair"))
}
