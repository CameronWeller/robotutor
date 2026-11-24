use crate::{runtime::Result, theme};
use eframe::egui::{self, Ui};
use rusqlite::{params, Connection};
use std::{collections::BTreeMap, path::Path};

const KEYS: [&str; 5] = ["purpose", "topic", "deliverable", "criterion", "checkpoint"];
const LABELS: [&str; 5] = [
    "Purpose",
    "Topic",
    "Deliverable",
    "Success check",
    "First checkpoint",
];
const GOALS: [&str; 3] = ["Explain an idea", "Compare options", "Support a claim"];
const COACHES: [&str; 3] = [
    "Find a starting point",
    "Check my evidence",
    "Revise a passage",
];
type Brief = BTreeMap<String, String>;

struct Record {
    status: String,
    brief: Brief,
    draft: String,
    version: i64,
    coach: String,
    checkpoint: String,
    turns: Vec<(String, String)>,
}
pub struct Assignment {
    db: Connection,
    id: i64,
    record: Record,
    draft: String,
    answer: String,
    next: String,
    message: String,
    conflict: bool,
}
impl Assignment {
    pub fn open(folder: &Path) -> Result<Self> {
        let db = Connection::open(folder.join("long-form-assignments.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(2))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS assignments(
          id INTEGER PRIMARY KEY,status TEXT NOT NULL DEFAULT 'planning',brief TEXT NOT NULL DEFAULT '{}',
          draft TEXT NOT NULL DEFAULT '',version INTEGER NOT NULL DEFAULT 0,coach TEXT NOT NULL DEFAULT '', checkpoint TEXT NOT NULL DEFAULT '');
          CREATE UNIQUE INDEX IF NOT EXISTS one_open_assignment ON assignments ((1)) WHERE status!='finished';
          CREATE TABLE IF NOT EXISTS turns(id INTEGER PRIMARY KEY,assignment_id INTEGER NOT NULL REFERENCES assignments(id),learner TEXT NOT NULL,helper TEXT NOT NULL);")?;
        let id = Self::active(&db)?;
        let record = Self::read(&db, id)?;
        Ok(Self {
            db,
            id,
            draft: record.draft.clone(),
            record,
            answer: String::new(),
            next: String::new(),
            message: String::new(),
            conflict: false,
        })
    }
    fn active(db: &Connection) -> Result<i64> {
        db.execute(
            "INSERT OR IGNORE INTO assignments(status) VALUES('planning')",
            [],
        )?;
        Ok(db.query_row(
            "SELECT id FROM assignments WHERE status!='finished'",
            [],
            |r| r.get(0),
        )?)
    }
    fn read(db: &Connection, id: i64) -> Result<Record> {
        let tx = db.unchecked_transaction()?;
        let (status, brief, draft, version, coach, checkpoint): (
            String,
            String,
            String,
            i64,
            String,
            String,
        ) = tx.query_row(
            "SELECT status,brief,draft,version,coach,checkpoint FROM assignments WHERE id=?",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )?;
        let mut query =
            tx.prepare("SELECT learner,helper FROM turns WHERE assignment_id=? ORDER BY id")?;
        let turns = query
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(query);
        tx.commit()?;
        Ok(Record {
            status,
            brief: serde_json::from_str(&brief)?,
            draft,
            version,
            coach,
            checkpoint,
            turns,
        })
    }
    fn select(&mut self, id: i64) -> Result<()> {
        let record = Self::read(&self.db, id)?;
        self.id = id;
        self.draft = record.draft.clone();
        self.record = record;
        self.answer.clear();
        self.next.clear();
        self.message.clear();
        self.conflict = false;
        Ok(())
    }
    fn field(&self) -> Option<&'static str> {
        KEYS.into_iter()
            .find(|k| !self.record.brief.contains_key(*k))
    }
    fn step(brief: &Brief, field: Option<&str>) -> (String, Vec<&'static str>) {
        let purpose = brief.get("purpose").map(String::as_str).unwrap_or("");
        let (prompt, choices) = match field {
            Some("purpose") => (
                "What would you like to practice over a few sessions?",
                GOALS.to_vec(),
            ),
            Some("topic") => (
                match purpose {
                    "Compare options" => "Which options would you like to compare?",
                    "Support a claim" => "What claim would you like to test?",
                    "Explain an idea" => "Which idea would you like to explain?",
                    _ => "What topic shall we use?",
                },
                vec![],
            ),
            Some("deliverable") => {
                return (
                    format!("What would you like to make about {}?", brief["topic"]),
                    vec!["An article", "An illustrated guide", "A report"],
                )
            }
            Some("criterion") => (
                "What will tell you that your work succeeded?",
                match purpose {
                    "Compare options" => {
                        vec!["Both options use the same test", "I explain one trade-off"]
                    }
                    "Support a claim" => {
                        vec!["Evidence supports the claim", "I address a counterexample"]
                    }
                    "Explain an idea" => vec![
                        "A reader can explain it back",
                        "My example shows how it works",
                    ],
                    _ => vec!["A reader can use it", "I can justify my decisions"],
                },
            ),
            Some("checkpoint") => (
                "What is one piece you can finish in your first sitting?",
                match purpose {
                    "Compare options" => vec![
                        "Pick a comparison test",
                        "Collect one data point",
                        "Sketch a table",
                    ],
                    "Support a claim" => vec![
                        "Find one source",
                        "List a counterexample",
                        "Draft the claim",
                    ],
                    _ => vec![
                        "Collect one example",
                        "Sketch an outline",
                        "Draft the opening",
                    ],
                },
            ),
            _ => ("Does this plan feel worth spending time on?", vec![]),
        };
        (prompt.into(), choices)
    }
    fn turn(&mut self, action: &str, words: &str) -> Result<()> {
        let r = &self.record;
        let mut brief = r.brief.clone();
        let mut status = r.status.clone();
        let mut coach = r.coach.clone();
        let mut checkpoint = r.checkpoint.clone();
        let mut words = words.trim().to_string();
        let reply = match (r.status.as_str(), action) {
            ("planning", "answer") if !words.is_empty() => {
                brief.insert(
                    self.field().ok_or("Plan already complete")?.into(),
                    words.clone(),
                );
                Self::step(&brief, KEYS.into_iter().find(|k| !brief.contains_key(*k))).0
            }
            ("planning", "change") if brief.contains_key(&words) => {
                brief.remove(&words);
                let prompt = Self::step(&brief, Some(&words)).0;
                words = format!("Change {words}");
                prompt
            }
            ("planning", "accept") if self.field().is_none() => {
                status = "working".into();
                checkpoint = brief["checkpoint"].clone();
                words = "Start this assignment".into();
                format!("Your first checkpoint is “{checkpoint}”.")
            }
            ("working", "coach") if COACHES.contains(&words.as_str()) => {
                coach = words.clone();
                match coach.as_str(){
                "Check my evidence"=>format!("Your success check is “{}”. Which example in your draft could help a reader see it?",brief["criterion"]),
                "Revise a passage"=>"Which sentence or passage would you like to change? Paste just that part.".into(),
                _=>format!("For “{checkpoint}”, what is the smallest piece you could put on the page?")}
            }
            ("working", "reply") if !coach.is_empty() && !words.is_empty() => {
                let excerpt: String = words.chars().take(140).collect();
                let reply=match coach.as_str(){
                "Check my evidence"=>format!("Beside “{excerpt}”, add one sentence explaining how the example supports your point about {}.",brief["topic"]),
                "Revise a passage"=>format!("Read “{excerpt}” aloud. Try a second version with the main idea first. Compare them against “{}”.",brief["criterion"]),
                _=>format!("Try “{excerpt}” as a rough first piece. Check it against “{}”.",brief["criterion"])};
                coach.clear();
                reply
            }
            ("working", "checkpoint") if !words.is_empty() => {
                let reply =
                    format!("You set “{words}” as the next checkpoint after “{checkpoint}”.");
                checkpoint = words.clone();
                coach.clear();
                reply
            }
            ("working", "finish") if !r.draft.trim().is_empty() => {
                status = "finished".into();
                coach.clear();
                words = "Finish this assignment".into();
                "Your finished draft and helper turns are saved here.".into()
            }
            _ => return Err("Complete the current step before continuing.".into()),
        };
        let tx = self.db.transaction()?;
        let changed=tx.execute("UPDATE assignments SET status=?,brief=?,coach=?,checkpoint=?,version=version+1 WHERE id=? AND version=? AND (?!='finish' OR draft=?)",
            params![status,serde_json::to_string(&brief)?,coach,checkpoint,self.id,r.version,action,r.draft])?;
        if changed != 1 {
            return Err(
                "Another window changed this assignment. Reopen the assignment before continuing."
                    .into(),
            );
        }
        tx.execute(
            "INSERT INTO turns(assignment_id,learner,helper) VALUES(?,?,?)",
            params![self.id, words, reply],
        )?;
        tx.commit()?;
        self.select(self.id)
    }
    fn go(&mut self, action: &str, value: String) {
        if let Err(e) = self.turn(action, &value) {
            self.message = e.to_string();
        }
    }
    fn save_draft(&mut self) -> Result<()> {
        if self.db.execute(
            "UPDATE assignments SET draft=? WHERE id=? AND status='working' AND draft=?",
            params![self.draft, self.id, self.record.draft],
        )? != 1
        {
            return Err(
                "Draft changed in another window. Copy the unsaved text before reopening.".into(),
            );
        }
        self.record.draft = self.draft.clone();
        Ok(())
    }
    fn brief(&self, ui: &mut Ui) {
        for (k, label) in KEYS.into_iter().zip(LABELS) {
            if let Some(v) = self.record.brief.get(k) {
                ui.label(format!("{label} · {v}"));
            }
        }
    }
    pub fn ui(&mut self, ui: &mut Ui, topic: &str) {
        let planning = self.record.status == "planning";
        let working = self.record.status == "working";
        ui.heading(if planning {
            "Plan an assignment"
        } else {
            self.record
                .brief
                .get("topic")
                .map(String::as_str)
                .unwrap_or("Assignment")
        });
        ui.add_space(10.);
        if planning {
            self.brief(ui);
            ui.add_space(8.);
            let field = self.field();
            let (q, choices) = Self::step(&self.record.brief, field);
            ui.label(egui::RichText::new(q).size(20.).strong());
            if let Some(f) = field {
                ui.horizontal_wrapped(|ui| {
                    for choice in choices {
                        if ui.button(choice).clicked() {
                            self.go("answer", choice.into());
                        }
                    }
                    if f == "topic" && !topic.is_empty() && ui.button(topic).clicked() {
                        self.go("answer", topic.into());
                    }
                });
                let entry = ui.add(
                    egui::TextEdit::singleline(&mut self.answer)
                        .hint_text("Your own answer…")
                        .desired_width(400.),
                );
                if !self.answer.trim().is_empty()
                    && (ui.button("Send").clicked()
                        || entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    self.go("answer", self.answer.clone());
                }
                if let Some(last) = KEYS
                    .into_iter()
                    .rev()
                    .find(|k| self.record.brief.contains_key(*k))
                {
                    if ui.button("Back one step").clicked() {
                        self.go("change", last.into());
                    }
                }
            } else {
                ui.label(format!(
                    "Use a first draft to practice “{}”, then revisit it against “{}”.",
                    self.record.brief["purpose"], self.record.brief["criterion"]
                ));
                if theme::action(ui, "Start this assignment").clicked() {
                    self.go("accept", String::new());
                }
                ui.collapsing("Edit the plan", |ui| {
                    for (k, label) in KEYS.into_iter().zip(LABELS) {
                        if ui.button(label).clicked() {
                            self.go("change", k.into());
                        }
                    }
                });
            }
        } else {
            ui.collapsing("Assignment brief", |ui| self.brief(ui));
            ui.label(format!("Checkpoint · {}", self.record.checkpoint));
            ui.heading("Your draft");
            let edit = ui.add(
                egui::TextEdit::multiline(&mut self.draft)
                    .interactive(working && !self.conflict)
                    .desired_rows(7)
                    .desired_width(f32::INFINITY),
            );
            if edit.changed() {
                if let Err(e) = self.save_draft() {
                    self.message = e.to_string();
                    self.conflict = true
                }
            }
            if working && !self.conflict {
                ui.label("Draft saves on this device as you type.");
                ui.add_space(12.);
                ui.heading("Assignment helper");
                if let Some((_, reply)) = self.record.turns.last() {
                    ui.label(reply);
                }
                if !self.record.coach.is_empty() {
                    ui.text_edit_singleline(&mut self.answer);
                    if ui.button("Send helper reply").clicked() {
                        self.go("reply", self.answer.clone());
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    for c in COACHES {
                        if ui.button(c).clicked() {
                            self.go("coach", c.into());
                        }
                    }
                });
                ui.collapsing("Set the next checkpoint", |ui| {
                    ui.text_edit_singleline(&mut self.next);
                    if ui.button("Save checkpoint").clicked() {
                        self.go("checkpoint", self.next.clone());
                    }
                });
                if ui
                    .add_enabled(
                        !self.draft.trim().is_empty(),
                        egui::Button::new("Mark assignment finished"),
                    )
                    .clicked()
                {
                    self.go("finish", String::new());
                }
            }
        }
        ui.collapsing("Previous turns", |ui| {
            for (learner, helper) in &self.record.turns {
                ui.label(format!("You · {learner}"));
                ui.label(format!("Helper · {helper}"));
                ui.separator();
            }
        });
        let mut select = None;
        if let Ok(mut query) = self
            .db
            .prepare("SELECT id,brief FROM assignments WHERE status='finished' ORDER BY id DESC")
        {
            let rows = query
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>());
            drop(query);
            if let Ok(rows) = rows {
                ui.collapsing("Finished assignments", |ui| {
                    for (id, b) in rows {
                        if let Ok(b) = serde_json::from_str::<Brief>(&b) {
                            if ui.button(&b["topic"]).clicked() {
                                select = Some(id);
                            }
                        }
                    }
                });
            }
        }
        if let Some(id) = select {
            if let Err(e) = self.select(id) {
                self.message = e.to_string();
            }
        }
        if !planning && !working && ui.button("Open assignment planner").clicked() {
            if let Err(e) = Self::active(&self.db).and_then(|id| self.select(id)) {
                self.message = e.to_string();
            }
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_plan_survives_reopen_and_preserves_another_writers_draft() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Assignment::open(root.path()).unwrap();
        first.turn("answer", "Explain an idea").unwrap();
        let mut resumed = Assignment::open(root.path()).unwrap();
        assert_eq!(first.id, resumed.id);
        for answer in [
            "Gear ratios",
            "An illustrated guide",
            "A reader can explain it back",
            "Sketch an outline",
        ] {
            resumed.turn("answer", answer).unwrap();
        }
        resumed.turn("accept", "").unwrap();
        resumed.draft = "One gear turns twice.".into();
        resumed.save_draft().unwrap();
        let mut stale = Assignment::open(root.path()).unwrap();
        resumed.draft.push_str(" Count its teeth.");
        resumed.save_draft().unwrap();
        stale.draft = "Replace the other draft".into();
        assert!(stale.save_draft().is_err());
        assert!(stale.turn("finish", "").is_err());
        resumed.turn("coach", COACHES[1]).unwrap();
        let mut back = Assignment::open(root.path()).unwrap();
        assert_eq!(back.record.coach, COACHES[1]);
        back.turn("reply", "Count the teeth").unwrap();
        assert!(back
            .record
            .turns
            .last()
            .unwrap()
            .1
            .contains("Count the teeth"));
        back.turn("finish", "").unwrap();
        assert_ne!(back.id, Assignment::open(root.path()).unwrap().id);
    }
}
