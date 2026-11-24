//! Reopen a parked criterion after an elective. Evidence attaches to the reviewed display.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::protocol::{transition, Effect, Event, Gate, Next, Phase, Question, State};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Contact {
    Related,
    Unrelated,
    Unreviewed,
    Declined,
}

fn classify(display: Option<&str>, catalog: &Value) -> Contact {
    let Some(display) = display else {
        return Contact::Declined;
    };
    let digest = format!("{:x}", Sha256::digest(display.as_bytes()));
    match catalog.get(&digest).and_then(Value::as_bool) {
        Some(true) => Contact::Related,
        Some(false) => Contact::Unrelated,
        None => Contact::Unreviewed,
    }
}

#[derive(Clone, Copy, Debug)]
struct Window {
    earliest: u32,
    complete: bool,
}

impl Window {
    fn after_contact(self, contact: Contact, day: u32, spacing: u32) -> Option<Self> {
        match contact {
            Contact::Related => Some(Self {
                earliest: self.earliest.max(day.checked_add(spacing)?),
                ..self
            }),
            Contact::Unreviewed => Some(Self {
                complete: false,
                ..self
            }),
            Contact::Declined | Contact::Unrelated => Some(self),
        }
    }

    fn open(self, day: u32, grant: Gate) -> Option<Effect> {
        if !self.complete || day < self.earliest {
            return None;
        }
        let prior = State {
            phase: Phase::FollowUp(Next::WaitReview),
            paused: false,
        };
        let (due, _) = transition(prior, Event::ReviewDue);
        Some(transition(due, Event::Ask(Question::DelayedReview, grant)).1)
    }
}

struct Observation {
    opened: bool,
    eligible: bool,
    practice_shown: bool,
    scored: bool,
}

fn monitor() -> Monitor<Observation> {
    let mut predicates = Predicates::builder();
    let safe = predicates.new_predicate(ClosurePredicate::new(
        |s: &Observation| (!s.opened || s.eligible) && (!s.practice_shown || !s.scored),
        "cold eligibility and practice-only evidence",
    ));
    let law = Property::always(Property::atomic(safe));
    let predicates = predicates.build();
    let automaton = law.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, automaton)
}

fn study() -> Value {
    let study: Value =
        serde_json::from_str(include_str!("../fixtures/violin-phrase-study.json")).unwrap();
    let source: Value =
        serde_json::from_str(include_str!("../fixtures/goals/violin.json")).unwrap();
    let review: Value =
        serde_json::from_str(include_str!("../fixtures/violin-phrase-exposure.json")).unwrap();
    let prior = Window {
        earliest: 1825,
        complete: true,
    };
    let mut records = Vec::new();
    for arm in ["pulse", "elective", "declined", "unreviewed"] {
        let display = match arm {
            "declined" => None,
            "unreviewed" => Some("Different text awaiting review"),
            _ => Some(study["arms"][arm]["response"]["display"].as_str().unwrap()),
        };
        let contact = classify(display, &review);
        let window = prior.after_contact(contact, 1825, 30).unwrap();
        let mut m = monitor();
        assert!(m.next_state(&Observation {
            opened: false,
            eligible: false,
            practice_shown: display.is_some(),
            scored: false
        }));
        for day in [1826, 1855] {
            let opened =
                window.open(day, Gate::Supported) == Some(Effect::Asked(Question::DelayedReview));
            let eligible = match contact {
                Contact::Related => day >= 1855,
                Contact::Unreviewed => false,
                Contact::Declined | Contact::Unrelated => day >= 1825,
            };
            assert!(m.next_state(&Observation {
                opened,
                eligible,
                practice_shown: false,
                scored: false
            }));
            let instruction = if opened {
                source["first_pack"]["check"].as_str().unwrap().to_owned()
            } else if window.complete {
                format!("Your pulse review is still open. You can choose ordinary practice today or wait until day {} for the delayed check.", window.earliest)
            } else {
                "Your review is still open. You can choose practice while we check whether the earlier activity used this skill.".to_owned()
            };
            records.push(json!({"arm":arm,"day":day,"contact":format!("{contact:?}"),"cold_opened":opened,
                "intent_only_would_open":prior.open(day, Gate::Supported)==Some(Effect::Asked(Question::DelayedReview)),
                "instruction":instruction}));
            // The second day describes the wait branch; an already opened review has no second opening.
            if opened {
                break;
            }
        }
    }
    json!(records)
}

fn main() {
    println!("{}", serde_json::to_string_pretty(&study()).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn instruction_changes_the_parked_review() {
        let rows = study();
        let rows = rows.as_array().unwrap();
        assert!(rows
            .iter()
            .any(|r| r["arm"] == "pulse" && r["day"] == 1826 && r["cold_opened"] == false));
        assert!(rows
            .iter()
            .any(|r| r["arm"] == "elective" && r["day"] == 1826 && r["cold_opened"] == true));
        assert!(rows
            .iter()
            .any(|r| r["arm"] == "pulse" && r["day"] == 1855 && r["cold_opened"] == true));
        assert!(rows
            .iter()
            .all(|r| r["arm"] != "unreviewed" || r["cold_opened"] == false));
    }
    #[test]
    fn source_hold_and_unknown_exposure_cannot_certify_freshness() {
        let w = Window {
            earliest: 10,
            complete: true,
        };
        assert_eq!(w.open(12, Gate::Unavailable), Some(Effect::Withheld));
        assert!(!monitor().next_state(&Observation {
            opened: true,
            eligible: false,
            practice_shown: false,
            scored: false
        }));
        assert!(!monitor().next_state(&Observation {
            opened: false,
            eligible: true,
            practice_shown: true,
            scored: true
        }));
        assert!(w.after_contact(Contact::Related, u32::MAX, 30).is_none());
    }
}
