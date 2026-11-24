use serde_json::{json, Value};
use tutor_lab::{
    curriculum::{
        goal_rehearsal::{run, with_author_input, Options, View},
        live_author::draft,
    },
    openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA},
};

const WRITER: &str = include_str!("../fixtures/goals/writer.json");

fn pin() -> String {
    let pins: Value = serde_json::from_str(include_str!("../fixtures/goals/reviews.json")).unwrap();
    pins["writer"].as_str().unwrap().into()
}

fn unit(id: u32) -> Options {
    Options {
        view: View::AuthorUnit,
        unit: Some(id),
        ..Options::default()
    }
}

fn authored(wire: &Value) -> Value {
    let brief: Value =
        serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
    json!({"job":brief["job"],"pack":{
        "lesson":"List what the reporter saw and heard. Mark any statement about a person's feelings as an inference.",
        "worked":"The clock chimed twice. Saying that its sound made the clock mender happy would require evidence.",
        "practice":"Underline two details the visitor could record at the bench.",
        "check":"Use one visual observation and one sound in a short profile. Give a possible interpretation that needs checking.",
        "criterion":"Ground the profile in two observations and identify an unsupported inference.",
        "rubric":"Locate each observed detail in the notes, then check that the writer distinguishes interpretation from observation."
    }})
}

fn chat(body: &Value) -> Value {
    json!({"model":DEFAULT_LUNA,"usage":{"prompt_tokens":65,"completion_tokens":80},"id":"synthetic-draft",
        "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":body.to_string()}}]})
}

fn attempt(
    opts: &Options,
    mut send: impl FnMut(Role, &Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let output = with_author_input(WRITER, &pin(), opts, |input| {
        draft(input, DEFAULT_JEV, DEFAULT_LUNA, &mut send).map(|v| v.to_string())
    })?;
    serde_json::from_str(&output).map_err(|e| e.to_string())
}

#[test]
fn an_unwritten_lesson_can_be_drafted_and_triaged_without_becoming_a_lesson() {
    let f: Value = serde_json::from_str(WRITER).unwrap();
    let source = &f["authoring_sources"]["102"];
    for disposition in ["held", "revise", "review"] {
        let mut calls = Vec::new();
        let report=attempt(&unit(102),|role,wire| {
            calls.push(role);
            assert!(!wire.to_string().contains(f["first_pack"]["rubric"].as_str().unwrap()));
            assert!(!wire.to_string().contains(f["scope_reply"].as_str().unwrap()));
            match role {
                Role::Luna => {
                    let brief:Value=serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
                    assert_eq!(brief["excerpts"],json!([{"cite":"S1","text":source["excerpt"]}]));
                    assert_eq!(wire["reasoning"],json!({"effort":"low","exclude":true}));
                    Ok(chat(&authored(wire)))
                }
                Role::Jev => {
                    assert!(wire.get("messages").is_none());
                    assert_eq!(wire["state"]["commission"]["objective"]["id"],102);
                    assert_eq!(wire["state"]["commission"]["excerpts"][0]["text"],source["excerpt"]);
                    assert_eq!(wire["state"]["candidate"].as_object().unwrap().len(),6);
                    let questions=wire["questions"].as_object().unwrap();
                    assert_eq!(questions.len(),1);
                    let key=questions.keys().next().unwrap();
                    assert!(key.len()<=48);
                    Ok(json!({"model":DEFAULT_JEV,"id":"synthetic-advice","usage":{"input_tokens":70},
                        "answers":{(key):{"type":"choice","choice":disposition}}}))
                }
            }
        }).unwrap();
        assert_eq!(calls, [Role::Luna, Role::Jev]);
        assert_eq!(report["status"], "unpublished");
        assert_eq!(report["candidate"]["job"], report["job"]);
        assert_eq!(report["advisory"]["disposition"], disposition);
        assert_eq!(report["author"]["reported_usage"]["completion_tokens"], 80);
        assert_eq!(report["author"]["response_id"], "synthetic-draft");
        assert_eq!(report["advisory"]["reported_usage"]["input_tokens"], 70);
        assert_eq!(report["advisory"]["response_id"], "synthetic-advice");
    }
    let still_unpublished = run(
        WRITER,
        &pin(),
        &Options {
            view: View::Learner,
            ..unit(102)
        },
    )
    .unwrap();
    assert!(still_unpublished.contains("independent subject review"));
}

#[test]
fn missing_rights_or_a_stale_or_unsupported_package_stops_before_the_next_paid_call() {
    for opts in [
        unit(103),
        unit(999),
        Options {
            view: View::AuthorReturn,
            withdraw_return: true,
            ..Options::default()
        },
        Options {
            changed_setting: Some("A separate scope".into()),
            ..unit(102)
        },
    ] {
        assert!(attempt(&opts, |_, _| panic!(
            "cannot send a job with no authorized source"
        ))
        .is_err());
    }
    for flaw in ["crossed", "abstained", "exposed"] {
        let mut calls = 0;
        let result = attempt(&unit(102), |role, wire| {
            assert_eq!(role, Role::Luna);
            calls += 1;
            let mut draft = authored(wire);
            match flaw {
                "crossed" => draft["job"] = json!("a separate job"),
                "abstained" => draft["pack"] = Value::Null,
                _ => draft["pack"]["practice"] = draft["pack"]["check"].clone(),
            }
            Ok(chat(&draft))
        });
        assert!(result.is_err() && calls == 1);
    }
}

#[test]
fn a_failed_or_crossed_advisory_preserves_only_an_unpublished_candidate() {
    for stale in [false, true] {
        let report = attempt(&unit(102), |role, wire| match role {
            Role::Luna => Ok(chat(&authored(wire))),
            Role::Jev if stale => Ok(json!({"model":DEFAULT_JEV,"id":"stale-reply",
                "answers":{"draft_for_another_job":{"type":"choice","choice":"review"}}})),
            Role::Jev => Err("decision service unavailable".into()),
        })
        .unwrap();
        assert_eq!(report["status"], "unpublished");
        assert_eq!(report["advisory"]["disposition"], "held");
        assert!(report["advisory"]["error"].is_string());
        assert!(report["candidate"]["pack"]["rubric"].is_string());
        assert!(report["advisory"].get("reported_usage").is_none());
    }
}

#[test]
fn comparing_checks_requires_the_exact_publication_commission_and_recipient() {
    use tutor_lab::curriculum::{
        author_input::ComparisonGrant,
        program::{Frame, Proposal},
        rehearsal_release::publish,
    };
    let f: Value = serde_json::from_str(WRITER).unwrap();
    let frame = Frame::from_json(&f).unwrap();
    let plan = Proposal::from_json(&f["proposal"]).unwrap();
    let release = publish(&frame, &plan, frame.review_digest(&plan), &f["first_pack"]).unwrap();
    let prior = release.material(true).unwrap();
    let (catalog, prior_work) = release.publication();
    let opts = Options {
        view: View::AuthorReturn,
        ..Options::default()
    };
    with_author_input(WRITER, &pin(), &opts, |input| {
        let mut grant = ComparisonGrant {
            commission: input.digest(),
            prior_work,
            prior_pack: prior.digest(),
            shared_criterion: f["criterion_claim"].as_str().unwrap(),
            changed_demand: "Choose between two actions using an observable label.",
            jev: DEFAULT_JEV,
            luna: DEFAULT_LUNA,
        };
        let attach = |g: &ComparisonGrant<'_>| input.for_comparison(g, catalog, &prior.sources);
        let compared = attach(&grant)?;
        assert_ne!(compared.job(), input.job());
        assert_eq!(compared.job(), attach(&grant)?.job());
        assert!(compared
            .decode(&json!({"job":input.job(), "pack":null}))
            .unwrap_err()
            .contains("does not match"));
        assert!(draft(&compared, DEFAULT_JEV, "other/editor", |_, _| panic!(
            "cannot dispatch"
        ))
        .is_err());
        assert!(input
            .for_comparison(&grant, catalog, &Default::default())
            .is_err());
        grant.changed_demand = "Use a new route that requires two decisions.";
        assert_ne!(compared.job(), attach(&grant)?.job());
        grant.prior_pack = [0; 32];
        assert!(attach(&grant).err().unwrap().contains("prior edition"));
        grant.prior_pack = prior.digest();
        grant.commission = [0; 32];
        assert!(attach(&grant).err().unwrap().contains("commission"));
        Ok(String::new())
    })
    .unwrap();
}
