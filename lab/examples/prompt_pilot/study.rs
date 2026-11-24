use super::*;
use tutor_lab::learning_epochs::pilot::{Observation, Trajectory};

fn object(fields: Value) -> Value {
    json!({"type":"object","properties":fields,"required":fields.as_object().unwrap().keys().collect::<Vec<_>>(),"additionalProperties":false})
}
fn string() -> Value {
    json!({"type":"string"})
}
fn choice(labels: &[&str]) -> Value {
    json!({"type":"string","enum":labels})
}
const EVENTS: &[&str] = &[
    "answer",
    "help_requested",
    "assisted",
    "refusal",
    "correction",
    "goal_change",
    "missed",
];
fn response_schema() -> Value {
    object(json!({"words":string(),"event":choice(EVENTS)}))
}
fn utterance(reply: &Value) -> Result<(&str, &str)> {
    let event = reply["event"]
        .as_str()
        .filter(|v| EVENTS.contains(v))
        .ok_or("Invalid learner event")?;
    let words = reply["words"]
        .as_str()
        .filter(|v| !v.trim().is_empty() || event == "missed")
        .ok_or("Invalid learner response")?;
    Ok((words, event))
}
fn present(trace: &mut Vec<Value>, day: u32, who: &str, words: &str, event: &str) {
    trace.push(json!({"day":day,"speaker":who,"words":words,"event":event}));
}
fn learner_packet(case: &Case, day: u32, history: &[Value], task: &str) -> Value {
    json!({"starting_brief":case.brief,"elapsed_weeks":day/7,"own_experience":history,"task":task})
}
struct Arm<'a, F> {
    case: &'a Case,
    f: &'a Frozen,
    sim: &'a str,
    label: &'a str,
    tutor: &'a str,
    seed: u32,
    journal: &'a mut Journal,
    send: &'a mut F,
    memory: Trajectory,
    trace: Vec<Value>,
    quality: Vec<Value>,
    outcomes: Vec<Value>,
}
impl<F: FnMut(&Value) -> Result<Value>> Arm<'_, F> {
    fn call(
        &mut self,
        day: u32,
        stage: &str,
        sys: &[&str],
        input: Value,
        schema: Value,
    ) -> Result<Value> {
        let id = format!("{}/{}/{}/{day}/{stage}", self.case.id, self.sim, self.label);
        let seed = case_seed(u64::from(self.seed), &format!("{day}/{stage}"));
        self.journal
            .call(&id, request(sys, input, schema, seed), self.send)
    }
    fn learner(
        &mut self,
        day: u32,
        stage: &str,
        task: &str,
        draw: Option<&Observation>,
    ) -> Result<Value> {
        let mut input = learner_packet(self.case, day, &self.trace, task);
        let role = if let Some(d) = draw {
            input["modeled_condition"] = json!(if d.help { "request help" } else { d.response });
            REALIZE
        } else {
            LEARNER
        };
        let reply = self.call(day, stage, &[role], input, response_schema())?;
        let (_, event) = utterance(&reply)?;
        if let Some(d) = draw {
            if event != if d.help { "help_requested" } else { "answer" } {
                return Err("Realizer contradicted the drawn learner event".into());
            }
        }
        Ok(reply)
    }
    fn tutor(&mut self, day: u32, stage: &str) -> Result<String> {
        let data = json!({"task":"deliver","subject":self.case.subject,"brief":self.case.brief,
            "visible_lesson":self.trace,"instruction":if stage=="teach"{"Respond to the learner and offer one next move."}else{"Use the learner's actual reply to decide how to continue this visit."}});
        let r = self.call(
            day,
            stage,
            &[self.tutor, FORMAT, ROUTE],
            data,
            object(json!({"words":string()})),
        )?;
        Ok(r["words"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("Empty teaching move")?
            .into())
    }
    fn teach(&mut self, day: u32) -> Result<()> {
        let draw = self.memory.observe(day, 0, false);
        if self.sim == "explicit" && !draw.attended {
            present(&mut self.trace, day, "visit", "", "missed");
            return Ok(());
        }
        let (task, first) = if day == 0 {
            (
                self.case.opening.clone(),
                json!({"event":"answer","words":self.case.answer}),
            )
        } else {
            let task = self.case.practice.clone();
            let reply = self.learner(
                day,
                "practice_response",
                &task,
                (self.sim == "explicit").then_some(&draw),
            )?;
            (task, reply)
        };
        let (words, event) = utterance(&first)?;
        if event == "missed" {
            present(&mut self.trace, day, "visit", "", "missed");
            return Ok(());
        }
        let start = self.trace.len();
        present(&mut self.trace, day, "task", &task, "offered");
        present(&mut self.trace, day, "learner", words, event);
        if matches!(event, "goal_change" | "refusal") {
            return Ok(());
        }
        let question = self.tutor(day, "teach")?;
        present(&mut self.trace, day, "tutor", &question, "delivered");
        let turn = self.memory.observe(day, 1, false);
        let reply = self.learner(
            day,
            "reply",
            &question,
            (self.sim == "explicit").then_some(&turn),
        )?;
        let (words, event) = utterance(&reply)?;
        present(&mut self.trace, day, "learner", words, event);
        if matches!(event, "missed" | "goal_change" | "refusal") {
            return Ok(());
        }
        let feedback = self.tutor(day, "follow_up")?;
        present(&mut self.trace, day, "tutor", &feedback, "delivered");
        let review=self.call(day,"quality",&[QUALITY],json!({"exchange":self.trace[start..]}),
            object(json!({"uncertainty":string(),"reply_quote":string(),"adaptation":string(),
                "reply_changed_move":choice(&["yes","no","unclear"]),"mapping":choice(&["worked","retrieval","unsupported"]),"mapping_reason":string()})))?;
        let quote = review["reply_quote"]
            .as_str()
            .filter(|s| !s.is_empty() && words.contains(*s))
            .ok_or("Teaching review quote is absent from the learner reply")?;
        let mapping = review["mapping"]
            .as_str()
            .ok_or("Missing delivered move mapping")?;
        if self.sim == "explicit" {
            match mapping {
                "retrieval" => self.memory.encounter(
                    day,
                    turn.response == "success"
                        && !turn.help
                        && !matches!(first["event"].as_str(), Some("help_requested" | "assisted")),
                ),
                "worked" => self.memory.encounter(day, false),
                "unsupported" => {}
                _ => return Err("Invalid modeled action".into()),
            }
        }
        self.quality
            .push(json!({"day":day,"review":review,"verified_reply_quote":quote}));
        Ok(())
    }
    fn probe(&mut self, day: u32) -> Result<()> {
        let (name, task, criteria) = if day == 28 {
            ("changed", &self.case.changed, &self.case.changed_criteria)
        } else {
            (
                "transfer",
                &self.case.transfer,
                &self.case.transfer_criteria,
            )
        };
        let preceding_control = if self.label == "control" {
            self.memory = Trajectory::new(self.f.settings.seed, self.seed);
            Some(std::mem::take(&mut self.trace))
        } else {
            None
        };
        let draw = self.memory.observe(day, 0, true);
        let mut hint = None;
        let reply = if self.sim == "agent" {
            self.learner(day, "delayed", task, None)?
        } else if !draw.attended {
            json!({"event":"missed","words":""})
        } else {
            let words = &self.f.answers[&self.case.id][name][draw.response];
            if draw.help {
                hint = words.as_str();
            }
            json!({"event":if draw.help{"assisted"}else{"answer"},"words":words})
        };
        let (words, event) = utterance(&reply)?;
        if event != "missed" {
            present(&mut self.trace, day, "task", task, "offered");
        }
        if let Some(h) = hint {
            present(&mut self.trace, day, "tutor", h, "worked_before_answer");
        }
        present(&mut self.trace, day, "learner", words, event);
        let score = if matches!(event, "answer" | "correction") {
            let facet =
                object(json!({"met":{"type":"boolean"},"evidence":string(),"reason":string()}));
            let scored = self.call(
                day,
                "score",
                &[SCORE],
                json!({"task":task,"criteria":criteria,"response":words}),
                object(json!({"first":facet,"second":facet})),
            )?;
            for part in ["first", "second"] {
                let met = scored[part]["met"]
                    .as_bool()
                    .ok_or("Missing criterion rating")?;
                let quote = scored[part]["evidence"]
                    .as_str()
                    .ok_or("Missing criterion evidence")?;
                if met && (quote.is_empty() || !words.contains(quote)) {
                    return Err("Unsupported positive criterion".into());
                }
            }
            Some(scored)
        } else {
            None
        };
        if self.sim == "explicit" && draw.attended {
            self.memory
                .encounter(day, !draw.help && draw.response == "success");
        }
        self.outcomes.push(json!({"day":day,"task":task,"response":reply,"worked_before_answer":hint,"score":score,
            "model_draw":(self.sim=="explicit").then(||json!({"attended":draw.attended,"help":draw.help,"outcome":draw.response,"recall_probability":draw.recall_probability}))}));
        if let Some(mut earlier) = preceding_control {
            earlier.append(&mut self.trace);
            self.trace = earlier;
        }
        Ok(())
    }
}
fn case_seed(seed: u64, id: &str) -> u32 {
    let digest = tutor_lab::material::document_id(format!("{seed}/{id}").as_bytes());
    u32::from_le_bytes(digest[..4].try_into().unwrap()) & 0x7fff_ffff
}
pub fn run(
    f: &Frozen,
    journal: &mut Journal,
    send: &mut impl FnMut(&Value) -> Result<Value>,
) -> Result<Value> {
    let mut pairs = vec![];
    for case in f.cases.iter().filter(|c| f.settings.cases.contains(&c.id)) {
        for sim in &f.settings.simulations {
            let mut arms = json!({});
            let seed = case_seed(f.settings.seed, &case.id);
            for (label, tutor) in [
                ("control", ""),
                ("baseline", f.versions[0].1.as_str()),
                ("candidate", f.versions[1].1.as_str()),
            ] {
                let mut arm = Arm {
                    case,
                    f,
                    sim,
                    label,
                    tutor,
                    seed,
                    journal,
                    send,
                    memory: Trajectory::new(f.settings.seed, seed),
                    trace: vec![],
                    quality: vec![],
                    outcomes: vec![],
                };
                if label != "control" {
                    arm.teach(0)?;
                    arm.teach(7)?;
                }
                arm.probe(28)?;
                arm.probe(84)?;
                arms[label] = json!({"transcript":arm.trace,"teaching_reviews":arm.quality,"outcomes":arm.outcomes,
                    "usage":arm.journal.arm_usage(&case.id,sim,label)?});
            }
            pairs.push(json!({"case":case.id,"subject":case.subject,"simulation":sim,"case_seed":seed,"arms":arms,"permissible_conclusion":case.conclusion}));
        }
    }
    Ok(report::compare(f, pairs, journal.usage()))
}
