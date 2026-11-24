//! Versioned teaching decisions. Model proposals are evidence candidates, never grades.
use super::assessment::Verdict;
use super::lesson::{object, Closed, Generate, Lesson, Route};
use super::session::Visit;
use super::study::Target;
use super::trace_view::Layout;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const FIRST_VERSION: &str = "teaching-judgment/1";
const SECOND_VERSION: &str = "teaching-judgment/2";
const THIRD_VERSION: &str = "teaching-judgment/3";
const FOURTH_VERSION: &str = "teaching-judgment/4";
pub(super) const FIFTH_VERSION: &str = "teaching-judgment/5";
pub(super) const SIXTH_VERSION: &str = "teaching-judgment/6";
pub(super) const SEVENTH_VERSION: &str = "teaching-judgment/7";
pub(super) const EIGHTH_VERSION: &str = "teaching-judgment/8";
pub(super) const NINTH_VERSION: &str = "teaching-judgment/9";
pub(super) const TENTH_VERSION: &str = "teaching-judgment/10";
pub(super) const ELEVENTH_VERSION: &str = "teaching-judgment/11";
pub const VERSION: &str = "teaching-judgment/12";
fn first_version() -> String {
    FIRST_VERSION.into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Method,
    PromptedMethod,
    Strength,
    Difficulty,
    Preference,
    Correct,
    Support,
    Narrow,
    Contradict,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub kind: Kind,
    pub source: String,
    pub speaker: String,
    pub task: String,
    pub quote: String,
    pub detail: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_order: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    #[serde(flatten)]
    pub proposal: Proposal,
}

impl Finding {
    fn supersedes(&self) -> bool {
        matches!(
            self.proposal.kind,
            Kind::Correct | Kind::Narrow | Kind::Contradict
        )
    }
}

pub fn active(all: &[Finding]) -> Vec<Finding> {
    let retired: BTreeSet<_> = all
        .iter()
        .filter(|f| f.supersedes())
        .map(|f| &f.proposal.target)
        .collect();
    let by_id: BTreeMap<_, _> = all
        .iter()
        .filter(|f| {
            !retired.contains(&f.id) && !matches!(f.proposal.kind, Kind::Contradict | Kind::Support)
        })
        .map(|f| (&f.id, f))
        .collect();
    by_id.into_values().cloned().collect()
}

pub fn task(lesson: &Lesson) -> &str {
    [&lesson.objective, &lesson.topic]
        .into_iter()
        .map(String::as_str)
        .find(|s| !s.is_empty())
        .or(lesson.selected.as_deref())
        .unwrap_or("lesson intake")
}
pub fn observe(
    visit: &Visit,
    source: &str,
    known: &[Finding],
    generate: &mut Generate,
) -> Result<Value, String> {
    let Some(words) = visit.pending.as_deref().filter(|s| !s.starts_with('/')) else {
        return Ok(json!({"findings":[]}));
    };
    let string = json!({"type":"string"});
    let item = object(
        json!({"kind":{"type":"string","enum":["method","prompted_method","strength","difficulty","preference","correct","support","narrow","contradict"]},
        "source":string,"speaker":string,"task":string,"quote":string,"detail":string,"target":string,"teaching_order":{"type":["string","null"],"enum":[null,"facts_first","framework_first"]}}),
    );
    let candidate = object(json!({"term":{"type":"string"},"definition_quote":{"type":"string"}}));
    let shape = object(
        json!({"findings":{"type":"array","items":item,"maxItems":4},"progression":{"anyOf":[crate::progression::schema(),{"type":"null"}]},"recall_candidate":{"anyOf":[candidate,{"type":"null"}]}}),
    );
    generate(
        &json!({"task":"observe","source":source,"speaker":"learner","task_label":task(&visit.lesson),
        "recent_question_demands":visit.recent_questions,"recent_reasoning":visit.lesson.reasoning,"course":visit.lesson.course,"open_step":visit.lesson.active_step(),"learner_message":words,"active_findings":known,"previous_tutor_reply":visit.teaching_history().last().map(|(_,r)|r)}),
        &shape,
    )
}

/// Match a whole term. In particular "miracle" is not a prior introduction of IRAC.
pub fn mentions(text: &str, term: &str) -> bool {
    let text = text.to_lowercase();
    let term = term.trim().to_lowercase();
    !term.is_empty()
        && text.match_indices(&term).any(|(i, _)| {
            !text[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
                && !text[i + term.len()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
        })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub target: Target,
    pub previous: String,
    pub tasks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inputs {
    #[serde(default = "first_version")]
    pub version: String,
    pub source: String,
    pub shadow: bool,
    pub proposals: Value,
    // Only other journals can supply these inputs. Local history is reconstructed on replay.
    pub external_findings: Vec<Finding>,
    pub external_cues: Vec<(String, String)>,
    pub probe: Option<Probe>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Decision {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progression: Option<crate::progression::Reading>,
    pub policy: String,
    pub source: String,
    pub shadow: bool,
    pub findings: Vec<Finding>,
    pub rejected: Vec<String>,
    pub alternatives: Vec<String>,
    pub selected: String,
    pub why: String,
    pub guidance: Vec<Finding>,
    pub delivered_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tutor_cue: Option<(String, String)>,
}

/// Compact delivery context for a continuation in a different visit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rationale {
    pub source: String,
    pub action: String,
    pub reason: String,
    pub evidence: Vec<(String, String)>,
    pub tutor_cue: Option<(String, String)>,
}

impl Decision {
    pub fn rationale(&self) -> Option<Rationale> {
        let check = self.next_check.as_ref()?;
        Some(Rationale {
            source: self.source.clone(),
            action: self.delivered_action.clone(),
            reason: format!("{} {}", self.why, check),
            evidence: self
                .guidance
                .iter()
                .rev()
                .map(|f| (f.id.clone(), f.proposal.quote.clone()))
                .collect(),
            tutor_cue: self.tutor_cue.clone(),
        })
    }
    fn set_next_check(&mut self) {
        if self.policy != VERSION || self.shadow {
            return;
        }
        self.next_check = Some(match self.selected.as_str() {
            "use_method" => "Apply your method to a changed fact. If the reasoning carries over, move to the next objective.",
            "use_prompted_method" => "Try the next case without a method cue. Independent recognition would let us skip that scaffold.",
            "partial" | "repair" => "Keep the part that worked. Resolving the remaining step would let us move on.",
            "support" | "explain_only" => "Use the explanation on a fresh example before checking independent use.",
            "complete" | "next" | "harder" => "Try the relationship under changed conditions. A difficulty there would tell us what to revisit.",
            "review" | "revisit" => "Try it again without help. A cue changes what we can credit independently.",
            "pause" => "Return to the open question when you are ready.",
            "preference" => "Check whether the next step fits your correction or preference.",
            _ => "Use the next response to decide whether to continue here or change the starting point.",
        }.into());
    }
    pub fn refine(&mut self, action: &str) {
        if self.policy == FIRST_VERSION || self.shadow || self.selected == action {
            return;
        }
        if !self
            .alternatives
            .iter()
            .any(|candidate| candidate == action)
        {
            self.alternatives.push(action.into());
        }
        self.why = match action {
            "support" => "A focused retry was already attempted. Work through the sticking point.",
            "partial" | "repair" | "complete" => {
                "The assessment of the submitted work determined the teaching move."
            }
            "question" => "Answer the question within the current topic.",
            _ => "The current lesson state determined the permitted delivery.",
        }
        .into();
        self.selected = action.into();
        self.set_next_check();
    }
    pub fn facts_first(&self) -> bool {
        self.policy != FIRST_VERSION
            && self.guidance.iter().any(|f| {
                let p = &f.proposal;
                if !matches!(p.kind, Kind::Correct | Kind::Narrow | Kind::Preference) {
                    return false;
                }
                match p.teaching_order.as_deref() {
                    Some("facts_first") => true,
                    None if p.kind == Kind::Correct => {
                        let quote = p.quote.to_lowercase();
                        [
                            "start with the concrete facts",
                            "start with the facts",
                            "give me the facts first",
                            "show me the facts first",
                        ]
                        .iter()
                        .any(|s| quote.starts_with(s) || quote.contains(&format!(", {s}")))
                    }
                    _ => false,
                }
            })
    }
    pub fn revises(&self) -> bool {
        self.findings.iter().any(Finding::supersedes)
    }
    pub fn uses_legacy_method_rules(&self) -> bool {
        matches!(self.policy.as_str(), FIRST_VERSION | SECOND_VERSION)
    }
    pub fn method(&self) -> Option<&str> {
        self.findings
            .iter()
            .find(|f| {
                if self.uses_legacy_method_rules() {
                    return matches!(f.proposal.kind, Kind::Method | Kind::PromptedMethod);
                }
                matches!(
                    (self.selected.as_str(), &f.proposal.kind),
                    ("use_method", Kind::Method) | ("use_prompted_method", Kind::PromptedMethod)
                )
            })
            .map(|f| f.proposal.detail.as_str())
    }
    pub fn check_draft(
        &self,
        draft: &Value,
        closed: &[Closed],
        previous: Option<&str>,
    ) -> Result<(), String> {
        if self.selected != "use_method" || self.shadow {
            return Ok(());
        }
        let method = self.method().ok_or("Missing method evidence.")?;
        let body = format!(
            "{} {}",
            draft["new_fact"].as_str().unwrap_or(""),
            draft["exercise"].as_str().unwrap_or("")
        );
        let next = draft["exercise"].as_str().unwrap_or("");
        let norm = |s: &str| {
            s.split(|c: char| !c.is_alphanumeric())
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        if [" stands for", " means", " ="].iter().any(|s| {
            body.to_lowercase()
                .contains(&format!("{}{s}", method.to_lowercase()))
        }) || previous
            .into_iter()
            .chain(closed.iter().map(|c| c.exercise.as_str()))
            .any(|old| norm(old) == norm(next))
        {
            return Err("Use the recognized method on a new issue without repeating its introduction or the old question.".into());
        }
        Ok(())
    }
}

fn bare_assent(words: &str) -> bool {
    let norm = words
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .to_lowercase();
    matches!(
        norm.as_str(),
        "yes"
            | "yeah"
            | "yep"
            | "exactly"
            | "yes exactly"
            | "yes exactly that"
            | "that's right"
            | "right"
            | "okay"
            | "ok"
            | "correct"
            | "got it"
            | "thanks"
    )
}

pub fn validate(input: &Inputs, visit: &Visit, known: &[Finding]) -> (Vec<Finding>, Vec<String>) {
    let mut findings = Vec::new();
    let mut rejected = Vec::new();
    if input.proposals.get("observer_unavailable").is_some() {
        rejected.push("Observer unavailable for this turn".into());
    }
    let Some(items) = input.proposals["findings"]
        .as_array()
        .filter(|v| v.len() <= 4)
    else {
        return (findings, vec!["Invalid observation list".into()]);
    };
    for (i, raw) in items.iter().enumerate() {
        let Ok(mut p) = serde_json::from_value::<Proposal>(raw.clone()) else {
            rejected.push(format!("Finding {} has an invalid shape", i + 1));
            continue;
        };
        let revision = matches!(
            p.kind,
            Kind::Correct | Kind::Narrow | Kind::Contradict | Kind::Support
        );
        let supplied = matches!(p.kind, Kind::Method | Kind::PromptedMethod)
            && (visit.history.iter().any(|(_, t)| mentions(t, &p.detail))
                || mentions(visit.lesson.active_step(), &p.detail)
                || input
                    .external_cues
                    .iter()
                    .any(|(_, t)| mentions(t, &p.detail)));
        // The observer nominates a quoted method. The recorded exchanges determine who named it.
        if input.version == VERSION && matches!(p.kind, Kind::Method | Kind::PromptedMethod) {
            p.kind = if supplied {
                Kind::PromptedMethod
            } else {
                Kind::Method
            };
        }
        let reason = if p.source != input.source
            || p.speaker != "learner"
            || p.task != task(&visit.lesson)
        {
            Some("unmatched source, speaker or task")
        } else if p.quote.trim().is_empty()
            || p.detail.trim().is_empty()
            || p.detail.len() > 800
            || !visit.pending.as_ref().is_some_and(|s| s.contains(&p.quote))
        {
            Some("unsupported learner citation")
        } else if p.teaching_order.as_deref().is_some_and(|order| {
            !matches!(order, "facts_first" | "framework_first")
                || !matches!(p.kind, Kind::Preference | Kind::Correct | Kind::Narrow)
        }) {
            Some("unsupported teaching order")
        } else if bare_assent(visit.pending.as_deref().unwrap_or("")) {
            Some("assent alone does not supply the tutor’s reasoning")
        } else if revision == p.target.is_empty()
            || revision && !known.iter().any(|f| f.id == p.target)
        {
            Some("unknown revision target")
        } else if matches!(p.kind, Kind::Method | Kind::PromptedMethod)
            && (!mentions(&p.quote, &p.detail) || visit.lesson.objective.is_empty())
        {
            Some("method missing from the evidence or no open instructional context")
        } else if p.kind == Kind::Method && supplied {
            Some("the tutor already supplied the method")
        } else if p.kind == Kind::PromptedMethod && !supplied {
            Some("no recorded tutor introduction")
        } else if findings.iter().any(|f: &Finding| {
            f.proposal == p || revision && f.supersedes() && f.proposal.target == p.target
        }) {
            Some("duplicate or conflicting finding")
        } else {
            None
        };
        if let Some(reason) = reason {
            rejected.push(format!("Finding {}: {reason}", i + 1));
        } else {
            findings.push(Finding {
                id: format!("{}/{}", input.source, i + 1),
                proposal: p,
            });
        }
    }
    (findings, rejected)
}

pub fn known(visit: &Visit, external: &[Finding]) -> Vec<Finding> {
    let mut all = external.to_vec();
    all.extend(visit.findings().cloned());
    active(&all)
}

fn select(input: &Inputs, visit: &Visit, route: &Route) -> Decision {
    let prior = known(visit, &input.external_findings);
    let (findings, rejected) = validate(input, visit, &prior);
    let mut options = Vec::new();
    // Ordered eligibility is the tie-break. Keep only moves actually considered.
    if findings.iter().any(|f| f.proposal.kind == Kind::Correct)
        && (input.version == FIRST_VERSION || route.action != "pause")
    {
        options.push((
            "preference",
            "The learner corrected an earlier reading. Apply the correction now.",
        ));
    }
    if matches!(
        route.action.as_str(),
        "pause"
            | "discussion"
            | "explain_only"
            | "preference"
            | "question"
            | "next"
            | "harder"
            | "revisit"
            | "support"
            | "use_source_or_topic"
            | "resume_practice"
            | "review"
    ) {
        options.push((
            route.action.as_str(),
            "Honor the learner’s current request.",
        ));
    }
    if visit.lesson.awaiting_guess
        && matches!(route.action.as_str(), "complete" | "partial" | "repair")
    {
        options.push((
            "discuss_guess",
            "A best guess was invited. Discuss it without grading.",
        ));
    }
    if matches!(
        input.version.as_str(),
        FIFTH_VERSION
            | SIXTH_VERSION
            | SEVENTH_VERSION
            | EIGHTH_VERSION
            | NINTH_VERSION
            | TENTH_VERSION
            | ELEVENTH_VERSION
            | VERSION
    ) && route.action == "complete"
        && (input.version != VERSION
            || visit.lesson.rubric.is_some()
            || visit.lesson.check_form.is_some()
            || !findings
                .iter()
                .any(|f| matches!(f.proposal.kind, Kind::Method | Kind::PromptedMethod)))
        && !visit.lesson.awaiting_guess
    {
        options.push((
            "complete",
            "Assess the answer against the open exercise before planning the next objective.",
        ));
    }
    if findings.iter().any(|f| f.proposal.kind == Kind::Method) && !visit.lesson.explanation_only {
        options.push(("use_method","The learner introduced a method in this task. Use it to explore a new issue and skip the introduction."));
    }
    if findings
        .iter()
        .any(|f| f.proposal.kind == Kind::PromptedMethod)
        && !visit.lesson.explanation_only
    {
        options.push((
            "use_prompted_method",
            "The tutor supplied the method. Use it as a scaffold for the next case-specific step.",
        ));
    }
    if !options.iter().any(|(a, _)| *a == route.action) {
        options.push((
            &route.action,
            "Continue the lesson under its current task and assessment rules.",
        ));
    }
    let (action, why) = options[0];
    let mut all = prior;
    all.extend(findings.clone());
    let guidance = active(&all)
        .into_iter()
        .filter(|f| {
            matches!(
                f.proposal.kind,
                Kind::Correct | Kind::Narrow | Kind::Preference
            ) || f.proposal.task == task(&visit.lesson)
        })
        .collect();
    let tutor_cue = findings
        .iter()
        .find(|f| f.proposal.kind == Kind::PromptedMethod)
        .and_then(|f| {
            let term = &f.proposal.detail;
            let session = input.source.rsplit_once('#')?.0;
            visit
                .history
                .iter()
                .enumerate()
                .find(|(_, (_, reply))| mentions(reply, term))
                .map(|(i, (_, reply))| (format!("{session}#{}", i + 1), reply.as_str()))
                .or_else(|| {
                    input
                        .external_cues
                        .iter()
                        .find(|(_, reply)| mentions(reply, term))
                        .map(|(id, reply)| (id.clone(), reply.as_str()))
                })
                .map(|(id, reply)| {
                    (
                        id,
                        reply
                            .lines()
                            .find(|line| mentions(line, term))
                            .unwrap_or(reply)
                            .chars()
                            .take(600)
                            .collect(),
                    )
                })
        });
    let mut decision = Decision {
        tutor_cue: tutor_cue.filter(|_| input.version == VERSION),
        next_check: None,
        progression: None,
        policy: input.version.clone(),
        source: input.source.clone(),
        shadow: input.shadow,
        findings,
        rejected,
        alternatives: options.iter().map(|(s, _)| s.to_string()).collect(),
        selected: action.into(),
        why: why.into(),
        guidance,
        delivered_action: String::new(),
    };
    decision.set_next_check();
    decision
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Call {
    pub task: String,
    pub input_hash: String,
    pub output: Value,
}
fn signature(packet: &Value, schema: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(packet, schema)).expect("JSON values serialize"))
    )
}
impl Call {
    pub fn new(packet: &Value, schema: &Value, output: Value) -> Self {
        Self {
            task: packet["task"].as_str().unwrap_or("").into(),
            input_hash: signature(packet, schema),
            output,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub inputs: Inputs,
    pub calls: Vec<Call>,
    pub assessment: Option<Verdict>,
    pub decision: Decision,
}

pub fn teach(
    visit: &Visit,
    input: &Inputs,
    generate: &mut Generate,
    assess: &mut dyn FnMut(&Lesson, &str, &str) -> Verdict,
) -> Result<(Decision, Lesson, String), String> {
    let mut lesson = visit.lesson.clone();
    lesson.clear_practice_trace();
    let words = visit.pending.as_deref();
    if input.version == VERSION && !input.shadow && words.is_some_and(|w| w.starts_with("/amend "))
    {
        let mut d = select(input, visit, &Route::control("preference"));
        let correction = d
            .findings
            .iter()
            .find(|f| f.proposal.kind == Kind::Correct)
            .ok_or("The correction needs an active source and the learner’s own words.")?;
        let corrected = &correction.proposal;
        let prior = known(visit, &input.external_findings);
        let original = prior
            .iter()
            .find(|f| f.id == corrected.target)
            .ok_or("Unknown correction target.")?;
        let reply = format!(
            "I’ve updated my reading of “{}”. You said “{}”.",
            original.proposal.quote, corrected.quote
        );
        d.why = format!(
            "Your correction supersedes my earlier reading at {}.",
            original.proposal.source
        );
        let question = match original.proposal.kind {
            Kind::Method | Kind::PromptedMethod | Kind::Strength | Kind::Difficulty => {
                let q = format!(
                    "Looking again at {}, which step can you work through independently?",
                    task(&lesson)
                );
                lesson.open_supervised(lesson.objective.clone(), q.clone(), "Identify and explain a step independently before revising the course placement.".into());
                q
            }
            _ => lesson.current_question(),
        };
        d.delivered_action = "preference".into();
        return Ok((d, lesson, format!("{reply}\n\n{question}")));
    }
    let auto_advance = matches!(
        input.version.as_str(),
        FOURTH_VERSION
            | FIFTH_VERSION
            | SIXTH_VERSION
            | SEVENTH_VERSION
            | EIGHTH_VERSION
            | NINTH_VERSION
            | TENTH_VERSION
            | ELEVENTH_VERSION
            | VERSION
    );
    if matches!(
        input.version.as_str(),
        EIGHTH_VERSION | NINTH_VERSION | TENTH_VERSION | ELEVENTH_VERSION | VERSION
    ) && !input.shadow
    {
        // Version 7 sometimes answered an ambiguous naming question through normal review.
        if lesson.name_recall.is_none()
            && visit
                .teaching_history()
                .last()
                .is_some_and(|(asked, reply)| {
                    asked.as_deref().is_some_and(crate::method::name_request)
                        && crate::method::supplies_name(reply)
                })
        {
            lesson.name_recall = Some(crate::method::NameRecall::Supplied);
        }
    }
    if matches!(
        input.version.as_str(),
        SEVENTH_VERSION
            | EIGHTH_VERSION
            | NINTH_VERSION
            | TENTH_VERSION
            | ELEVENTH_VERSION
            | VERSION
    ) && !input.shadow
    {
        let meaning = if matches!(
            input.version.as_str(),
            NINTH_VERSION | TENTH_VERSION | ELEVENTH_VERSION | VERSION
        ) {
            lesson.capture_meaning(visit, input);
            lesson.meaning_turn(
                words,
                matches!(input.version.as_str(), ELEVENTH_VERSION | VERSION),
                generate,
            )?
        } else {
            None
        };
        if let Some((action, why, text)) = meaning.or_else(|| lesson.method_turn(visit, input)) {
            if action.starts_with("meaning_") {
                lesson.method_action(action);
            }
            let mut decision = select(input, visit, &Route::control(action));
            decision.refine(action);
            decision.why = why.into();
            decision.delivered_action = action.into();
            // A vocabulary exchange cannot provide evidence about executing the method.
            decision.findings.retain(|f| {
                !matches!(
                    f.proposal.kind,
                    Kind::Method | Kind::PromptedMethod | Kind::Strength | Kind::Difficulty
                )
            });
            return Ok((decision, lesson, super::display(&text)));
        }
    }
    let route = if matches!(
        input.version.as_str(),
        EIGHTH_VERSION | NINTH_VERSION | TENTH_VERSION | ELEVENTH_VERSION | VERSION
    ) && !input.shadow
        && words == Some("/practice irac")
    {
        // The authored cases are exhausted. Request a new reviewed application.
        Route::control("harder")
    } else if input.probe.is_some() {
        Route::control("review")
    } else {
        lesson.route(
            words,
            auto_advance,
            matches!(
                input.version.as_str(),
                SIXTH_VERSION
                    | SEVENTH_VERSION
                    | EIGHTH_VERSION
                    | NINTH_VERSION
                    | TENTH_VERSION
                    | ELEVENTH_VERSION
                    | VERSION
            ),
            generate,
        )?
    };
    let mut decision = select(input, visit, &route);
    let guided = !input.shadow;
    if guided
        && matches!(
            input.version.as_str(),
            TENTH_VERSION | ELEVENTH_VERSION | VERSION
        )
        && (input.version != VERSION || decision.method().is_none())
        && input.probe.is_none()
        && lesson.selected.is_none()
        && !lesson.explanation_only
        && !lesson.objective.is_empty()
        && matches!(
            route.action.as_str(),
            "partial" | "repair" | "complete" | "discussion" | "harder" | "support"
        )
    {
        decision.progression = words
            .filter(|w| !w.starts_with('/'))
            .and_then(|w| crate::progression::read(input, &lesson, w));
    }
    let route = if guided && route.action != decision.selected && decision.progression.is_none() {
        Route::control(&decision.selected)
    } else {
        route
    };
    let mut text = if let Some(probe) = input.probe.as_ref().filter(|_| route.action == "review") {
        if input.version == VERSION {
            if let (Some(application), Some(source)) = (&probe.application, &probe.source) {
                decision.why = format!(
                    "Revisit the earlier work on {} ({source}) in a fresh application.",
                    probe.target.objective
                );
                decision.next_check = Some(format!("Use that reasoning while working on {application}. An unaided explanation supports transfer here. A hint calls for another independent check."));
            }
        }
        let exercise = lesson.probe(
            &probe.target,
            &probe.previous,
            &probe.tasks,
            &mut |packet, schema| {
                if guided && input.version != FIRST_VERSION {
                    let mut request = packet.clone();
                    request["teaching_decision"] = json!(decision);
                    if input.version == VERSION {
                        if let Some(application) = &probe.application {
                            request["new_application"] = json!(application);
                            request["instruction"] = json!(format!("{} Embed the earlier objective in the new application. Ask a single question that requires the earlier reasoning without naming the method. Check the exercise against both the new context and the earlier criterion.",packet["instruction"].as_str().unwrap_or_default()));
                        }
                    }
                    generate(&request, schema)
                } else {
                    generate(packet, schema)
                }
            },
        )?;
        if !input.shadow && input.version == VERSION && matches!(words, Some("/next" | "/harder")) {
            if let (Some(course), Some(application)) = (&mut lesson.course, &probe.application) {
                if course
                    .milestones
                    .get(course.focus.saturating_add(1))
                    .is_some_and(|m| &m.application == application)
                {
                    course.navigate("next");
                }
            }
        }
        exercise
    } else {
        let scope = Target::from_lesson(&visit.lesson);
        let demonstrated: Vec<_> = visit
            .assessments
            .values()
            .filter(|o| {
                o.demonstrated() && o.target.label() == scope.label() && o.target.card == scope.card
            })
            .map(|o| o.target.objective.as_str())
            .collect();
        lesson.deliver(
            words,
            route,
            guided.then_some(&decision),
            auto_advance,
            &demonstrated,
            generate,
            assess,
        )?
    };
    if matches!(
        input.version.as_str(),
        SEVENTH_VERSION
            | EIGHTH_VERSION
            | NINTH_VERSION
            | TENTH_VERSION
            | ELEVENTH_VERSION
            | VERSION
    ) {
        lesson.update_name_recall(&visit.lesson);
        if matches!(
            input.version.as_str(),
            EIGHTH_VERSION | NINTH_VERSION | TENTH_VERSION | ELEVENTH_VERSION | VERSION
        ) && !input.shadow
            && words.is_some_and(crate::method::name_request)
            && crate::method::supplies_name(&text)
        {
            lesson.name_recall = Some(crate::method::NameRecall::Supplied);
        }
    }
    if matches!(
        input.version.as_str(),
        NINTH_VERSION | TENTH_VERSION | ELEVENTH_VERSION | VERSION
    ) && !input.shadow
    {
        lesson.finish_meaning_turn(
            &visit.lesson,
            words,
            &input.source,
            &mut text,
            matches!(input.version.as_str(), ELEVENTH_VERSION | VERSION),
        );
    }
    decision.delivered_action = lesson.action().into();
    decision.refine(lesson.action());
    if matches!(
        input.version.as_str(),
        NINTH_VERSION | TENTH_VERSION | ELEVENTH_VERSION | VERSION
    ) && !input.shadow
    {
        if let Some(item) = lesson.asking_meaning() {
            decision.why.push_str(&format!(
                " Intervening work made a check of {} appropriate (/show {}).",
                item.term, item.source
            ));
        }
    }
    if let Some(p) = &decision.progression {
        decision
            .why
            .push_str(&format!(" {} (/show {}).", p.reason, p.source));
        lesson.reasoning.push(p.clone());
        if lesson.reasoning.len() > 12 {
            lesson.reasoning.remove(0);
        }
    }
    Ok((decision, lesson, super::display(&text)))
}

impl Receipt {
    pub fn verify(&self, visit: &Visit, saved: &Lesson, text: &str) -> Result<(), String> {
        if self.inputs.version != self.decision.policy
            || !matches!(
                self.decision.policy.as_str(),
                FIRST_VERSION
                    | SECOND_VERSION
                    | THIRD_VERSION
                    | FOURTH_VERSION
                    | FIFTH_VERSION
                    | SIXTH_VERSION
                    | SEVENTH_VERSION
                    | EIGHTH_VERSION
                    | NINTH_VERSION
                    | TENTH_VERSION
                    | ELEVENTH_VERSION
                    | VERSION
            )
        {
            return Err(format!(
                "Unsupported teaching policy {}",
                self.decision.policy
            ));
        }
        let mut calls: VecDeque<_> = self.calls.iter().collect();
        let mut verdict = self.assessment.clone();
        let mut missing_assessment = false;
        let (decision, lesson, reply) = teach(
            visit,
            &self.inputs,
            &mut |packet, schema| {
                let call = calls
                    .pop_front()
                    .ok_or("Replay is missing a model proposal.")?;
                if signature(packet, schema) != call.input_hash {
                    return Err(format!("Replay input mismatch at {}", call.task));
                }
                Ok(call.output.clone())
            },
            &mut |_, _, action| {
                verdict.take().unwrap_or_else(|| {
                    missing_assessment = true;
                    Verdict::provisional(action)
                })
            },
        )?;
        if !calls.is_empty()
            || missing_assessment
            || verdict.is_some()
            || self.decision != decision
            || text != reply
            || serde_json::to_value(saved).unwrap() != serde_json::to_value(lesson).unwrap()
        {
            return Err(format!(
                "Decision, learner record or delivered reply differs at {}",
                self.inputs.source
            ));
        }
        Ok(())
    }
}

pub fn trace(visit: &Visit, n: usize, later: &[Finding], full: bool) -> String {
    render_trace(visit, n, later, full, Layout::interactive())
}

pub fn render_trace(
    visit: &Visit,
    n: usize,
    later: &[Finding],
    full: bool,
    layout: Layout,
) -> String {
    let Some((_, reply)) = n.checked_sub(1).and_then(|i| visit.history.get(i)) else {
        return "No delivered turn at that number.".into();
    };
    if let Some(reason) = visit.discovery_trace.get(&n) {
        return layout.render(&[("Why this step", reason)], None, full.then_some(reply));
    }
    let Some(receipt) = &visit.decisions[n - 1] else {
        return layout.render(
            &[("Trace unavailable", "earlier turn")],
            None,
            full.then_some(reply),
        );
    };
    let d = &receipt.decision;
    if d.shadow && !full {
        return String::new();
    }
    let joined = |v: Vec<String>, otherwise: &str| {
        if v.is_empty() {
            otherwise.into()
        } else {
            v.join(" · ")
        }
    };
    let observed = joined(
        d.findings
            .iter()
            .map(|f| format!("“{}” (/show {})", f.proposal.quote, f.proposal.source))
            .collect(),
        "no new observation",
    );
    let observed = if !full && observed.chars().count() > 130 {
        format!(
            "{}… (/show {})",
            observed.chars().take(100).collect::<String>(),
            d.source
        )
    } else {
        observed
    };
    let inferred=joined(d.findings.iter().map(|f|match f.proposal.kind {
        Kind::Method=>format!("You recognized {} here. Independent use and transfer remain unverified.",f.proposal.detail),
        Kind::PromptedMethod=>format!("You referred to {} after the tutor introduced it. Independent recognition remains unverified.",f.proposal.detail),
        Kind::Correct=>format!("Corrected reading: {}",f.proposal.detail),
        _=>format!("Tentative: {}",f.proposal.detail)
    }).collect(),"none");
    let next = joined(
        visit
            .findings()
            .chain(later.iter())
            .filter(|f| d.findings.iter().any(|old| old.id == f.proposal.target))
            .map(|f| format!("{:?} · /show {}", f.proposal.kind, f.proposal.source))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        "pending",
    );
    let changed = match d.delivered_action.as_str() {
        "use_prompted_method" => "Used the tutor-supplied framework to focus the open case.",
        "use_method" => "Skipped the method introduction and opened a new application.",
        "discuss_guess" => "Discussed the invited guess without grading it.",
        "preference" => "Adapted this turn to the learner’s stated preference.",
        "method_name" => "Supplied the name and saved a later recall check.",
        "method_practice" => "Opened a new case that calls for a reasoned recommendation.",
        "meaning_check" => "Asked for the meaning of a previously taught concept.",
        "meaning_answer" => "Recorded concept recall and resumed the open lesson.",
        "name_check" => "Asked for the name after intervening work.",
        "name_answer" => "Recorded the vocabulary response and returned to the lesson.",
        "discussion" => "Responded to the reasoning in the discussion.",
        "question" => "Answered the learner’s question.",
        "support" => "Offered support with the current difficulty.",
        "complete" => "Moved on from the assessed exercise.",
        "open_next_topic" => "Opened the next topic after the previous exercise was completed.",
        "partial" | "repair" => "Focused feedback on the current exercise.",
        "review" => "Opened a fresh review task.",
        "start" => "Opened the lesson.",
        "acknowledge" => "Kept the current step open.",
        "explain_only" => "Set practice aside to explain.",
        "pause" => "Paused the lesson.",
        "harder" | "next" => "Retired the earlier task and moved ahead.",
        "use_source_or_topic" => "Followed the requested topic or scope.",
        "revisit" => "Returned to the requested skill.",
        "resume_practice" => "Reopened practice on the current objective.",
        _ => &d.delivered_action,
    };
    let order = if d.facts_first()
        && !d.shadow
        && !matches!(
            d.delivered_action.as_str(),
            "acknowledge" | "pause" | "review"
        ) {
        " Opened with concrete facts as requested."
    } else {
        ""
    };
    let rationale = [Some(d.why.as_str()), d.next_check.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let why = if d.shadow {
        "Shadow candidate only. The ordinary lesson policy delivered the reply."
    } else {
        &rationale
    };
    layout.render(
        &[
            ("Observed", &observed),
            ("Inferred", &inferred),
            ("Changed", &format!("{changed}{order}")),
            ("Why this move", why),
            ("What happened next", &next),
        ],
        full.then(|| serde_json::to_string_pretty(d).unwrap()),
        full.then_some(reply),
    )
}

#[cfg(test)]
mod tests;
