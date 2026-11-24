//! Capture the exact successful Luna exchange in the selected private workspace.
use crate::platform::{BackgroundCommand, PrivateOpen};
use rusqlite::{params, Connection};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

pub fn record(body: &str, response: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let (Some(folder), Some(worker)) = (
        env::var_os("ROBOTUTOR_ASTRA_DIR"),
        env::var_os("ROBOTUTOR_ASTRA_WORKER"),
    ) else {
        return Ok(());
    };
    let dir = Path::new(&folder);
    crate::platform::create_private_dir(dir, true)?;
    let path = dir.join("reviews.sqlite3");
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open_private(&path)?;
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    db.execute_batch(include_str!("trailing.sql"))?;
    let request: Value = serde_json::from_str(body)?;
    let task = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|last| last["content"].as_str())
        .and_then(|content| serde_json::from_str::<Value>(content).ok());
    let stage = task
        .as_ref()
        .and_then(|turn| turn["task"].as_str())
        .or_else(|| {
            request
                .pointer("/response_format/json_schema/name")
                .and_then(Value::as_str)
        })
        .unwrap_or("completion");
    let session = env::var("ROBOTUTOR_ASTRA_SESSION").unwrap_or_default();
    let text = super::assistant_text(response)?;
    let digest = format!("{:x}", Sha256::digest(body.as_bytes()));
    let receipt = response["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}:{:x}", digest, Sha256::digest(text.as_bytes())));
    db.execute("INSERT OR IGNORE INTO reviews(receipt,session,stage,digest,packet,luna,model) VALUES(?,?,?,?,?,?,?)",
        params![receipt,session,stage,digest,body,text,response["model"].as_str().or(request["model"].as_str())])?;
    wake(Path::new(&worker), dir)?;
    Ok(())
}

/// Obtain an independent service before the foreground tutor can quit.
/// Only local process registration happens here. Codex runs in the worker.
pub fn wake(worker: &Path, dir: &Path) -> std::io::Result<()> {
    let python = crate::platform::python();
    if env::var_os("INVOCATION_ID").is_some() {
        let mut run = Command::new("systemd-run");
        run.args(["--user", "--quiet", "--collect", "--property=UMask=0077"]);
        for key in ["ROBOTUTOR_CODEX", "CODEX_HOME"] {
            if env::var_os(key).is_some() {
                run.arg(format!("--setenv={key}"));
            }
        }
        if run
            .arg(&python)
            .arg(worker)
            .arg("drain")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Ok(());
        }
    }
    let mut child = Command::new(python)
        .arg(worker)
        .arg("drain")
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
