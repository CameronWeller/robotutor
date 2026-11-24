use super::judgment::{Finding, Receipt};
use super::lesson::Lesson;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use tutor_lab::platform::PrivateOpen;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceRef {
    pub source: String,
    pub digest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Open {
        version: u8,
        selected: Option<String>,
    },
    /// Visible turns recovered from a legacy terminal, plus the state for continuing it.
    /// Earlier routing decisions were never recorded by that terminal.
    Recovered {
        exchanges: Vec<(Option<String>, String)>,
        continuation: Box<Lesson>,
    },
    RestoredGoal {
        goal: String,
    },
    SupervisorPlan {
        #[serde(default)]
        replace_prompt: bool,
        revision: i64,
        objective: String,
        task: String,
        criterion: String,
    },
    Planned {
        topic: String,
        outline: Vec<String>,
    },
    Continued {
        from: SourceRef,
        lesson: Box<Lesson>,
    },
    SubmissionPreview {
        preview: Option<crate::assignment::Preview>,
    },
    WorkspaceDraft {
        turn: usize,
        draft: crate::practice::Draft,
    },
    PracticeAnswer {
        turn: usize,
        work: crate::practice::Draft,
        text: String,
    },
    Timing {
        sample: crate::metrics::Timing,
    },
    Learner {
        text: String,
    },
    ResumedInterrupted {
        text: String,
    },
    Delivered {
        text: String,
        lesson: Box<Lesson>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sources: Vec<SourceRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        judgment: Option<Box<Receipt>>,
    },
    /// Diagnostic record. Drafts and reviewer comments never enter delivered history.
    Review {
        draft: serde_json::Value,
        verdict: serde_json::Value,
    },
    TurnStep {
        step: crate::recovery::Step,
    },
    Rejected {
        draft: serde_json::Value,
        reason: String,
    },
    DiscoveryFeedback {
        at: u64,
        text: String,
    },
    Pilot {
        at: u64,
        note: super::pilot::Note,
    },
    Failed {
        message: String,
    },
}
#[derive(Default)]
pub struct Visit {
    pub preview: Option<crate::assignment::Preview>,
    pub draft: crate::practice::Draft,
    // Rebuild suspended subjects from committed switches, including the draft before the command.
    pub subjects: std::collections::BTreeMap<String, (Lesson, crate::practice::Draft, usize)>,
    pub restored_turn: Option<usize>,
    pub submitted_work: Option<crate::practice::Draft>,
    pub practice_corrections: Vec<crate::practice::Correction>,
    pub practice_history: Vec<crate::practice::Evidence>,
    pub timings: Vec<crate::metrics::Timing>,
    pub discovery_feedback: Vec<(u64, String)>,
    pub discovery_trace: std::collections::BTreeMap<usize, String>,
    pub assessments: std::collections::BTreeMap<String, crate::study::Observation>,
    pub supervisor_revision: i64,
    pub from: Option<SourceRef>,
    pub history: Vec<(Option<String>, String)>,
    delivery_hashes: std::collections::BTreeMap<String, String>,
    pub recent_questions: Vec<(String, String)>,
    pub last_delivery_at: u64,
    pub last_plan_at: Option<u64>,
    pub notes: Vec<(u64, super::pilot::Note)>,
    pub decisions: Vec<Option<Box<Receipt>>>,
    pub lesson: Lesson,
    pub pending: Option<String>,
    pub checkpoints: crate::recovery::Checkpoints,
    interrupted: Option<(String, Lesson)>,
    resume_history_len: Option<usize>,
}
impl Visit {
    pub fn interrupted_input(&self) -> Option<&str> {
        self.interrupted.as_ref().map(|(text, _)| text.as_str())
    }
    pub fn teaching_history(&self) -> &[(Option<String>, String)] {
        &self.history[..self.resume_history_len.unwrap_or(self.history.len())]
    }
    fn remember_assessment(&mut self, lesson: &Lesson) {
        if let Some(o) = lesson.observation.as_ref().filter(|o| o.verified()) {
            self.assessments.insert(o.target.key(), o.clone());
        }
    }
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.decisions
            .iter()
            .flatten()
            .filter(|r| !r.inputs.shadow)
            .flat_map(|r| &r.decision.findings)
    }

    pub fn apply(&mut self, event: Event) -> Result<(), String> {
        self.commit(event, 0, |_| Ok(()))
    }

    /// Validate against the current visit, persist the event, then advance memory.
    /// An exact delivery replay is already committed and needs no extra journal row.
    pub fn commit(
        &mut self,
        event: Event,
        at: u64,
        persist: impl FnOnce(&Event) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Event::Delivered { lesson, .. } = &event {
            crate::practice::verify_delivery(self, lesson)?;
        }
        let receipt_hash = match &event {
            Event::PracticeAnswer { turn, work, text }
                if *turn != self.history.len()
                    || self.pending.is_some()
                    || crate::practice::recorded_submission(self)?.as_ref()
                        != Some(&(work.clone(), text.clone())) =>
            {
                return Err("Stale or inconsistent practice submission.".into())
            }
            Event::WorkspaceDraft { turn, .. }
                if *turn != self.history.len() || self.pending.is_some() =>
            {
                return Err("This draft belongs to another turn.".into())
            }
            Event::Open { version, .. } if *version != 1 => {
                return Err("Unsupported session version.".into())
            }
            Event::Recovered { exchanges, .. }
                if !self.history.is_empty() || self.pending.is_some() || exchanges.is_empty() =>
            {
                return Err("Recovered turns must begin a visit.".into())
            }
            Event::ResumedInterrupted { text }
                if self.interrupted_input() != Some(text.as_str()) =>
            {
                return Err("No interrupted input matches this recovery.".into())
            }
            Event::Delivered {
                judgment: Some(receipt),
                lesson,
                text,
                ..
            } => {
                let hash = delivery_hash(receipt, lesson, text);
                if let Some(prior) = self.delivery_hashes.get(&receipt.inputs.source) {
                    return if prior == &hash {
                        Ok(())
                    } else {
                        Err("Conflicting duplicate decision.".into())
                    };
                }
                let (session, ordinal) = receipt
                    .inputs
                    .source
                    .rsplit_once('#')
                    .ok_or("Invalid decision source.")?;
                if ordinal.parse::<usize>().ok() != Some(self.history.len() + 1)
                    || self
                        .delivery_hashes
                        .keys()
                        .next()
                        .is_some_and(|id| id.rsplit_once('#').map(|(s, _)| s) != Some(session))
                {
                    return Err("Conflicting or out-of-order decision.".into());
                }
                receipt.verify(self, lesson, text)?;
                Some(hash)
            }
            _ => None,
        };
        persist(&event)?;
        if let Event::Learner { text } = &event {
            if crate::lesson::switch_destination(text).is_some() {
                if let Some(current) = self
                    .lesson
                    .declared_goal
                    .as_deref()
                    .or_else(|| self.lesson.discovery.as_ref().map(|d| d.subject.as_str()))
                    .filter(|g| !g.is_empty())
                {
                    let mut draft = self.draft.clone();
                    if draft.text.trim() == text.trim() {
                        draft.text.clear();
                    }
                    self.subjects.insert(
                        current.trim().to_lowercase(),
                        (
                            self.lesson.clone(),
                            draft,
                            self.restored_turn.unwrap_or(self.history.len()),
                        ),
                    );
                }
            }
        }
        if matches!(
            &event,
            Event::Learner { .. }
                | Event::ResumedInterrupted { .. }
                | Event::SupervisorPlan { .. }
                | Event::Continued { .. }
                | Event::RestoredGoal { .. }
        ) {
            self.checkpoints = Default::default();
        }
        if matches!(
            &event,
            Event::Learner { .. }
                | Event::SupervisorPlan { .. }
                | Event::Delivered { .. }
                | Event::PracticeAnswer { .. }
        ) {
            self.draft = Default::default();
            self.submitted_work = None;
        }
        match event {
            Event::SubmissionPreview { preview } => self.preview = preview,
            Event::WorkspaceDraft { draft, .. } => self.draft = draft,
            Event::Timing { sample } => self.timings.push(sample),
            Event::PracticeAnswer { work, text, .. } => {
                self.checkpoints = Default::default();
                self.interrupted = None;
                self.pending = Some(text);
                self.submitted_work = Some(work);
            }
            Event::Open { selected, .. } => {
                self.lesson.declared_goal = selected.clone();
                self.lesson.selected = selected;
            }
            Event::Recovered {
                exchanges,
                continuation,
            } => {
                self.decisions.resize_with(exchanges.len(), || None);
                self.history = exchanges;
                self.last_delivery_at = at;
                self.last_plan_at = None;
                self.remember_assessment(&continuation);
                self.lesson = *continuation;
            }
            // Old versions let viewing /plan write a proposed goal. These entries have
            // no learner authorization and do not supersede delivered lesson state.
            Event::RestoredGoal { goal } => {
                self.lesson.topic = goal.clone();
                self.lesson.declared_goal = Some(goal);
            }
            Event::SupervisorPlan {
                replace_prompt,
                revision,
                objective,
                task,
                criterion,
            } => {
                self.interrupted = None;
                self.last_plan_at = Some(at);
                self.supervisor_revision = revision;
                self.lesson.open_supervised(objective, task, criterion);
                if replace_prompt {
                    self.lesson.defer_open_recall();
                }
            }
            Event::Planned { .. } => {}
            Event::Continued { lesson, from } => {
                self.remember_assessment(&lesson);
                self.lesson = *lesson;
                self.from = Some(from);
            }
            Event::Learner { text } => {
                self.interrupted =
                    if text.starts_with('\x1b') && text.trim_start_matches('\x1b') == "/retry" {
                        self.pending
                            .take()
                            .map(|original| (original, self.lesson.clone()))
                    } else {
                        None
                    };
                self.resume_history_len = None;
                self.pending = Some(text);
            }
            Event::ResumedInterrupted { .. } => {
                let (original, mut lesson) = self
                    .interrupted
                    .take()
                    .expect("validated interrupted input");
                if lesson.course.is_none() {
                    lesson.course = self
                        .lesson
                        .course
                        .clone()
                        .filter(|c| Some(&c.goal) == lesson.declared_goal.as_ref());
                }
                let delivered_detour = self.pending.is_none();
                self.resume_history_len = Some(
                    self.history
                        .len()
                        .saturating_sub(usize::from(delivered_detour)),
                );
                self.pending = Some(original);
                self.lesson = lesson;
            }
            Event::Delivered {
                text,
                mut lesson,
                judgment,
                ..
            } => {
                if let (Some(receipt), Some(hash)) = (&judgment, receipt_hash) {
                    self.delivery_hashes
                        .insert(receipt.inputs.source.clone(), hash);
                }
                self.checkpoints = Default::default();
                if judgment.is_none() {
                    if let Some(d) = &lesson.discovery {
                        self.discovery_trace
                            .insert(self.history.len() + 1, d.reason.clone());
                    }
                }
                if let Some(e) = &lesson.practice_evidence {
                    if e.source
                        .rsplit_once('#')
                        .and_then(|(_, n)| n.parse::<usize>().ok())
                        == Some(self.history.len() + 1)
                    {
                        if e.decision == "provisional"
                            || (e.decision == "support"
                                && lesson
                                    .activity
                                    .as_ref()
                                    .is_some_and(|r| r.public()["id"] == e.activity))
                        {
                            self.draft = e.work.clone();
                        } else if lesson
                            .activity
                            .as_ref()
                            .is_some_and(|r| r.public()["id"] == "revision")
                        {
                            if let Some(original) = e.work.fields.get("artifact") {
                                self.draft
                                    .fields
                                    .insert("artifact".into(), original.clone());
                            }
                        }
                        self.practice_history.push(e.clone());
                        self.discovery_trace
                            .insert(self.history.len() + 1, e.feedback.clone());
                    }
                }
                if let Some(c) = &lesson.practice_correction {
                    if self
                        .pending
                        .as_deref()
                        .is_some_and(|s| s.starts_with("/amend "))
                    {
                        self.practice_corrections.push(c.clone());
                        self.discovery_trace.insert(
                            self.history.len() + 1,
                            format!("You corrected {} · {}", c.source, c.words),
                        );
                    }
                }
                if let Some(selection) = &lesson.prepared_selection {
                    self.discovery_trace.insert(
                        self.history.len() + 1,
                        format!("“{}” · {}", selection.quote, selection.reason),
                    );
                }
                if let Some(r) = &judgment {
                    lesson.rationale = r.decision.rationale();
                }
                self.decisions.push(judgment);
                self.restored_turn = self
                    .pending
                    .as_deref()
                    .and_then(crate::lesson::switch_destination)
                    .and_then(|g| self.subjects.get(&g.trim().to_lowercase()))
                    .filter(|(_, _, turn)| lesson.resumed_from_turn == Some(*turn))
                    .map(|(_, draft, turn)| {
                        self.draft = draft.clone();
                        *turn
                    });
                if let Some(draft) = lesson.return_draft.take() {
                    self.draft = draft;
                } else if self.restored_turn.is_none() {
                    lesson.recover_metadata(Some(&self.lesson), self.pending.as_deref());
                }
                self.remember_assessment(&lesson);
                let visible = text.rsplit("\n\n").next().unwrap_or(&text);
                self.recent_questions.push((
                    lesson.objective.clone(),
                    visible.chars().take(900).collect(),
                ));
                if self.recent_questions.len() > 6 {
                    self.recent_questions.remove(0);
                }
                self.history.push((self.pending.take(), text));
                self.last_delivery_at = at;
                self.last_plan_at = None;
                self.lesson = *lesson;
                self.resume_history_len = None;
            }
            Event::DiscoveryFeedback { at, text } => self.discovery_feedback.push((at, text)),
            Event::Pilot { at, note } => self.notes.push((at, note)),
            Event::TurnStep { step } => self.checkpoints.insert(step),
            Event::Failed { message } => {
                if !crate::recovery::interrupted(&message) {
                    self.checkpoints.retry_after_failure();
                }
            }
            Event::Rejected { .. } | Event::Review { .. } => {}
        }
        Ok(())
    }
}

pub fn delivery_hash(receipt: &Receipt, lesson: &Lesson, text: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(receipt, lesson, text)).expect("journal values serialize")
        )
    )
}

fn read_committed(file: impl Read, visit: &mut Visit) -> Result<u64, String> {
    let mut reader = BufReader::new(file);
    let mut offset = 0;
    let mut line = Vec::new();
    loop {
        line.clear();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(offset);
        }
        if !line.ends_with(b"\n") {
            if offset == 0 {
                return Err("The file does not contain a complete session header.".into());
            }
            return Ok(offset);
        }
        let row: serde_json::Value =
            serde_json::from_slice(&line).map_err(|_| "Invalid event in the session file.")?;
        let at = row["recorded_at"].as_u64().unwrap_or_default();
        let event: Event =
            serde_json::from_value(row).map_err(|_| "Invalid event in the session file.")?;
        if (offset == 0) != matches!(event, Event::Open { .. }) {
            return Err("Invalid session header.".into());
        }
        visit.commit(event, at, |_| Ok(()))?;
        offset += count as u64;
    }
}

pub struct Journal {
    pub path: std::path::PathBuf,
    pub workspace_mode: bool,
    pub assignment: Option<serde_json::Value>,
    file: File,
    #[cfg(windows)]
    _lock: File,
    write_failed: bool,
    has_header: bool,
}
impl Journal {
    pub fn open(path: &Path, visit: &mut Visit) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.append(true);
        let file = options
            .open_private(path)
            .map_err(|e| format!("Cannot open the session file: {e}"))?;
        #[cfg(windows)]
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open_private(path.with_extension("jsonl.lock"))
            .map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let locking_file = &lock;
        #[cfg(unix)]
        let locking_file = &file;
        locking_file
            .try_lock()
            .map_err(|_| "This session is already open or cannot be locked.")?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        // The assignment directory contains the pinned context used by either frontend.
        let assignment = match std::fs::read(path.with_extension("assignment.json")) {
            Ok(data) => {
                let bound: serde_json::Value =
                    serde_json::from_slice(&data).map_err(|e| e.to_string())?;
                if !bound["activity"].is_object() || !bound["version"].is_number() {
                    return Err("Invalid assignment binding.".into());
                }
                Some(bound["activity"].clone())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.to_string()),
        };
        let end = read_committed(&file, visit)?;
        if end != meta.len() {
            file.set_len(end).map_err(|e| e.to_string())?;
        }
        #[cfg(windows)]
        {
            use std::io::{Seek, SeekFrom};
            // The writer lock serializes appends. GENERIC_WRITE also permits crash-tail truncation.
            (&file).seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            workspace_mode: false,
            assignment,
            file,
            #[cfg(windows)]
            _lock: lock,
            write_failed: false,
            has_header: end > 0,
        })
    }

    pub fn is_empty(&self) -> Result<bool, String> {
        self.file
            .metadata()
            .map(|m| m.len() == 0)
            .map_err(|e| e.to_string())
    }

    pub fn save(&mut self, event: &Event) -> Result<(), String> {
        self.save_at(event, crate::session_time()?)
    }
    pub fn save_at(&mut self, event: &Event, at: u64) -> Result<(), String> {
        if self.write_failed {
            return Err(
                "Cannot save the session. Reopen it to recover the last committed event.".into(),
            );
        }
        if self.has_header == matches!(event, Event::Open { .. }) {
            return Err("Invalid session header.".into());
        }
        let mut record = serde_json::to_value(event).map_err(|e| e.to_string())?;
        record["recorded_at"] = at.into();
        let mut bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .and_then(|()| self.file.sync_data())
            .map_err(|e| {
                self.write_failed = true;
                format!("Cannot save the session: {e}")
            })?;
        self.has_header = true;
        Ok(())
    }
}

/// Inspect a private journal without creating it, truncating it, or loading credentials.
pub fn replay(path: &Path) -> Result<Visit, String> {
    replay_prefix(path, u64::MAX)
}

pub fn replay_prefix(path: &Path, limit: u64) -> Result<Visit, String> {
    let file = OpenOptions::new()
        .read(true)
        .open_private(path)
        .map_err(|e| e.to_string())?;
    let mut visit = Visit::default();
    if read_committed(file.take(limit), &mut visit)? == 0 {
        return Err("Missing session header.".into());
    }
    Ok(visit)
}
