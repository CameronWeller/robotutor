use super::{
    runtime::{self, read, s, Result},
    theme, Desktop,
};
use eframe::egui::{self, Ui};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
#[derive(Default)]
pub struct Panels {
    pub events: Vec<Value>,

    pub resources: Vec<Value>,

    pub resource_check: Option<Instant>,
    selected: usize,
    reviews: Vec<Value>,
    review_time: Option<Instant>,
    courses: Vec<(PathBuf, Value)>,
    course_time: Option<Instant>,
    directions: BTreeMap<PathBuf, String>,
}
impl Panels {
    pub fn inspector(&mut self, ui: &mut Ui) {

        ui.collapsing(format!("Research resources ({})", self.resources.len()), |ui| {
            ui.label("Resources available in this lesson can help explain how a learner approached a question. Mastery still requires evidence from the learner.");
            for source in &self.resources {
                ui.label(s(&source["resource"]));
                if let Some(title) = source["title"].as_str().filter(|t| !t.is_empty()) {
                    ui.strong(title);
                }
                for heading in source["available_headings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    ui.label(heading);
                }
            }
        });
        if self.events.is_empty() {
            ui.label("Waiting for lesson events.");
            return;
        }
        egui::ComboBox::from_label("Event")
            .selected_text(format!("Event {}", self.selected + 1))
            .show_ui(ui, |ui| {
                for (i, e) in self.events.iter().enumerate() {
                    let name = e
                        .get("event")
                        .and_then(Value::as_str)
                        .unwrap_or("Journal entry");
                    ui.selectable_value(&mut self.selected, i, format!("{} · {name}", i + 1));
                }
            });
        let row = &self.events[self.selected];
        for (key, value) in row.as_object().into_iter().flatten() {
            if matches!(key.as_str(), "next_offset" | "offset") {
                continue;
            }
            ui.collapsing(key, |ui| {
                ui.label(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default()),
                );
            });
        }
    }
    pub fn reviews(&mut self, folder: &Path) -> Result<()> {
        let path = folder.join(".astra/reviews.sqlite3");
        self.reviews.clear();
        self.review_time = Some(Instant::now());
        if !path.is_file() {
            return Ok(());
        }
        let db = Connection::open(path)?;
        let mut q=db.prepare("SELECT id,session,stage,status,resolved,error,analysis FROM reviews ORDER BY (status='note' AND resolved=0) DESC,id DESC")?;
        for row in q.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"session":r.get::<_,String>(1)?,"stage":r.get::<_,String>(2)?,
            "status":r.get::<_,String>(3)?,"resolved":r.get::<_,bool>(4)?,"error":r.get::<_,Option<String>>(5)?,
            "analysis":r.get::<_,Option<String>>(6)?.and_then(|v|serde_json::from_str::<Value>(&v).ok())})))?{self.reviews.push(row?);}
        Ok(())
    }
    fn courses(&mut self, folder: &Path) -> Result<()> {
        self.course_time = Some(Instant::now());
        self.courses.clear();
        let dir = folder.join(".courses");
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file() && path.extension().is_some_and(|x| x == "json") {
                self.courses.push((path.clone(), read(&path)?));
            }
        }
        self.courses.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(())
    }
}
fn python() -> std::ffi::OsString {
    std::env::var_os("ROBOTUTOR_PYTHON").unwrap_or_else(|| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    })
}
impl Desktop {
    pub fn review_ui(&mut self, ui: &mut Ui) -> Result<()> {
        ui.heading("Astra reviews");
        if self
            .tools
            .review_time
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
        {
            self.tools.reviews(&self.folder)?
        }
        if self.tools.reviews.is_empty() {
            ui.label("No comparisons yet.");
        }
        let mut action = None;
        for r in &self.tools.reviews {
            ui.push_id(r["id"].as_i64(), |ui| {
                ui.collapsing(
                    format!(
                        "{} · {} · {}",
                        s(&r["session"]),
                        s(&r["stage"]),
                        if r["resolved"] == true {
                            "reviewed"
                        } else {
                            s(&r["status"])
                        }
                    ),
                    |ui| {
                        for (k, title) in [
                            ("observation", "What Astra would improve"),
                            ("luna_evidence", "Luna wrote"),
                            ("astra_evidence", "Astra wrote"),
                            ("luna_change", "Proposed Luna change"),
                            ("replay_check", "How to test the change"),
                        ] {
                            if let Some(v) = r["analysis"][k].as_str() {
                                ui.strong(title);
                                ui.label(v);
                            }
                        }
                        if let Some(e) = r["error"].as_str().filter(|s| !s.is_empty()) {
                            ui.label(e);
                        }
                        if r["status"] == "blocked" && ui.button("Retry blocked review").clicked() {
                            action = Some((r["id"].as_i64().unwrap(), true))
                        }
                        if r["status"] == "note"
                            && ui
                                .button(if r["resolved"] == true {
                                    "Reopen note"
                                } else {
                                    "Mark reviewed"
                                })
                                .clicked()
                        {
                            action = Some((r["id"].as_i64().unwrap(), false))
                        }
                    },
                );
            });
        }
        if let Some((id, retry)) = action {
            let dir = self.folder.join(".astra");
            let db = Connection::open(dir.join("reviews.sqlite3"))?;
            if retry {
                db.execute(
                    "UPDATE reviews SET status='pending',error='' WHERE id=? AND status='blocked'",
                    [id],
                )?;
                let mut child = runtime::command(python())
                    .arg(self.runtime.root.join("desktop/astra.py"))
                    .arg("wake")
                    .arg(dir)
                    .spawn()?;
                std::thread::spawn(move || child.wait());
            } else {
                db.execute(
                    "UPDATE reviews SET resolved=1-resolved WHERE id=? AND status='note'",
                    [id],
                )?;
            }
            self.tools.reviews(&self.folder)?;
        }
        Ok(())
    }
    pub fn personal_courses(&mut self, ui: &mut Ui) -> Result<()> {
        if self
            .tools
            .course_time
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.tools.courses(&self.runtime.personal)?;
        }
        let mut selected = None;
        let mut revision = None;
        for (path, state) in &self.tools.courses {
            let course = &state["outline"];
            ui.separator();
            if course.is_null() {
                ui.heading(s(&state["seed"]["goal"]));
            } else {
                ui.heading(s(&course["title"]));
                ui.label(s(&course["destination"]));
            }
            for unit in course["units"].as_array().into_iter().flatten() {
                ui.strong(s(&unit["title"]));
                for a in unit["activities"].as_array().into_iter().flatten() {
                    ui.label(s(&a["objective"]));
                    if ui.button(s(&a["title"])).clicked() {
                        selected = Some((path.clone(), course.clone(), a.clone()));
                    }
                }
            }
            if let Some(problem) = state["problem"].as_str() {
                ui.label(problem);
            }
            if let Some(changes) = state["changes"].as_str() {
                ui.label(changes);
            }
            let direction = self.tools.directions.entry(path.clone()).or_default();
            ui.add(
                egui::TextEdit::singleline(direction)
                    .hint_text("How would you like to change your course?"),
            );
            if ui
                .button(if course.is_null() {
                    "Retry proposal"
                } else {
                    "Revise course"
                })
                .clicked()
            {
                revision = Some((state["seed"].clone(), direction.clone()));
            }
        }
        if let Some((mut seed, request)) = revision {
            if self.snapshot["working_plan"]["subject"] == seed["goal"] {
                seed["working_plan"] = self.snapshot["working_plan"].clone();
            }
            let mut child = runtime::command(python())
                .arg(self.runtime.root.join("desktop/course_agent.py"))
                .arg("--revise-personal")
                .arg(&self.runtime.personal)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?;
            child.stdin.take().unwrap().write_all(
                json!({"seed":seed,"request":request})
                    .to_string()
                    .as_bytes(),
            )?;
            std::thread::spawn(move || child.wait());
            self.notice = "Preparing your revised course…".into();
        }
        if let Some((path, course, a)) = selected {
            let key = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("Invalid course key")?;
            let name = activity_name(
                key,
                a.get("key")
                    .and_then(Value::as_str)
                    .unwrap_or(s(&a["title"])),
            );
            let location = self.runtime.personal.join(&name);
            if !location.exists() {
                let exercise = format!("{}\n\n{}", s(&a["instructions"]), s(&a["criterion"]));
                let lesson = json!({"topic":course["title"],"declared_goal":course["destination"],"selected":null,"stage":0,"objective":a["objective"],
                    "next_objective":"","exercise":exercise,"closed":[],"challenge":false,"washington_mentioned":false});
                let mut file = NamedTempFile::new_in(&self.runtime.personal)?;
                writeln!(
                    file,
                    "{}",
                    json!({"event":"open","version":1,"selected":null})
                )?;
                writeln!(
                    file,
                    "{}",
                    json!({"event":"delivered","text":exercise,"lesson":lesson,"sources":[]})
                )?;
                file.as_file().sync_all()?;
                if let Err(e) = file.persist_noclobber(location) {
                    if e.error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(e.into());
                    }
                }
            }
            self.refresh(Some(name))?;
        }
        if self.tools.courses.is_empty() {
            theme::card(theme::PANEL).show(ui,|ui|{ui.label("Personal course outlines appear here after you agree on a starting point with Luna.");});
        }
        Ok(())
    }
}
// Match the existing journal key, including Python JSON's ASCII escaping and comma spacing.
fn activity_name(course: &str, activity: &str) -> String {
    let encode = |s: &str| -> String {
        let mut out = String::new();
        for c in serde_json::to_string(s).unwrap().chars() {
            if c as u32 >= 127 {
                for u in c.encode_utf16(&mut [0; 2]) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            } else {
                out.push(c)
            }
        }
        out
    };
    let key = format!("[{}, {}]", encode(course), encode(activity));
    let digest: String = Sha256::digest(key)[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("course-activity-{digest}.jsonl")
}

#[test]
fn activity_key_reopens_existing_unicode_journal() {
    assert_eq!(
        activity_name("course", "Gear ⚙ and 🙂"),
        "course-activity-30c93e285a59a055df1f259c.jsonl"
    );
}
