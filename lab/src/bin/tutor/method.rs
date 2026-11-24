//! Keep recall of a method's name separate from evidence on a substantive exercise.
use crate::{
    judgment::{mentions, Inputs},
    lesson::Lesson,
    session::Visit,
};
use serde::{Deserialize, Serialize};

pub const CHECK: &str = "A quick memory question. What four-letter name did we use for the way you organized a legal argument? A guess or ‘skip’ is fine.";
const NAME: &str = "IRAC stands for Issue, Rule, Application, Conclusion.";
const CASES: &[&str] = &[
    "In this fictional town, a library may exclude a visitor only for materially disrupting library services. Mira quietly handed out flyers. Six people stopped to read them, and a wheelchair user had to wait for the doorway to clear. The library excluded Mira for a month. As her adviser, would you challenge the exclusion? Use the rule and the facts to make your case, then address the library’s strongest response.",
    "A fictional grant program accepts late applications only when circumstances beyond the applicant’s control prevented timely filing. A storm disabled the portal during its final hour. Jo had the completed form the previous day and chose to wait. As the reviewer, write the decision you would defend, including the strongest argument the other way.",
];
const CRITERION: &str = "Identify the disputed issue, apply the supplied rule to material facts, weigh a plausible opposing reading, and reach a defensible conclusion. Either conclusion can qualify. Acronym recall and headings are optional.";

/// Case-specific scaffolds keep either conclusion available after a coaching draft fails.
pub fn coaching_question(exercise: &str) -> Option<&'static str> {
    if exercise == CASES[0] {
        Some("How much weight would you give the wait at the doorway in deciding whether library service was materially disrupted? You can also say what you would need to know.")
    } else if exercise == CASES[1] {
        Some("How would you weigh the outage against the opportunity to file earlier when deciding whether the delay was beyond Jo’s control?")
    } else {
        None
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NameRecall {
    Supplied,
    Due,
    Asking,
    Recalled,
    Reminded,
    Skipped,
}

pub fn supplies_name(reply: &str) -> bool {
    let opening = reply.split("\n\n").next().unwrap_or_default();
    ["irac", "issue", "rule", "application", "conclusion"]
        .iter()
        .all(|term| mentions(opening, term))
}

pub fn name_request(words: &str) -> bool {
    let s = words.trim().to_lowercase();
    (s.contains("acronym")
        || s.contains("four-letter")
        || s.contains("four letter")
        || mentions(&s, "irac"))
        && [
            "what",
            "remind me",
            "forgot",
            "can't remember",
            "cant remember",
            "can’t remember",
        ]
        .iter()
        .any(|part| s.contains(part))
}

impl Lesson {
    pub fn studying_law(&self) -> bool {
        ["law", "legal", "lawyer", "jurisprudence"]
            .iter()
            .any(|term| {
                mentions(&self.topic, term)
                    || mentions(self.declared_goal.as_deref().unwrap_or(""), term)
            })
    }
    pub fn recall_due(&self) -> bool {
        self.name_recall == Some(NameRecall::Due)
    }
    pub fn asking_name(&self) -> bool {
        self.name_recall == Some(NameRecall::Asking)
    }
    pub fn update_name_recall(&mut self, previous: &Lesson) {
        if self.declared_goal != previous.declared_goal {
            self.name_recall = None;
        } else if self.name_recall == Some(NameRecall::Supplied)
            && self
                .observation
                .as_ref()
                .is_some_and(|o| !o.quote.trim().is_empty() && !o.exercise.is_empty())
        {
            self.name_recall = Some(NameRecall::Due);
        } else if self.asking_name() {
            self.name_recall = Some(NameRecall::Skipped);
        }
    }

    pub fn method_turn(
        &mut self,
        visit: &Visit,
        inputs: &Inputs,
    ) -> Option<(&'static str, &'static str, String)> {
        let words = visit.pending.as_deref().unwrap_or("").trim();
        let lower = words.to_lowercase();
        let label_due = if matches!(
            inputs.version.as_str(),
            crate::judgment::NINTH_VERSION
                | crate::judgment::TENTH_VERSION
                | crate::judgment::ELEVENTH_VERSION
                | crate::judgment::VERSION
        ) {
            self.label_check_due()
        } else {
            self.recall_due()
        };
        let (action, why, reply) = if (label_due || self.asking_name())
            && matches!(words, "" | "/continue")
        {
            self.name_recall = Some(NameRecall::Asking);
            (
                "name_check",
                "Intervening case work makes this a separate check of the framework’s name.",
                CHECK.into(),
            )
        } else if self.asking_name() && recall_response(&lower).is_some() {
            let (state, answer) = recall_response(&lower)?;
            self.name_recall = Some(state);
            (
                "name_answer",
                "This answer concerns vocabulary only.",
                format!("{answer}\n\n{}", self.resume_task()),
            )
        } else if lower == "/practice irac"
            || (mentions(&lower, "irac")
                && [
                    "give me a practice",
                    "let me practice",
                    "can i practice",
                    "i want to practice",
                    "give me an irac",
                    "give me another irac",
                ]
                .iter()
                .any(|s| lower.starts_with(s)))
        {
            let case = CASES
                .iter()
                .find(|c| self.exercise != **c && !self.closed.iter().any(|t| t.exercise == **c));
            let Some(case) = case else { return None };
            if !self.studying_law() {
                self.course = None;
                self.outside_goal_for_method();
            }
            let why = "The learner requested an application. The criterion covers their argument and recommendation.";
            self.open_method_case(case, CRITERION);
            if self.name_recall.is_none() {
                self.name_recall = Some(NameRecall::Supplied);
            }
            let evidence = crate::judgment::known(visit, &inputs.external_findings)
                .into_iter()
                .rev()
                .find(|f| {
                    f.proposal.kind == crate::judgment::Kind::Strength
                        && (f.proposal.task == visit.lesson.objective
                            || visit.teaching_history().iter().any(|(words, _)| {
                                words
                                    .as_deref()
                                    .is_some_and(|s| s.contains(&f.proposal.quote))
                            }))
                        && [
                            "rule",
                            "legal",
                            "law",
                            "reasoning",
                            "authority",
                            "interpretation",
                            "argument",
                        ]
                        .iter()
                        .any(|term| {
                            mentions(&f.proposal.detail, term) || mentions(&f.proposal.task, term)
                        })
                });
            let reply = match evidence.filter(|_| {
                matches!(
                    inputs.version.as_str(),
                    crate::judgment::NINTH_VERSION
                        | crate::judgment::TENTH_VERSION
                        | crate::judgment::ELEVENTH_VERSION
                        | crate::judgment::VERSION
                        | crate::judgment::EIGHTH_VERSION
                )
            }) {
                Some(f) => format!(
                    "Earlier reasoning to build on: {} (/show {})\n\n{case}",
                    f.proposal.detail, f.proposal.source
                ),
                None => case.to_string(),
            };
            ("method_practice", why, reply)
        } else if name_request(words)
            && (inputs.version == crate::judgment::SEVENTH_VERSION
                || !mentions(words, "irac")
                || lower.contains("acronym")
                || lower.contains("stand for")
                || lower.contains("stands for")
                || lower.contains("what is irac")
                || lower.contains("what does irac mean"))
        {
            let legal = self.studying_law();
            let labels = ["IRAC", "CREAC", "CRAC", "FIRAC"];
            let last = visit
                .teaching_history()
                .iter()
                .rev()
                .map(|(_, reply)| reply.as_str())
                .chain(inputs.external_cues.iter().map(|(_, t)| t.as_str()))
                .find(|s| labels.iter().any(|term| mentions(s, term)));
            if !mentions(words, "IRAC")
                && !(legal
                    && last.is_some_and(|s| {
                        mentions(s, "IRAC") && !labels[1..].iter().any(|term| mentions(s, term))
                    }))
            {
                return None;
            }
            self.name_recall = Some(NameRecall::Supplied);
            ("method_name","The learner asked for the label. Its recall can be checked after they have done other work.", format!("{NAME}\n\nUse /practice irac for a fresh case."))
        } else {
            return None;
        };
        self.method_action(action);
        Some((action, why, reply))
    }

    fn outside_goal_for_method(&mut self) {
        self.declared_goal = Some("Practice legal reasoning".into());
        self.topic = "Practice legal reasoning".into();
        self.next_objective.clear();
        self.outline.clear();
        self.name_recall = None;
    }

    fn resume_task(&self) -> &str {
        if self.active_step().is_empty() {
            "Use /continue to pick up the lesson."
        } else {
            self.active_step()
        }
    }
}

fn recall_response(lower: &str) -> Option<(NameRecall, &'static str)> {
    let words = lower.trim_end_matches(['.', '?', '!']).trim();
    if words == "skip" {
        return Some((NameRecall::Skipped, "We’ll leave that one for now."));
    }
    let guess = [
        "i think it was ",
        "i think it's ",
        "i think ",
        "was it ",
        "is it ",
        "it was ",
        "it’s ",
        "it's ",
    ]
    .iter()
    .find_map(|prefix| words.strip_prefix(prefix))
    .unwrap_or(words)
    .trim();
    if guess == "irac" {
        return Some((NameRecall::Recalled, "Yes, IRAC. You recalled its name."));
    }
    let unknown = [
        "hint",
        "i forgot",
        "i forget",
        "forgot",
        "i don't know",
        "i don’t know",
        "i dont know",
        "no idea",
        "i can't remember",
        "i can’t remember",
        "i cant remember",
        "tell me",
    ];
    if unknown.contains(&words)
        || (guess.len() >= 3
            && guess.len() <= 6
            && guess.bytes().all(|c| c.is_ascii_alphabetic())
            && ![
                "pause", "stop", "quit", "wait", "next", "ready", "okay", "hello", "thanks",
            ]
            .contains(&guess))
    {
        return Some((NameRecall::Reminded, NAME));
    }
    None
}
