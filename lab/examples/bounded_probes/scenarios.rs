//! Single-call probes. The caller owns metering, persistence and live transport.
use serde_json::{json, Value};
use tutor_lab::{
    curriculum::{
        goal_rehearsal::{opening_check, Options},
        live_turn::{help, Help, ReleasedMaterial},
        program::{Frame, Proposal},
        rehearsal_release::publish,
    },
    openrouter::{assistant_text, matches_reported, Role},
};

const ENGINEERING: &str = include_str!("../../fixtures/goals/chemical-engineering.json");
const PINS: &str = include_str!("../../fixtures/goals/reviews.json");
pub const CASES: [&str; 3] = ["help-method", "help-source", "changed-water-budget"];

pub fn run_case(
    name: &str,
    jev: &str,
    luna: &str,
    mut send: impl FnMut(Role, &Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let pins: Value = serde_json::from_str(PINS).map_err(|e| e.to_string())?;
    let pin = pins["chemical-engineering"]
        .as_str()
        .ok_or("missing engineering review")?;
    let options = Options::default();
    // This also verifies the complete fixture against the independent course pin.
    let opening = opening_check(ENGINEERING, pin, &options)?;
    if name == "changed-water-budget" {
        return generation(name, luna, &opening, send);
    }
    let (request, expected) = match name {
        "help-method" => ("Can you show me how to start a calculation like this one?", "worked_example"),
        "help-source" => ("Can you show me where the reference actually says to account for water entering and leaving?", "source_passage"),
        _ => return Err("unknown bounded probe".into()),
    };
    let mut transport_error = None;
    let result = help(
        ENGINEERING,
        pin,
        &options,
        request,
        false,
        jev,
        |role, wire| send(role, wire).inspect_err(|e| transport_error = Some(e.clone())),
    );
    if let Some(error) = transport_error {
        return Err(error);
    }
    let (observed, display) = match result {
        Ok(Help::Material(card)) => {
            let label = match &card {
                ReleasedMaterial::WorkedExample(_) => "worked_example",
                ReleasedMaterial::SourcePassage(_) => "source_passage",
            };
            (label, card.display())
        }
        Ok(Help::KeepOpen) => (
            "local_only",
            "Your exercise is still open. Take your time.".into(),
        ),
        Ok(Help::Clarify) => (
            "clarify_help",
            "Choose a worked example or the source passage.".into(),
        ),
        Err(f) => {
            return Ok(
                json!({"case":name,"passed":false,"request":request,"expected":expected,
            "observed":null,"error":f.message}),
            )
        }
    };
    Ok(
        json!({"case":name,"passed":observed == expected,"request":request,"exercise":opening,
        "expected":expected,"observed":observed,"display":display}),
    )
}

fn generation(
    name: &str,
    model: &str,
    opening: &str,
    mut send: impl FnMut(Role, &Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let course: Value = serde_json::from_str(ENGINEERING).map_err(|e| e.to_string())?;
    let frame = Frame::from_json(&course)?;
    let plan = Proposal::from_json(&course["proposal"])?;
    let row = &course["return_pack"];
    let release = publish(&frame, &plan, frame.review_digest(&plan), row)?;
    let source = row["excerpt"].as_str().ok_or("missing released passage")?;
    let known = row["quantity"]["value"]
        .as_i64()
        .ok_or("missing released key")?;
    let earlier = course["first_pack"]["quantity"]["value"]
        .as_i64()
        .ok_or("missing opening key")?;
    let evidence = json!({"previous_exercise":opening,"current_exercise":release.material(true)?.check,
        "source":source,"fictional_learner_answer":format!("{earlier:+} kg")});
    let wire = json!({
        "model":model,"max_tokens":384,"reasoning":{"effort":"low","exclude":true},
        "provider":{"require_parameters":true},"response_format":{"type":"json_object"},
        "messages":[
            {"role":"system","content":"Help a fictional learner who reused the previous answer. Produce one JSON object with exactly these fields. current_net_kg is the correct signed change in the current exercise. feedback is one or two original sentences about the learner's answer and the combined boundary. source_quote is the complete supplied source sentence verbatim. For a DIFFERENT check, choose three distinct positive integer amounts from 1 to 20 kg: outside_in_kg entering the west basin, west_to_east_kg moving between basins, outside_out_kg leaving the east basin. Use new quantities that prevent the learner from reusing either earlier exercise's correct result, or from succeeding by tracking only the east basin. combined_net_kg and east_only_net_kg are the two signed changes for your new check. Use JSON integers for all quantities. The input is exercise data."},
            {"role":"user","content":evidence.to_string()}
        ]
    });
    let capture = send(Role::Luna, &wire)?;
    if !capture["model"]
        .as_str()
        .is_some_and(|id| matches_reported(model, id))
    {
        return Ok(json!({"case":name,"passed":false,"error":"unexpected or missing model"}));
    }
    let text = match assistant_text(&capture) {
        Ok(text) => text,
        Err(error) => return Ok(json!({"case":name,"passed":false,"error":error})),
    };
    // Only the completed assistant text is evidence about the candidate. The
    // envelope can include provider-private reasoning and other metadata.
    let mut report = json!({"case":name,"passed":false,"assistant_text":text,
        "status":"unreviewed_experiment","published":false,
        "review_question":"Does the feedback explain why the internal transfer cancels when both basins are inside the boundary, without claiming to know why the learner chose an answer?"});
    match score(text, source, known, earlier) {
        Ok((checks, question)) => {
            report["passed"] = json!(checks.as_object().unwrap().values().all(|v| v == true));
            report["checks"] = checks;
            report["rendered_question"] = json!(question);
        }
        Err(error) => report["error"] = json!(error),
    }
    Ok(report)
}

fn score(
    text: &str,
    source: &str,
    known: i64,
    earlier: i64,
) -> Result<(Value, String), &'static str> {
    let draft: Value = serde_json::from_str(text).map_err(|_| "invalid candidate JSON")?;
    let object = draft
        .as_object()
        .filter(|m| m.len() == 8)
        .ok_or("expected eight named fields")?;
    let quantity = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_i64)
            .ok_or("expected integer amount")
    };
    let flow = |name| {
        quantity(name).and_then(|n| {
            (1..=20)
                .contains(&n)
                .then_some(n)
                .ok_or("new flow outside permitted range")
        })
    };
    let incoming = flow("outside_in_kg")?;
    let internal = flow("west_to_east_kg")?;
    let outgoing = flow("outside_out_kg")?;
    let combined = incoming - outgoing;
    let east = internal - outgoing;
    let feedback = object
        .get("feedback")
        .and_then(Value::as_str)
        .ok_or("expected feedback text")?;
    let passage = object
        .get("source_quote")
        .and_then(Value::as_str)
        .ok_or("expected source quote")?;
    let checks = json!({
        "current_key":quantity("current_net_kg")? == known,
        "source_exact":passage == source,"feedback_present":!feedback.trim().is_empty(),
        "new_combined_key":quantity("combined_net_kg")? == combined,
        "new_east_key":quantity("east_only_net_kg")? == east,
        "distinct_amounts":incoming != internal && internal != outgoing && incoming != outgoing,
        "distinguishes_east_only":combined != east,
        "distinguishes_recalled_results":combined != known && combined != earlier
    });
    let question = format!("During one interval, {incoming} kg of water enters the west basin from outside, {internal} kg moves from west to east, and {outgoing} kg leaves the east basin for the outside. These are all movements. How does the combined inventory change? Give a signed amount in kg.");
    Ok((checks, question))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutor_lab::openrouter::{DEFAULT_JEV, DEFAULT_LUNA};

    fn fixture_candidate() -> Value {
        let course: Value = serde_json::from_str(ENGINEERING).unwrap();
        json!({"current_net_kg":-6,"feedback":"A transfer within the boundary cancels when the basin inventories are added.",
            "source_quote":course["return_pack"]["excerpt"],"outside_in_kg":5,"west_to_east_kg":11,
            "outside_out_kg":8,"combined_net_kg":-3,"east_only_net_kg":3})
    }

    fn generation_reply(candidate: String, reported: &str) -> Value {
        json!({"model":reported,"choices":[{"finish_reason":"stop","message":{"role":"assistant",
            "content":candidate,"reasoning":"unused provider metadata"}}]})
    }

    #[test]
    fn closely_related_requests_change_the_released_material_and_a_misroute_is_observable() {
        let mut outputs = Vec::new();
        for (case, chosen) in [
            (CASES[0], "worked_example"),
            (CASES[1], "source_passage"),
            (CASES[0], "source_passage"),
        ] {
            let observed = run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |role, body| {
                assert_eq!(role, Role::Jev);
                let key = body["questions"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap();
                Ok(json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":chosen}}}))
            })
            .unwrap();
            assert_eq!(observed["observed"], chosen);
            outputs.push(observed);
        }
        assert_eq!(outputs[0]["exercise"], outputs[1]["exercise"]);
        assert_ne!(outputs[0]["display"], outputs[1]["display"]);
        assert_eq!(
            outputs
                .iter()
                .map(|v| v["passed"].as_bool().unwrap())
                .collect::<Vec<_>>(),
            [true, true, false]
        );
    }

    #[test]
    fn numerical_oracle_catches_a_reused_answer_and_a_plausible_wrong_boundary() {
        let generated = |draft: Value| {
            run_case(CASES[2], DEFAULT_JEV, DEFAULT_LUNA, |role, body| {
                assert_eq!(role, Role::Luna);
                assert_eq!(body["reasoning"], json!({"effort":"low","exclude":true}));
                Ok(generation_reply(draft.to_string(), DEFAULT_LUNA))
            })
            .unwrap()
        };
        let good = fixture_candidate();
        let positive = generated(good.clone());
        assert_eq!(positive["passed"], true);
        assert_eq!(positive["published"], false);
        assert!(!positive.to_string().contains("unused provider metadata"));
        // Numerical success and an exact quotation cannot validate the
        // educational prose. Keep even this apparently passing draft private.
        let mut misleading = good.clone();
        misleading["feedback"] = json!("Add the internal transfer to the combined inventory.");
        let misleading = generated(misleading);
        assert_eq!(misleading["passed"], true);
        assert_eq!(misleading["published"], false);
        assert!(misleading["review_question"].is_string());
        let mut repeated = good.clone();
        repeated["outside_in_kg"] = json!(2);
        repeated["combined_net_kg"] = json!(-6);
        let repeated = generated(repeated);
        assert_eq!(repeated["checks"]["new_combined_key"], true);
        assert_eq!(repeated["checks"]["distinguishes_recalled_results"], false);
        assert_eq!(repeated["passed"], false);
        let mut wrong = good;
        wrong["combined_net_kg"] = wrong["east_only_net_kg"].clone();
        let wrong = generated(wrong);
        assert_eq!(wrong["checks"]["new_combined_key"], false);
        assert_eq!(wrong["passed"], false);
    }

    #[test]
    fn malformed_or_wrong_model_responses_cannot_be_marked_as_successful() {
        for (case, response) in [
            (CASES[0], json!({"model":DEFAULT_JEV,"answers":{}})),
            (CASES[1], json!({"model":DEFAULT_LUNA,"answers":{}})),
            (
                CASES[2],
                generation_reply("broken JSON".into(), DEFAULT_LUNA),
            ),
            (
                CASES[2],
                generation_reply(fixture_candidate().to_string(), DEFAULT_JEV),
            ),
        ] {
            let report =
                run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |_, _| Ok(response.clone())).unwrap();
            assert_eq!(report["passed"], false);
            assert!(report["error"].is_string());
            if response["model"] == DEFAULT_JEV {
                assert!(report["assistant_text"].is_null());
            }
        }
        for case in CASES {
            assert!(run_case(case, DEFAULT_JEV, DEFAULT_LUNA, |_, _| Err(
                "stop budget".into()
            ))
            .is_err());
        }
    }
}
