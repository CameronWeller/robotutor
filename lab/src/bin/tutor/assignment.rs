//! Both clients confirm the exact submission text and reuse the same outbox on retry.
use crate::{practice, Event, Journal, Visit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::Path,
};
use tutor_lab::platform::PrivateOpen;
type Result<T> = std::result::Result<T, String>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub id: u64,
    pub key: String,
    pub work: String,
}
fn api(body: &Value) -> Result<Value> {
    let port = std::env::var("ROBOTUTOR_PORT")
        .unwrap_or_else(|_| if cfg!(windows) { "47322" } else { "47321" }.into())
        .parse::<u16>()
        .map_err(|_| "Invalid LMS port.")?;
    tutor_lab::lms_client::post(port, body)
}
fn private_json(path: &Path) -> Result<Value> {
    let f = OpenOptions::new()
        .read(true)
        .open_private(path)
        .map_err(|e| e.to_string())?;
    serde_json::from_reader(f.take(2_000_000)).map_err(|e| e.to_string())
}
fn identifier() -> Result<String> {
    Ok(uuid::Uuid::new_v4().to_string())
}
fn spool(path: &Path, p: &Preview) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{}", identifier()?));
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open_private(&temp)
        .map_err(|e| e.to_string())?;
    let written = (|| {
        serde_json::to_writer(&mut f, p)?;
        f.sync_all()?;
        tutor_lab::platform::replace_staged(path.parent().unwrap_or(Path::new(".")), &temp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(temp);
    }
    written.map_err(|e: std::io::Error| e.to_string())
}
fn status(folder: &Path, id: u64, version: &Value) -> Result<Value> {
    let a = api(&json!({"action":"state"}))
        .ok()
        .and_then(|state| {
            state["assignments"]
                .as_array()?
                .iter()
                .find(|a| a["id"] == id)
                .cloned()
        })
        .map(Ok)
        .unwrap_or_else(|| private_json(&folder.join("assignment.json")))?;
    if a["version"] != *version || a["id"] != id {
        return Err("The assignment binding changed.".into());
    }
    Ok(a)
}
fn exact_preview(visit: &Visit, arg: &str) -> String {
    if !arg.is_empty() {
        return arg.into();
    }
    crate::practice::current(visit)
        .map(|form| visit.draft.visible(&form))
        .unwrap_or_else(|| visit.draft.text.clone())
}
pub fn handle(
    cmd: &str,
    arg: &str,
    visit: &mut Visit,
    journal: &mut Option<Journal>,
) -> Result<bool> {
    if !matches!(
        cmd,
        "/assignment" | "/preview" | "/submit" | "/revision" | "/confirm-preview"
    ) {
        return Ok(false);
    }
    let j = journal
        .as_ref()
        .filter(|j| j.assignment.is_some())
        .ok_or("Open an assigned lesson first.")?;
    let binding = private_json(&j.path.with_extension("assignment.json"))?;
    let folder = j
        .path
        .parent()
        .ok_or("Missing assignment folder.")?
        .to_owned();
    let id = binding["id"].as_u64().ok_or("Missing assignment ID.")?;
    let outbox = folder.join("pending.json");
    match cmd {
        "/assignment" | "/revision" => {
            let a = status(&folder, id, &binding["version"])?;
            let latest = a["submissions"].as_array().and_then(|s| s.last());
            if cmd == "/revision" {
                let text = latest
                    .and_then(|s| s["work"].as_str())
                    .filter(|_| a["status"] == "Revision requested")
                    .ok_or("There is no requested revision.")?;
                let mut draft = visit.draft.clone();
                draft.text = text.into();
                practice::edit(visit, journal, draft)?;
                crate::visit_notice("The reviewed version is in your private draft.");
            } else {
                crate::visit_notice(&format!(
                    "{} · {}\n{}\nSubmit · {}\nLatest shared version · {}\nTeacher feedback · {}",
                    a["activity"]["title"],
                    a["status"],
                    a["activity"]["objective"],
                    a["activity"]["criterion"],
                    latest.and_then(|s| s["work"].as_str()).unwrap_or("None"),
                    a["feedback"].as_str().unwrap_or("")
                ));
            }
            return Ok(true);
        }
        "/preview" | "/confirm-preview" => {
            if outbox.exists() {
                return Err(
                    "A submission is awaiting acknowledgment. Use /submit to retry it.".into(),
                );
            }
            let text = exact_preview(visit, arg);
            if text.trim().is_empty() || text.len() > 32000 {
                return Err("Preview between 1 and 32,000 bytes of work.".into());
            }
            let preview = Preview {
                id,
                key: identifier()?,
                work: text.clone(),
            };
            crate::append(
                journal,
                visit,
                Event::SubmissionPreview {
                    preview: Some(preview),
                },
            )?;
            if cmd == "/preview" {
                crate::visit_notice(&format!(
                    "Submission preview\n{text}\nUse /submit to share exactly this text."
                ));
                return Ok(true);
            }
        }
        _ => {}
    }
    let pending: Preview = if outbox.exists() {
        serde_json::from_value(private_json(&outbox)?).map_err(|e| e.to_string())?
    } else {
        let p = visit
            .preview
            .clone()
            .ok_or("Use /preview to choose the text to share first.")?;
        spool(&outbox, &p)?;
        p
    };
    if pending.id != id {
        return Err("Pending submission belongs to another assignment.".into());
    }
    let response = api(&json!({"action":"submit","id":id,"work":pending.work,"key":pending.key}));
    if let Ok(receipt) = response {
        if receipt["revision"].as_u64().is_none() {
            return Err("Missing acknowledgment for the shared work.".into());
        }
        crate::append(journal, visit, Event::SubmissionPreview { preview: None })?;
        fs::remove_file(&outbox).map_err(|e| e.to_string())?;
        crate::visit_notice(&format!(
            "Submission acknowledged · revision {}",
            receipt["revision"]
        ));
    } else {
        crate::visit_notice("Submission pending acknowledgment. Use /submit to retry.");
    }
    if std::env::var_os("ROBOTUTOR_DESKTOP").is_some() {
        println!("\x1eSUBMISSION {}", json!({"pending":outbox.exists()}))
    }
    Ok(true)
}
