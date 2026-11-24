//! One requested intervention at an observed, attended simulated visit.
use crate::learning_epochs::{
    dispenser_question, transfer_question, worked_example, Lesson, INITIAL_EIGHTHS_QUESTION,
    TRANSFER_RULE,
};
use serde_json::{json, Value};
use tutor_lab::{
    curriculum::goal_evidence::check_quantity_text,
    openrouter::{assistant_text, matches_reported, DecisionMenu, Role},
    protocol::Judgment,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Route,
    Worked,
    Transfer,
}

impl Focus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Worked => "worked",
            Self::Transfer => "transfer",
        }
    }
}

fn amount(v: &Value, k: &str) -> Option<u16> {
    u16::try_from(v.get(k)?.as_u64()?).ok()
}

fn public_rows(rows: &Value, day: u64, cap: usize, keys: &[&str]) -> Vec<Value> {
    let mut recent = Vec::new();
    let Some(entries) = rows.as_array() else {
        return recent;
    };
    for entry in entries
        .iter()
        .rev()
        .filter(|v| v["day"].as_u64().is_some_and(|d| d < day))
        .take(cap)
    {
        let mut selected = json!({"day":entry["day"]});
        for &key in keys {
            if let Some(value) = entry.get(key).filter(|v| {
                v.is_boolean() || v.is_number() || v.as_str().is_some_and(|s| s.len() <= 72)
            }) {
                selected[key] = value.clone();
            }
        }
        recent.push(selected);
    }
    recent.reverse();
    recent
}

struct Visit {
    q: u16,
    r: u16,
    answer: i64,
    assisted: bool,
    level: u64,
    packet: Value,
}

impl Visit {
    fn read(ctx: &Value) -> Result<Self, String> {
        let q = amount(ctx, "quarters")
            .filter(|n| (1..=200).contains(n))
            .ok_or("invalid observed quarter-litres")?;
        let r = amount(ctx, "remaining_eighths")
            .filter(|n| *n > 0 && *n < 2 * q)
            .ok_or("invalid observed remainder")?;
        let response = ctx["response"]
            .as_str()
            .ok_or("no response at this visit")?;
        let answer: i64 = response
            .trim()
            .strip_suffix(" eighths")
            .and_then(|v| v.parse().ok())
            .ok_or("missing signed count of eighths")?;
        let assisted = ctx["assisted"]
            .as_bool()
            .ok_or("missing assistance history")?;
        let day = ctx["day"].as_u64().ok_or("missing observed day")?;
        let coverage = ctx["coverage"]
            .as_bool()
            .ok_or("missing exposure coverage")?;
        let prompt = ctx["prompt"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 600)
            .ok_or("missing observed exercise")?;
        if prompt != dispenser_question(i64::from(q), i64::from(r)) {
            return Err("recorded prompt does not match its quantities".into());
        }
        let expected = i64::from(2 * q - r);
        let judgment = if answer == expected {
            "Correct"
        } else {
            "Incorrect"
        };
        if ctx["judgment"] != judgment {
            return Err("grade conflicts with recorded exercise".into());
        }
        let level = ctx["specificity"]
            .as_u64()
            .or_else(|| ctx["stage"].as_u64())
            .filter(|n| (1..=3).contains(n))
            .ok_or("specificity must be 1, 2, or 3")?;
        // Project earlier public observations; synthetic latent memory and
        // private transport data cannot become coaching context by accident.
        let recent = public_rows(
            ctx.get("history").unwrap_or(&ctx["recent_history"]),
            day,
            [0, 1, 3][(level - 1) as usize],
            &[
                "kind",
                "judgment",
                "assisted",
                "quarters",
                "remaining_eighths",
                "response",
                "lesson",
            ],
        );
        let previous = public_rows(
            &ctx["previous_interventions"],
            day,
            (level - 1) as usize,
            &[
                "selected",
                "validated",
                "delivered",
                "kind",
                "judgment",
                "assisted",
                "skipped",
                "assessed",
                "lesson",
                "quarters",
                "remaining_eighths",
                "response",
            ],
        );
        let prior = ctx["prior_route"].as_str().filter(|s| {
            level >= 2 && {
                matches!(
                    *s,
                    "compare_units"
                        | "rehearse_subtraction"
                        | "request_intermediate"
                        | "continue_without_coaching"
                )
            }
        });
        let packet = json!({"day":day,"quarters":q,"remaining_eighths":r,"source":TRANSFER_RULE,
            "prompt":prompt,"response":response,"judgment":judgment,"assisted":assisted,
            "coverage":coverage,"specificity":level,"recent_history":recent,"previous_interventions":previous,"prior_route":prior});
        Ok(Self {
            q,
            r,
            answer,
            assisted,
            level,
            packet,
        })
    }

    fn indicated(&self) -> &'static str {
        let (q, r) = (i64::from(self.q), i64::from(self.r));
        if self.assisted {
            "request_intermediate"
        } else if self.answer == 2 * q - r {
            "continue_without_coaching"
        } else if self.answer == q - r {
            "compare_units"
        } else if self.answer == 2 * q + r {
            "rehearse_subtraction"
        } else {
            "request_intermediate"
        }
    }

    fn last_units_check(&self) -> Option<Judgment> {
        let diagnostic = self.packet["previous_interventions"]
            .as_array()?
            .iter()
            .rev()
            .find(|receipt| {
                receipt["validated"] == true
                    && receipt["delivered"] == true
                    && receipt["kind"] == "initial_eighths_diagnostic"
                    && receipt["selected"] == "request_intermediate"
            })?;
        if diagnostic["skipped"] != false
            || diagnostic["assessed"] != true
            || diagnostic["assisted"] != false
        {
            return None;
        }
        let quarters = amount(diagnostic, "quarters").filter(|n| (1..=200).contains(n))?;
        let checked = check_quantity_text(
            2 * i64::from(quarters),
            "eighths",
            diagnostic["response"].as_str()?,
        );
        match (checked, diagnostic["judgment"].as_str()?) {
            (Judgment::Correct, "Correct") | (Judgment::Incorrect, "Incorrect") => Some(checked),
            _ => None,
        }
    }

    fn worked_focus(&self) -> (&'static str, &'static str) {
        match self.indicated() {
            "compare_units" => ("compare_units","Today's unaided amount matches subtracting before converting. Show both amounts in eighths."),
            "rehearse_subtraction" => ("rehearse_subtraction","Today's unaided amount matches adding the remainder. Work through the missing amount."),
            "continue_without_coaching" => ("compare_units","Today's unaided answer matched the whole problem. Use the scheduled worked example as further practice."),
            _ => match self.last_units_check() {
                Some(Judgment::Incorrect) => ("compare_units","The last unaided initial-volume answer differed from the converted starting amount. Revisit that step."),
                Some(Judgment::Correct) => ("rehearse_subtraction","The last unaided initial-volume answer matched the converted starting amount. Use that amount in a subtraction example. The earlier question assessed conversion alone."),
                _ if self.packet["prior_route"]=="rehearse_subtraction" => ("rehearse_subtraction","Continue the previously chosen subtraction practice. The current answer gives no new independent indication."),
                _ => ("compare_units","There is no independent indication of a particular difficulty. Start the worked example by expressing both amounts in eighths."),
            }
        }
    }
}

fn base(focus: Focus, visit: &Visit) -> Value {
    json!({"focus":focus.label(),"day":visit.packet["day"],"specificity":visit.level,
        "passed":false,"selected":null,"display":null,"verified_lesson":null,
        "verified_transfer":null,"verified_worked":null,"published":false})
}

pub fn run(
    focus: Focus,
    ctx: &Value,
    jev: &str,
    luna: &str,
    mut send: impl FnMut(Role, &Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let visit = Visit::read(ctx)?;
    let mut result = base(focus, &visit);
    if focus == Focus::Route {
        let scope = json!({"visit":ctx["visit"],"mode":ctx["mode"],"ordinal":ctx["stage"]});
        let menu = DecisionMenu::new("epoch-volume-next/1",jev,&scope,&visit.packet,
            "Choose the next teaching action based on the observed response. An answer pattern supports a targeted check, never a diagnosis. If assistance was present, ask for an intermediate step first. Otherwise, prefer the specific operation indicated by the amount; after an independently correct answer offer continued work.",
            json!({"compare_units":"The amount matches subtracting the remainder before converting the quarter-litres. Show a conversion demonstration.",
                "rehearse_subtraction":"The amount matches adding the remainder after conversion. Check subtraction with a missing-part demonstration.",
                "request_intermediate":"Ask how many eighths there were at the start when help was disclosed or neither error pattern matches.",
                "continue_without_coaching":"The response fits the requested quantity and no help was recorded. Leave the learner free to proceed."}))?;
        let raw = send(Role::Jev, menu.request())?;
        let selected = match menu.selected(&raw) {
            Ok(s) => s,
            Err(e) => {
                result["error"] = json!(e);
                return Ok(result);
            }
        };
        result["selected"] = json!(selected);
        result["expected"] = json!(visit.indicated());
        result["passed"] = json!(selected == visit.indicated());
        if selected != visit.indicated() {
            return Ok(result);
        }
        match selected {
            "compare_units" | "rehearse_subtraction" => {
                result["verified_lesson"] = json!(selected);
                result["display"] = json!(if selected == "compare_units" {
                    "Offer a worked conversion example."
                } else {
                    "Offer a worked subtraction example."
                });
            }
            "request_intermediate" => result["display"] = json!(INITIAL_EIGHTHS_QUESTION),
            _ => result["display"] = json!("You can continue when you are ready."),
        }
        return Ok(result);
    }
    let mut context = json!({"focus":focus.label(),"observed":visit.packet});
    if focus == Focus::Worked {
        let (teaching_move, basis) = visit.worked_focus();
        context["teaching_move"] = json!(teaching_move);
        context["teaching_basis"] = json!(basis);
        result["teaching_basis"] = json!(basis);
    }
    let wire = json!({"model":luna,"max_tokens":384,"reasoning":{"effort":"low","exclude":true},
        "provider":{"require_parameters":true},"response_format":{"type":"json_object"},
        "messages":[{"role":"system","content":INSTRUCTIONS},{"role":"user","content":context.to_string()}]});
    let raw = send(Role::Luna, &wire)?;
    if !raw["model"]
        .as_str()
        .is_some_and(|m| matches_reported(luna, m))
    {
        result["error"] = json!("unexpected or missing model");
        return Ok(result);
    }
    let text = match assistant_text(&raw) {
        Ok(s) => s,
        Err(e) => {
            result["error"] = json!(e);
            return Ok(result);
        }
    };
    result["assistant_text"] = json!(text);
    match serde_json::from_str(text) {
        Ok(draft) => score(focus, &visit, &draft, &mut result),
        Err(_) => result["error"] = json!("invalid candidate JSON"),
    }
    Ok(result)
}

const INSTRUCTIONS: &str = concat!(
    "Create a household measuring-jug activity for this observed synthetic visit. ",
    "For worked, draft a brief explanation emphasizing teaching_move within the evidence described by teaching_basis. Avoid claims about the learner's private reasoning. For transfer, draft an invitation to an unsolved jug-and-bowl exercise. ",
    "Output exactly 8 JSON fields: observation_quote (exact response), source_quote (entire supplied source verbatim), ",
    "quarters (integer 2–40 initially in jug), remaining_eighths (integer 1 through 2*quarters minus 1 after filling the bowl), ",
    "initial_eighths, delivered_eighths, unconverted_result (quarters minus remaining_eighths), and draft_text (your original sentence). ",
    "Change both observed quantities and the correct answer. Avoid all previously seen pairs in the evidence. ",
    "At specificity 2 and 3, make unconverted_result different from the observed response. ",
    "At 3 also require a positive unconverted_result and a correct answer different from the observed response and the correct answers of earlier delivered examples. ",
    "Be concise and use only the quoted source. The program will check the numbers."
);

fn previously_seen(visit: &Visit, q: u16, r: u16) -> bool {
    let history = &visit.packet["recent_history"];
    let interventions = &visit.packet["previous_interventions"];
    history
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            interventions
                .as_array()
                .into_iter()
                .flatten()
                .filter(|v| v["delivered"] == true),
        )
        .any(|v| amount(v, "quarters") == Some(q) && amount(v, "remaining_eighths") == Some(r))
}

fn repeats_delivered_answer(visit: &Visit, net: i64) -> bool {
    visit.packet["previous_interventions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["delivered"] == true)
        .filter_map(|v| {
            let initial = 2 * i64::from(amount(v, "quarters")?);
            if v["kind"] == "initial_eighths_diagnostic" {
                // That question only asked for the initial count; it had no
                // missing-volume answer, and a skipped prompt had no submission.
                return (v["assessed"] == true).then_some(initial);
            }
            Some(initial - i64::from(amount(v, "remaining_eighths")?))
        })
        .any(|earlier| earlier == net)
}

fn score(focus: Focus, visit: &Visit, draft: &Value, result: &mut Value) {
    let Some(object) = draft.as_object().filter(|v| v.len() == 8) else {
        result["error"] = json!("expected eight candidate fields");
        return;
    };
    let Some((q, r)) = amount(draft, "quarters").zip(amount(draft, "remaining_eighths")) else {
        result["error"] = json!("expected integer exercise amounts");
        return;
    };
    let Ok(question) = transfer_question(q, r) else {
        result["error"] = json!("invalid household exercise bounds");
        return;
    };
    // Compute both the correct count and the observable near-miss independently.
    let initial = i64::from(q) * 2;
    let net = initial - i64::from(r);
    let wrong = i64::from(q) - i64::from(r);
    let checks = json!({
        "bound_observation":draft["observation_quote"]==visit.packet["response"],
        "bound_source":draft["source_quote"]==TRANSFER_RULE,
        "initial_amount":draft["initial_eighths"].as_i64()==Some(initial),
        "delivered_amount":draft["delivered_eighths"].as_i64()==Some(net),
        "wrong_conversion_amount":draft["unconverted_result"].as_i64()==Some(wrong),
        "has_draft":object.get("draft_text").and_then(Value::as_str).is_some_and(|s|!s.trim().is_empty() && s.len()<=640),
        "new_operands_and_answer":q!=visit.q && r!=visit.r && net!=i64::from(2*visit.q-visit.r),
        "fresh_in_public_history":!previously_seen(visit,q,r),
        "level_two":visit.level < 2 || wrong!=visit.answer,
        "level_three":visit.level < 3 || (wrong>0 && net!=visit.answer && !repeats_delivered_answer(visit,net))
    });
    let passed = checks.as_object().unwrap().values().all(|v| v == true);
    result["checks"] = checks;
    result["passed"] = json!(passed);
    if passed {
        let quantities = json!({"quarters":q,"remaining_eighths":r});
        match focus {
            Focus::Worked => {
                let (kind, _) = visit.worked_focus();
                result["verified_lesson"] = json!(kind);
                result["verified_worked"] = quantities;
                let lesson = if kind == "compare_units" {
                    Lesson::CompareUnits
                } else {
                    Lesson::RehearseSubtraction
                };
                result["display"] =
                    json!(worked_example(lesson, q, r).expect("validated household quantities"));
            }
            Focus::Transfer => {
                result["verified_transfer"] = quantities;
                result["display"] = json!(question);
            }
            Focus::Route => unreachable!(),
        }
    }
}

/// Offline control only. Derives its decisions and quantities from the same
/// observed evidence supplied to the live models. The caller labels the run.
pub fn scripted(role: Role, wire: &Value) -> Result<Value, String> {
    match role {
        Role::Jev => {
            let visit = Visit::read(&wire["state"])?;
            let key = wire["questions"]
                .as_object()
                .and_then(|q| q.keys().next())
                .ok_or("missing choice")?;
            Ok(
                json!({"model":wire["model"],"answers":{(key):{"type":"choice","choice":visit.indicated()}}}),
            )
        }
        Role::Luna => {
            let user: Value = serde_json::from_str(
                wire["messages"][1]["content"]
                    .as_str()
                    .ok_or("missing public brief")?,
            )
            .map_err(|e| e.to_string())?;
            let visit = Visit::read(&user["observed"])?;
            // Vary the fixture by the observed visit. Keep its checks independent
            // of the scorer so it can serve as a functional transport control.
            let start = (visit.packet["day"].as_u64().ok_or("missing fixture day")? % 39) as u16;
            let mut rows = (0..39)
                .map(|n| 2 + (start + n) % 39)
                .flat_map(|q| (1..q).map(move |r| (q, r)));
            let (q, r) = rows
                .find(|&(q, r)| {
                    let net = i64::from(2 * q - r);
                    q != visit.q
                        && r != visit.r
                        && net != i64::from(2 * visit.q - visit.r)
                        && i64::from(q - r) != visit.answer
                        && net != visit.answer
                        && !previously_seen(&visit, q, r)
                        && !repeats_delivered_answer(&visit, net)
                })
                .ok_or("no changed control case")?;
            let draft = json!({"observation_quote":visit.packet["response"],"source_quote":TRANSFER_RULE,"quarters":q,
                "remaining_eighths":r,"initial_eighths":2*q,"delivered_eighths":2*q-r,"unconverted_result":i64::from(q)-i64::from(r),
                "draft_text":if user["focus"]=="transfer"{"How much water reached the bowl?"}else{"Express the starting and ending volumes using the same unit before subtracting."}});
            Ok(
                json!({"model":wire["model"],"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":draft.to_string()}}]}),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutor_lab::openrouter::{DEFAULT_JEV, DEFAULT_LUNA};

    fn context(submitted: i64, assisted: bool, level: u8) -> Value {
        json!({"day":180,"quarters":9,"remaining_eighths":3,
            "prompt":dispenser_question(9,3),"response":format!("{submitted} eighths"),
            "judgment":if submitted==15 {"Correct"} else {"Incorrect"},
            "assisted":assisted,"coverage":true,"specificity":level,"visit":4})
    }

    fn candidate(q: i64, r: i64) -> Value {
        json!({"quarters":q,"remaining_eighths":r,"initial_eighths":2*q,"delivered_eighths":2*q-r,
            "unconverted_result":q-r,"source_quote":TRANSFER_RULE,"observation_quote":"6 eighths",
            "draft_text":"An unreviewed sentence claiming that a quarter has seven eighths."})
    }

    fn generation(focus: Focus, ctx: &Value, draft: &Value) -> Value {
        run(focus, ctx, DEFAULT_JEV, DEFAULT_LUNA, |role, wire| {
            assert_eq!(role, Role::Luna);
            assert_eq!(wire["reasoning"], json!({"effort":"low","exclude":true}));
            Ok(
                json!({"model":wire["model"],"choices":[{"finish_reason":"stop","message":{
                "role":"assistant","content":draft.to_string(),"reasoning":"PRIVATE"}}]}),
            )
        })
        .unwrap()
    }

    #[test]
    fn answer_and_assistance_change_the_choice_and_a_valid_but_wrong_choice_cannot_teach() {
        for (answer, assisted, expected) in [
            (6, false, "compare_units"),
            (21, false, "rehearse_subtraction"),
            (15, false, "continue_without_coaching"),
            (15, true, "request_intermediate"),
            (8, false, "request_intermediate"),
        ] {
            let report = run(
                Focus::Route,
                &context(answer, assisted, 1),
                DEFAULT_JEV,
                DEFAULT_LUNA,
                scripted,
            )
            .unwrap();
            assert_eq!(report["selected"], expected);
            assert_eq!(report["passed"], true);
        }
        // Make a well-formed answer using a genuine menu key but pick the other demonstration.
        let report = run(
            Focus::Route,
            &context(21, false, 1),
            DEFAULT_JEV,
            DEFAULT_LUNA,
            |role, wire| {
                let mut raw = scripted(role, wire)?;
                let answer = raw["answers"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap();
                answer["choice"] = json!("compare_units");
                Ok(raw)
            },
        )
        .unwrap();
        assert_eq!(report["passed"], false);
        assert!(report["verified_lesson"].is_null() && report["display"].is_null());
    }

    #[test]
    fn a_verified_units_answer_changes_the_next_worked_brief_when_the_new_answer_is_ambiguous() {
        fn observe(ctx: &Value) -> (Value, Value) {
            let mut brief = Value::Null;
            let result = run(
                Focus::Worked,
                ctx,
                DEFAULT_JEV,
                DEFAULT_LUNA,
                |role, wire| {
                    brief = serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap())
                        .unwrap();
                    scripted(role, wire)
                },
            )
            .unwrap();
            (brief, result)
        }
        let diagnostic = json!({"kind":"initial_eighths_diagnostic","selected":"request_intermediate",
            "day":90,"validated":true,"delivered":true,"assisted":false,"assessed":true,"skipped":false,
            "quarters":7,"remaining_eighths":1,"response":"7 eighths","judgment":"Incorrect"});
        // Everything today is identical. Only the response and grade in the earlier
        // conversion question differ between these visits.
        let mut ctx = context(8, false, 2);
        ctx["previous_interventions"] = json!([diagnostic]);
        let (incorrect, first) = observe(&ctx);
        assert_eq!(
            incorrect["observed"]["previous_interventions"][0]["kind"],
            "initial_eighths_diagnostic"
        );
        assert_eq!(incorrect["teaching_move"], "compare_units");
        assert_eq!(first["verified_lesson"], "compare_units");
        assert!(first["teaching_basis"]
            .as_str()
            .unwrap()
            .contains("differed"));
        ctx["previous_interventions"][0]["response"] = json!("14 eighths");
        ctx["previous_interventions"][0]["judgment"] = json!("Correct");
        let (correct, second) = observe(&ctx);
        assert_eq!(correct["teaching_move"], "rehearse_subtraction");
        assert_eq!(second["verified_lesson"], "rehearse_subtraction");
        assert!(second["teaching_basis"]
            .as_str()
            .unwrap()
            .contains("assessed conversion alone"));

        // A current answer clearly matching the other operation takes priority.
        ctx["response"] = json!("6 eighths");
        let (_, current) = observe(&ctx);
        assert_eq!(current["verified_lesson"], "compare_units");
        assert!(current["teaching_basis"]
            .as_str()
            .unwrap()
            .starts_with("Today's"));
        ctx["response"] = json!("8 eighths");

        for (key, value) in [
            ("assisted", json!(true)),
            ("skipped", json!(true)),
            ("assessed", json!(false)),
            ("delivered", json!(false)),
            ("validated", json!(false)),
            ("response", json!("7 eighths")),
        ] {
            let mut unverified = ctx.clone();
            unverified["previous_interventions"][0][key] = value;
            let (brief, _) = observe(&unverified);
            assert!(
                brief["teaching_basis"]
                    .as_str()
                    .unwrap()
                    .starts_with("There is no independent"),
                "{key}"
            );
        }
    }

    #[test]
    fn three_levels_place_progressively_stricter_requirements_on_the_same_candidates() {
        // Observed: 9 quarters, 3 eighths left, the student submitted 6. 10-4 repeats
        // that answer as the proposed distractor. 4-5 is implausible as a positive volume.
        for (q, r, expected) in [
            (10, 4, [true, false, false]),
            (4, 5, [true, true, false]),
            (14, 5, [true, true, true]),
        ] {
            for (i, passes) in expected.into_iter().enumerate() {
                let report = generation(
                    Focus::Transfer,
                    &context(6, false, i as u8 + 1),
                    &candidate(q, r),
                );
                assert_eq!(report["passed"], passes, "{report}");
                assert_eq!(report["verified_transfer"].is_object(), passes);
            }
        }
        let mut ctx = context(6, false, 3);
        ctx["previous_interventions"] =
            json!([{"day":125,"delivered":true,"quarters":14,"remaining_eighths":5}]);
        let repeated = generation(Focus::Transfer, &ctx, &candidate(14, 5));
        assert_eq!(repeated["checks"]["fresh_in_public_history"], false);
        // This new pair also yields 23, already taught in the worked example.
        let changed_pair = generation(Focus::Transfer, &ctx, &candidate(15, 7));
        assert_eq!(changed_pair["checks"]["new_operands_and_answer"], true);
        assert_eq!(changed_pair["checks"]["level_three"], false);
        // A diagnostic asks for the *initial* count. It never taught the
        // missing volume on that occasion.
        ctx["previous_interventions"][0]["kind"] = json!("initial_eighths_diagnostic");
        ctx["previous_interventions"][0]["assessed"] = json!(true);
        let new_net = generation(Focus::Transfer, &ctx, &candidate(15, 7));
        assert_eq!(new_net["passed"], true);
        let repeated_initial = generation(Focus::Transfer, &ctx, &candidate(17, 6));
        assert_eq!(repeated_initial["checks"]["level_three"], false);
    }

    #[test]
    fn the_worked_display_uses_validated_numbers_and_the_known_source() {
        let ctx = context(6, false, 3);
        let generated = candidate(14, 5);
        let report = generation(Focus::Worked, &ctx, &generated);
        assert_eq!(
            report["display"],
            worked_example(Lesson::CompareUnits, 14, 5).unwrap()
        );
        assert_eq!(
            report["verified_worked"],
            json!({"quarters":14,"remaining_eighths":5})
        );
        assert!(report["assistant_text"].as_str().unwrap().contains("seven"));
        assert!(!report.to_string().contains("PRIVATE"));
        for (key, replacement) in [
            ("source_quote", json!("Trust me.")),
            ("observation_quote", json!("8 eighths")),
            ("initial_eighths", json!(14)),
            ("delivered_eighths", json!(33)),
            ("unconverted_result", json!(7)),
        ] {
            let mut wrong = generated.clone();
            wrong[key] = replacement;
            let failed = generation(Focus::Worked, &ctx, &wrong);
            assert_eq!(failed["passed"], false, "{key}");
            assert!(failed["display"].is_null() && failed["verified_lesson"].is_null());
        }
        let mut wrong_fields = generated;
        wrong_fields.as_object_mut().unwrap().remove("draft_text");
        wrong_fields["extra"] = json!(true);
        assert_eq!(
            generation(Focus::Worked, &ctx, &wrong_fields)["passed"],
            false
        );
    }

    #[test]
    fn model_context_excludes_future_private_and_unobserved_state_and_binds_to_original_grade() {
        let mut ctx = context(6, false, 3);
        ctx["history"] = json!([{"day":90,"quarters":7,"remaining_eighths":2,"latent":"PRIVATE"},{"day":210,"response":"FUTURE"}]);
        ctx["previous_interventions"] = json!([{"day":135,"delivered":true,"quarters":8,"remaining_eighths":3,"meta":"PRIVATE"}]);
        ctx["latent"] = json!("PRIVATE");
        let result = run(
            Focus::Transfer,
            &ctx,
            DEFAULT_JEV,
            DEFAULT_LUNA,
            |role, wire| {
                let packet: Value =
                    serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
                let observed = &packet["observed"];
                assert_eq!(observed["recent_history"].as_array().unwrap().len(), 1);
                assert_eq!(observed["previous_interventions"][0]["quarters"], 8);
                assert!(
                    !wire.to_string().contains("PRIVATE") && !wire.to_string().contains("FUTURE")
                );
                let mut reply = scripted(role, wire)?;
                reply["model"] = json!(DEFAULT_JEV);
                Ok(reply)
            },
        )
        .unwrap();
        assert_eq!(result["passed"], false);
        assert!(result["display"].is_null());
        ctx["judgment"] = json!("Correct");
        assert!(run(
            Focus::Route,
            &ctx,
            DEFAULT_JEV,
            DEFAULT_LUNA,
            |_, _| panic!("untrustworthy observation reached transport")
        )
        .is_err());
    }
}
