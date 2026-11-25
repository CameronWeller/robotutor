use super::assessment::{Rubric, Verdict};
use super::material;
use super::study::{Observation, Target};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub type Generate<'a> = dyn FnMut(&Value, &Value) -> Result<Value, String> + 'a;
const ACTIONS: &[&str] = &[
    "complete",
    "partial",
    "repair",
    "next",
    "harder",
    "question",
    "acknowledge",
    "support",
    "pause",
    "explain_only",
    "use_source_or_topic",
    "revisit",
    "start",
    "preference",
    "resume_practice",
];
const ROUTING: &str = "Pick the learner's intent. For next, harder or revisit, set target_stage to a listed stage number only if the learner specifically requests its skill; otherwise set it to -1. For a question use the immediately preceding exchange to resolve referents; do not turn a request for definitions into another quiz. If a learner wants to change pace, honor that even when the message contains an answer.";

pub(super) fn object(fields: Value) -> Value {
    let required: Vec<_> = fields.as_object().unwrap().keys().collect();
    json!({"type":"object","additionalProperties":false,"required":required,"properties":fields})
}
fn strings(names: &[&str]) -> Map<String, Value> {
    names
        .iter()
        .map(|&name| (name.into(), json!({"type":"string"})))
        .collect()
}
fn schema(moves: &[&str], cards: &Map<String, Value>, assigned: bool) -> Value {
    let mut shapes = Vec::new();
    for &kind in moves {
        let names: &[&str] = match kind {
            "teach" => &[
                "objective",
                "explanation",
                "worked_example",
                "exercise",
                "next_objective",
            ],
            "explain" => &[
                "objective",
                "explanation",
                "worked_example",
                "next_step",
                "next_objective",
            ],
            "advance" => &["bridge_and_instruction", "exercise", "next_objective"],
            "guided_step" => &["exercise"],
            "apply_method" => &["new_fact", "exercise"],
            "coach" | "practice" => &["feedback", "exercise"],
            "guide" => &["feedback", "why_it_matters", "exercise"],
            _ => &["response", "next_step"],
        };
        let mut fields = strings(names);
        if kind == "teach" {
            fields.insert(
                "outline".into(),
                json!({"type":"array","items":{"type":"string"},"minItems":2,"maxItems":8}),
            );
        }
        if assigned && matches!(kind, "teach" | "advance") {
            fields.remove("exercise");
        }
        fields.insert("move".into(), json!({"type":"string","enum":[kind]}));
        shapes.push(object(fields.into()));
    }
    if !cards.is_empty() {
        shapes.push(object(json!({"move":{"type":"string","enum":["read"]},
            "card":{"type":"string","enum":cards.keys().collect::<Vec<_>>()}})));
    }
    object(json!({"turn":{"anyOf":shapes}}))
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Lesson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from_turn: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_draft: Option<crate::practice::Draft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<crate::judgment::Rationale>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub practice_correction: Option<crate::practice::Correction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_selection: Option<crate::preparation::Selection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<crate::practice::Released>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub practice_evidence: Option<crate::practice::Evidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<crate::discovery::Discovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning: Vec<crate::progression::Reading>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_recall: Option<crate::method::NameRecall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recall_items: Vec<crate::recall::Item>,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub declared_goal: Option<String>,
    #[serde(default)]
    pub assisted: bool,
    #[serde(default)]
    pub observation: Option<Observation>,
    #[serde(default)]
    pub is_probe: bool,
    #[serde(default)]
    pub check_form: Option<String>,
    #[serde(default)]
    pub claim: Option<String>,
    #[serde(default)]
    pub rubric: Option<Rubric>,
    #[serde(default)]
    pub preferences: Vec<String>,
    #[serde(default)]
    pub outline: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course: Option<super::course::Course>,
    pub selected: Option<String>,
    pub stage: usize,
    pub objective: String,
    pub next_objective: String,
    pub exercise: String,
    pub closed: Vec<Closed>,
    pub challenge: bool,
    pub washington_mentioned: bool,
    #[serde(default)]
    pub explanation_only: bool,
    /// The visible follow-up within the original exercise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_step: Option<String>,
    /// Learner submissions on this case, kept while a follow-up or clarification is open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_attempts: Vec<String>,
    #[serde(default)]
    pub(super) awaiting_guess: bool,
    #[serde(default)]
    last_decision: Option<Route>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Closed {
    pub objective: String,
    pub exercise: String,
    pub reason: String,
    pub learner_evidence: String,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(super) struct Route {
    pub action: String,
    evidence: String,
    note: String,
    target_stage: i64,
}
impl Route {
    pub fn control(action: &str) -> Self {
        Self {
            action: action.into(),
            evidence: String::new(),
            note: String::new(),
            target_stage: -1,
        }
    }
    fn assessment(&self) -> bool {
        matches!(self.action.as_str(), "complete" | "partial" | "repair")
    }
}
fn checked<T>(
    generate: &mut Generate,
    packet: &Value,
    schema: &Value,
    mut parse: impl FnMut(Value) -> Result<T, String>,
) -> Result<T, String> {
    let mut request = packet.clone();
    for attempt in 0..2 {
        match parse(generate(&request, schema)?) {
            Ok(v) => return Ok(v),
            Err(error) if attempt == 0 => request["repair"] = json!(error),
            Err(error) => return Err(format!("Luna could not deliver this turn: {error}")),
        }
    }
    unreachable!()
}

fn review(
    generate: &mut Generate,
    packet: &Value,
    draft: &Value,
    rubric: &mut Option<Rubric>,
    approved_feedback: &mut Option<String>,
    assisted: &mut bool,
) -> Result<Option<String>, String> {
    let mut shape = object(
        json!({"verdict":{"type":"string","enum":["accept","revise"]},"reason":{"type":"string"},"criterion":{"type":"string"},"mode":{"type":"string","enum":["written","witness","private","unassessable"]}}),
    );
    let separate_feedback = draft["move"] == "coach"
        && matches!(
            packet["teaching_decision"]["policy"].as_str(),
            Some(super::judgment::ELEVENTH_VERSION | super::judgment::VERSION)
        );
    if separate_feedback {
        shape["properties"]["standalone_feedback"] = json!({"type":"string"});
        shape["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("standalone_feedback"));
    }
    let mut review = json!({"task":"review","learner_message":packet["learner_message"],"route":packet["route"],"instruction":packet["instruction"],
        "teaching_decision":packet["teaching_decision"],"source_scope":packet["current_card"]["scope"],"source_locator":packet["current_card"]["locator"],
        "source_objective":packet["current_card"]["objective"],
        "target":packet["target"],"explanation_only":packet["explanation_only"],
        "prior_attempts":packet["prior_attempts"],"prior_criterion":packet["prior_criterion"],"last_requested_step":packet["last_requested_step"],
        "previous_objective":packet["previous_objective"],"open_exercise":packet["open_exercise"],
        "worked_exercise":packet["worked_exercise"],"closed_tasks":packet["closed_tasks_this_visit"],"draft":draft});
    // The reviewer needs the supplied passage to spot a missing premise or a
    // changed quotation. The learner only sees material included in the turn.
    for field in ["excerpt", "reading"] {
        if let Some(source) = packet["current_card"].get(field) {
            review[field] = source.clone();
        }
    }
    if let Some(p) = packet.get("progression") {
        review["progression"] = p.clone();
    }
    if let Some(course) = packet.get("course") {
        review["course"] = course.clone();
        review["current_milestone"] = packet["current_milestone"].clone();
    }
    if let Some(plan) = packet.get("working_plan") {
        review["working_plan"] = plan.clone();
        if draft["move"] == "advance" && draft["objective"].is_string() {
            review["target"] = draft["objective"].clone();
        }
    }
    checked(generate, &review, &shape, |v| match v["verdict"].as_str() {
        Some("accept") => {
            if let Some(cue) = v["method_cue"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
                let visible_fields = [
                    "bridge_and_instruction", "worked_example", "explanation", "exercise",
                    "feedback", "visible_reply", "new_fact",
                ];
                *assisted |= visible_fields.iter().any(|field| {
                    draft[field].as_str().is_some_and(|text| text.contains(cue))
                });
            }
            *rubric = v["criterion"]
                .as_str()
                .zip(v["mode"].as_str())
                .map(|(c, m)| Rubric {
                    criterion: c.into(),
                    mode: m.into(),
                });
            Ok(None)
        }
        Some("revise") => v["reason"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                if separate_feedback {
                    if let Some(feedback) = v["standalone_feedback"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        *approved_feedback = Some(feedback.to_owned());
                    }
                }
                Some(s.to_string())
            })
            .ok_or_else(|| "Specify the change needed.".into()),
        _ => Err("Choose accept or revise.".into()),
    })
}

fn objective_key(title: &str) -> String {
    title
        .trim()
        .trim_end_matches(['.', '?', '!'])
        .trim_end()
        .to_lowercase()
}

fn distinct_objectives(
    turn: &Value,
    previous: &str,
    advancing: bool,
    demonstrated: &[&str],
) -> bool {
    let objective = turn["objective"].as_str().unwrap_or(previous);
    let next = turn["next_objective"].as_str().unwrap_or("");
    if objective.len() > 600 || next.len() > 600 {
        return false;
    }
    let current = objective_key(objective);
    let next = objective_key(next);
    let previous = objective_key(previous);
    !next.is_empty()
        && current != next
        && (!advancing || (current != previous && next != previous))
        && !demonstrated.iter().any(|title| {
            let covered = objective_key(title);
            next == covered || (advancing && current == covered)
        })
}

/// Explicit changes of subject can suspend the current task before routing or recall.
pub fn switch_destination(words: &str) -> Option<&str> {
    let words = words.trim().trim_start_matches('\x1b').trim();
    let lower = words.to_ascii_lowercase();
    [
        "/goal ",
        "switch to ",
        "switch my goal to ",
        "change my goal to ",
        "let's switch to ",
        "let’s switch to ",
        "instead teach me ",
        "i want to study ",
        "i want to learn about ",
        "i want to learn ",
        "i would like to study ",
        "i would like to learn about ",
        "i would like to learn ",
    ]
    .iter()
    .find(|prefix| lower.starts_with(**prefix))
    .map(|prefix| {
        words[prefix.len()..]
            .trim_end_matches(['.', '!'])
            .trim_end_matches(" now")
            .trim()
    })
    .filter(|goal| !goal.is_empty())
}

fn explicit_switch(text: &str) -> bool {
    let words = text.trim().to_lowercase();
    [
        "/goal ",
        "/lesson ",
        "switch to ",
        "switch my goal to ",
        "change my goal to ",
        "let's switch to ",
        "let’s switch to ",
        "instead teach me ",
        "i want to study ",
        "i want to learn ",
        "i would like to learn ",
        "i would like to study ",
    ]
    .iter()
    .any(|prefix| words.starts_with(prefix))
}

impl Lesson {
    pub fn method_action(&mut self, action: &str) {
        self.observation = None;
        self.last_decision = Some(Route::control(action));
    }
    pub fn open_method_case(&mut self, exercise: &str, criterion: &str) {
        if !self.exercise.is_empty() {
            self.closed.push(Closed {
                objective: self.objective.clone(),
                exercise: std::mem::take(&mut self.exercise),
                reason: "method_practice".into(),
                learner_evidence: String::new(),
            });
        }
        self.current_step = None;
        self.prior_attempts.clear();
        self.selected = None;
        self.stage = 0;
        self.awaiting_guess = false;
        self.explanation_only = false;
        self.assisted = false;
        self.is_probe = false;
        self.check_form = None;
        self.claim = None;
        self.objective = "Apply a rule and defend a recommendation".into();
        self.exercise = exercise.into();
        self.rubric = Some(Rubric {
            criterion: criterion.into(),
            mode: "written".into(),
        });
    }
    pub fn clear_practice_trace(&mut self) {
        self.practice_evidence = None;
        self.rationale = None;
        self.practice_correction = None;
        self.prepared_selection = None;
    }
    pub fn open_supervised(&mut self, objective: String, task: String, criterion: String) {
        let previous = std::mem::take(self);
        *self = Lesson {
            discovery: previous.discovery,
            reasoning: previous.reasoning,
            name_recall: previous.name_recall,
            recall_items: previous.recall_items,
            course: previous.course,
            topic: previous.topic,
            declared_goal: previous.declared_goal,
            closed: previous.closed,
            preferences: previous.preferences,
            objective,
            exercise: task,
            rubric: Some(crate::assessment::Rubric {
                criterion,
                mode: "written".into(),
            }),
            ..Lesson::default()
        };
        let current = self.active_step().to_owned();
        for item in &mut self.recall_items {
            item.redirect(&current);
        }
    }

    // Only an intake or a learner-directed change can establish the course goal.
    pub fn goal_request(&self, learner: Option<&str>) -> Option<String> {
        let words = learner.unwrap_or_default().trim();
        for prefix in ["/goal ", "/lesson "] {
            if let Some(goal) = words.strip_prefix(prefix) {
                return Some(goal.trim().to_owned()).filter(|g| !g.is_empty());
            }
        }
        let route = self.last_decision.as_ref().map(|r| r.action.as_str());
        if route == Some("method_practice") {
            return self.declared_goal.clone();
        }
        if route == Some("start") && !words.is_empty() {
            return Some(words.to_owned());
        }
        if route == Some("use_source_or_topic") && explicit_switch(words) {
            return Some(words.to_owned());
        }
        None
    }
    pub fn recover_metadata(&mut self, prior: Option<&Self>, learner: Option<&str>) {
        if self
            .discovery
            .as_ref()
            .is_some_and(crate::discovery::Discovery::active)
        {
            return;
        }
        self.declared_goal = self
            .goal_request(learner)
            .or_else(|| prior.and_then(|p| p.declared_goal.clone()))
            .or_else(|| self.declared_goal.clone());
        self.topic = self
            .declared_goal
            .clone()
            .or_else(|| self.selected.clone())
            .unwrap_or_default();
        // Earlier snapshots have no case history. Rebuild it from delivered exchanges on replay.
        if self.prior_attempts.is_empty() {
            if let Some(before) = prior {
                match self.last_decision.as_ref().map(|r| r.action.as_str()) {
                    Some("partial" | "repair") => {
                        self.prior_attempts.clone_from(&before.prior_attempts);
                        self.remember_attempt(learner);
                    }
                    Some("question" | "acknowledge" | "preference") => {
                        self.prior_attempts.clone_from(&before.prior_attempts);
                    }
                    _ => {}
                }
            }
        }
        if self.observation.is_none() {
            if let (Some(before), Some(route), Some(words)) = (prior, &self.last_decision, learner)
            {
                if route.assessment()
                    && !before.exercise.is_empty()
                    && !route.evidence.is_empty()
                    && words.contains(&route.evidence)
                {
                    self.observation = Some(Observation {
                        target: Target::from_lesson(before),
                        exercise: before.exercise.clone(),
                        action: route.action.clone(),
                        quote: route.evidence.clone(),
                        assisted: true,
                        validation: "legacy_unverified".into(),
                        family: String::new(),
                    });
                }
            }
        }
    }
    fn remember_attempt(&mut self, learner: Option<&str>) {
        if let Some(words) = learner.filter(|s| !s.trim().is_empty()) {
            if self.prior_attempts.len() >= 6 {
                // Keep the initial reasoning when reading a long chain from an older journal.
                self.prior_attempts.remove(1);
            }
            self.prior_attempts.push(words.chars().take(1500).collect());
        }
    }

    pub fn active_step(&self) -> &str {
        self.current_step.as_deref().unwrap_or(&self.exercise)
    }

    pub fn current_question(&self) -> String {
        if let Some(discovery) = self.discovery.as_ref().filter(|d| d.active()) {
            return discovery.question.clone();
        }
        if let Some(item) = self.asking_meaning() {
            return item.prompt();
        }
        if self.asking_name() {
            return super::method::CHECK.to_owned();
        }
        self.active_step().to_owned()
    }

    pub fn remaining_objectives(&self, outline_start: usize, demonstrated: &[&str]) -> Vec<&str> {
        let mut seen: std::collections::BTreeSet<_> =
            demonstrated.iter().map(|s| objective_key(s)).collect();
        seen.insert(objective_key(&self.objective));
        seen.insert(String::new());
        let mut remaining = Vec::new();
        let has_next = seen.insert(objective_key(&self.next_objective));
        if has_next {
            remaining.push(self.next_objective.as_str());
        }
        for title in self
            .outline
            .iter()
            .skip(outline_start.saturating_add(usize::from(has_next)))
        {
            if seen.insert(objective_key(title)) {
                remaining.push(title);
            }
        }
        remaining
    }

    pub fn taught(&self) -> bool {
        !self
            .discovery
            .as_ref()
            .is_some_and(crate::discovery::Discovery::active)
            && !self.is_probe
            && self.last_decision.as_ref().is_none_or(|r| {
                !matches!(
                    r.action.as_str(),
                    "pause"
                        | "acknowledge"
                        | "method_name"
                        | "name_check"
                        | "name_answer"
                        | "meaning_check"
                        | "meaning_answer"
                )
            })
    }
    pub fn probe(
        &mut self,
        target: &Target,
        previous: &str,
        recent_tasks: &[String],
        generate: &mut Generate,
    ) -> Result<String, String> {
        let mut pending = self.clone();
        pending.current_step = None;
        pending.prior_attempts.clear();
        pending.awaiting_guess = false;
        pending.selected = target.card.clone();
        pending.stage = target.stage;
        pending.topic = target.topic.clone();
        pending.objective = target.objective.clone();
        pending.claim = target.claim.clone();
        pending.assisted = false;
        pending.explanation_only = false;
        pending.observation = None;
        pending.is_probe = true;
        pending.check_form = None;
        pending.rubric = None;
        pending.last_decision = Some(Route::control("review"));
        let novel = |s: &str| {
            s != previous
                && !recent_tasks.contains(
                    &s.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase(),
                )
        };
        let mut released = None;
        if let Some(name) = &target.card {
            let stages = if target.stage < 2 {
                vec![target.stage, 1 - target.stage]
            } else {
                vec![target.stage]
            };
            for stage in stages {
                if let Some(check) = material::check(name, stage)?.filter(|c| novel(&c.prompt)) {
                    pending.stage = stage;
                    released = Some(check);
                    break;
                }
            }
        }
        let exercise = if let Some(check) = released {
            pending.claim = Some(check.claim.clone());
            pending.objective = check.claim;
            pending.rubric = Some(Rubric {
                criterion: check.rubric,
                mode: check.mode,
            });
            pending.check_form = Some(check.id);
            check.prompt
        } else {
            let mut packet = json!({"task":"deliver","instruction":"Give ONE fresh, self-contained application problem testing the specified objective. Change the case or representation. Do not explain the method, give hints, answer it, or credit the learner. Respect the medium and supplied source.",
                "route":{"action":"review"},"target":target.objective,"previous_exercise":previous,"already_attempted":recent_tasks,"current_card":target.card.as_deref().map(|n|material::card(n,target.stage)).transpose()?});
            let shape = object(json!({"exercise":{"type":"string"}}));
            let mut result = None;
            for attempt in 0..3 {
                let exercise = checked(generate, &packet, &shape, |v| {
                    v["exercise"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && novel(s))
                        .map(str::to_owned)
                        .ok_or("Give a fresh exercise.".into())
                })?;
                if let Some(reason) = review(
                    generate,
                    &packet,
                    &json!({"move":"review","exercise":exercise}),
                    &mut pending.rubric,
                    &mut None,
                    &mut pending.assisted,
                )? {
                    packet["revision"] = json!({"issue":reason});
                    if attempt == 1 {
                        packet["task"] = json!("edit_turn");
                        packet["editor_review"] = json!({"issue":reason,"draft":exercise});
                    }
                } else {
                    result = Some(exercise);
                    break;
                }
            }
            result.ok_or("A fresh retest could not be reviewed. Try /retry.")?
        };
        pending.exercise = exercise.clone();
        *self = pending;
        Ok(exercise)
    }
    pub fn action(&self) -> &str {
        self.last_decision.as_ref().map_or("opening", |r| &r.action)
    }

    // Older tutors could commit the new objective without delivering its opening task.
    pub fn awaiting_topic_opening(&self) -> bool {
        self.action() == "complete"
            && !self.objective.is_empty()
            && self.active_step().is_empty()
            && !self.explanation_only
            && self
                .observation
                .as_ref()
                .is_some_and(|o| o.action == "complete")
    }

    pub fn route(
        &self,
        text: Option<&str>,
        auto_advance: bool,
        discussion_enabled: bool,
        generate: &mut Generate,
    ) -> Result<Route, String> {
        let Some(text) = text else {
            if auto_advance && self.awaiting_topic_opening() {
                return Ok(Route::control("open_next_topic"));
            }
            return Ok(Route::control("start"));
        };
        if text.starts_with("/goal ")
            || text
                .strip_prefix("/lesson ")
                .is_some_and(|id| material::CARDS.iter().any(|(k, _)| *k == id))
        {
            return Ok(Route::control("use_source_or_topic"));
        }
        let ready_after_pause = auto_advance
            && self.action() == "pause"
            && matches!(
                text.trim()
                    .trim_end_matches(['.', '!'])
                    .to_lowercase()
                    .as_str(),
                "ready"
                    | "i'm ready"
                    | "i’m ready"
                    | "i am ready"
                    | "let's continue"
                    | "let’s continue"
            );
        if (text == "/continue" || ready_after_pause) && self.exercise.is_empty() {
            return Ok(Route::control("resume_practice"));
        }
        if matches!(text, "/next" | "/harder") {
            return Ok(Route::control(&text[1..]));
        }
        let short_acknowledgment = !self.exercise.is_empty()
            && matches!(
                text.trim()
                    .trim_end_matches(['.', '!'])
                    .to_ascii_lowercase()
                    .as_str(),
                "ok" | "okay" | "got it" | "understood" | "thanks" | "thank you"
            );
        if short_acknowledgment
            && self
                .last_decision
                .as_ref()
                .is_some_and(|r| matches!(r.action.as_str(), "question" | "acknowledge"))
        {
            return Ok(Route::control("acknowledge"));
        }
        let mut stages = self
            .selected
            .as_deref()
            .map(material::syllabus)
            .transpose()?
            .unwrap_or_default();
        if stages.is_empty() {
            stages = self.outline.clone();
        }
        let packet = json!({"task":"route","instruction":ROUTING,"learner_message":text,"current_objective":self.objective,
            "explanation_only":self.explanation_only,"open_exercise":self.exercise,"last_requested_step":self.current_step,"criterion":self.rubric,"prior_attempts":self.prior_attempts,"awaiting_guess":self.awaiting_guess,"zero_based_stages":stages,"current_stage":self.stage});
        let no_task = discussion_enabled && self.exercise.is_empty();
        let actions: Vec<_> = ACTIONS
            .iter()
            .copied()
            .chain((no_task && !self.objective.is_empty()).then_some("discussion"))
            .filter(|action| !(no_task && matches!(*action, "complete" | "partial" | "repair")))
            .filter(|&action| {
                if self.objective.is_empty() {
                    matches!(
                        action,
                        "start"
                            | "support"
                            | "question"
                            | "explain_only"
                            | "pause"
                            | "use_source_or_topic"
                            | "resume_practice"
                    )
                } else {
                    action != "start" && (action != "acknowledge" || !self.exercise.is_empty())
                }
            })
            .collect();
        let shape = object(
            json!({"action":{"type":"string","enum":actions},"evidence":{"type":"string"},
            "note":{"type":"string"},"target_stage":{"type":"integer"}}),
        );
        let mut requested_quote_repair = false;
        checked(generate, &packet, &shape, |v| {
            let mut route: Route = serde_json::from_value(v).map_err(|e| e.to_string())?;
            if no_task && route.assessment() && !self.objective.is_empty() {
                return Ok(Route::control("discussion"));
            }
            if !actions.contains(&route.action.as_str())
                || route.target_stage < -1
                || (route.target_stage >= 0 && route.target_stage as usize >= stages.len())
            {
                return Err("Choose a listed action and valid stage.".into());
            }
            if short_acknowledgment && route.action == "next" {
                return Ok(Route::control("acknowledge"));
            }
            if route.assessment() && self.exercise.is_empty() {
                return Err("An assessment requires an open exercise.".into());
            }
            if route.assessment()
                && (route.evidence.trim().is_empty() || !text.contains(route.evidence.trim()))
            {
                if !requested_quote_repair {
                    requested_quote_repair = true;
                    return Err("For an attempted exercise copy exact words from the current learner message into evidence.".into());
                }
                // Preserve the actual artifact when the interpreter twice misquotes it.
                route.evidence = text.to_owned();
                route.note =
                    "Use the original submitted work. Credit only the reasoning present there."
                        .into();
            }
            if !route.assessment() && !route.evidence.is_empty() {
                return Err("Leave evidence empty for navigation or questions.".into());
            }
            Ok(route)
        }).or_else(|error| {
            if !error.starts_with("Luna could not deliver this turn:") { return Err(error); }
            // If classification remains ambiguous, teach without grading the message.
            Ok(Route::control(if self.objective.is_empty() {"start"} else if self.exercise.is_empty() && discussion_enabled {"discussion"} else {"question"}))
        })
    }

    #[cfg(test)]
    pub fn respond(
        &mut self,
        text: Option<&str>,
        generate: &mut Generate,
    ) -> Result<String, String> {
        self.respond_checked(text, generate, &mut |_, _, action| {
            Verdict::provisional(action)
        })
    }
    #[cfg(test)]
    pub fn respond_checked(
        &mut self,
        text: Option<&str>,
        generate: &mut Generate,
        assess: &mut dyn FnMut(&Lesson, &str, &str) -> Verdict,
    ) -> Result<String, String> {
        // Publish the change of task, assessment, and exercise together, after validation.
        let mut pending = self.clone();
        let route = self.route(text, true, true, generate)?;
        let response = pending.deliver(text, route, None, true, &[], generate, assess)?;
        *self = pending;
        Ok(response)
    }

    pub fn deliver(
        &mut self,
        text: Option<&str>,
        mut route: Route,
        decision: Option<&super::judgment::Decision>,
        auto_advance: bool,
        demonstrated: &[&str],
        generate: &mut Generate,
        assess: &mut dyn FnMut(&Lesson, &str, &str) -> Verdict,
    ) -> Result<String, String> {
        // A cached continuation from an older build may contain only the last quoted attempt.
        if self.prior_attempts.is_empty() {
            if let Some(previous) = self.last_decision.as_ref().filter(|r| {
                matches!(r.action.as_str(), "partial" | "repair") && !r.evidence.trim().is_empty()
            }) {
                self.prior_attempts
                    .push(previous.evidence.chars().take(1500).collect());
            }
        }
        let selected_card = text
            .and_then(|s| s.strip_prefix("/lesson "))
            .and_then(|id| material::CARDS.iter().find(|(k, _)| *k == id));
        if route.action == "acknowledge" {
            self.observation = None;
            self.last_decision = Some(route);
            return Ok(self.active_step().to_owned());
        }
        // A topical question can introduce terminology without replacing the active lesson.
        if route.action == "use_source_or_topic"
            && !self.objective.is_empty()
            && text.is_some_and(|t| {
                let words = t.to_lowercase();
                [
                    "what does ",
                    "what is ",
                    "is there an acronym",
                    "isn't there an acronym",
                    "isnt there an acronym",
                    "what are those",
                    "how does ",
                ]
                .iter()
                .any(|prefix| words.starts_with(prefix))
                    && !explicit_switch(t)
            })
        {
            route = Route::control("question");
        }
        let prior_target = Target::from_lesson(self);
        let previous_observation = self.observation.take();
        if self.awaiting_guess && route.assessment() {
            // A first impression is the starting point for discussion, never a mastery check.
            route = Route::control("discuss_guess");
        }
        if route.assessment() {
            let verdict = assess(self, text.unwrap_or_default(), &route.action);
            if !verdict.note.is_empty() {
                route.note = verdict.note;
            }
            route.action = verdict.action;
            self.observation = Some(Observation {
                target: prior_target,
                exercise: self.exercise.clone(),
                action: route.action.clone(),
                quote: route.evidence.clone(),
                assisted: self.assisted,
                validation: verdict.method,
                family: verdict.family,
            });
        }
        let pacing = decision.and_then(|d| d.progression.as_ref());
        let change = pacing.filter(|p| {
            matches!(p.choice.as_str(), "advance" | "distinguish") && self.selected.is_none()
        });
        if change.is_some() {
            route = Route::control("next");
        } else if pacing.is_some_and(|p| matches!(p.choice.as_str(), "stay" | "requested_practice"))
            && matches!(route.action.as_str(), "complete" | "harder")
        {
            route = Route::control("discussion");
        }
        let previous_step = self.current_step.take();
        let prior_criterion = self.rubric.clone();
        // One focused retry is useful. If it still leaves a gap, teach the missing link.
        if matches!(route.action.as_str(), "partial" | "repair") && !self.prior_attempts.is_empty()
        {
            route = Route::control("support");
        }
        if route.action == "preference" && decision.is_none_or(|d| !d.revises()) {
            if let Some(message) = text {
                if !self.preferences.iter().any(|s| s == message) {
                    self.preferences.push(message.chars().take(500).collect());
                    if self.preferences.len() > 8 {
                        self.preferences.remove(0);
                    }
                }
            }
        }
        self.is_probe = false;
        if !matches!(route.action.as_str(), "question" | "preference") {
            self.check_form = None;
            if !matches!(route.action.as_str(), "partial" | "repair") {
                self.rubric = None;
            }
        }
        if !matches!(
            route.action.as_str(),
            "question" | "preference" | "partial" | "repair" | "support" | "discuss_guess"
        ) {
            self.claim = None;
        }
        let action = route.action.as_str();
        let mut final_decision = decision.cloned();
        if let Some(d) = &mut final_decision {
            d.refine(action);
        }
        let decision = final_decision.as_ref();
        self.last_decision = Some(route.clone());
        if let Some(goal) = self.goal_request(text) {
            self.declared_goal = Some(goal);
        }
        self.topic = self
            .declared_goal
            .clone()
            .or_else(|| self.selected.clone())
            .unwrap_or_default();
        match action {
            "explain_only" => self.explanation_only = true,
            "resume_practice" => self.explanation_only = false,
            _ => {}
        }
        let words: Vec<_> = text
            .unwrap_or_default()
            .split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            .collect();
        if action == "use_source_or_topic" {
            self.washington_mentioned = false;
        }
        if words.iter().any(|w| w == "washington" || w == "wa") {
            self.washington_mentioned = !words.iter().any(|w| w == "dc")
                && !words
                    .windows(2)
                    .any(|w| w == ["d", "c"] || w == ["of", "columbia"]);
        }
        let advance = !self.objective.is_empty()
            && matches!(action, "complete" | "next" | "harder" | "revisit");
        let reopening = action == "open_next_topic";
        let opening_after_completion =
            auto_advance && (reopening || (advance && action == "complete"));
        let acknowledgement = if reopening {
            &previous_observation
        } else {
            &self.observation
        }
        .as_ref()
        .map_or("Let’s build on that.", |o| match o.validation.as_str() {
            "independent_agreement" | "exact_key" if o.assisted => {
                "You’ve got this step. We worked through it together."
            }
            "independent_agreement" | "exact_key" => "It’s clear you get this part.",
            _ => "Let’s build on that.",
        });
        let redirect = action == "use_source_or_topic";
        let guide_practice =
            action == "support" && !self.exercise.is_empty() && !self.explanation_only;
        let instruction = match action {
            "complete" | "open_next_topic" if opening_after_completion => "The tutor supplies a short acknowledgement. Briefly name the specific reasoning the learner demonstrated, then teach the target using a concrete example. End with one fresh, answerable question on the new topic. Start it in this turn without asking permission to continue.",
            "use_prompted_method" => "Offer one concrete application step for the case the learner is discussing. The method came from the tutor. Do not assess the learner, grade their use of the method, ask for the whole framework, or repeat an inference they already gave. Rust introduces the method; give just the next question.",
            "use_method" => "Rust will introduce the named method. Supply just one new concrete fact about this same case in new_fact, then one short question in exercise about a genuinely unresolved issue that fact opens. Let the learner make a small judgment. Do not explain the acronym, introduce another case, summarize the learner, demand the full framework, or give the answer.",
            "complete" => "Acknowledge exactly the reasoning supplied. Now teach the next skill and give its challenge.",
            "harder" if text == Some("/practice irac") => "Create a fresh fictional dispute for IRAC practice. Supply the entire rule and facts supporting both sides. Ask for a defensible recommendation and the best counterargument. Use different facts and stakes from the closed cases. Evaluate substantive application, with headings and name recall optional.",
            "harder"|"next" => "Retire the old task without grading it. Make the new decision genuinely more demanding. Start it now. No apology, rerun, or invitation to choose what to do.",
            "discussion" if decision.is_some_and(|d| matches!(d.policy.as_str(), super::judgment::SEVENTH_VERSION | super::judgment::EIGHTH_VERSION | super::judgment::NINTH_VERSION | super::judgment::TENTH_VERSION| super::judgment::ELEVENTH_VERSION | super::judgment::VERSION)) => "Answer the actual contribution. Name a specific inference in it and explain its practical consequence within this subject. End with one natural next step, for instance a fact that would change the decision. This is an ungraded conversation. No exercise is open.",
            "discussion" => "Respond directly to the learner’s contribution to the discussion. Connect it to the larger course application. Teach the next substantive idea with a different example. Keep it ungraded. Honor explanation-only mode.",
            "discuss_guess" => "Take the guess as given. Discuss what that choice would mean, kindly explaining any misconception yourself. Offer a fresh small application of the same idea. Do not grade, ask for a justification of the guess, or increase difficulty.",
            "support" if !guide_practice => "Respond to what feels difficult. If the topic is known, use one smaller or more familiar example to show the missing step and why it helps. End with one concrete thing to notice or try. If the subject is unknown, gently ask one easy orienting question. Respect explanation-only mode and leave the exercise closed.",
            "support" => "Give the learner a breather. Respond directly to their uncertainty or question and recognize one defensible idea in their words. Work through the old sticking point yourself. Explain the human or practical reason for the distinction, then change one fact or give a fresh tiny example and invite just a best guess. Close the old question here.",
            "partial" => "Confirm just the part the learner gave. Stay in the same case. Elicit the one missing inference without supplying it or restarting the problem.",
            "repair" => "Give specific feedback on the mistake and offer a focused retry.",
            "question" => "Answer directly. Explain each unfamiliar term in context; finish with one specific thing the learner can do with that explanation in the open exercise, if there is one.",
            "preference" => "Honor the stated preference immediately. Continue teaching concretely from the current point without announcing memory or asking the student to pick again.",
            "pause" => "Respect the pause and close the exercise.",
            "explain_only" => "Set the exercise aside. Explain what was requested without assigning another.",
            "use_source_or_topic" => "Follow the new topic or scope. Read a matching source before making local claims.",
            "revisit" => "Return to the skill the learner explicitly requested.",
            _ => "Teach one useful concept immediately. Align the exercise and reasoning with the objective.",
        };
        if !self.exercise.is_empty()
            && (advance
                || redirect
                || matches!(
                    action,
                    "pause" | "explain_only" | "support" | "discuss_guess" | "use_method"
                ))
        {
            self.closed.push(Closed {
                objective: self.objective.clone(),
                exercise: std::mem::take(&mut self.exercise),
                reason: action.into(),
                learner_evidence: if route.assessment() {
                    route.evidence.clone()
                } else {
                    String::new()
                },
            });
        }
        if !matches!(action, "question" | "preference") {
            self.awaiting_guess = guide_practice;
        }
        let prior_attempts = self.prior_attempts.clone();
        match action {
            "partial" | "repair" => self.remember_attempt(text),
            "question" | "preference" => {}
            _ => self.prior_attempts.clear(),
        }
        if advance {
            self.stage = match usize::try_from(route.target_stage) {
                Ok(target) if action == "revisit" || target > self.stage => target,
                _ if action == "revisit" => self.stage.saturating_sub(1),
                _ => self.stage.saturating_add(1),
            };
        }
        if redirect {
            self.selected = None;
            self.stage = 0;
            self.objective.clear();
            self.next_objective.clear();
            self.outline.clear();
            self.course = None;
            self.reasoning.clear();
        }
        self.challenge |= action == "harder";
        if let Some((id, _)) = selected_card {
            self.selected = Some((*id).into());
            self.topic = (*id).into();
            self.explanation_only = false;
        }
        self.assisted = match action {
            "partial" | "repair" | "question" => !self.exercise.is_empty(),
            "support" => guide_practice,
            "preference" => self.assisted,
            _ => false,
        };
        if action == "pause" {
            return Ok(if self.objective.is_empty() {
                "We can stop here. When you’re ready, tell me what you’d like to learn."
            } else if auto_advance {
                "We can stop here. Just say “ready” when you want to pick this back up."
            } else {
                "We can stop here. Type /continue when you’re ready to resume."
            }
            .into());
        }
        if (self.selected.is_none() || self.course.is_some())
            && decision.is_some_and(|d| {
                matches!(
                    d.policy.as_str(),
                    super::judgment::SIXTH_VERSION
                        | super::judgment::SEVENTH_VERSION
                        | super::judgment::EIGHTH_VERSION
                        | super::judgment::NINTH_VERSION
                        | super::judgment::TENTH_VERSION
                        | super::judgment::ELEVENTH_VERSION
                        | super::judgment::VERSION
                )
            })
        {
            if let Some(goal) = self.declared_goal.as_deref().filter(|g| !g.is_empty()) {
                if self.course.as_ref().is_none_or(|c| c.goal != goal) {
                    self.course = super::course::Course::create(
                        goal,
                        self,
                        demonstrated,
                        &mut |packet, schema| {
                            if let Some(d) =
                                decision.filter(|d| d.policy == super::judgment::VERSION)
                            {
                                let mut packet = packet.clone();
                                packet["teaching_decision"] = json!(d);
                                generate(&packet, schema)
                            } else {
                                generate(packet, schema)
                            }
                        },
                    )?;
                } else if let Some(course) = &mut self.course {
                    if change.is_none()
                        && (action != "discussion"
                            || decision.is_none_or(|d| {
                                !matches!(
                                    d.policy.as_str(),
                                    super::judgment::TENTH_VERSION
                                        | super::judgment::ELEVENTH_VERSION
                                        | super::judgment::VERSION
                                )
                            }))
                    {
                        if decision.is_some_and(|d| d.policy == super::judgment::VERSION)
                            && action == "complete"
                        {
                            if self
                                .observation
                                .as_ref()
                                .is_some_and(|o| o.demonstrated() && !o.assisted)
                                && course
                                    .current()
                                    .is_some_and(|m| m.skill.eq_ignore_ascii_case(&self.objective))
                            {
                                course.navigate("next");
                            }
                        } else {
                            course.navigate(action);
                        }
                    }
                }
            }
        }
        if let (Some(p), Some(course)) = (change, self.course.as_mut()) {
            course.focus_on(
                &p.next_skill,
                &p.application,
                decision.is_some_and(|d| d.policy == super::judgment::VERSION),
            );
        }
        let mut moves: &[&str] = match action {
            "support" if guide_practice => &["guide"],
            "discussion"
                if decision.is_some_and(|d| {
                    matches!(
                        d.policy.as_str(),
                        super::judgment::SEVENTH_VERSION
                            | super::judgment::EIGHTH_VERSION
                            | super::judgment::NINTH_VERSION
                            | super::judgment::TENTH_VERSION
                            | super::judgment::ELEVENTH_VERSION
                            | super::judgment::VERSION
                    )
                }) =>
            {
                &["reply"]
            }
            "question" | "preference" | "support" => &["reply"],
            "use_method" => &["apply_method"],
            "use_prompted_method" => &["guided_step"],
            "discuss_guess" => &["practice"],
            _ if self.explanation_only => &["explain"],
            _ if advance || reopening => &["advance"],
            "partial" | "repair" => &["coach"],
            _ => &["teach"],
        };
        let mut available: Map<String, Value> = material::CARDS
            .iter()
            .filter(|(name, _)| {
                text.is_some()
                    && (moves.contains(&"teach") || moves.contains(&"explain"))
                    && Some(*name) != self.selected.as_deref()
                    && (*name != "washington-civil" || self.washington_mentioned)
            })
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        let tail = self.closed.len().saturating_sub(12);
        let mut packet = json!({"task":"deliver","learner_message":text,"route":route,"instruction":instruction,
            "preferences":self.preferences,"declared_goal":self.declared_goal,"previous_objective":self.objective,"open_exercise":self.exercise,"closed_tasks_this_visit":self.closed[tail..],
            "explanation_only":self.explanation_only,"prefers_harder_work":self.challenge,"available_cards":available});
        if let Some(course) = self.course.as_mut() {
            course.turns = course.turns.saturating_add(1);
            packet["course"] = json!(course);
            packet["current_milestone"] = json!(course.current());
        }
        let facts_first = decision.is_some_and(|d| d.facts_first());
        let adaptive = self.selected.is_none()
            && decision.is_some_and(|d| {
                matches!(
                    d.policy.as_str(),
                    super::judgment::FIFTH_VERSION
                        | super::judgment::SIXTH_VERSION
                        | super::judgment::SEVENTH_VERSION
                        | super::judgment::EIGHTH_VERSION
                        | super::judgment::NINTH_VERSION
                        | super::judgment::TENTH_VERSION
                        | super::judgment::ELEVENTH_VERSION
                        | super::judgment::VERSION
                )
            });
        let choose_objective = adaptive
            && advance
            && action == "complete"
            && self
                .observation
                .as_ref()
                .is_some_and(Observation::demonstrated);
        let has_new_evidence = self.observation.as_ref().is_some_and(Observation::verified)
            || decision.is_some_and(|d| {
                d.findings.iter().any(|f| {
                    use super::judgment::Kind;
                    matches!(
                        f.proposal.kind,
                        Kind::Strength | Kind::Method | Kind::PromptedMethod | Kind::Difficulty
                    )
                })
            });
        let revise_next = adaptive
            && has_new_evidence
            && matches!(
                action,
                "partial"
                    | "repair"
                    | "support"
                    | "discuss_guess"
                    | "use_method"
                    | "use_prompted_method"
            );
        if choose_objective || revise_next {
            packet["working_plan"] = json!({"goal":self.declared_goal,"demonstrated":demonstrated,
            "previous_next":self.next_objective,"outline":self.outline,"instruction": if choose_objective {
                "The learner has completed the previous objective. Choose the objective to teach now from their demonstrated reasoning and remaining needs. The suggested target and outline are provisional. Teach that objective in this turn and name a distinct subsequent next_objective."
            } else {
                "Keep the current objective open and teach the remaining step. Revise next_objective to build on the particular understanding or difficulty evidenced in this answer. Preserve the learner's goal. A guess or a prompted method is not completion."
            }});
        }
        if facts_first {
            packet["required_opening"]=json!("Start with a concrete fact or brief scene from the case. Put it in opening_fact. Rust displays that paragraph before all objectives, explanations and framework labels. Continue from this fact in the remaining fields, without repeating it.");
        }
        if let Some(d) = decision {
            packet["teaching_decision"] = json!(d);
        }
        if guide_practice {
            packet["worked_exercise"] = json!(self.closed.last().map(|task| &task.exercise));
        }
        if matches!(
            action,
            "partial" | "repair" | "support" | "question" | "complete" | "preference"
        ) {
            packet["prior_attempts"] = json!(prior_attempts);
            packet["prior_criterion"] = json!(prior_criterion);
            packet["last_requested_step"] = json!(previous_step);
        }
        if let Some(name) = self.selected.as_deref() {
            packet["current_card"] = material::card(name, self.stage)?;
        }
        if self.washington_mentioned && self.selected.is_none() {
            let reference = material::card("washington-civil", 0)?;
            packet["reference_if_relevant"] = json!({"scope":reference["scope"],"sources":reference["sources"],"reading":reference["reading"]});
        }
        let course = self
            .selected
            .as_deref()
            .map(material::program)
            .transpose()?
            .unwrap_or_default();
        if !course.is_empty() {
            packet["course_program"] = json!(course);
        }
        if reopening {
            packet["target"] = json!(self.objective);
        } else if advance {
            let target = if adaptive && route.target_stage < 0 && action != "revisit" {
                self.remaining_objectives(self.stage, demonstrated).first().copied()
            } else {
                packet["current_card"]["objective"].as_str().filter(|s| !s.is_empty())
                    .or_else(|| self.outline.get(self.stage).map(String::as_str))
                    .or_else(|| (!self.next_objective.is_empty()).then_some(self.next_objective.as_str()))
            }.unwrap_or("Make a harder decision using the evidence, and explain the limits of the conclusion.");
            let target = self
                .course
                .as_ref()
                .and_then(|c| c.current())
                .filter(|_| {
                    self.selected.is_none() && route.target_stage < 0 && action != "revisit"
                })
                .map_or(target, |m| m.skill.as_str());
            packet["target"] = json!(target);
        }
        if let Some(p) = pacing {
            packet["progression"] = json!(p);
            if change.is_some() {
                packet["target"] = json!(p.next_skill);
                packet["instruction"] = json!("Acknowledge the cited reasoning, teach the target using a concrete bridge, and ask the new application. Preserve any unresolved detail as a later check. Change the decision the learner must actually make. For example, after repeated classification under a rule, supply the classification and ask the learner to select a source that could overturn it, decide which missing evidence matters, or recommend an action under uncertainty. Asking whether another object fits a rule repeats classification even if the facts, rule or setting change. In any subject give as a premise the operation already demonstrated. Do not grade this navigation.");
            }
        }
        let mut rejected = 0;
        let mut approved_feedback = None;
        let mut assigned = None;
        let turn = loop {
            // Teach the worked case before revealing the authored transfer problem.
            if assigned.is_none() && moves.iter().any(|m| matches!(*m, "teach" | "advance")) {
                assigned = packet
                    .get_mut("current_card")
                    .and_then(Value::as_object_mut)
                    .and_then(|c| c.remove("practice"))
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .filter(|exercise| {
                        action == "revisit" || !self.closed.iter().any(|c| &c.exercise == exercise)
                    });
            }
            packet["exercise_appended_by_workspace"] = json!(assigned.is_some());
            let mut shape = schema(moves, &available, assigned.is_some());
            if let Some(strength) = decision
                .filter(|d| d.policy == super::judgment::VERSION && d.method().is_some())
                .and_then(|d| {
                    d.findings
                        .iter()
                        .find(|f| f.proposal.kind == super::judgment::Kind::Strength)
                })
            {
                packet["grounded_feedback"] = json!({"evidence":strength.proposal.quote, "instruction":"In one sentence, tell the learner specifically which part of their reasoning held up before asking the next question."});
                for variant in shape["properties"]["turn"]["anyOf"].as_array_mut().unwrap() {
                    variant["properties"]["grounded_feedback"] =
                        json!({"type":"string","minLength":1});
                    variant["required"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!("grounded_feedback"));
                }
            }
            if choose_objective || revise_next {
                let field = if choose_objective {
                    "objective"
                } else {
                    "next_objective"
                };
                for variant in shape["properties"]["turn"]["anyOf"].as_array_mut().unwrap() {
                    if variant["properties"]["move"]["enum"][0] != "read"
                        && variant["properties"].get(field).is_none()
                    {
                        variant["properties"][field] = json!({"type":"string"});
                        variant["required"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(field));
                    }
                }
            }
            if facts_first {
                for variant in shape["properties"]["turn"]["anyOf"].as_array_mut().unwrap() {
                    if variant["properties"]["move"]["enum"][0] != "read" {
                        variant["properties"]["opening_fact"] = json!({"type":"string"});
                        variant["required"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!("opening_fact"));
                    }
                }
            }
            let proposal = checked(generate, &packet, &shape, |v| {
                let t = &v["turn"];
                let kind = t["move"].as_str().ok_or("Missing lesson move.")?;
                let variant = shape["properties"]["turn"]["anyOf"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["properties"]["move"]["enum"][0] == kind)
                    .ok_or("Choose a permitted move.")?;
                if variant["required"].as_array().unwrap().iter().any(|k| {
                    if k == "outline" {
                        return t["outline"].as_array().is_none_or(|a| {
                            a.len() < 2
                                || a.len() > 8
                                || a.iter().any(|x| {
                                    x.as_str()
                                        .is_none_or(|s| s.trim().is_empty() || s.len() > 600)
                                })
                        });
                    }
                    t[k.as_str().unwrap()]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                }) {
                    return Err("Fill every text field of this move.".into());
                }
                if kind == "read" && !available.contains_key(t["card"].as_str().unwrap()) {
                    return Err("Choose an available card.".into());
                }
                if matches!(kind, "advance" | "guide" | "practice")
                    && self.closed.iter().any(|c| t["exercise"] == c.exercise)
                {
                    return Err("Use the new target; the previous task is closed.".into());
                }
                if facts_first
                    && kind != "read"
                    && t["opening_fact"]
                        .as_str()
                        .is_none_or(|s| s.ends_with('?') || s.len() > 800)
                {
                    return Err("Open on a concrete case fact in one short paragraph.".into());
                }
                if let Some(d) = decision {
                    d.check_draft(t, &self.closed, previous_step.as_deref())?;
                }
                Ok(t.clone())
            });
            let mut turn = match proposal {
                Ok(t) => t,
                Err(error)
                    if packet["task"] != "edit_turn"
                        && error.starts_with("Luna could not deliver this turn:") =>
                {
                    packet["task"] = json!("edit_turn");
                    packet["editor_review"] = json!({"issue":error});
                    rejected = 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if turn["move"] != "read" {
                if let Some(exercise) = &assigned {
                    turn["exercise"] = json!(exercise);
                }
                if turn["move"] == "guided_step" {
                    let method = decision
                        .and_then(|d| d.method())
                        .ok_or("Missing prompted method.")?;
                    turn["visible_reply"] = json!(format!(
                        "Let’s use {method} for one step. {}",
                        turn["exercise"].as_str().unwrap().trim()
                    ));
                }
                if turn["move"] == "apply_method" {
                    let method = decision
                        .and_then(|d| d.method())
                        .ok_or("Missing recognized method.")?;
                    turn["visible_reply"] = json!(format!(
                        "Using {method}, consider this change. {}\n\n{}",
                        turn["new_fact"].as_str().unwrap().trim(),
                        turn["exercise"].as_str().unwrap().trim()
                    ));
                }
                if let (Some(feedback), Some(reply)) = (
                    turn["grounded_feedback"].as_str(),
                    turn["visible_reply"].as_str(),
                ) {
                    turn["visible_reply"] = json!(format!("{}\n\n{reply}", feedback.trim()));
                }
                let mut reviewed_rubric = None;
                let permission_gate = opening_after_completion
                    && turn["exercise"]
                        .as_str()
                        .is_some_and(|s| s.to_lowercase().contains("/continue"));
                let invalid_plan = (choose_objective || revise_next)
                    && !distinct_objectives(&turn, &self.objective, choose_objective, demonstrated);
                // On the last coaching draft, keep the saved plan if the proposed
                // revision is unusable. The visible step still gets content review.
                let keep_existing_plan = invalid_plan && rejected == 1 && turn["move"] == "coach";
                if keep_existing_plan {
                    turn.as_object_mut().unwrap().remove("next_objective");
                }
                let issue = if invalid_plan && !keep_existing_plan {
                    Some("Choose a distinct remaining skill within the learner’s goal.".into())
                } else if permission_gate {
                    Some("Give the learner a concrete first task on the new topic now.".into())
                } else {
                    review(
                        generate,
                        &packet,
                        &turn,
                        &mut reviewed_rubric,
                        &mut approved_feedback,
                        &mut self.assisted,
                    )?
                };
                if let Some(reason) = issue {
                    if opening_after_completion && rejected == 1 {
                        break self.fallback_topic_opening(
                            &packet,
                            assigned.as_deref(),
                            facts_first,
                            previous_observation.as_ref(),
                        );
                    }
                    if rejected == 1
                        && !auto_advance
                        && !facts_first
                        && action == "complete"
                        && self.observation.as_ref().is_some_and(|o| {
                            matches!(o.validation.as_str(), "exact_key" | "independent_agreement")
                        })
                    {
                        // A failed draft of the next task cannot undo an already checked submission.
                        let target = packet["target"]
                            .as_str()
                            .unwrap_or(&self.next_objective)
                            .to_owned();
                        self.objective = target.clone();
                        self.next_objective.clear();
                        self.exercise.clear();
                        let supported = if self.observation.as_ref().is_some_and(|o| o.assisted) {
                            "Your response meets the criterion with support."
                        } else {
                            "Your response meets the criterion for this exercise."
                        };
                        return Ok(format!("{supported} The next objective is {target} Type /continue when you’re ready."));
                    }
                    if rejected == 1
                        && turn["move"] == "coach"
                        && decision.is_none_or(|d| {
                            !matches!(
                                d.policy.as_str(),
                                super::judgment::ELEVENTH_VERSION | super::judgment::VERSION
                            )
                        })
                    {
                        // Older receipts replay the original quote-only recovery.
                        // Both drafts failed. Keep the assessed submission, rubric,
                        // and open case. No part of a rejected draft reaches the learner.
                        let feedback = if route.evidence.trim().is_empty() {
                            "Let’s work through one part of your answer.".to_owned()
                        } else {
                            format!("You wrote, “{}”", route.evidence.trim())
                        };
                        let step = crate::method::coaching_question(&self.exercise)
                            .unwrap_or("Pick one step in your answer. What could you use to check whether that step holds?");
                        break json!({"move":"coach","feedback":feedback,"exercise":step});
                    }
                    if rejected == 1 && turn["move"] == "guide" {
                        // If the new question keeps getting in the way, teach the example on its own.
                        moves = &["reply"];
                        self.awaiting_guess = false;
                        packet["explanation_only"] = json!(true);
                        packet["instruction"] = json!("Use a different representation to walk the learner through the worked_exercise. Explain why the puzzling step works. End by pointing out one concrete feature they can inspect in your example. No new exercise or reply is required.");
                        packet["revision"] = json!({"issue":reason});
                        rejected = 0;
                        continue;
                    }
                    if rejected == 1 && packet["task"] != "edit_turn" {
                        packet["task"] = json!("edit_turn");
                        packet["editor_review"] = json!({"issue":reason,"draft":turn});
                        continue;
                    }
                    if rejected == 1 {
                        // Even if every new question fails review, the learner can
                        // receive independently approved feedback on this answer.
                        if let Some(feedback) = approved_feedback.take() {
                            self.current_step = previous_step;
                            return Ok(feedback);
                        }
                        return Err(
                            "Luna could not produce a suitable exercise on this turn.".into()
                        );
                    }
                    rejected += 1;
                    if assigned.is_some() {
                        turn.as_object_mut().unwrap().remove("exercise");
                    }
                    packet["revision"] = json!({"issue":reason,"rejected_draft":turn});
                    if opening_after_completion {
                        packet["revision"]["direction"] = json!("Use a different, short worked example on the target. Ask for one concrete observation or prediction the learner can make immediately.");
                    }
                    continue;
                }
                let keep_rubric = match turn["move"].as_str() {
                    Some("teach" | "advance" | "practice" | "apply_method") => true,
                    // Earlier receipts omitted the guided follow-up criterion.
                    Some("guided_step") => decision.is_some_and(|d| !d.uses_legacy_method_rules()),
                    _ => false,
                };
                if keep_rubric {
                    self.rubric = reviewed_rubric;
                }
                break turn;
            }
            let name = turn["card"].as_str().unwrap();
            assigned = None;
            self.selected = Some(name.into());
            self.stage = 0;
            available.clear();
            packet["available_cards"] = json!({});
            packet["current_card"] = material::card(name, 0)?;
        };
        if choose_objective && turn["objective"].is_string() {
            packet["target"] = turn["objective"].clone();
        }
        if revise_next {
            if let Some(next) = turn["next_objective"].as_str() {
                self.next_objective = next.trim().into();
            }
        }
        let mut reply = self.render(&turn, &packet)?;
        if decision.is_some_and(|d| d.policy == super::judgment::VERSION && d.method().is_some()) {
            if let Some(course) = self.course.as_mut() {
                if let Some(milestone) = course.milestones.get_mut(course.focus) {
                    milestone.application =
                        turn["exercise"].as_str().unwrap_or_default().to_owned();
                }
            }
        }
        if (choose_objective || change.is_some())
            && decision.is_some_and(|d| {
                matches!(
                    d.policy.as_str(),
                    super::judgment::TENTH_VERSION
                        | super::judgment::ELEVENTH_VERSION
                        | super::judgment::VERSION
                )
            })
        {
            if let Some(course) = self.course.as_mut() {
                course.focus_on(
                    &self.objective,
                    &self.exercise,
                    decision.is_some_and(|d| d.policy == super::judgment::VERSION),
                );
            }
        }
        if adaptive
            && self.selected.is_none()
            && !self.outline.is_empty()
            && (revise_next
                || matches!(turn["move"].as_str(), Some("teach" | "advance" | "explain")))
        {
            for (i, title) in [
                (self.stage, &self.objective),
                (self.stage.saturating_add(1), &self.next_objective),
            ] {
                if !title.is_empty() {
                    if let Some(slot) = self.outline.get_mut(i) {
                        slot.clone_from(title);
                    } else if i == self.outline.len() {
                        self.outline.push(title.clone());
                    }
                }
            }
        }
        if opening_after_completion {
            reply = format!("{acknowledgement}\n\n{reply}");
        }
        Ok(match turn["opening_fact"].as_str() {
            Some(fact) => format!("{}\n\n{reply}", fact.trim()),
            None => reply,
        })
    }
    // Both drafts failed review. Start the topic with published material or an ungraded
    // first impression tied to the example the learner has already seen.
    fn fallback_topic_opening(
        &mut self,
        packet: &Value,
        assigned: Option<&str>,
        facts_first: bool,
        previous_observation: Option<&Observation>,
    ) -> Value {
        let target = packet["target"].as_str().expect("advancing has a target");
        let worked = packet["current_card"]["worked"].as_str();
        let (bridge, exercise, fact) = if let Some((example, task)) = worked.zip(assigned) {
            if facts_first {
                (target, task.to_owned(), Some(example.to_owned()))
            } else {
                (example, task.to_owned(), None)
            }
        } else {
            self.awaiting_guess = true;
            let fact = self
                .observation
                .as_ref()
                .or(previous_observation)
                .filter(|o| facts_first && !o.quote.trim().is_empty())
                .map(|o| format!("You said, “{}”", o.quote.trim()));
            (
                "Let’s use the example we just discussed to explore the next idea.",
                format!("{target}\n\nWhich detail from that example would you look at first?"),
                fact,
            )
        };
        let mut opening = json!({"move":"advance","bridge_and_instruction":bridge,
            "exercise":exercise,"next_objective":""});
        if let Some(fact) = fact {
            opening["opening_fact"] = json!(fact);
        }
        opening
    }

    fn render(&mut self, turn: &Value, packet: &Value) -> Result<String, String> {
        let field = |key: &str| turn[key].as_str().unwrap();
        let kind = field("move");
        if kind == "guided_step" {
            self.current_step = Some(field("exercise").trim().into());
            self.assisted = true;
            return Ok(field("visible_reply").into());
        }
        if kind == "reply" {
            if !self.exercise.is_empty() {
                self.current_step = Some(field("next_step").trim().to_owned());
            }
            return Ok([field("response").trim(), field("next_step").trim()].join("\n\n"));
        }
        if kind == "explain" {
            self.objective = field("objective").into();
            self.next_objective = field("next_objective").into();
            self.exercise.clear();
            return Ok([
                field("explanation"),
                field("worked_example"),
                field("next_step").trim(),
            ]
            .join("\n\n"));
        }
        if kind == "coach" {
            let step = field("exercise").trim();
            self.current_step = Some(step.into());
            return Ok([field("feedback"), step].join("\n\n"));
        }
        self.exercise = field("exercise").into();
        if kind == "apply_method" {
            self.exercise = field("visible_reply").into();
            self.assisted = true;
            return Ok(self.exercise.clone());
        }
        if kind == "practice" {
            return Ok([field("feedback"), &self.exercise].join("\n\n"));
        }
        if kind == "guide" {
            return Ok([field("feedback"), field("why_it_matters"), &self.exercise].join("\n\n"));
        }
        self.next_objective = field("next_objective").into();
        if kind == "advance" {
            self.objective = packet["target"].as_str().unwrap().into();
            return Ok(format!(
                "{}\n\n{}",
                field("bridge_and_instruction"),
                self.exercise
            ));
        }
        self.objective = field("objective").into();
        if self.outline.is_empty() {
            self.outline = turn["outline"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            if !self.outline.is_empty() {
                self.outline[0] = self.objective.clone();
                if self.outline.len() > 1 {
                    self.outline[1] = self.next_objective.clone();
                }
            }
        }
        Ok([
            &self.objective,
            field("explanation"),
            field("worked_example"),
            &self.exercise,
        ]
        .join("\n\n"))
    }
}
