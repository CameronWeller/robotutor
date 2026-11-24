//! Track a learner's chosen course objective separately from one-visit help.
use crate::curriculum::{
    goal_rehearsal::{opening_feedback, Options},
    program::{Frame, Proposal},
    rehearsal_release::{publish, PublishedLesson},
};
use crate::supply::Contact;
use crate::{
    material::document_id,
    protocol::{transition, Effect, Event, Gate, Question, State},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const WORSHIP: &str = include_str!("../../fixtures/goals/worship.json");
const PINS: &str = include_str!("../../fixtures/goals/reviews.json");

pub(super) fn published_psalm_lesson() -> Result<PublishedLesson, String> {
    let data: Value = serde_json::from_str(WORSHIP).map_err(|e| e.to_string())?;
    let frame = Frame::from_json(&data)?;
    let plan = Proposal::from_json(&data["proposal"])?;
    publish(
        &frame,
        &plan,
        frame.review_digest(&plan),
        &data["first_pack"],
    )
}

/// Shared source record also used by the original local source-comparison experiment.
pub fn parish_record() -> Value {
    let source_url = "https://www.churchofengland.org/resources/parishes/putting-together-services/leading-prayer-through-intercessions";
    let excerpt = "Some intercessions include a response from the congregation, thus:";
    serde_json::json!({"unit":103,"source_url": source_url,
        "locator":"The Church of England, Leading prayer through intercessions, introduction to the responses.",
        "excerpt":excerpt,
        "lesson":"The Church of England's parish guidance describes a response that can occur in gathered prayer. The word ‘some’ limits the claim.",
        "worked":"On an invented service sheet, the leader names neighbors who need shelter. A printed line for everyone follows. You can recognize an invitation to join by locating that second line.",
        "practice":"What would you look for on another service sheet to tell when a group is invited to speak?",
        "check":"A different fictional sheet prints a leader's concern about prisoners followed by silence. Which part of this sheet, if any, instructs the whole group to speak?",
        "criterion":"Identify the evidence for a speaking part on this particular sheet.",
        "rubric":"Accept an answer grounded in the lines on the sheet; leave conclusions about personal faith and universal practice outside this task."})
}

#[derive(Clone, Default)]
pub(super) struct Choice {
    intended: Option<u32>,
    grant: bool,
    active: bool,
    present: Option<u32>,
    worked: BTreeSet<u32>,
    guided: BTreeSet<u32>,
    checked: BTreeMap<u32, Contact>,
    extra: super::case_supply::Supply,
    extra_open: bool,
    pending: Option<(u32, Question)>,
}

impl Choice {
    pub(super) fn report(&self) -> Option<Value> {
        self.active.then(|| {
            let contact = |level: &Contact| match level {Contact::Delivered => "confirmed", Contact::Possible => "possible"};
            json!({"course":"worship","requested_unit":self.intended,"parish_grant":self.grant,"presented_unit":self.present,"awaiting":self.pending.map(|(_,q)|format!("{q:?}")),
                "base_contact":self.checked.get(&101).map(contact),"extra_contact":self.extra.contact().as_ref().map(contact),
                "extra_available":self.extra.available(),"worked":self.worked.contains(&101)})
        })
    }

    pub(super) fn apply(&mut self, args: &[Value], current: bool) -> Result<Value, String> {
        let words: Vec<_> = args
            .iter()
            .map(|a| a.as_str().ok_or("goal command requires text"))
            .collect::<Result<_, _>>()?;
        let data: Value = serde_json::from_str(WORSHIP).map_err(|e| e.to_string())?;
        let pins: Value = serde_json::from_str(PINS).map_err(|e| e.to_string())?;
        let hash: String = document_id(WORSHIP.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if pins["worship"].as_str() != Some(&hash) {
            return Err("course fixture changed since review".into());
        }
        let frame = Frame::from_json(&data)?;
        let plan = Proposal::from_json(&data["proposal"])?;
        let id = |word: &str| -> Result<u32, String> {
            let id = word
                .parse::<u32>()
                .map_err(|_| "expected a reviewed unit ID")?;
            plan.units()
                .iter()
                .any(|u| u.id == id)
                .then_some(id)
                .ok_or_else(|| "unit outside selected course".into())
        };
        self.active = true;
        let response=match words.as_slice() {
            ["choose", target]=>{self.intended = Some(id(target)?);self.present=None;self.pending=None; json!({"contribution":"goal_chosen"})}
            ["clear"]=>{self.intended=None;self.present=None;self.pending=None; json!({"contribution":"goal_cleared"})}
            ["source", state @ ("held"|"granted")]=>{self.grant=*state=="granted";if !self.grant && self.present==Some(103){self.present=None;self.pending=None;}json!({"contribution":"source_status_changed"})}
            ["request", "101"] if self.intended==Some(101) && self.checked.contains_key(&101)=>self.extra.request(),
            ["familiar", "101", offered] if self.intended == Some(101) && self.worked.contains(&101) => {
                match *offered {
                    "first" if !self.checked.contains_key(&101) => { self.checked.insert(101, Contact::Possible); }
                    "second" if self.checked.contains_key(&101) => self.extra.report_familiar()?,
                    _ => return Err("that case is not currently offered".into()),
                }
                json!({"contribution":"prior_contact_reported","display":"I’ll set that case aside. You can still use the worked example or guided practice."})
            }
            ["wait"]=>{self.present=None;self.pending=None;json!({"contribution":"wait","display":null})},
            [kind @ ("visit" | "requested-visit")]=>{
                let target=self.intended.ok_or("no current goal")?;
                self.open(target, if *kind=="visit" {"repeat"} else {"visit"}, &frame,&plan,&data)?
            }
            ["try", original, alternative]=>{
                let original=id(original)?;
                let alternative=id(alternative)?;
                if self.intended!=Some(original) || original==alternative {
                    return Err("alternative does not correspond to the current distinct goal".into());
                }
                self.open(alternative,"repeat",&frame,&plan,&data)?
            }
            ["study", target, choice @ ("repeat"|"practice"|"check"|"private")]=>{
                let target=id(target)?;
                if self.intended!=Some(target) {return Err("select this learning goal first".into());}
                let mut result = self.open(target, choice,&frame,&plan,&data)?;
                if result["contribution"] == "new_case_unavailable" {
                    self.pending=None;
                    result["display"]["availability"]=if self.extra.contact().is_some() {
                        json!("This lesson has no further reviewed case. The worked example and guided exercise are available.")
                    } else {
                        json!("The available case may already have reached you. You can revisit the example, try guided practice, or ask for a different reviewed case.")
                    };
                }
                result
            }
            ["received", target]=>{
                let target=id(target)?;
                if self.pending != Some((target,Question::RequestedProbe)) {return Err("no case awaiting receipt".into());}
                if self.extra_open {self.extra.confirm()?;} else {
                    *self.checked.get_mut(&target).ok_or("no reserved base case")?=Contact::Delivered;
                }
                json!({"contribution":"case_receipt_confirmed"})
            }
            ["move", target, choice @ ("repeat"|"proceed"|"practice"|"check"|"private")]=>{
                let target=id(target)?;
                if self.present!=Some(target){return Err("this is not the active lesson; request a visit first".into());}
                self.open(target, choice,&frame,&plan,&data)?
            }
            _=>return Err("goal choose UNIT | try ORIGINAL ALTERNATIVE | visit | wait | clear | source held|granted".into()),
        };
        if current
            && matches!(
                response["contribution"].as_str(),
                Some("goal_options" | "new_case_unavailable")
            )
        {
            self.pending = None;
        }
        Ok(response)
    }
    pub(super) fn review_case(&mut self, candidate: &str) -> Result<Value, String> {
        let base = published_psalm_lesson()?;
        self.extra.review(base.material(true)?, candidate)
    }

    fn open(
        &mut self,
        id: u32,
        activity: &str,
        frame: &Frame,
        plan: &Proposal,
        data: &Value,
    ) -> Result<Value, String> {
        let unit = plan
            .units()
            .iter()
            .find(|u| u.id == id)
            .ok_or("unknown objective")?;
        let hold = |why: &str| json!({"contribution":"goal_held","attempted_unit":id,"reason":why,"display":null});
        if id == 103 && !self.grant {
            self.present = None;
            self.pending = None;
            return Ok(hold("required parish source unavailable"));
        }
        if !unit.requires.is_empty() {
            self.present = None;
            self.pending = None;
            return Ok(hold(
                "no prior evidence supplied for this unit's prerequisites",
            ));
        }
        let parish;
        let record = if id == 103 {
            parish = parish_record();
            &parish
        } else if data["first_pack"]["unit"].as_u64() == Some(u64::from(id)) {
            &data["first_pack"]
        } else {
            self.present = None;
            self.pending = None;
            return Ok(hold("no released teaching material"));
        };
        let release = publish(frame, plan, frame.review_digest(plan), record)?;
        let pack = release.material(true)?;
        let activity = match activity {
            "visit" if self.worked.contains(&id) => "options",
            "visit" => "repeat",
            "proceed" if !self.guided.contains(&id) => "practice",
            "proceed" => "check",
            choice => choice,
        };
        self.present = Some(id);
        let base =
            json!({"title":unit.title,"source":record["source_url"],"excerpt":record["excerpt"]});
        let mut display = base;
        let contribution = match activity {
            "options" => {
                display["invitation"] = if id == 101 && self.extra.available() {
                    json!("A different reviewed fictional case is available if you would like it. You can also repeat the example, try guided practice, or stop.")
                } else if self.checked.contains_key(&id) {
                    json!("You have seen the available fictional case. Would you like to hear the example again, try guided practice, or stop here?")
                } else {
                    json!("Would you like to hear the example again, try guided practice, try the changed fictional case, or keep your work private?")
                };
                "goal_options"
            }
            "private" => {
                self.pending = None;
                self.present = None;
                display = json!({"message":"You may keep your prayer and any writing entirely private. We can stop here."});
                "private_choice"
            }
            "repeat" => {
                self.worked.insert(id);
                self.pending = None;
                display["worked"] = json!(pack.worked);
                "worked_example"
            }
            "practice" | "check" => {
                let question = if activity == "practice" {
                    Question::Practice
                } else {
                    Question::RequestedProbe
                };
                let extra_ready = activity == "check"
                    && id == 101
                    && self.checked.contains_key(&id)
                    && self.extra.available();
                if activity == "check" && self.checked.contains_key(&id) && !extra_ready {
                    display["availability"]=json!("You have already seen the released fictional case for this objective. You can revisit the explanation or guided practice, or wait for another reviewed case.");
                    return Ok(
                        json!({"contribution":"new_case_unavailable","shown_unit":id,"display":display,"credit":null}),
                    );
                }
                // The learner explicitly requested a new instructional lane.
                // The earlier worked example is an actual recorded teaching event.
                let (taught, effect) = transition(State::default(), Event::Teach(Gate::Supported));
                if !self.worked.contains(&id) || effect != Effect::Taught {
                    return Err("a teaching encounter must have been shown first".into());
                }
                let (_, effect) = transition(taught, Event::Ask(question, Gate::Supported));
                if effect != Effect::Asked(question) {
                    return Err("the teaching protocol did not open this move".into());
                }
                self.pending = Some((id, question));
                if activity == "practice" {
                    self.guided.insert(id);
                    display["practice"] = json!(pack.practice);
                    "guided_practice"
                } else {
                    self.checked.entry(id).or_insert(Contact::Possible);
                    self.extra_open = extra_ready;
                    display["case"] = if extra_ready {
                        json!(self.extra.open(pack)?)
                    } else {
                        json!(pack.check)
                    };
                    "changed_fictional_case"
                }
            }
            _ => return Err("unknown teaching move".into()),
        };
        Ok(json!({"contribution":contribution,"shown_unit":id,"display":display,"credit":null}))
    }

    /// Evaluate private, live text only while its released prompt is open.
    /// Keep reviewed sample answers out of the decision about this response.
    pub(super) fn private_reply(&self, id: u32, text: &str) -> Result<Option<Value>, String> {
        let Some((unit, question)) = self.pending else {
            return Err("no writing task is open".into());
        };
        if id != unit || self.present != Some(id) {
            return Err("response does not belong to this opening".into());
        }
        if text.trim().is_empty() || text.len() > 4096 {
            return Err("submit between 1 and 4096 bytes, or choose private".into());
        }
        if id == 101 && question == Question::RequestedProbe && !self.extra_open {
            let pins: Value = serde_json::from_str(PINS).map_err(|e| e.to_string())?;
            let pin = pins["worship"]
                .as_str()
                .ok_or("missing reviewed course pin")?;
            let feedback = opening_feedback(
                WORSHIP,
                pin,
                &Options {
                    unit: Some(id),
                    ..Options::default()
                },
                text,
                false,
            )?;
            Ok(Some(json!(feedback)))
        } else {
            Ok(None)
        }
    }

    // The fixture has written criteria and no authenticated reviewer for a live
    // response. The host verifies a nonempty submitted text then persists this
    // receipt only; it has no evidence with which to grade the line later.
    pub(super) fn submitted(&mut self, id: u32) -> Result<Value, String> {
        let Some((unit, q)) = self.pending else {
            return Err("no practice or fictional case is open".into());
        };
        if unit != id || self.present != Some(id) || (id == 103 && !self.grant) {
            return Err("writing does not match the current authorized lesson".into());
        }
        self.pending = None;
        let case = q == Question::RequestedProbe;
        Ok(
            json!({"shown_unit":id,"contribution":if case{"fictional_response_unreviewed"}else{"guided_work_submitted"},
            "received_text":true,"observation":null,"credit":null,
            "display":if case{"Your response was received for this fictional case. Its meaning has not been assessed. You can keep the words private."}
                          else{"You have tried the guided exercise. You can continue with a different fictional case or keep your writing private."}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::rehearsal::{Action, Session};
    use serde_json::Value;

    fn goal(session: &mut Session, words: &[&str]) -> Result<(Value, Value), String> {
        session.apply(Action::Goal(
            words.iter().map(|s| (*s).to_owned()).collect(),
        ))
    }

    #[test]
    fn a_return_offers_available_moves_and_never_counts_or_preserves_a_private_response() {
        let mut session = Session::new().unwrap();
        for command in [
            vec!["choose", "103"],
            vec!["try", "103", "101"],
            vec!["choose", "101"],
        ] {
            goal(&mut session, &command).unwrap();
        }
        let (_, options) = goal(&mut session, &["visit"]).unwrap();
        assert_eq!(options["contribution"], "goal_options");
        assert!(options["display"]["worked"].is_null());
        let (_, repeated) = goal(&mut session, &["move", "101", "repeat"]).unwrap();
        assert!(repeated["display"]["worked"]
            .as_str()
            .unwrap()
            .contains("chair"));
        let (_, guided) = goal(&mut session, &["move", "101", "proceed"]).unwrap();
        assert!(guided["display"]["practice"]
            .as_str()
            .unwrap()
            .contains("gardener"));
        let (_, changed) = goal(&mut session, &["move", "101", "proceed"]).unwrap();
        assert!(changed["display"]["case"]
            .as_str()
            .unwrap()
            .contains("night worker"));
        let (event, receipt) = session
            .apply(Action::GoalWriting(101, "My personal words".into()))
            .unwrap();
        assert_eq!(event, serde_json::json!(["goal-writing-received", 101]));
        assert_eq!(receipt["contribution"], "fictional_response_unreviewed");
        assert!(!receipt.to_string().contains("My personal words"));
        assert!(receipt["credit"].is_null());
        let (_, out) = goal(&mut session, &["visit"]).unwrap();
        assert!(out["display"]["invitation"]
            .as_str()
            .unwrap()
            .contains("seen the available"));
        assert_eq!(
            goal(&mut session, &["move", "101", "check"]).unwrap().1["contribution"],
            "new_case_unavailable"
        );
        goal(&mut session, &["move", "101", "private"]).unwrap();
        assert!(session
            .apply(Action::GoalWriting(101, "unrequested writing".into()))
            .is_err());
        assert_eq!(
            session.status().unwrap()["checkpoint"],
            options["checkpoint"]
        );
    }

    #[test]
    fn writing_requires_the_current_display_to_offer_a_task() {
        for close in [vec!["visit"], vec!["move", "101", "check"]] {
            let mut session = Session::new().unwrap();
            for command in [
                vec!["choose", "101"],
                vec!["visit"],
                vec!["move", "101", "check"],
            ] {
                goal(&mut session, &command).unwrap();
            }
            let mut old = session.clone();
            let (event, result) = goal(&mut session, &close).unwrap();
            assert!(matches!(
                result["contribution"].as_str(),
                Some("goal_options" | "new_case_unavailable")
            ));
            assert!(session
                .apply(Action::GoalWriting(101, "unrequested".into()))
                .is_err());
            let mut restored = old.clone().restore_next(&event).unwrap();
            assert!(restored
                .apply(Action::GoalWriting(101, "unrequested".into()))
                .is_err());
            let mut historical = event;
            historical[0] = serde_json::json!("goal");
            old.replay(&historical).unwrap();
            old.replay(&serde_json::json!(["goal-writing-received", 101]))
                .unwrap();
        }
    }

    #[test]
    fn a_legacy_visit_replays_the_worked_material_that_was_displayed_then() {
        let mut session = Session::new().unwrap();
        for e in [
            serde_json::json!(["goal", "choose", "101"]),
            serde_json::json!(["goal", "visit"]),
        ] {
            session.replay(&e).unwrap();
        }
        let replay = session
            .replay(&serde_json::json!(["goal", "visit"]))
            .unwrap();
        assert_eq!(replay["contribution"], "worked_example");
        assert_eq!(
            goal(&mut session, &["visit"]).unwrap().1["contribution"],
            "goal_options"
        );
    }

    #[test]
    fn a_sample_is_one_visit_and_restoration_cannot_reverse_a_new_goal() {
        for (choice, expected_return) in [("wait", 103), ("try", 103), ("choose", 101)] {
            let mut session = Session::new().unwrap();
            let initial = session.status().unwrap()["checkpoint"].clone();
            let mut history = vec![];
            for command in [
                vec!["choose", "103"],
                vec!["visit"],
                match choice {
                    "wait" => vec!["wait"],
                    "try" => vec!["try", "103", "101"],
                    _ => vec!["choose", "101"],
                },
            ] {
                let (event, out) = goal(&mut session, &command).unwrap();
                history.push(event);
                if command[0] == "visit" {
                    assert_eq!(out["contribution"], "goal_held");
                }
                if command[0] == "try" {
                    assert_eq!(out["shown_unit"], 101);
                }
            }
            let mut restored = Session::new().unwrap();
            for event in history {
                restored.replay(&event).unwrap();
            }
            let (_, authorized) = goal(&mut restored, &["source", "granted"]).unwrap();
            assert!(authorized["display"].is_null());
            let (_, shown) = goal(&mut restored, &["visit"]).unwrap();
            assert_eq!(shown["shown_unit"], expected_return);
            assert!(shown["credit"].is_null());
            assert_eq!(shown["checkpoint"], initial);
        }
    }

    #[test]
    fn an_old_offer_and_unscoped_writing_cannot_change_the_saved_goal() {
        let mut s = Session::new().unwrap();
        goal(&mut s, &["choose", "103"]).unwrap();
        goal(&mut s, &["choose", "101"]).unwrap();
        let before = s.status().unwrap();
        assert!(goal(&mut s, &["try", "103", "101"]).is_err());
        assert!(goal(&mut s, &["choose", "a private account"]).is_err());
        assert_eq!(s.status().unwrap(), before);
    }
}
