//! The course spans visits. A focus marks what is being taught, independently of grades.
use crate::lesson::{object, Generate, Lesson};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Milestone {
    pub skill: String,
    pub application: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Course {
    pub goal: String,
    pub destination: String,
    pub milestones: Vec<Milestone>,
    pub focus: usize,
    pub turns: usize,
}

#[derive(Deserialize)]
struct Proposal {
    destination: String,
    milestones: Vec<Milestone>,
    focus: usize,
}

pub fn working_plan(lesson: &Lesson) -> Option<Value> {
    let mut plan = lesson
        .discovery
        .as_ref()
        .map(|d| d.working_plan())
        .or_else(|| {
            lesson
                .declared_goal
                .as_ref()
                .map(|goal| json!({"subject":goal,"phase":"instruction","observations":[]}))
        })?;
    plan["current_objective"] = json!(lesson.objective);
    plan["route"] = json!(lesson.course);
    plan["teaching_step"] = json!(lesson.rationale);
    if let Some(e) = &lesson.practice_evidence {
        plan["practice"] = json!({"source":e.source,"feedback":e.feedback,"action":e.decision,"assisted":e.earlier_help || e.work.helped});
    }
    Some(plan)
}

pub fn agreed_seed(lesson: &Lesson) -> Option<Value> {
    lesson.discovery.as_ref().filter(|d| !d.active())?;
    let goal = lesson.declared_goal.as_ref()?;
    Some(
        json!({"goal":goal, "starting_objective":lesson.objective, "preferences":lesson.preferences,
                "course":lesson.course, "outline":lesson.outline, "working_plan":working_plan(lesson)}),
    )
}

pub fn prepare_private(folder: &std::path::Path, seed: &Value) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let packaged = std::env::current_exe()?.with_file_name("course_agent.py");
    let worker = if packaged.is_file() {
        packaged
    } else {
        tutor_lab::platform::project_root().join("desktop/course_agent.py")
    };
    let mut child = Command::new(tutor_lab::platform::python())
        .arg(worker)
        .arg("--personal")
        .arg(folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(seed.to_string().as_bytes())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

impl Course {
    pub fn create(
        goal: &str,
        lesson: &Lesson,
        demonstrated: &[&str],
        generate: &mut Generate,
    ) -> Result<Option<Self>, String> {
        let string = json!({"type":"string"});
        let milestone = object(json!({"skill":string,"application":string}));
        let schema = object(
            json!({"destination":string,"milestones":{"type":"array","items":milestone,"minItems":4,"maxItems":7},"focus":{"type":"integer","minimum":0,"maximum":6}}),
        );
        let mut packet = json!({"task":"course_plan","goal":goal,"current_objective":lesson.objective,
            "suggested_followup":lesson.next_objective,"earlier_outline":lesson.outline,"demonstrated":demonstrated});
        if goal.split(|c: char| !c.is_alphabetic()).any(|w| {
            matches!(
                w.to_ascii_lowercase().as_str(),
                "law" | "legal" | "lawyer" | "jurisprudence"
            )
        }) {
            let survey: Value =
                serde_json::from_str(include_str!("../../../fixtures/law-survey.json"))
                    .map_err(|e| e.to_string())?;
            packet["available_survey_areas"] = survey["facets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| json!({"area":f["name"],"performance":f["performance"]}))
                .collect();
        }
        for _ in 0..2 {
            if let Ok(p) = serde_json::from_value::<Proposal>(generate(&packet, &schema)?) {
                let valid = |s: &str| !s.trim().is_empty() && s.len() <= 800;
                let mut skills = HashSet::new();
                if valid(&p.destination)
                    && (4..=7).contains(&p.milestones.len())
                    && p.focus < p.milestones.len()
                    && p.milestones.iter().all(|m| {
                        valid(&m.skill)
                            && valid(&m.application)
                            && skills.insert(m.skill.trim().to_lowercase())
                    })
                {
                    return Ok(Some(Self {
                        goal: goal.to_owned(),
                        destination: p.destination,
                        milestones: p.milestones,
                        focus: p.focus,
                        turns: 0,
                    }));
                }
            }
            packet["repair"] = json!("Provide a concrete destination, four to seven distinct capabilities with applications, and a zero-based focus in that list.");
        }
        Ok(None)
    }

    pub fn current(&self) -> Option<&Milestone> {
        self.milestones.get(self.focus)
    }

    pub fn focus_on(&mut self, skill: &str, application: &str, retain_route: bool) {
        self.focus = if let Some(index) = self
            .milestones
            .iter()
            .position(|m| m.skill.eq_ignore_ascii_case(skill))
        {
            self.milestones[index].application = application.into();
            index
        } else {
            let end = if retain_route {
                self.milestones.len()
            } else {
                self.milestones.len().saturating_sub(1)
            };
            let index = self.focus.saturating_add(1).min(end);
            let next = Milestone {
                skill: skill.into(),
                application: application.into(),
            };
            if retain_route {
                self.milestones.insert(index, next);
            } else if self.milestones.is_empty() {
                self.milestones.push(next);
            } else {
                self.milestones[index] = next;
            }
            index
        };
        self.turns = 0;
    }
    pub fn navigate(&mut self, action: &str) {
        if matches!(action, "next" | "harder")
            || self.turns >= 2 && matches!(action, "complete" | "discussion")
        {
            let next = self.focus.saturating_add(1);
            if next < self.milestones.len() {
                self.focus = next;
                self.turns = 0;
            }
        } else if action == "revisit" {
            self.focus = self.focus.saturating_sub(1);
            self.turns = 0;
        }
    }
}
