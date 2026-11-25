#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
mod assignment;
#[path = "../../../lab/src/installation.rs"]
mod installation;
mod instance;
#[path = "../../../lab/src/lms_client.rs"]
mod lms_client;
mod practice;

mod research;
mod runtime;
mod theme;
mod tools;
use assignment::Assignment;
use eframe::egui::{self, RichText, Ui};
use runtime::{read, s, save, Result, Runtime, Tutor};
use serde_json::{json, Map, Value};
use std::{
    env,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Lesson,
    Assignment,
    Courses,
    Reviews,
    Inspector,
    Preview,
}
struct Desktop {
    instance: instance::Instance,
    runtime: Runtime,
    personal: Runtime,
    profile: String,
    choices: Vec<(String, String)>,
    key: String,
    folder: PathBuf,
    visit: String,
    tutor: Option<Tutor>,
    ready: bool,
    snapshot: Value,
    history: Vec<(String, String)>,
    draft: String,
    fields: Map<String, Value>,
    status: String,
    notice: String,
    plan: String,
    page: Page,
    assignment: Option<Assignment>,
    preview: String,
    classroom: Value,
    point: usize,
    logo: egui::TextureHandle,
    course_result: Option<Receiver<ResultString>>,
    tools: tools::Panels,
    inspect_offset: u64,

    research: research::Window,
}
fn preceding_turns<'a>(history: &'a [(String, String)], question: &str) -> &'a [(String, String)] {
    match history.split_last() {
        Some(((speaker, text), earlier)) if speaker == "Luna" && text.trim() == question.trim() => {
            earlier
        }
        _ => history,
    }
}

fn key_is_assignment(key: &str) -> bool {
    key.parse::<u64>().is_ok()
}
type ResultString = std::result::Result<String, String>;
impl Desktop {
    fn new(
        ctx: &egui::Context,
        rt: Runtime,
        requested: Option<String>,
        profile: String,
        reset: bool,
        instance: instance::Instance,
    ) -> Result<Self> {
        let logo = theme::setup(ctx);
        let base = rt.clone();
        let rt = if profile.is_empty() {
            rt
        } else {
            rt.profile(&profile, reset)?
        };
        let mut app = Self {
            instance,
            runtime: rt,
            personal: base,
            profile,
            choices: vec![],
            key: String::new(),
            folder: PathBuf::new(),
            visit: String::new(),
            tutor: None,
            ready: false,
            snapshot: Value::Null,
            history: vec![],
            draft: String::new(),
            fields: Map::new(),
            status: String::new(),
            notice: String::new(),
            plan: String::new(),
            page: Page::Lesson,
            assignment: None,
            preview: String::new(),
            classroom: Value::Null,
            point: 0,
            logo,
            course_result: None,
            tools: tools::Panels::default(),
            inspect_offset: 0,

            research: research::Window::default(),
        };
        app.refresh(requested)?;
        Ok(app)
    }
    fn refresh(&mut self, requested: Option<String>) -> Result<()> {
        let choices = self.runtime.choices()?;
        let previous = read(&self.runtime.assigned.join("last-lesson.json"))
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        let key = requested
            .or(previous)
            .filter(|k| choices.iter().any(|c| &c.0 == k))
            .unwrap_or_else(|| choices[0].0.clone());
        self.choices = choices;
        if key == self.key && key_is_assignment(&key) {
            self.classroom = read(&self.folder.join("assignment.json"))?;
        }
        if self.key != key || self.tutor.is_none() {
            self.open(key)?;
        }
        Ok(())
    }
    fn open(&mut self, key: String) -> Result<()> {
        let (folder, visit) = self.runtime.visit(&key)?;

        self.research.close();
        // Release the old journal writer before starting another session in the same workspace.
        self.tutor.take();
        self.tutor = Some(Tutor::start(&self.runtime, &folder, &visit)?);
        self.key = key;
        self.classroom = if key_is_assignment(&self.key) {
            read(&folder.join("assignment.json"))?
        } else {
            Value::Null
        };
        self.folder = folder;
        self.visit = visit;
        self.ready = false;
        self.snapshot = Value::Null;
        self.history.clear();
        self.draft.clear();
        self.fields.clear();
        self.plan.clear();
        self.notice.clear();
        self.preview.clear();
        self.tools = tools::Panels::default();
        self.inspect_offset = 0;
        self.assignment = None;
        self.page = Page::Lesson;
        self.status = "Opening lesson…".into();
        save(
            &self.runtime.assigned.join("last-lesson.json"),
            &json!(self.key),
            false,
        )
    }
    fn write(&mut self, data: Value, wait: bool) {
        let res = self
            .tutor
            .as_mut()
            .ok_or("No open lesson".into())
            .and_then(|t| t.write(&data));
        if let Err(e) = res {
            self.ready = false;
            self.status = e.to_string()
        } else if wait {
            self.ready = false;
            self.status = "Waiting for Luna…".into();
            self.notice.clear();
        }
    }
    fn cmd(&mut self, cmd: &str) {
        if self.ready {
            self.write(json!(cmd), true)
        }
    }
    fn writable(&self) -> bool {
        self.ready && !self.snapshot["pending"].as_bool().unwrap_or(false)
    }
    fn edit(&mut self) {
        if self.writable() {
            self.write(json!({"turn":self.snapshot["workspace"]["turn"],"draft":{"text":self.draft,"fields":self.fields}}),false)
        }
    }
    fn submit(&mut self) {
        if !self.writable() {
            return;
        }
        if self.draft.starts_with('/') {
            let cmd = std::mem::take(&mut self.draft);
            self.edit();
            self.cmd(&cmd)
        } else {
            self.write(json!({"turn":self.snapshot["workspace"]["turn"],"submit":{"text":self.draft,"fields":self.fields}}),true);
        }
    }
    fn poll(&mut self) {
        if let Some(result) = self
            .course_result
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.course_result = None;
            match result {
                Ok(id) if !id.is_empty() => {
                    if let Err(e) = self.refresh(Some(id)) {
                        self.notice = e.to_string()
                    }
                }
                Err(e) => self.notice = e,
                _ => {}
            }
        }
        let mut disconnected = false;
        while let Some(tutor) = &self.tutor {
            let line = match tutor.output.try_recv() {
                Ok(l) => l,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
                Err(_) => break,
            };
            if let Some(frame) = line.strip_prefix('\x1e') {
                let Some((kind, body)) = frame.split_once(' ') else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(body) else {
                    self.notice = "Tutor returned an unreadable response.".into();
                    continue;
                };
                match kind {
                    "READY" => {
                        let start = v["history_start"].as_u64().unwrap_or(0) as usize;
                        if start == 0 {
                            self.history.clear()
                        }
                        for pair in v["history_delta"].as_array().into_iter().flatten() {
                            if let Some(w) = pair[0].as_str() {
                                self.history.push(("You".into(), w.into()));
                            }
                            self.history.push(("Luna".into(), s(&pair[1]).into()));
                        }
                        self.draft = s(&v["workspace"]["draft"]["text"]).to_string();
                        self.fields = v["workspace"]["draft"]["fields"]
                            .as_object()
                            .cloned()
                            .unwrap_or_default();
                        self.status = if v["pending"] == true {
                            "Message saved · retry to resume"
                        } else {
                            "Ready"
                        }
                        .into();
                        self.snapshot = v;
                        self.ready = true;
                    }
                    "PLAN" => self.plan = s(&v["text"]).into(),
                    "FEEDBACK" | "NOTICE" => self.notice = s(&v).into(),
                    "SUBMISSION" => {
                        if v["pending"] == false {
                            let _ = self.refresh(Some(self.key.clone()));
                        }
                        self.notice = if v["pending"] == true {
                            "Submission pending acknowledgment"
                        } else {
                            "Submission acknowledged"
                        }
                        .into()
                    }
                    _ => {}
                }
            } else if line.contains("already open")
                || line.contains("cannot be locked")
                || line.contains("Your message is saved")
            {
                self.status = line;
            }
        }
        if disconnected {
            self.tutor.take();
            self.ready = false;
            if !self.status.contains("locked") && !self.status.contains("already open") {
                self.status = "Tutor disconnected. Reopen the lesson to resume.".into();
            }
        }
    }
    fn courses(&mut self) {
        if self.runtime.testing {
            self.notice = "Classroom courses are available in My workspace.".into();
            return;
        }
        if self.course_result.is_some() {
            return;
        }
        let rt = self.runtime.clone();
        let (tx, rx) = mpsc::channel();
        self.course_result = Some(rx);
        thread::spawn(move || {
            let result = {
                let mut c = runtime::command(&rt.courses);
                if cfg!(target_os = "linux") {
                    c.arg(format!("http://127.0.0.1:{}/", rt.port));
                }
                c
            }
            .env("ROBOTUTOR_PORT", rt.port.to_string())
            .env("ROBOTUTOR_WEB_DATA", rt.state.join("course-webview"))
            .output()
            .map_err(|e| e.to_string())
            .and_then(|o| {
                if o.status.success() {
                    Ok(String::from_utf8_lossy(&o.stdout).trim().into())
                } else {
                    Err(String::from_utf8_lossy(&o.stderr).into_owned())
                }
            });
            let _ = tx.send(result);
        });
    }
    fn navigate(&mut self, page: Page) {
        self.notice.clear();
        if page == Page::Assignment {
            match Assignment::open(&self.folder) {
                Ok(a) => self.assignment = Some(a),
                Err(e) => {
                    self.notice = e.to_string();
                    return;
                }
            }
        }
        if page == Page::Reviews {
            if let Err(e) = self.tools.reviews(&self.folder) {
                self.notice = e.to_string();
            }
        }
        if page == Page::Inspector {
            if let Err(e) = self.inspect() {
                self.notice = e.to_string();
            }
        }
        self.page = page;
    }

    fn refresh_resources(&mut self) -> Result<()> {
        self.tools.resource_check = Some(std::time::Instant::now());
        self.tools.resources.clear();
        self.tools.resources = research::read_resources(&self.folder, &self.visit)?;
        Ok(())
    }
    fn inspect(&mut self) -> Result<()> {
        self.refresh_resources()?;
        let o = runtime::command(&self.runtime.tutor)
            .arg("--inspect-journal")
            .arg(self.folder.join(&self.visit))
            .arg(self.inspect_offset.to_string())
            .arg("128")
            .output()?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).into_owned().into());
        }
        for line in String::from_utf8(o.stdout)?.lines() {
            let v: Value = serde_json::from_str(line)?;
            self.inspect_offset = v["next_offset"]
                .as_u64()
                .ok_or("Missing journal position")?;
            self.tools.events.push(v);
        }
        Ok(())
    }
    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("mast")
            .frame(
                egui::Frame::new()
                    .fill(theme::NAVY)
                    .inner_margin(egui::Margin::symmetric(16, 9)),
            )
            .show(ctx, |ui| {
                let compact = ui.available_width() < 760.;
                ui.horizontal_wrapped(|ui| {
                    ui.image((self.logo.id(), egui::vec2(40., 40.)));
                    ui.label(
                        RichText::new("Robotutor")
                            .font(egui::FontId::new(
                                25.,
                                egui::FontFamily::Name("Nunito Bold".into()),
                            ))
                            .color(theme::SKY),
                    );
                    ui.add_space(14.);
                    for (p, title) in [
                        (Page::Lesson, "Lesson"),
                        (Page::Assignment, "Assignment"),
                        (Page::Courses, "Courses"),
                        (Page::Reviews, "Astra reviews"),
                    ] {
                        if compact && matches!(p, Page::Courses | Page::Reviews) {
                            continue;
                        }
                        let active = self.page == p;
                        if ui
                            .add(
                                egui::Button::new(RichText::new(title).color(if active {
                                    theme::NAVY
                                } else {
                                    theme::SKY
                                }))
                                .fill(if active {
                                    theme::SKY
                                } else {
                                    theme::NAVY
                                }),
                            )
                            .clicked()
                        {
                            self.navigate(p);
                        }
                    }
                    ui.menu_button(if compact { "More" } else { "Workspace" }, |ui| {
                        if compact {
                            for (p, label) in
                                [(Page::Courses, "Courses"), (Page::Reviews, "Astra reviews")]
                            {
                                if ui.button(label).clicked() {
                                    ui.close();
                                    self.navigate(p);
                                }
                            }
                            ui.separator();
                        }
                        for (name, label) in [
                            ("", "My workspace"),
                            ("new", "Brand new"),
                            ("returning", "Returning"),
                            ("known", "Well known"),
                        ] {
                            if ui.selectable_label(self.profile == name, label).clicked() {
                                ui.close();
                                if let Err(e) = self.switch_profile(name, false) {
                                    self.notice = e.to_string();
                                }
                            }
                        }
                        if !self.profile.is_empty() && ui.button("Reset test learner").clicked() {
                            ui.close();
                            if let Err(e) = self.switch_profile(&self.profile.clone(), true) {
                                self.notice = e.to_string();
                            }
                        }
                    });
                });
            });
    }
    fn switch_profile(&mut self, name: &str, reset: bool) -> Result<()> {
        let rt = if name.is_empty() {
            self.personal.clone()
        } else {
            self.personal.profile(name, reset)?
        };

        self.research.close();
        self.tutor.take();
        self.key.clear();
        self.runtime = rt;
        self.profile = name.into();
        self.course_result = None;
        self.refresh(if matches!(name, "returning" | "known") {
            Some("current.jsonl".into())
        } else {
            None
        })
    }
    fn lesson(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let title = s(&self.snapshot["lesson_title"]);
            ui.heading(if title.is_empty() {
                "Getting to know you"
            } else {
                title
            });
            let mut key = self.key.clone();
            egui::ComboBox::from_id_salt("visits")
                .selected_text(
                    self.choices
                        .iter()
                        .find(|c| c.0 == key)
                        .map(|c| c.1.as_str())
                        .unwrap_or("Visit"),
                )
                .show_ui(ui, |ui| {
                    for (val, label) in &self.choices {
                        ui.selectable_value(&mut key, val.clone(), label);
                    }
                });
            if key != self.key {
                if let Err(e) = self.open(key) {
                    self.notice = e.to_string();
                }
            }
            if ui.button("Refresh").clicked() {
                if let Err(e) = self.refresh(Some(self.key.clone())) {
                    self.notice = e.to_string()
                }
            }

            if ui.button("Research browser").clicked() {
                if let Err(error) = self
                    .research
                    .launch(&self.runtime, &self.folder, &self.visit)
                {
                    self.notice = error.to_string();
                }
            }
            if ui.button("Backend explorer").clicked() {
                self.navigate(Page::Inspector);
            }
        });
        let question = s(&self.snapshot["exercise"]).to_string();
        let hist_h = (ui.ctx().available_rect().height() * 0.31).clamp(95., 320.);
        let history = preceding_turns(&self.history, &question);
        if !history.is_empty() || self.snapshot["pending_text"].is_string() {
            theme::card(theme::PANEL).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("conversation")
                    .max_height(hist_h)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for (who, text) in history {
                            ui.label(RichText::new(who).strong());
                            if who == "Luna" {
                                theme::tutor_text(ui, text, theme::NAVY, 16.);
                            } else {
                                ui.label(text);
                            }
                            ui.add_space(4.);
                        }
                        if let Some(words) = self.snapshot["pending_text"].as_str() {
                            ui.strong("You");
                            ui.label(words);
                        }
                    });
            });
        }
        ui.add_space(4.);
        if !question.is_empty() {
            theme::card(theme::NAVY).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new("Current question")
                        .small()
                        .color(theme::AMBER),
                );
                theme::tutor_text(ui, &question, theme::SKY, 19.);
            });
        }
        let choices = self.snapshot["choices"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ui.add_enabled_ui(self.writable() && self.draft.is_empty(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for c in choices {
                    if ui.button(s(&c)).clicked() {
                        self.draft = s(&c).into();
                        self.submit();
                    }
                }
            })
        });
        let form = self.snapshot["workspace"]["form"].clone();
        ui.add_enabled_ui(self.writable(), |ui| {
            if !form.is_null() {
                let (edit, cmd) = practice::editor(
                    ui,
                    &form,
                    &mut self.fields,
                    &self.snapshot["workspace"]["draft"],
                    &mut self.point,
                );
                if edit {
                    self.edit()
                }
                if let Some(cmd) = cmd {
                    self.cmd(&cmd)
                }
            }
            let id = ui.make_persistent_id("reply");
            let send_enter = ui.memory(|m| m.has_focus(id))
                && ui.input_mut(|i| {
                    let index = i.events.iter().position(|e| matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.is_none()));
                    if let Some(index) = index { i.events.remove(index); true } else { false }
                });
            let reply = ui.add(
                egui::TextEdit::multiline(&mut self.draft)
                    .id(id)
                    .hint_text("Reply to Luna…")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            if reply.changed() {
                self.edit();
            }
            ui.horizontal_wrapped(|ui| {
                let can = !self.draft.trim().is_empty() || !form.is_null();
                if ui
                    .add_enabled_ui(can, |ui| theme::action(ui, "Send"))
                    .inner
                    .clicked()
                    || send_enter && can
                {
                    self.submit();
                }
                ui.small("Enter to send · Shift+Enter for a new line");
            });
        });
        ui.horizontal_wrapped(|ui| {
            if self.ready && self.snapshot["pending"] == true && ui.button("Retry").clicked() {
                self.cmd("/retry");
            }
            if ui
                .add_enabled(self.ready, egui::Button::new("Plan"))
                .clicked()
            {
                self.cmd("/plan");
            }
            ui.menu_button("Actions", |ui| {
                for (title, cmd) in [
                    ("My skills", "/skills"),
                    ("Teaching trace", "/trace"),
                    ("Resume", "/continue"),
                    ("Graph", "/activity graph"),
                    ("Build a table", "/activity table"),
                    ("Order steps", "/activity order"),
                    ("Repair a formula", "/activity repair"),
                    ("More practice formats", "/activities"),
                    ("Correct an interpretation", "/amend "),
                    ("Import text document", "/import "),
                ] {
                    if ui.button(title).clicked() {
                        ui.close();
                        if cmd.ends_with(' ') {
                            self.draft = cmd.into();
                            self.edit();
                        } else {
                            self.cmd(cmd);
                        }
                    }
                }
            });
            if self.key.parse::<u64>().is_ok() {
                self.submission_actions(ui);
            }
            ui.small(&self.status);
        });
        ui.collapsing("Why this step", |ui| {
            let trace = s(&self.snapshot["workspace"]["trace"]);
            ui.label(if trace.is_empty() {
                s(&self.snapshot["discovery_reason"])
            } else {
                trace
            });
        });
        if !self.plan.is_empty() {
            ui.separator();
            ui.label(&self.plan);
        }
    }
    fn submission_actions(&mut self, ui: &mut Ui) {
        let a = self.classroom.clone();
        let status = s(&a["status"]);
        let pending = self.folder.join("pending.json").exists();
        if pending {
            if ui.button("Retry submission").clicked() {
                self.cmd("/submit");
            }
        } else if !matches!(status, "Accepted" | "Awaiting review")
            && ui.button("Preview submission").clicked()
        {
            let mut parts = Vec::new();
            for f in self.snapshot["workspace"]["form"]["fields"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(v) = self
                    .fields
                    .get(s(&f["id"]))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    parts.push(format!("{}: {v}", s(&f["label"])));
                }
            }
            if !self.draft.is_empty() {
                parts.push(self.draft.clone());
            }
            self.preview = parts.join("\n");
            self.page = Page::Preview;
        }
        if status == "Revision requested" && ui.button("Revise reviewed work").clicked() {
            if let Some(last) = a["submissions"].as_array().and_then(|v| v.last()) {
                self.draft = s(&last["work"]).into();
                self.edit();
            }
        }
        ui.collapsing("Assignment details", |ui| {
            ui.label(s(&a["activity"]["instructions"]));
            ui.label(s(&a["activity"]["criterion"]));
            ui.label(s(&a["feedback"]));
            ui.label(status);
        });
    }
}
impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.research.finished_with_error() {
            self.notice =
                "The research browser closed unexpectedly. Reopen it from this lesson.".into();
        }

        if self.page == Page::Inspector
            && self
                .tools
                .resource_check
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
        {
            if let Err(error) = self.refresh_resources() {
                self.notice = error.to_string();
            }
        }
        while let Ok(request) = self.instance.requests.try_recv() {
            let profile = s(&request["profile"]);
            let reset = request["reset"].as_bool().unwrap_or(false);
            if profile != self.profile || reset {
                if let Err(e) = self.switch_profile(profile, reset) {
                    self.notice = e.to_string();
                    continue;
                }
            }
            if let Some(key) = request["key"].as_str() {
                if let Err(e) = self.refresh(Some(key.into())) {
                    self.notice = e.to_string();
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.poll();
        self.header(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::SKY).inner_margin(18))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("page", self.page as u8))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        match self.page {
                            Page::Lesson => self.lesson(ui),
                            Page::Assignment => {
                                if let Some(a) = &mut self.assignment {
                                    a.ui(ui, s(&self.snapshot["working_plan"]["subject"]));
                                }
                            }
                            Page::Courses => {
                                ui.heading("Courses");
                                if ui
                                    .add_enabled(
                                        !self.runtime.testing && self.course_result.is_none(),
                                        egui::Button::new("Open classroom courses"),
                                    )
                                    .clicked()
                                {
                                    self.courses();
                                }
                                if let Err(e) = self.personal_courses(ui) {
                                    self.notice = e.to_string();
                                }
                            }
                            Page::Reviews => {
                                if let Err(e) = self.review_ui(ui) {
                                    self.notice = e.to_string();
                                }
                            }
                            Page::Inspector => {
                                ui.heading("Backend explorer");
                                if ui.button("Load more events").clicked() {
                                    if let Err(e) = self.inspect() {
                                        self.notice = e.to_string()
                                    }
                                }
                                self.tools.inspector(ui);
                            }
                            Page::Preview => {
                                ui.heading("Preview submission");
                                ui.label("Only this text will be shared with your teacher.");
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.preview)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(8),
                                );
                                ui.horizontal(|ui| {
                                    if !self.preview.trim().is_empty()
                                        && theme::action(ui, "Submit this text").clicked()
                                    {
                                        self.cmd(&format!("/confirm-preview {}", self.preview));
                                        self.page = Page::Lesson;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        self.page = Page::Lesson;
                                    }
                                });
                            }
                        }
                        if !self.notice.is_empty() {
                            ui.separator();
                            ui.label(&self.notice);
                        }
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}
struct StartupError(String);
impl eframe::App for StartupError {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Robotutor could not open");
            ui.label(&self.0);
        });
    }
}
fn main() -> eframe::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("robotutor-desktop {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cfg!(windows) && env::var_os("ROBOTUTOR_PYTHON").is_none() {
        let msys = env::var_os("MSYS2_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\msys64"));
        let python = msys.join("ucrt64/bin/python.exe");
        if python.is_file() {
            env::set_var("ROBOTUTOR_PYTHON", python);
        }
    }
    let startup = (|| -> Result<_> {
        let mut args = env::args().skip(1);
        let mut rebuild = false;
        let mut reset = false;
        let mut profile = String::new();
        let mut key = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--rebuild" => rebuild = true,
                "--reset" => reset = true,
                "--test" => profile = args.next().ok_or("Missing test profile")?,
                k if !k.starts_with('-') && key.is_none() => {
                    key = Some(
                        k.strip_prefix("robotutor://assignment/")
                            .unwrap_or(k)
                            .to_string(),
                    )
                }
                _ => return Err(format!("Unknown argument {arg}").into()),
            }
        }
        if reset && profile.is_empty() {
            return Err("Reset requires a test learner.".into());
        }
        let rt = Runtime::open(rebuild)?;
        let instance = instance::Instance::open(
            &rt.personal,
            json!({"profile":profile,"reset":reset,"key":key}),
        )?;
        if key.is_none() && matches!(profile.as_str(), "returning" | "known") {
            key = Some("current.jsonl".into());
        }
        Ok(instance.map(|i| (rt, key, profile, reset, i)))
    })();
    if matches!(startup, Ok(None)) {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080., 740.])
            .with_min_inner_size([520., 400.])
            .with_icon(theme::logo()),
        ..Default::default()
    };
    eframe::run_native(
        "Robotutor",
        options,
        Box::new(|cc| {
            let result = startup.and_then(|s| {
                let (rt, key, profile, reset, instance) = s.ok_or("Window already open")?;
                Desktop::new(&cc.egui_ctx, rt, key, profile, reset, instance)
            });
            Ok(match result {
                Ok(app) => Box::new(app) as Box<dyn eframe::App>,
                Err(e) => {
                    let _logo = theme::setup(&cc.egui_ctx);
                    Box::new(StartupError(e.to_string()))
                }
            })
        }),
    )
}

#[cfg(test)]
mod lesson_tests {
    use super::preceding_turns;

    #[test]
    fn only_the_active_tutor_prompt_is_removed_from_the_transcript_view() {
        let history = [
            ("Luna".into(), "Pick a puzzle".into()),
            ("You".into(), "A pattern".into()),
            ("Luna".into(), "What comes next?".into()),
        ];
        assert_eq!(preceding_turns(&history, "What comes next?"), &history[..2]);
        assert_eq!(preceding_turns(&history[..2], "A pattern"), &history[..2]);
        assert!(preceding_turns(&history[..1], "Pick a puzzle").is_empty());
        assert_eq!(preceding_turns(&history, "A new question"), &history);
    }
}
