//! Local locked event file used by the longitudinal rehearsal and learner conversation.
use super::{Action, Session, FORMAT};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

fn sha(v: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).expect("JSON value"))
    )
}
fn line(file: &mut File, v: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}
fn head_error(expected: &str, actual: &str) -> String {
    format!("expected head {expected}, recovered {actual}")
}
fn attach(mut v: Value, head: String) -> Value {
    v["head"] = json!(head);
    v
}
pub fn create(path: &Path) -> Result<Value, String> {
    let header = json!({"format":FORMAT});
    let head = sha(&header);
    let session = Session::new()?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(
        &mut options,
        windows_sys::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH,
    );
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    line(&mut file, &header)?;
    crate::platform::sync_created_entry(path).map_err(|e| e.to_string())?;
    Ok(attach(session.status()?, head))
}

struct Loaded {
    file: File,
    session: Session,
    head: String,
    complete: usize,
    total: usize,
}

fn load(path: &Path, apply: bool, expected: Option<&str>) -> Result<Loaded, String> {
    let header = json!({"format":FORMAT});
    let mut head = sha(&header);
    let mut session = Session::new()?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(apply)
        .open(path)
        .map_err(|e| e.to_string())?;
    if apply {
        file.lock()
    } else {
        file.lock_shared()
    }
    .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let complete = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or("missing complete journal header")?
        + 1;
    let mut rows = bytes[..complete - 1].split(|b| *b == b'\n');
    let read = |s: &[u8]| serde_json::from_slice::<Value>(s).map_err(|e| e.to_string());
    if read(rows.next().ok_or("empty journal")?)? != header {
        return Err("unsupported rehearsal journal".into());
    }
    for (i, line) in rows.enumerate() {
        let r = read(line)?;
        let body = json!({"previous":head,"event":r["event"]});
        let next = sha(&body);
        if r != json!({"previous":head,"event":r["event"],"head":next}) {
            return Err(format!("broken record chain at action {}", i + 1));
        }
        session = session.restore_next(&r["event"])?;
        head = next;
    }
    if let Some(expected) = expected {
        if expected != head {
            return Err(head_error(expected, &head));
        }
    }

    Ok(Loaded {
        file,
        session,
        head,
        complete,
        total: bytes.len(),
    })
}

/// The projection and any expected-head check occur under the same shared lock.
pub fn read(
    path: &Path,
    expected: Option<&str>,
    project: impl FnOnce(&Session) -> Result<Value, String>,
) -> Result<Value, String> {
    let loaded = load(path, false, expected)?;
    Ok(attach(project(&loaded.session)?, loaded.head))
}

/// Check the caller's head before constructing a possibly private input action.
pub fn apply(
    path: &Path,
    expected: &str,
    action: impl FnOnce() -> Result<Action, String>,
) -> Result<Value, String> {
    let Loaded {
        mut file,
        mut session,
        head,
        complete,
        total,
    } = load(path, true, Some(expected))?;
    let (event, out) = session.apply(action()?)?;
    let mut record = json!({"previous":head,"event":event});
    let next = sha(&record);
    record["head"] = json!(next);
    if total != complete {
        file.set_len(complete as u64).map_err(|e| e.to_string())?;
    }
    file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    line(&mut file, &record)?;
    Ok(attach(out, next))
}
