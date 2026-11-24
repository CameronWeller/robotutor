use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lesson {
    CompareUnits,
    RehearseSubtraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermObservation {
    pub day: u32,
    pub attended: bool,
    pub independent_correct: Option<bool>,
}

pub const INITIAL_EIGHTHS_QUESTION: &str =
    "How many eighth-litres were in the dispenser before any water left? You can skip.";

fn amounts(quarters: u16, remaining: u16) -> Result<(u16, u16), String> {
    if !(2..=40).contains(&quarters) || !(1..2 * quarters).contains(&remaining) {
        return Err("the jug needs 2–40 quarter-litres with a positive remainder smaller than its starting volume".into());
    }
    Ok((2 * quarters, 2 * quarters - remaining))
}

pub fn transfer_question(quarters: u16, remaining_eighths: u16) -> Result<String, String> {
    amounts(quarters, remaining_eighths)?;
    Ok(format!("A kitchen measuring jug held {quarters} quarter-litres of water. After filling a bowl, {remaining_eighths} eighth-litres remained in the jug. How many eighth-litres went into the bowl? Respond in eighths."))
}

pub fn worked_example(lesson: Lesson, quarters: u16, remaining: u16) -> Result<String, String> {
    let (start, delivered) = amounts(quarters, remaining)?;
    Ok(match lesson {
        Lesson::CompareUnits => format!("Starting with {quarters} quarter-litres gives {start} eighth-litres because each quarter-litre is two eighth-litres. Taking away {remaining} eighth-litres leaves {delivered} eighth-litres delivered."),
        Lesson::RehearseSubtraction => format!("The starting amount is {start} eighth-litres. Of those, {remaining} eighth-litres remain. Subtract the remainder: {start} − {remaining} = {delivered} eighth-litres delivered."),
    })
}

/// One learner on the regular schedule. An observer sees only events up to the
/// most recently completed term. A delivery changes this learner in place.
pub struct TrackedLearner {
    participant: Participant,
    state: Learner,
    term: u32,
    history: Vec<Value>,
}

impl TrackedLearner {
    pub fn new(seed: u64, learner: u32) -> Self {
        let participant = Participant { seed, id: learner };
        let card = FractionCard::propose(FractionTask::MissingPart, 3, 2, 16).unwrap();
        let revoked = BTreeSet::new();
        let source = practice_source(&revoked);
        let (prompt, _) = card.question(&source).unwrap();
        let judgment = card.assess("2/16", &source).unwrap();
        let entry = json!({"kind":"entry","day":0,"seed":seed,"learner":learner,"policy":Arm::Regular.label(),
            "prompt":prompt,"response":"2/16","assisted":false,"judgment":format!("{judgment:?}")});
        Self {
            state: Learner::new(&participant, true),
            participant,
            term: 0,
            history: vec![entry],
        }
    }

    pub fn history(&self) -> &[Value] {
        &self.history
    }

    pub fn advance(&mut self) -> TermObservation {
        let revoked = BTreeSet::new();
        let rule = practice_source(&revoked);
        let session = TermSession {
            participant: &self.participant,
            term: self.term,
            arm: Arm::Regular,
            rule: &rule,
        };
        let mut counts = Counts::default();
        let independent_correct = self.state.advance_term(
            &session,
            &mut counts,
            &mut Counts::default(),
            &mut Events::Capture(&mut self.history),
        );
        self.term += 1;
        TermObservation {
            day: self.term * TERM,
            attended: counts.missed_probes == 0,
            independent_correct,
        }
    }

    // A missed visit has no audience. A delivery already made after this
    // visit closes it, even if the caller retries with different numbers.
    fn open_probe(&self) -> Result<&Value, String> {
        let latest = self
            .history
            .last()
            .expect("tracked histories start at entry");
        if latest["kind"] != "synthetic_probe" || latest["occasion"] != "Scheduled" {
            return Err(
                "delivery needs an attended scheduled check with its intervention still open"
                    .into(),
            );
        }
        Ok(latest)
    }

    fn validate_delivery(&self, quarters: u16, remaining: u16) -> Result<(), String> {
        let (_, answer) = amounts(quarters, remaining)?;
        let latest = self.open_probe()?;
        let q = latest["quarters"]
            .as_u64()
            .expect("observed starting amount");
        let r = latest["remaining_eighths"]
            .as_u64()
            .expect("observed remainder");
        if q == u64::from(quarters) || r == u64::from(remaining) || 2 * q - r == u64::from(answer) {
            return Err("the proposed exercise must change both quantities and the answer".into());
        }
        if self
            .state
            .questions_seen
            .contains(&dispenser_question(quarters.into(), remaining.into()))
        {
            return Err("the learner has already seen these quantities".into());
        }
        Ok(())
    }

    fn record(&mut self, quarters: u16, remaining: u16, mut receipt: Value) -> Value {
        // A later scheduled probe with the same quantities cannot be called fresh.
        self.state
            .questions_seen
            .insert(dispenser_question(quarters.into(), remaining.into()));
        let day = self.term * TERM;
        self.state.contact(day);
        receipt["day"] = json!(day);
        receipt["seed"] = json!(self.participant.seed);
        receipt["learner"] = json!(self.participant.id);
        receipt["policy"] = json!(Arm::Regular.label());
        receipt["quarters"] = json!(quarters);
        receipt["remaining_eighths"] = json!(remaining);
        self.history.push(receipt.clone());
        receipt
    }

    /// Choose fresh arithmetic for a route that names a lesson but supplies
    /// no exercise. The candidate order is public and uses only prior contact.
    pub fn teach(&mut self, lesson: Lesson) -> Result<Value, String> {
        self.open_probe()?;
        for q in 2..=40 {
            for r in 1..2 * q {
                if self.validate_delivery(q, r).is_ok() {
                    return self.teach_worked(lesson, q, r);
                }
            }
        }
        Err("all verified examples available for this visit have been seen".into())
    }

    /// Ask for the conversion step of the problem the learner just saw. This
    /// is a prompted observation. Showing it counts as contact, even on skip.
    /// Recording a response changes neither memory nor the evidence checkpoint.
    pub fn ask_initial_eighths(&mut self) -> Result<Value, String> {
        let probe = self.open_probe()?;
        let operand = |name: &str| {
            probe[name]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| format!("unsupported observed {name}"))
        };
        let quarters = operand("quarters")?;
        let remaining = operand("remaining_eighths")?;
        let prior_worked = probe["worked"].is_string();
        let assisted = probe["assisted"] == true || prior_worked;
        // Slot six is reserved for this question. It cannot alter future
        // attendance, assistance or answer draws on the original schedule.
        let term = self.term - 1;
        let skipped = self.participant.draw(term, 6, Noise::Attendance) >= 0.88;
        let response = (!skipped).then(|| {
            let converts =
                self.participant.draw(term, 6, Noise::Convert) < self.state.memory.conversion;
            let submitted = if converts {
                2 * i64::from(quarters)
            } else {
                i64::from(quarters)
            };
            format!("{submitted} eighths")
        });
        let judgment = response.as_ref().map(|answer| {
            format!(
                "{:?}",
                check_quantity_text(2 * i64::from(quarters), "eighths", answer)
            )
        });
        Ok(self.record(quarters,remaining,json!({"kind":"initial_eighths_diagnostic","display":INITIAL_EIGHTHS_QUESTION,
            "scope":"initial_amount_in_eighths","prompted":true,"prior_worked":prior_worked,"assisted":assisted,
            "skipped":skipped,"response":response,"judgment":judgment,"assessed":!skipped,"durability_credit":false})))
    }

    pub fn teach_worked(
        &mut self,
        lesson: Lesson,
        quarters: u16,
        remaining_eighths: u16,
    ) -> Result<Value, String> {
        self.validate_delivery(quarters, remaining_eighths)?;
        let display = worked_example(lesson, quarters, remaining_eighths)?;
        let memory = &mut self.state.memory;
        // The causal effect is a simulation assumption. Unit instruction closes
        // one quarter of the remaining conversion gap. No observation is forged.
        if lesson == Lesson::CompareUnits && memory.learns {
            memory.conversion += 0.25 * (1.0 - memory.conversion);
        }
        memory.encounter(self.term * TERM, false);
        Ok(self.record(
            quarters,
            remaining_eighths,
            json!({"kind":"worked_intervention","lesson":format!("{lesson:?}"),
            "display":display,"assessed":false,"durability_credit":false}),
        ))
    }

    pub fn attempt_transfer(
        &mut self,
        quarters: u16,
        remaining_eighths: u16,
    ) -> Result<Value, String> {
        self.validate_delivery(quarters, remaining_eighths)?;
        let prompt = transfer_question(quarters, remaining_eighths)?;
        let day = self.term * TERM;
        // A distinct random channel for the extra problem leaves all calendar
        // attendance, help and scheduled response draws paired with the baseline.
        let (assisted, response, judgment) = self.state.memory.volume_answer(
            &self.participant,
            self.term - 1,
            5,
            day,
            quarters.into(),
            remaining_eighths.into(),
        );
        let correct = judgment == Judgment::Correct;
        let worked = if assisted || !correct {
            Some(worked_example(
                Lesson::CompareUnits,
                quarters,
                remaining_eighths,
            )?)
        } else {
            None
        };
        // Retrieving the unit conversion on a changed application closes a
        // tenth of the modeled conversion gap. An error gets the printed
        // correction, with the ordinary worked-encounter memory gain.
        if !assisted && correct && self.state.memory.learns {
            self.state.memory.conversion += 0.10 * (1.0 - self.state.memory.conversion);
        }
        self.state.memory.encounter(day, correct && !assisted);
        Ok(self.record(quarters,remaining_eighths,json!({"kind":"transfer_intervention","prompt":prompt,
            "response":response,"assisted":assisted,"judgment":format!("{judgment:?}"),"worked":worked,
            "worked_before_answer":assisted,"occasion":"ImmediateReapplication","durability_credit":false})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finishing_followup_records_one_conversion_without_advancing_learning() {
        let mut learner = TrackedLearner::new(DEFAULT_SEED, 1);
        let mut comparison = TrackedLearner::new(DEFAULT_SEED, 1);
        assert!(learner.ask_initial_eighths().is_err());
        for _ in 0..2 {
            assert_eq!(learner.advance(), comparison.advance());
        }
        let last = learner.history().last().unwrap();
        assert_eq!(last["assisted"], true);
        assert_eq!(last["quarters"], 6);
        let checkpoint = learner.state.anchor.verified;
        let old_len = learner.history().len();
        let contacts = learner.state.contacts.len();
        let diagnostic = learner.ask_initial_eighths().unwrap();
        assert_eq!(diagnostic["day"], 182);
        assert_eq!(diagnostic["prompted"], true);
        assert_eq!(diagnostic["assisted"], true);
        assert_eq!(diagnostic["prior_worked"], true);
        assert_eq!(diagnostic["response"], "12 eighths");
        assert_eq!(diagnostic["judgment"], "Correct");
        assert_eq!(diagnostic["durability_credit"], false);
        assert_eq!(learner.state.contacts.len(), contacts + 1);
        assert_eq!(learner.state.anchor.verified, checkpoint);
        assert_eq!(
            learner.state.memory.conversion,
            comparison.state.memory.conversion
        );
        assert_eq!(
            learner.state.memory.stability_days,
            comparison.state.memory.stability_days
        );
        assert!(learner.ask_initial_eighths().is_err());
        assert!(learner.teach(Lesson::CompareUnits).is_err());
        assert_eq!(learner.history().len(), old_len + 1);
        println!("{diagnostic}");
        for _ in 2..24 {
            assert_eq!(learner.advance(), comparison.advance());
        }
    }

    #[test]
    fn diagnostic_skip_and_previously_worked_answer_have_distinct_receipts() {
        let mut responses = Vec::new();
        for id in 0..96 {
            let mut learner = TrackedLearner::new(DEFAULT_SEED, id);
            if !learner.advance().attended {
                assert!(learner.ask_initial_eighths().is_err());
                continue;
            }
            let prior = learner.history().last().unwrap().clone();
            let receipt = learner.ask_initial_eighths().unwrap();
            assert_eq!(
                receipt["assisted"],
                prior["assisted"] == true || prior["worked"].is_string()
            );
            if receipt["skipped"] == true {
                assert_eq!(receipt["assessed"], false);
                assert!(receipt["response"].is_null() && receipt["judgment"].is_null());
            } else {
                let submitted = receipt["response"].as_str().unwrap();
                assert_eq!(
                    receipt["judgment"],
                    format!(
                        "{:?}",
                        check_quantity_text(
                            2 * receipt["quarters"].as_i64().unwrap(),
                            "eighths",
                            submitted
                        )
                    )
                );
            }
            responses.push((prior, receipt));
        }
        assert!(responses.iter().any(|(_, r)| r["skipped"] == true));
        assert!(responses.iter().any(|(p, r)| p["assisted"] == false
            && r["assisted"] == true
            && r["assessed"] == true));
        assert!(responses.iter().any(|(_, r)| r["judgment"] == "Incorrect"));
        assert!(responses
            .iter()
            .any(|(_, r)| r["assisted"] == false && r["judgment"] == "Correct"));
    }

    #[test]
    fn unknown_or_closed_visits_cannot_accept_a_lesson_or_expose_later_receipts() {
        let mut learner = TrackedLearner::new(DEFAULT_SEED, 0);
        assert!(learner.teach_worked(Lesson::CompareUnits, 29, 7).is_err());
        let first = learner.advance();
        assert!(!first.attended);
        let prefix = learner.history().to_owned();
        assert!(learner.attempt_transfer(29, 7).is_err());
        assert_eq!(&prefix, learner.history());
        let second = learner.advance();
        assert!(second.attended);
        assert_eq!(&learner.history()[..prefix.len()], &prefix);
        assert_eq!(learner.history().last().unwrap()["day"], 182);
        let before = learner.state.anchor.verified;
        assert!(learner.attempt_transfer(6, 3).is_err());
        let transfer = learner.attempt_transfer(7, 1).unwrap();
        assert_eq!(transfer["prompt"], transfer_question(7, 1).unwrap());
        assert_eq!(transfer["durability_credit"], false);
        let response = transfer["response"].as_str().unwrap();
        assert_eq!(
            transfer["judgment"],
            format!("{:?}", check_quantity_text(13, "eighths", response))
        );
        assert!(learner.teach_worked(Lesson::CompareUnits, 29, 7).is_err());
        assert_eq!(before, learner.state.anchor.verified);
        learner.advance();
        let next = learner.history().last().unwrap();
        assert_eq!(next["kind"], "synthetic_probe");
        assert_eq!(next["quarters"], 7);
        assert_eq!(next["remaining_eighths"], 1);
        assert_eq!(next["fresh"], false);
        assert_eq!(next["finding"], "Ineligible");
        for _ in 3..24 {
            learner.advance();
        }
        assert!(learner
            .history()
            .windows(2)
            .all(|pair| pair[0]["day"].as_u64() <= pair[1]["day"].as_u64()));
        assert!(learner
            .history()
            .iter()
            .any(|event| event["kind"] == "term_absence"));
        assert!(learner
            .history()
            .iter()
            .any(|event| event["occasion"] == "ReturnFromAbsence"));
    }

    #[test]
    fn teaching_can_change_a_later_unaided_answer_only_under_the_learning_assumption() {
        let paired = |id, learns| {
            let mut baseline = TrackedLearner::new(DEFAULT_SEED, id);
            let mut taught = TrackedLearner::new(DEFAULT_SEED, id);
            baseline.state.memory.learns = learns;
            taught.state.memory.learns = learns;
            let start = baseline.advance();
            assert_eq!(taught.advance(), start);
            assert_eq!(taught.history(), baseline.history());
            if !start.attended {
                return false;
            }
            let anchor = taught.state.anchor.verified;
            let worked = taught.teach_worked(Lesson::CompareUnits, 29, 7).unwrap();
            assert_eq!(
                worked["display"],
                worked_example(Lesson::CompareUnits, 29, 7).unwrap()
            );
            assert_eq!(worked["assessed"], false);
            assert_eq!(taught.state.anchor.verified, anchor);
            let next = baseline.advance();
            let other = taught.advance();
            assert_eq!(next.attended, other.attended);
            if !learns {
                assert_eq!(next, other);
            }
            next.independent_correct == Some(false) && other.independent_correct == Some(true)
        };
        let cohort = 0..96;
        let changed: Vec<_> = cohort.clone().filter(|&id| paired(id, true)).collect();
        assert!(!changed.is_empty());
        assert!(!cohort.clone().any(|id| paired(id, false)));
        println!("seed {DEFAULT_SEED} day 182 independently correct only after day 91 unit lesson, learners {changed:?}");
    }
}
