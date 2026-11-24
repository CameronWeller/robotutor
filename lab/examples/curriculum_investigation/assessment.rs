//! Compare two unsigned writing rubrics against the same commissioned lesson.
use super::Course;
use serde_json::{json, Value};
use tutor_lab::curriculum::{
    creation::{Batch, Candidate, Error, ReviewerGrant},
    goal_rehearsal::{with_author_input, Options, View},
};
use tutor_lab::openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA};

const PRESENCE_RULE: &str = "Award full credit exactly when the response contains all four literal fragments: side door, Friday, 6 p.m., and reader. Ignore the order of the fragments and other text.";

// These judgments are recorded by the experiment's reviewer on fixed passages.
// They are response witnesses for inspecting a proposed rubric, not an NLP grader.
struct Witness {
    label: &'static str,
    answer: &'static str,
    visitor_entrance: bool,
    latest_arrival: bool,
    explanation_of_reader_need: bool,
    unsupported_instruction: Option<&'static str>,
    expected: bool,
}

const WITNESSES: [Witness; 7] = [
    Witness {
        label: "direct",
        answer: "Bring your lamp to the side door by 6 p.m. Friday. I named the door because a new reader may never have visited.",
        visitor_entrance: true,
        latest_arrival: true,
        explanation_of_reader_need: true,
        unsupported_instruction: None,
        expected: true
    },
    Witness {
        label: "different voice",
        answer: "Lamp in hand? Come to the side entrance no later than six on Friday evening. I replaced ‘usual’ because someone new will need to know which way in.",
        visitor_entrance: true,
        latest_arrival: true,
        explanation_of_reader_need: true,
        unsupported_instruction: None,
        expected: true
    },
    Witness {
        label: "wrong person",
        answer: "Visitors bringing lamps use the main entrance by 6 p.m. Friday. Volunteers use the side door. I named the entrance for the new reader.",
        visitor_entrance: false,
        latest_arrival: true,
        explanation_of_reader_need: true,
        unsupported_instruction: None,
        expected: false
    },
    Witness {
        label: "reversed deadline",
        answer: "Bring your lamp to the side door after 6 p.m. Friday. I spelled out the time because a new reader cannot guess the deadline.",
        visitor_entrance: true,
        latest_arrival: false,
        explanation_of_reader_need: true,
        unsupported_instruction: None,
        expected: false
    },
    Witness {
        label: "missing reason",
        answer: "Bring your lamp to the side door by 6 p.m. Friday. I liked how the word reader sounds.",
        visitor_entrance: true,
        latest_arrival: true,
        explanation_of_reader_need: false,
        unsupported_instruction: None,
        expected: false
    },
    Witness {
        label: "invented fee",
        answer: "Bring your lamp and the required ten-dollar fee to the side door by 6 p.m. Friday. I named the door because a new reader needs a way in.",
        visitor_entrance: true,
        latest_arrival: true,
        explanation_of_reader_need: true,
        unsupported_instruction: Some("the required ten-dollar fee"),
        expected: false
    },
    Witness {
        label: "greeting",
        answer: "Hello, neighbours! Bringing a lamp? Use the side door by 6 p.m. Friday. I named the entrance because a new reader needs to know where to go.",
        visitor_entrance: true,
        latest_arrival: true,
        explanation_of_reader_need: true,
        unsupported_instruction: None,
        expected: true
    },
];

fn contrasts() -> Result<(), String> {
    let mut errors = [0; 3];
    for w in &WITNESSES {
        let literal = ["side door", "Friday", "6 p.m.", "reader"]
            .iter()
            .all(|fragment| w.answer.contains(fragment));
        let reviewed = w.visitor_entrance && w.latest_arrival && w.explanation_of_reader_need;
        if let Some(quote) = w.unsupported_instruction {
            if w.answer.matches(quote).count() != 1 {
                return Err(format!("stale evidence for {}", w.label));
            }
        }
        let grounded = reviewed && w.unsupported_instruction.is_none();
        println!("{}: expected={} fragments={literal} entrance/time/rationale={reviewed} source-bounded={grounded}", w.label, w.expected);
        for (i, decision) in [literal, reviewed, grounded].into_iter().enumerate() {
            errors[i] += usize::from(decision != w.expected);
        }
    }
    println!("Errors on {} constructed responses: fragments={} three criteria={} with grounded additions={}",WITNESSES.len(),errors[0],errors[1],errors[2]);
    Ok(())
}

pub(super) fn examine(course: &Course) -> Result<(), String> {
    let first = course.first_unit()?;
    let record = &course.data["first_pack"];
    let options = Options {
        view: View::AuthorFirst,
        ..Options::default()
    };
    for (name, rubric) in [
        (
            "reviewed writing rubric",
            record["rubric"].as_str().ok_or("missing rubric")?,
        ),
        ("four literal fragments", PRESENCE_RULE),
    ] {
        let message = with_author_input(course.raw, &course.pin, &options, |input| {
            let mut body = json!({});
            for key in ["lesson", "worked", "practice", "check", "criterion"] {
                body[key] = record[key].clone();
            }
            body["rubric"] = Value::from(rubric);
            let pack = input.decode(&json!({"job":input.job(),"pack":body}))?;
            let pins = pack.sources.clone();
            let work = course.frame.commission(
                &course.plan,
                course.frame.review_digest(&course.plan),
                first.id,
                &pins,
            )?;
            let mut batch = Batch::new(work.release, [work]).map_err(|e| format!("{e:?}"))?;
            let digest = batch
                .stage(vec![Candidate {
                    work,
                    author: 1,
                    pack,
                }])
                .map_err(|e| format!("{e:?}"))?
                .pop()
                .ok_or("missing staged row")?
                .map_err(|e| format!("{e:?}"))?;
            let held = batch.available(work, &pins) == Err(Error::Unapproved);
            // A deliberate, synthetic rubber stamp in a fresh batch for each candidate.
            batch
                .approve(work, digest, &ReviewerGrant { actor: 2, work }, &pins)
                .map_err(|e| format!("{e:?}"))?;
            let readable = batch.available(work, &pins).is_ok();
            Ok(format!("{name}: decoded, {} bound source span(s), held before sign-off={held}, accessible after blanket sign-off={readable}",pins.len()))
        })?;
        println!("{message}");
    }
    contrasts()
}

/// Return both complete editorial request bodies. Replies are selected locally,
/// including a deliberately permissive Jev recommendation on an answer-reuse case.
pub(super) fn compare_checks(
    course: &Course,
    subject: &str,
    reuse: bool,
) -> Result<String, String> {
    use tutor_lab::curriculum::{author_input::ComparisonGrant, rehearsal_release::publish};
    let change = match subject {
        "writer" => "Require the writer to distinguish two actions according to a visible label. A single entrance and deadline addressed to one visitor must no longer solve the task.",
        "chemical-engineering" => "Require a combined boundary with an internal transfer, and select external totals so the previous tank's signed quantity cannot solve the new check.",
        _ => return Err("the comparison rehearsal has reviewed writing and engineering briefs".into()),
    };
    let prior = publish(
        &course.frame,
        &course.plan,
        course.frame.review_digest(&course.plan),
        &course.data["first_pack"],
    )?;
    let prior_pack = prior.material(true)?;
    let (catalog, prior_work) = prior.publication();
    let opts = Options {
        view: View::AuthorReturn,
        ..Options::default()
    };
    with_author_input(course.raw, &course.pin, &opts, |base| {
        let grant = ComparisonGrant {
            commission: base.digest(),
            prior_work,
            prior_pack: prior_pack.digest(),
            shared_criterion: course.data["criterion_claim"]
                .as_str()
                .ok_or("missing shared criterion")?,
            changed_demand: change,
            jev: DEFAULT_JEV,
            luna: DEFAULT_LUNA,
        };
        let revised = base.for_comparison(&grant, catalog, &prior_pack.sources)?;
        let mut candidate = json!({});
        let returning = &course.data["return_pack"];
        for key in [
            "lesson",
            "worked",
            "practice",
            "check",
            "criterion",
            "rubric",
        ] {
            candidate[key] = returning[key].clone();
        }
        if reuse {
            if subject == "writer" {
                candidate["check"] = json!("A lamp visitor is new to the repair café. Volunteers use the main door. Visitors bringing lamps use the side door by six on Friday evening. Rewrite 'Use our normal entrance before closing' for that visitor and explain one revision.");
                candidate["criterion"] = json!("Tell a newcomer where and by when to act. Explain how one revision helps them.");
                candidate["rubric"] = json!("Accept the side entrance and latest time of six on Friday plus a reason tied to the visitor's need, in any voice.");
            } else {
                candidate["check"] = json!("Two basins share one outer boundary. A delivery brings in 18 kg. A pump moves 11 kg between the basins, and 8 kg leave through the drain. What signed quantity in kg was added to the combined inventory?");
                candidate["rubric"] = json!("Accept a signed change of +10 kg. The pump moves water inside the outer boundary. Score the quantity only.");
            }
        }
        let (original_requests, _) = capture_editorial(base, &candidate)?;
        let (requests, report) = capture_editorial(&revised, &candidate)?;
        let pack = revised.decode(&report["candidate"])?;
        let pins = pack.sources.clone();
        let unit = returning["unit"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("invalid transfer unit")?;
        let work = course.frame.commission(
            &course.plan,
            course.frame.review_digest(&course.plan),
            unit,
            &pins,
        )?;
        let mut pending = Batch::new(work.release, [work]).map_err(|e| format!("{e:?}"))?;
        pending
            .stage(vec![Candidate {
                work,
                author: 1,
                pack,
            }])
            .map_err(|e| format!("{e:?}"))?
            .remove(0)
            .map_err(|e| format!("{e:?}"))?;
        let read = pending
            .available(work, &pins)
            .err()
            .ok_or("an unsigned draft became learner-readable")?;
        // The editor solves the public tank question independently, using its
        // posted readings. No learner submission is passed to either model.
        let quantity_control = if subject == "chemical-engineering" {
            let target = if reuse {
                18 - 8
            } else {
                returning["quantity"]["value"]
                    .as_i64()
                    .ok_or("missing reviewed quantity")?
            };
            let response = format!("{} kg", (4 - 2) * 5);
            Some(format!(
                "{:?}",
                tutor_lab::curriculum::goal_evidence::check_quantity_text(target, "kg", &response)
            ))
        } else {
            None
        };
        Ok(
            json!({"original_requests":original_requests, "comparison_requests":requests,
                "disposition_from_offline_reply": report["advisory"]["disposition"],
                "editor_report_status":report["status"],
                "learner_read":format!("{read:?}"),
                "public_tank_answer_on_candidate":quantity_control,
            })
            .to_string(),
        )
    })
}

fn capture_editorial(
    input: &tutor_lab::curriculum::author_input::AuthorInput,
    candidate: &Value,
) -> Result<(Value, Value), String> {
    let mut requests = json!({});
    let reply = json!({"job":input.job(), "pack":candidate});
    let report = tutor_lab::curriculum::live_author::draft(
        input,
        DEFAULT_JEV,
        DEFAULT_LUNA,
        |role, request| match role {
            Role::Luna => {
                requests["luna"] = request.clone();
                Ok(
                    json!({"model":DEFAULT_LUNA,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":reply.to_string()}}]}),
                )
            }
            Role::Jev => {
                requests["jev"] = request.clone();
                let key = request["questions"]
                    .as_object()
                    .and_then(|v| v.keys().next())
                    .ok_or("missing editorial decision")?;
                Ok(
                    json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":"review"}}}),
                )
            }
        },
    )?;
    Ok((requests, report))
}
