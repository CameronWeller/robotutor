//! The journal owns the question, its evidence, and the learner's consent.
use crate::{
    lesson::{object, Generate, Lesson},
    session::Visit,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Learner,
    Reflection,
    Subject,
    Review,
    Instruction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    subject: String,
    area: String,
    kind: String,
    quote: String,
    reading: String,
    replaces: Vec<String>,
    #[serde(default)]
    task: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Profile {
    facts: BTreeMap<String, Evidence>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    agreements: BTreeMap<String, AgreedStart>,
    areas: BTreeMap<String, BTreeSet<String>>,
}
impl Profile {
    pub fn merge(&mut self, other: &Self) {
        self.agreements.extend(other.agreements.clone());
        self.facts.extend(other.facts.clone());
        for (subject, areas) in &other.areas {
            self.areas
                .entry(subject.clone())
                .or_default()
                .extend(areas.iter().cloned());
        }
    }
    fn superseded(&self) -> BTreeSet<&String> {
        self.facts.values().flat_map(|e| &e.replaces).collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Start {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    card: Option<String>,
    #[serde(default)]
    provisional: bool,
    objective: String,
    exercise: String,
    criterion: String,
    why: String,
    sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AgreedStart {
    subject: String,
    learner_reply: String,
    proposal: Start,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discovery {
    pub phase: Phase,
    pub profile: Profile,
    pub subject: String,
    #[serde(default)]
    selected_subject: bool,
    pub question: String,
    pub reason: String,
    #[serde(default)]
    awaiting_subject_to_start: bool,
    #[serde(default)]
    thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agreement: Option<String>,
    diagnostic: Option<String>,
    cued: bool,
    proposal: Option<Start>,
}

#[derive(Deserialize)]
struct Finding {
    area: String,
    kind: String,
    quote: String,
    reading: String,
    replaces: Vec<String>,
}
#[derive(Deserialize)]
struct Decision {
    action: String,
    #[serde(default)]
    early_request: String,
    #[serde(default)]
    start: Start,
}

#[derive(Deserialize)]
struct Move {
    decision: Decision,
    #[serde(default)]
    qualified_claim: String,
    subject: String,
    areas: Vec<String>,
    findings: Vec<Finding>,
    area: String,
    reply: String,
    reason: String,
}

fn shape(state: &Discovery, early: bool) -> Value {
    let mut actions = vec!["follow", "pause"];
    match state.phase {
        Phase::Learner | Phase::Reflection => actions.push("reflect"),
        Phase::Subject | Phase::Review => {
            actions.extend(["explore", "diagnostic"]);
            if state.diagnostic.is_some() {
                actions.push("hint");
            }
        }
        Phase::Instruction => {}
    }

    let mut kinds = vec!["reported", "unfamiliar", "declined", "correction"];
    if state.diagnostic.is_some() {
        kinds.extend(["demonstrated", "difficulty"]);
    }

    let s = json!({"type":"string"});
    let list = json!({"type":"array","items":s});
    let superseded = state.profile.superseded();
    let mut sources = vec!["current"];
    sources.extend(
        state
            .profile
            .facts
            .iter()
            .filter(|(id, f)| {
                !superseded.contains(id) && (f.subject.is_empty() || f.subject == state.subject)
            })
            .map(|(id, _)| id.as_str()),
    );
    let citations = json!({"type":"array","items":{"type":"string","enum":sources}});
    let finding = object(json!({"kind":{"type":"string","enum":kinds},
        "area":s,"quote":s,"reading":s,"replaces":list}));
    let required_text = json!({"type":"string","minLength":1});
    let start = object(
        json!({"objective":required_text,"exercise":required_text,"criterion":required_text,"why":required_text,"sources":citations}),
    );
    let mut choices = Vec::new();
    if !early {
        choices.push(object(json!({"action":{"type":"string","enum":actions}})));
        choices.push(object(json!({"action":{"type":"string","enum":["provisional"]},"early_request":required_text,"start":start})));
    }
    if early || matches!(state.phase, Phase::Subject | Phase::Review) {
        choices.push(object(
            json!({"action":{"type":"string","enum":["propose"]},"start":start}),
        ));
    }
    object(json!({
        "decision":{"anyOf":choices}, "subject":s, "areas":list,
        "findings":{"type":"array","items":finding}, "area":s,
        "reply":required_text,"reason":required_text,
        "qualified_claim":{"type":"string","maxLength":180}
    }))
}
pub const OLD_OPENING: &str =
    "What’s something you know well, care about, have made, or figured out?";
pub const OPENING: &str = "What shall we try first: a pattern, a word puzzle, or your own topic?";
pub const OPENING_REASON: &str =
    "Your pick gives us a small first step. You can change direction anytime.";
const PATTERN: &str = "2, 4, 6, … What number would you try next?";
const WORD: &str = "Raincoat and rainstorm both start with rain. Can you name one more rain word?";
const TOPIC: &str = "What topic should we try? A word or two is plenty.";

const REVIEW: &str = "Does this sound like you? You can correct anything, explore further, or say “start” to try the proposed lesson.";
const REFLECT: &str = "Does that sound like you? Tell me what to change, or say “looks right” to explore the subject.";
pub const FIT: &str = "After you try it, use /fit followed by your thoughts: Did my account sound like you, and did this lesson start in the right place?";

impl Discovery {
    pub fn new(subject: String, profile: Profile) -> Self {
        Self {
            phase: Phase::Learner,
            selected_subject: !subject.is_empty(),
            subject,
            profile,
            question: OPENING.into(),
            reason: OPENING_REASON.into(),
            awaiting_subject_to_start: false,
            thread: String::new(),
            agreement: None,
            diagnostic: None,
            cued: false,
            proposal: None,
        }
    }
    pub fn quick_choices(&self) -> &[&str] {
        if self.phase != Phase::Learner {
            return &[];
        }
        match self.question.as_str() {
            OPENING | OLD_OPENING => &["A pattern", "A word puzzle", "My own topic"],
            _ => &[],
        }
    }

    fn ask_subject(&mut self) -> String {
        self.awaiting_subject_to_start = true;
        self.question = "Which subject would you like the first lesson to cover?".into();
        self.reason =
            "I can suggest a provisional exercise once I know what you want to study.".into();
        self.question.clone()
    }
    pub fn return_context(&self) -> Option<String> {
        (!self.thread.is_empty()).then(|| format!("We were discussing “{}”.", self.thread))
    }
    pub fn return_prompt(&self) -> String {
        match self.return_context() {
            Some(context) => format!("{context}\n{}", self.question),
            None => self.question.clone(),
        }
    }
    pub fn active(&self) -> bool {
        self.phase != Phase::Instruction
    }
    pub fn merge(&mut self, profile: &Profile) {
        self.profile.merge(profile);
        self.invalidate_recommendation();
    }
    fn invalidate_recommendation(&mut self) {
        if self.proposal.as_ref().is_some_and(|p| {
            p.sources
                .iter()
                .any(|id| self.profile.superseded().contains(id))
        }) {
            self.proposal = None;
            if self.phase == Phase::Instruction {
                return;
            }
            self.phase = Phase::Subject;
            self.question = "You corrected part of my account. What would you like me to use as a starting point now?".into();
            self.reason = "The previous suggestion depended on a corrected observation.".into();
        }
    }
    pub fn account(&self) -> String {
        let replaced = self.profile.superseded();
        let mut lines = vec![format!(
            "Getting to know you · {}",
            if self.subject.is_empty() {
                "subject still open"
            } else {
                &self.subject
            }
        )];
        for (id, e) in &self.profile.facts {
            if !e.subject.is_empty() && e.subject != self.subject {
                continue;
            }
            let status = if replaced.contains(id) {
                "Earlier account, since corrected"
            } else {
                match e.kind.as_str() {
                    "independent" => "Observed independently",
                    "cued" => "Observed after a cue",
                    "difficulty" => "Observed difficulty",
                    "declined" => "Set aside",
                    "unfamiliar" => "You said this is unfamiliar",
                    "correction" => "Your correction",
                    _ => "You said",
                }
            };
            let area = if e.area.is_empty() {
                String::new()
            } else {
                format!(" ({})", e.area)
            };
            lines.push(format!("{status}{area} · “{}” [{id}]", e.quote));
            if !replaced.contains(id) && !e.reading.is_empty() {
                lines.push(format!("My interpretation · {} [{id}]", e.reading));
            }
        }
        if !self.profile.facts.is_empty() {
            lines.push("To correct a saved detail, tell me in your own words or use /amend SOURCE followed by your correction.".into());
        }
        for area in self.profile.areas.get(&self.subject).into_iter().flatten() {
            let explored = self.profile.facts.iter().any(|(id, e)| {
                e.subject == self.subject
                    && &e.area == area
                    && !replaced.contains(id)
                    && e.kind != "declined"
            });
            if !explored {
                lines.push(format!("Unexplored · {area}"));
            }
        }
        for (source, agreed) in &self.profile.agreements {
            if agreed.subject == self.subject {
                lines.push(format!(
                    "You agreed to try {} · “{}” [{source}]",
                    agreed.proposal.objective, agreed.learner_reply
                ));
            }
        }
        if let Some(p) = &self.proposal {
            let label = if self.phase == Phase::Instruction {
                "Agreed first lesson"
            } else {
                "Suggested first lesson"
            };
            lines.push(format!(
                "{label} · {}\nFirst exercise · {}\nWhy here · {}",
                p.objective, p.exercise, p.why
            ));
            if p.provisional {
                lines.push(
                    "Provisional start · We can keep exploring the rest of the subject as we go."
                        .into(),
                );
            }
            if !p.sources.is_empty() {
                lines.push(format!("Evidence · {}", p.sources.join(", ")));
            }
        }
        lines.join("\n")
    }
    pub fn working_plan(&self) -> Value {
        let superseded = self.profile.superseded();
        let observations: Vec<_> = self
            .profile
            .facts
            .iter()
            .filter(|(_, e)| e.subject.is_empty() || e.subject == self.subject)
            .map(|(id, e)| {
                json!({"source":id,"learner_quote":e.quote,"kind":e.kind,
                "area":e.area,"tutor_interpretation":e.reading,"tutor_prompt":e.task,
                "replaces":e.replaces,"superseded":superseded.contains(id)})
            })
            .collect();
        let agreements: Vec<_> = self.profile.agreements.iter().filter(|(_, a)|a.subject == self.subject)
            .map(|(source,a)| json!({"source":source,"learner_reply":a.learner_reply,"tutor_proposal":a.proposal,
                 "evidence_since_corrected":a.proposal.sources.iter().any(|s|superseded.contains(s))})).collect();
        json!({"agreements":agreements,"subject":self.subject, "phase":self.phase,"observations":observations,
            "suggested_start":self.proposal, "agreement_source":self.agreement,
            "question":self.question,"reason":self.reason})
    }
    pub fn teaching_context(&self) -> Value {
        // A proposal cites its rationale. The next lesson also needs corrections
        // and the conditions under which other relevant work was demonstrated.
        let superseded = self.profile.superseded();
        let mut sources: Vec<_> = self
            .profile
            .facts
            .iter()
            .rev()
            .filter(|(id, fact)| {
                !superseded.contains(id)
                    && (fact.subject.is_empty() || fact.subject == self.subject)
            })
            .collect();
        sources.sort_by_key(|(id, fact)| {
            let cited = self
                .proposal
                .as_ref()
                .is_some_and(|p| p.sources.contains(id));
            (!cited, fact.kind != "correction")
        });
        let observations: Vec<_> = sources.into_iter().take(8)
            .map(|(id, fact)| json!({
                "source":id, "kind":fact.kind, "learner_quote":fact.quote, "tutor_interpretation":fact.reading,
                "tutor_prompt":fact.task, "replaces":fact.replaces
            })).collect();
        json!({"goal":self.subject,"agreed_start":self.proposal,"agreement_source":self.agreement,"observations":observations})
    }
    pub fn decision_sources(&self) -> Vec<(String, String)> {
        let current = self.correctable_sources();
        match &self.proposal {
            Some(p) => current
                .into_iter()
                .filter(|(id, _)| p.sources.contains(id))
                .collect(),
            None => current,
        }
    }
    pub fn change_subject(&mut self, subject: &str) {
        self.agreement = None;
        self.thread.clear();
        self.subject = subject.into();
        self.selected_subject = true;
        self.proposal = None;
        self.diagnostic = None;
        self.cued = false;
        if !matches!(self.phase, Phase::Learner | Phase::Reflection) {
            self.phase = Phase::Subject;
            self.question = format!("What work in {subject} already feels familiar to you?");
            self.reason = "Your earlier account can guide us as we explore this subject.".into();
        }
    }
    pub fn opening(&self) -> String {
        if self.profile.facts.is_empty() {
            self.question.clone()
        } else {
            format!(
                "We can use or correct what you’ve shared before.\n{}\n\n{}",
                self.account(),
                self.question
            )
        }
    }
    fn prepare(
        &mut self,
        words: &str,
        source: &str,
        m: Move,
        early: bool,
    ) -> Result<String, String> {
        if m.reply.trim().is_empty()
            || m.reason.trim().is_empty()
            || m.reply.matches('?').count() > 1
        {
            return Err("Supply one follow-up and its reason.".into());
        }
        if !self.selected_subject
            && !m.subject.trim().is_empty()
            && (self.subject.is_empty() || matches!(self.phase, Phase::Learner | Phase::Reflection))
        {
            let name = m.subject.trim();
            self.subject = self
                .profile
                .areas
                .keys()
                .find(|prior| prior.eq_ignore_ascii_case(name))
                .cloned()
                .unwrap_or_else(|| name.into());
        }
        let early = early
            || (self.phase == Phase::Review
                && m.decision.action == "propose"
                && self.proposal.as_ref().is_some_and(|p| p.provisional))
            || (m.decision.action == "provisional"
                && !m.decision.early_request.trim().is_empty()
                && words.contains(&m.decision.early_request));
        let subject_phase = matches!(self.phase, Phase::Subject | Phase::Review);
        if (subject_phase || early) && !self.subject.is_empty() {
            self.profile
                .areas
                .entry(self.subject.clone())
                .or_default()
                .extend(m.areas.into_iter().filter(|a| !a.trim().is_empty()));
        }
        let mut added = vec![];
        let mut correction = false;
        for (i, f) in m
            .findings
            .into_iter()
            .filter(|_| !words.starts_with('/'))
            .enumerate()
        {
            if f.quote.trim().is_empty() || !words.contains(&f.quote) {
                return Err("Quote the current learner's exact words.".into());
            }
            if f.kind == "correction"
                && (f.replaces.is_empty()
                    || f.replaces
                        .iter()
                        .any(|id| !self.profile.facts.contains_key(id)))
            {
                return Err("Link the correction to the saved source it changes.".into());
            }
            if f.kind != "correction" && !f.replaces.is_empty() {
                return Err("Only a correction supersedes earlier evidence.".into());
            }
            let mut area = f.area;
            let kind = match f.kind.as_str() {
                "difficulty" | "demonstrated" => {
                    area = self
                        .diagnostic
                        .clone()
                        .ok_or("Record a report unless answering an open diagnostic.")?;
                    if f.kind == "difficulty" {
                        "difficulty"
                    } else if self.cued {
                        "cued"
                    } else {
                        "independent"
                    }
                }
                "correction" => {
                    correction = true;
                    "correction"
                }
                "reported" | "unfamiliar" | "declined" => &f.kind,
                _ => return Err("Unknown evidence type.".into()),
            };
            if !subject_phase {
                area.clear();
            }
            let id = format!("{source}.{}", i + 1);
            self.profile.facts.insert(
                id.clone(),
                Evidence {
                    subject: if area.is_empty() {
                        String::new()
                    } else {
                        self.subject.clone()
                    },
                    area,
                    kind: kind.into(),
                    quote: f.quote,
                    reading: f.reading,
                    replaces: f.replaces,
                    task: self.diagnostic.as_ref().map(|_| self.question.clone()),
                },
            );
            added.push(id);
        }
        let revising = correction && self.phase == Phase::Review;
        if revising
            && !matches!(
                m.decision.action.as_str(),
                "propose" | "provisional" | "explore" | "pause"
            )
        {
            return Err("Revise the suggested exercise using the correction and cite current. Use explore only if the learner asked to explore further.".into());
        }
        if correction {
            self.invalidate_recommendation();
        }
        if !words.starts_with('/')
            && m.decision.action != "pause"
            && (!added.is_empty() || m.decision.action == "follow")
        {
            self.thread = words.chars().take(180).collect();
        }
        let prior_question = self.question.clone();
        self.reason = m.reason;
        self.question = m.reply;
        let claim = m.qualified_claim.trim();
        if self.phase == Phase::Learner && m.decision.action == "follow" && !claim.is_empty() {
            if !words.contains(claim) || claim.contains('?') || claim.chars().count() > 180 {
                return Err(
                    "Quote the learner's qualified claim exactly, or leave qualified_claim empty."
                        .into(),
                );
            }
            self.question =
                if prior_question.starts_with("What in your observations led you to write") {
                    format!("What new observation would change your confidence in “{claim}”?")
                } else {
                    format!("What in your observations led you to write “{claim}”?")
                };
            self.reason =
                "You qualified a conclusion. Your reason will help me choose what to work on next."
                    .into();
        }
        if m.decision.action == "pause" {
            let reply = self.question.clone();
            self.question = prior_question;
            return Ok(reply);
        }
        let previous = self.diagnostic.take();
        self.cued = false;
        match m.decision.action.as_str() {
            "propose" | "provisional" if early || (subject_phase && m.decision.action == "propose") => {
                if self.subject.is_empty() {return Ok(self.ask_subject());}
                let summary = self.question.clone();
                self.review(m.decision.start, added, early || revising)?;
                return Ok(format!("{summary}\n\n{}\n\n{REVIEW}", self.account()));
            }
            "reflect" if !subject_phase => {
                self.phase = Phase::Reflection;
                if self.question.contains('?') {
                    self.question.push_str(" You can correct me, or say “looks right” to explore the subject.");
                } else {
                    self.question = format!("{}\n\n{REFLECT}", self.question);
                }
            }
            "hint" if subject_phase && previous.as_ref() == Some(&m.area) => {
                self.diagnostic = previous;
                self.cued = true;
            }
            "diagnostic" if subject_phase && self.profile.areas.get(&self.subject).is_some_and(|areas| areas.contains(&m.area)) => {
                self.diagnostic = Some(m.area);
            }
            "follow" | "explore" => {}
            _ => return Err("Choose a move allowed in this phase. A hint continues the pending diagnostic in the same area.".into()),
        }
        if self.phase == Phase::Review {
            self.phase = Phase::Subject;
            self.proposal = None;
        }
        if self.phase == Phase::Reflection && m.decision.action != "reflect" {
            self.phase = Phase::Learner;
        }
        Ok(self.question.clone())
    }

    pub fn correctable_sources(&self) -> Vec<(String, String)> {
        let superseded = self.profile.superseded();
        self.profile
            .facts
            .iter()
            .filter(|(id, e)| {
                !superseded.contains(*id) && (e.subject.is_empty() || e.subject == self.subject)
            })
            .map(|(id, e)| (id.clone(), e.quote.clone()))
            .collect()
    }
    fn review(
        &mut self,
        mut proposal: Start,
        added: Vec<String>,
        early: bool,
    ) -> Result<(), String> {
        let replaced = self.profile.superseded();
        let current = |id: &String, fact: &Evidence| {
            !replaced.contains(id) && (fact.subject.is_empty() || fact.subject == self.subject)
        };
        if !early {
            let covered = self.profile.areas.get(&self.subject).is_some_and(|areas| {
                !areas.is_empty()
                    && areas.iter().all(|area| {
                        self.profile
                            .facts
                            .iter()
                            .any(|(id, fact)| current(id, fact) && &fact.area == area)
                    })
            });
            if !covered {
                return Err("Explore a remaining subject area first, or honor an explicit request to start sooner.".into());
            }
        }
        let fields = [
            &proposal.objective,
            &proposal.exercise,
            &proposal.criterion,
            &proposal.why,
        ];
        if fields.iter().any(|s| s.trim().is_empty()) {
            return Err("Give a concrete exercise, objective, criterion and reason.".into());
        }
        if proposal.sources.iter().any(|s| s == "current") {
            proposal.sources.retain(|s| s != "current");
            proposal.sources.extend(added);
        }
        let has_evidence = self
            .profile
            .facts
            .iter()
            .any(|(id, f)| current(id, f) && f.kind != "declined");
        if (has_evidence && proposal.sources.is_empty())
            || proposal.sources.iter().any(|id| {
                self.profile
                    .facts
                    .get(id)
                    .is_none_or(|fact| !current(id, fact))
            })
        {
            return Err("Cite specific current evidence from this learner and subject.".into());
        }
        proposal.provisional = early;
        self.proposal = Some(proposal);
        self.phase = Phase::Review;
        self.question = REVIEW.into();
        Ok(())
    }

    pub fn next(
        &self,
        lesson: &mut Lesson,
        words: &str,
        source: &str,
        assigned: bool,
        generate: &mut Generate,
    ) -> Result<String, String> {
        let choice = words.trim().trim_end_matches(['.', '!']).to_lowercase();
        let mut state = self.clone();
        if !self.quick_choices().is_empty() {
            let prompt = match choice.as_str() {
                "a pattern" | "pattern" => Some(PATTERN),
                "a word puzzle" | "word puzzle" => Some(WORD),
                "my own topic" | "own topic" => Some(TOPIC),
                _ => None,
            };
            if let Some(prompt) = prompt {
                state.question = prompt.into();
                state.reason =
                    "One short reply will give us something concrete to build on.".into();
                state.thread = words.trim().into();
                lesson.discovery = Some(state);
                return Ok(prompt.into());
            }
        }
        if self.phase == Phase::Learner && self.question == PATTERN {
            let follow_up = match choice.as_str() {
                "8" | "eight" => Some("8 fits a jump of two. If we kept that rule, what number would come just before 2?"),
                n if n.parse::<i32>().is_ok() => Some("I was using jumps of two. Were you using a different rule?"),
                _ => None,
            };
            if let Some(prompt) = follow_up {
                state.question = prompt.into();
                state.reason = "The rule you noticed helps me choose what to try next.".into();
                lesson.discovery = Some(state);
                return Ok(prompt.into());
            }
        }
        if self.phase == Phase::Learner
            && self.question == WORD
            && choice.starts_with("rain")
            && choice.len() > 4
            && choice.chars().all(char::is_alphabetic)
            && choice.chars().count() < 32
        {
            let prompt = format!(
                "You started “{choice}” with rain. What does the second part add to its meaning?"
            );
            state.question = prompt.clone();
            state.reason = "We can use your word to see how its parts work together.".into();
            lesson.discovery = Some(state);
            return Ok(prompt);
        }
        if let Some(edit) = words.strip_prefix("/amend ") {
            let Some((target, correction)) = edit.trim().split_once(char::is_whitespace) else {
                return Ok("Use /amend SOURCE followed by your correction. Plan shows the source beside each detail.".into());
            };
            let Some(prior) = state.profile.facts.get(target) else {
                return Ok(
                    "I couldn’t find that source. Plan lists the saved details and their sources."
                        .into(),
                );
            };
            if correction.trim().is_empty()
                || state.profile.superseded().contains(&target.to_owned())
            {
                return Ok("Choose a current source and say what you would change.".into());
            }
            let revisit_placement = !assigned
                && state.phase == Phase::Instruction
                && state.proposal.as_ref().is_some_and(|p| {
                    p.objective == lesson.objective && p.sources.iter().any(|id| id == target)
                });
            let area = prior.area.clone();
            let entry = Evidence {
                subject: prior.subject.clone(),
                area: prior.area.clone(),
                kind: "correction".into(),
                quote: correction.trim().into(),
                reading: String::new(),
                replaces: vec![target.into()],
                task: None,
            };
            state.profile.facts.insert(format!("{source}.1"), entry);
            state.invalidate_recommendation();
            if revisit_placement {
                state.phase = Phase::Subject;
                state.reason = "Your correction changes the evidence for this starting point. Your previous exercise is saved.".into();
                state.question = if area.is_empty() {
                    "What would make the starting exercise a better fit?".into()
                } else {
                    format!("What would you like to try on your own in {area}?")
                };
            } else {
                state.reason = "You corrected the account I use to choose what we work on.".into();
            }
            let text = format!(
                "I’ve saved your correction: “{}”.\n{}",
                correction.trim(),
                state.question
            );
            lesson.discovery = Some(state);
            return Ok(text);
        }
        if choice == "/resume-lesson" && !lesson.exercise.is_empty() {
            state.phase = Phase::Instruction;
            state.proposal = None;
            state.question = lesson.active_step().into();
            state.reason = "You chose to return to the exercise you already had open.".into();
            lesson.observation = None;
            let text = format!("Let’s return to your exercise.\n\n{}", state.question);
            lesson.discovery = Some(state);
            return Ok(text);
        }
        let text = if self.phase == Phase::Review
            && matches!(
                choice.as_str(),
                "start"
                    | "/start"
                    | "yes, start"
                    | "yes, let’s start"
                    | "let’s start"
                    | "start the lesson"
                    | "let’s do it"
                    | "let's do it"
                    | "yes, please start"
                    | "yes, let's start"
                    | "that sounds right, let's do it"
                    | "yes, i'd like to try it"
                    | "let's start"
                    | "accept"
                    | "/accept"
            ) {
            let p = self.proposal.as_ref().ok_or("Missing proposed lesson.")?;
            lesson.open_supervised(p.objective.clone(), p.exercise.clone(), p.criterion.clone());
            if lesson.declared_goal.is_none() {
                lesson.declared_goal = Some(self.subject.clone());
            }
            if let Some(name) = &p.card {
                lesson.selected = Some(name.clone());
            }
            state.phase = Phase::Instruction;
            state.profile.agreements.insert(
                source.into(),
                AgreedStart {
                    subject: self.subject.clone(),
                    learner_reply: words.into(),
                    proposal: p.clone(),
                },
            );
            state.agreement = Some(source.into());
            state.question = p.exercise.clone();
            state.reason = p.why.clone();
            let example = p
                .card
                .as_deref()
                .map(|id| crate::material::card(id, 0))
                .transpose()?
                .and_then(|c| c["worked"].as_str().map(str::to_owned))
                .unwrap_or_default();
            format!(
                "Let’s begin. {}\n\n{}\n\n{}{}",
                p.why,
                FIT,
                if example.is_empty() {
                    String::new()
                } else {
                    format!("{example}\n\n")
                },
                p.exercise
            )
        } else {
            if self.phase == Phase::Review
                && matches!(choice.as_str(), "yes" | "looks right" | "/looks-right")
            {
                state.reason = "You confirmed the account. The first exercise is ready when you choose to start.".into();
                lesson.discovery = Some(state);
                return Ok("Thank you. Say “start” to open this exercise, or tell me what you’d like to explore first.".into());
            }
            let confirmed = self.phase == Phase::Reflection
                && matches!(
                    choice.as_str(),
                    "looks right"
                        | "/looks-right"
                        | "yes"
                        | "yes, that sounds like me"
                        | "that's right"
                        | "that sounds like me"
                        | "yes, that sounds right"
                        | "you got it"
                        | "that's a fair picture"
                        | "that sounds right"
                );
            if confirmed {
                state.phase = Phase::Subject;
            }
            let early = state.awaiting_subject_to_start
                || matches!(
                    choice.as_str(),
                    "/start-now"
                        | "start now"
                        | "let's start now"
                        | "start teaching me"
                        | "i want to start now"
                );
            if early
                && self.subject.is_empty()
                && !self.awaiting_subject_to_start
                && lesson.declared_goal.is_none()
                && !assigned
            {
                let text = state.ask_subject();
                lesson.discovery = Some(state);
                return Ok(text);
            }
            state.awaiting_subject_to_start = false;
            let mut packet = json!({"task":"discovery","state":state,"learner_message":words,"reflection_confirmed":confirmed,"provisional_requested":early,"source":source});
            let mut result = None;
            for attempt in 0..2 {
                let reply = generate(&packet, &shape(&state, early))?;
                let mut candidate = state.clone();
                let parsed = serde_json::from_value(reply)
                    .map_err(|e| e.to_string())
                    .and_then(|m: Move| {
                        if early && m.decision.action != "propose" {
                            return Err("Offer a provisional lesson now.".into());
                        }
                        candidate.prepare(words, source, m, early)
                    });
                match parsed {
                    Ok(text) => {
                        result = Some(text);
                        state = candidate;
                        break;
                    }
                    Err(e) if attempt == 0 => packet["repair"] = e.into(),
                    Err(e) => return Err(e),
                }
            }
            result.ok_or("Missing discovery turn.")?
        };
        lesson.discovery = Some(state);
        Ok(text)
    }
}

pub fn active(visit: &Visit) -> bool {
    visit
        .lesson
        .discovery
        .as_ref()
        .is_some_and(Discovery::active)
}

// Resolve only directories that the personal pilot already associates with this learner.
// A custom workspace outside that pair remains an independent learner.
pub fn related_profile(
    dir: Option<&std::path::Path>,
    current: Option<&std::path::Path>,
) -> Result<Profile, String> {
    use std::{env, fs, path::PathBuf};
    let mut profile = Profile::default();
    let Some(dir) = dir else { return Ok(profile) };
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let default = crate::terminal::default_workspace()?;
    let personal = env::var_os("ROBOTUTOR_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|| default.clone());
    let assignments = env::var_os("ROBOTUTOR_ASSIGNMENTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| default.parent().unwrap().join("assigned"));
    let mut dirs = BTreeSet::from([dir.clone()]);
    let linked = personal.canonicalize().ok().as_ref() == Some(&dir)
        || (dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().parse::<u64>().is_ok())
            && assignments.canonicalize().ok().as_deref() == dir.parent());
    if linked {
        if personal.exists() {
            dirs.insert(personal);
        }
        if assignments.exists() {
            for entry in fs::read_dir(assignments).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_dir()
                    && entry.file_name().to_string_lossy().parse::<u64>().is_ok()
                {
                    dirs.insert(entry.path());
                }
            }
        }
    }
    for folder in dirs {
        for path in crate::replay::journals(&folder)? {
            if Some(path.as_path()) == current {
                continue;
            }
            let Some(visit) =
                crate::optional_context("earlier discovery journal", crate::session::replay(&path))
            else {
                continue;
            };
            if let Some(d) = visit.lesson.discovery {
                profile.merge(&d.profile);
            }
        }
    }
    Ok(profile)
}

/// An explicitly selected published lesson can be the learner's provisional start.
pub fn offer_card(
    visit: &mut Visit,
    journal: &mut Option<crate::Journal>,
    id: &str,
    profile: Profile,
) -> Result<(), String> {
    let card = crate::material::card(id, 0)?;
    let mut state = Discovery::new(id.into(), profile);
    let mut lesson = visit.lesson.clone();
    lesson.clear_practice_trace();
    let objective = card["objective"]
        .as_str()
        .ok_or("Missing reviewed objective.")?;
    let why=String::from("You asked for this lesson. We can start with its opening exercise and use your work to explore the rest of the subject.");
    state.phase = Phase::Review;
    state.reason = why.clone();
    state.question = REVIEW.into();
    state.proposal = Some(Start {
        card: Some(id.into()),
        provisional: true,
        objective: objective.into(),
        exercise: card["practice"]
            .as_str()
            .ok_or("Missing reviewed exercise.")?
            .into(),
        criterion: objective.into(),
        why,
        sources: vec![],
    });
    let text = format!("{}\n\n{REVIEW}", state.account());
    lesson.selected = Some(id.into());
    lesson.declared_goal = Some(id.into());
    lesson.discovery = Some(state);
    deliver(visit, journal, lesson, text)
}

pub fn begin(
    visit: &mut Visit,
    journal: &mut Option<crate::Journal>,
    mut state: Discovery,
    legacy: bool,
) -> Result<(), String> {
    let mut lesson = visit.lesson.clone();
    lesson.clear_practice_trace();
    if state
        .profile
        .facts
        .values()
        .any(|f| f.subject.is_empty() && f.kind != "declined")
    {
        state.phase = Phase::Reflection;
        state.question = REFLECT.into();
        state.reason =
            "You can correct the earlier account before we use it in this lesson.".into();
    }
    if legacy {
        let examples: Vec<_> = visit
            .history
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, (s, _))| Some((i + 1, s.as_ref()?)))
            .filter(|(_, s)| !s.starts_with('/'))
            .take(2)
            .collect();
        if !examples.is_empty() {
            state.phase = Phase::Learner;
            state.question = format!(
                "{}\nWhich part of this earlier work still feels representative of how you think?",
                examples
                    .iter()
                    .map(|(n, s)| format!("You said “{s}” in turn {n}."))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }
    if legacy
        && visit
            .history
            .iter()
            .all(|(words, _)| words.as_deref().is_none_or(|s| s.starts_with('/')))
    {
        if let Some(obs) = visit.lesson.observation.as_ref().filter(|o| o.verified()) {
            state.question=format!("In earlier work you wrote “{}”. What does that example tell me about how you approach {} today?",obs.quote,obs.target.objective);
        } else if !visit.lesson.exercise.is_empty() {
            state.question = format!(
                "We were working on “{}”. What feels familiar to you about that work?",
                visit.lesson.active_step()
            );
        }
    }
    let mut text = state.opening();
    if legacy && !lesson.exercise.is_empty() {
        text.push_str("\nYou can return to the open exercise with /resume-lesson.");
    }
    lesson.discovery = Some(state);
    deliver(visit, journal, lesson, text)
}

pub fn deliver(
    visit: &mut Visit,
    journal: &mut Option<crate::Journal>,
    lesson: Lesson,
    text: String,
) -> Result<(), String> {
    crate::append(
        journal,
        visit,
        crate::Event::Delivered {
            text: text.clone(),
            lesson: Box::new(lesson),
            judgment: None,
            sources: vec![],
        },
    )?;
    println!(
        "\nLuna › {}\n{}",
        crate::display(&text),
        crate::judgment::trace(visit, visit.history.len(), &[], false)
    );
    Ok(())
}

pub fn profile_for(journal: Option<&crate::Journal>) -> Result<Profile, String> {
    match journal.filter(|j| j.workspace_mode) {
        Some(j) => related_profile(j.path.parent(), Some(&j.path)),
        None => Ok(Profile::default()),
    }
}

pub fn respond(
    config: &mut Option<crate::Config>,
    journal: &mut Option<crate::Journal>,
    visit: &mut Visit,
    session: &str,
) -> Result<(), String> {
    use tutor_lab::openrouter::{assistant_text, Role};
    let mut state = visit
        .lesson
        .discovery
        .clone()
        .ok_or("Missing discovery state.")?;
    let mut lesson = visit.lesson.clone();
    lesson.clear_practice_trace();
    let words = visit.pending.as_deref().ok_or("Missing learner message.")?;
    let assigned = journal.as_ref().and_then(|j| j.assignment.clone());
    // An early request may arrive before Luna has named the subject. The
    // pinned activity provides a concrete starting point in that case.
    if state.subject.is_empty() {
        if let Some(title) = assigned.as_ref().and_then(|a| a["title"].as_str()) {
            state.subject = title.into();
        }
    }
    let path = journal.as_ref().map(|j| j.path.clone());
    state.merge(&profile_for(journal.as_ref())?);
    let source = if let Some(parent) = path
        .as_deref()
        .and_then(|p| p.parent())
        .filter(|_| assigned.is_some())
    {
        format!(
            "assignment-{}/{session}",
            parent.file_name().unwrap_or_default().to_string_lossy()
        )
    } else {
        session.to_owned()
    };
    let source = format!("{source}#{}", visit.history.len() + 1);
    let work =
        crate::recovery::TurnWork::new(std::mem::take(&mut visit.checkpoints), journal.as_mut());
    let response = state.next(
        &mut lesson,
        words,
        &source,
        assigned.is_some(),
        &mut |packet, schema| {
            let c = crate::connection(config)?;
            let mut packet = packet.clone();
            packet["assigned_activity"] = json!(assigned);
            let request = crate::request(visit.teaching_history(), &packet, schema)?;
            work.get_or_run("discovery", c.model(Role::Luna), &request, || {
                let attempt = if packet.get("repair").is_some() { 2 } else { 1 };
                work.model_attempt("discovery", c.model(Role::Luna), attempt)?;
                let r = crate::recovery::while_waiting("discovery", true, || {
                    crate::metrics::provider("discovery", || c.complete(Role::Luna, &request))
                })?;
                serde_json::from_str(assistant_text(&r)?).map_err(|e| e.to_string())
            })
        },
    );
    visit.checkpoints = work.finish();
    match response {
        Ok(text) => deliver(visit, journal, lesson, text)?,
        Err(message) => {
            *config = None;
            crate::append(
                journal,
                visit,
                crate::Event::Failed {
                    message: message.clone(),
                },
            )?;
            eprintln!(
                "{} Your message is saved. Use /retry to resume.",
                crate::display(&message)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
