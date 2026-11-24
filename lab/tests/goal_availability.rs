use serde_json::{json, Value};
use tutor_lab::{
    curriculum::{
        availability::orientation,
        goal_rehearsal::{
            interactive_opening, opening_check, opening_feedback, run, Options, View,
        },
    },
    material::document_id,
};

const WRITER: &str = include_str!("../fixtures/goals/writer.json");
const CHEM: &str = include_str!("../fixtures/goals/chemical-engineering.json");

fn pin(slug: &str) -> String {
    let reviews: Value =
        serde_json::from_str(include_str!("../fixtures/goals/reviews.json")).unwrap();
    reviews[slug].as_str().unwrap().into()
}

fn select(id: u32) -> Options {
    Options {
        unit: Some(id),
        view: View::Learner,
        ..Options::default()
    }
}

fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    row[name].as_str().unwrap()
}

#[test]
fn a_released_guided_opening_excludes_test_answers_and_requires_actual_parent_evidence() {
    for (slug, raw) in [("writer", WRITER), ("chemical-engineering", CHEM)] {
        let f: Value = serde_json::from_str(raw).unwrap();
        let first = &f["first_pack"];
        let transfer = &f["return_pack"];
        let pin = pin(slug);
        let lesson = run(raw, &pin, &select(101)).unwrap();
        for key in ["lesson", "worked", "practice", "excerpt", "locator"] {
            assert!(lesson.contains(field(first, key)));
        }
        for key in ["check", "rubric", "practice_response", "practice_feedback"] {
            assert!(!lesson.contains(field(first, key)));
        }
        let planned = run(raw, &pin, &select(201)).unwrap();
        assert!(planned.contains("prerequisite") && !planned.contains(field(transfer, "lesson")));
        let answered = Options {
            first_response: Some(field(&first["attempts"]["demonstrated"], "response").into()),
            ..select(201)
        };
        let opening = run(raw, &pin, &answered).unwrap();
        assert_eq!(
            opening.contains(field(transfer, "lesson")),
            slug == "chemical-engineering"
        );
        assert!(!opening.contains(field(transfer, "check")));
        let withdrawn = Options {
            withdraw_return: true,
            ..answered
        };
        let hold = run(raw, &pin, &withdrawn).unwrap();
        assert!(hold.contains("withdrawn") && !hold.contains(field(transfer, "excerpt")));
        let author = Options {
            view: View::AuthorUnit,
            ..withdrawn
        };
        assert!(run(raw, &pin, &author).is_err());
    }
}

#[test]
fn a_scoped_draft_brief_does_not_promote_an_unwritten_lesson() {
    let f: Value = serde_json::from_str(WRITER).unwrap();
    let pin = pin("writer");
    let source = &f["authoring_sources"]["102"];
    let learner = run(WRITER, &pin, &select(102)).unwrap();
    assert!(learner.contains("A draft and independent subject review are still needed"));
    assert!(!learner.contains(field(source, "excerpt")));
    let wire = run(
        WRITER,
        &pin,
        &Options {
            view: View::AuthorUnit,
            ..select(102)
        },
    )
    .unwrap();
    let wire: Value = serde_json::from_str(&wire).unwrap();
    let brief: Value =
        serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(brief["objective"]["id"], 102);
    assert_eq!(
        brief["excerpts"],
        json!([{"cite":"S1","text":source["excerpt"]}])
    );
    assert!(!brief
        .to_string()
        .contains(field(&f["first_pack"], "rubric")));
    let unit = &f["proposal"]["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == 104)
        .unwrap();
    let absent = run(WRITER, &pin, &select(104)).unwrap();
    assert!(absent.contains(field(unit, "performance")) && absent.contains("authorize a passage"));
    assert!(absent.contains("102") && absent.contains("private"));
    assert!(run(
        WRITER,
        &pin,
        &Options {
            view: View::AuthorUnit,
            ..select(104)
        }
    )
    .is_err());
}

#[test]
fn a_missing_outline_or_unlisted_objective_has_a_private_self_work_path() {
    let unknown = run(WRITER, &pin("writer"), &select(999)).unwrap();
    assert!(unknown.contains("outside this outline") && unknown.contains(orientation()));
    let rec = json!({"scope":{"goal":"I want to study a new subject."}}).to_string();
    let expected: String = document_id(rec.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let empty = run(&rec, &expected, &select(3)).unwrap();
    assert!(empty.contains("no reviewed outline") && empty.contains(orientation()));
    assert!(run(
        &rec,
        &expected,
        &Options {
            view: View::AuthorUnit,
            ..select(3)
        }
    )
    .is_err());
    assert!(opening_check(&rec, &expected, &Options::default()).is_err());
    let changed = run(
        WRITER,
        &pin("writer"),
        &Options {
            changed_setting: Some("A different intended audience".into()),
            ..select(101)
        },
    )
    .unwrap();
    assert!(changed.contains(orientation()));
    assert!(run(WRITER, &expected, &select(101)).is_err());
}

#[test]
fn a_resumed_question_and_its_response_have_no_scripted_or_future_turns() {
    for (slug, raw) in [("writer", WRITER), ("chemical-engineering", CHEM)] {
        let pin = pin(slug);
        let f: Value = serde_json::from_str(raw).unwrap();
        let first = &f["first_pack"];
        let opts = Options::default();
        assert_eq!(
            opening_check(raw, &pin, &opts).unwrap(),
            field(first, "check")
        );
        let answer = field(&first["attempts"]["demonstrated"], "response");
        let reply = opening_feedback(raw, &pin, &opts, answer, false).unwrap();
        assert!(!reply.contains(field(&first["attempts"]["demonstrated"], "feedback")));
        assert!(!reply.contains(field(&f["return_pack"], "check")));
        assert_eq!(
            reply.contains("number and unit match"),
            slug == "chemical-engineering"
        );
        let guided = opening_feedback(raw, &pin, &opts, answer, true).unwrap();
        assert!(guided.starts_with("Guided practice") && !guided.contains("SupportsDurability"));
        assert!(opening_check(raw, &pin, &select(201)).is_err());
        assert!(opening_feedback(raw, &pin, &select(201), answer, false).is_err());
        let changed = Options {
            changed_setting: Some("A different audience".into()),
            ..opts
        };
        assert!(opening_check(raw, &pin, &changed).is_err());
        assert!(opening_feedback(raw, &pin, &changed, answer, false).is_err());
    }
}

/// The test host fixes an independent scenario snapshot before each read. Changing the
/// records models separate catalog states; no rehearsal promotes or mutates the fixture.
fn snapshot(data: &Value) -> (String, String) {
    let raw = data.to_string();
    let pin = document_id(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (raw, pin)
}

#[test]
fn ordinary_and_interactive_openings_are_usable_with_zero_or_one_published_pack() {
    let mut data: Value = serde_json::from_str(CHEM).unwrap();
    let first = data["first_pack"].clone();
    let transfer = data["return_pack"].take();
    let (raw, pin) = snapshot(&data);
    let options = Options::default();
    let (preview, awaiting) = interactive_opening(&raw, &pin, &options).unwrap();
    assert!(awaiting && preview.ends_with(field(&first, "check")));
    assert_eq!(
        opening_check(&raw, &pin, &options).unwrap(),
        field(&first, "check")
    );
    let result = opening_feedback(&raw, &pin, &options, "10 kg", false).unwrap();
    assert!(result.contains("number and unit match"));
    for view in [View::Learner, View::Audit] {
        let output = run(
            &raw,
            &pin,
            &Options {
                view,
                ..Options::default()
            },
        )
        .unwrap();
        assert!(output.contains(field(&first["attempts"]["demonstrated"], "feedback")));
        assert!(
            output.contains("A fresh return exercise")
                && !output.contains(field(&transfer, "check"))
        );
        assert!(!output.contains("SupportsDurability"));
    }
    assert!(run(
        &raw,
        &pin,
        &Options {
            view: View::AuthorReturn,
            ..Options::default()
        }
    )
    .is_err());
    data["first_pack"] = Value::Null;
    let (raw, pin) = snapshot(&data);
    for view in [View::Opening, View::Learner, View::Audit] {
        let text = run(
            &raw,
            &pin,
            &Options {
                view,
                ..Options::default()
            },
        )
        .unwrap();
        assert!(text.contains("authorize a passage") && text.contains("private"));
        assert!(!text.contains(field(&first, "check")));
    }
    let (offer, awaiting) = interactive_opening(&raw, &pin, &options).unwrap();
    assert!(!awaiting && offer.contains("authorize a passage"));
    assert!(opening_check(&raw, &pin, &options).is_err());
    assert!(opening_feedback(&raw, &pin, &options, "10 kg", false).is_err());
}

#[test]
fn interactive_status_is_distinct_from_an_actual_check_for_every_preview() {
    let pin = pin("writer");
    for id in [101, 102, 201, 999] {
        let (offer, awaiting) = interactive_opening(WRITER, &pin, &select(id)).unwrap();
        assert!(!awaiting && !offer.is_empty());
    }
    let (raw, expected) = snapshot(&json!({"scope":{"goal":"My new subject"}}));
    let (offer, awaiting) = interactive_opening(&raw, &expected, &Options::default()).unwrap();
    assert!(!awaiting && offer.contains(orientation()));
    let changed = Options {
        changed_setting: Some("A different scope".into()),
        ..Options::default()
    };
    let (offer, awaiting) = interactive_opening(WRITER, &pin, &changed).unwrap();
    assert!(!awaiting && offer.contains(orientation()));
}

#[test]
fn a_third_published_unit_opens_its_own_guided_material_by_id() {
    let mut data: Value = serde_json::from_str(WRITER).unwrap();
    let source = &data["authoring_sources"]["102"];
    let record = json!({
        "unit":102, "source_url":source["source_url"], "locator":source["locator"], "excerpt":source["excerpt"],
        "lesson":"Mark the difference between the reporter's observations and the mender's possible feelings.",
        "worked":"The reporter heard two chimes. Saying that the mender felt proud would require evidence about him.",
        "practice":"Write a caption with one thing the reporter could see and one thing she could hear.",
        "check":"Draft a profile lead using two observations from the bench. Identify an inference that needs a follow-up question.",
        "criterion":"Separate observable detail from interpretation in the profile lead.",
        "rubric":"An editor checks the two chosen observations against the reporting notes and locates the proposed inference."
    });
    data["published_packs"] = json!([record]);
    let (raw, pin) = snapshot(&data);
    let (lesson, independent) = interactive_opening(&raw, &pin, &select(102)).unwrap();
    assert!(!independent);
    for key in ["lesson", "worked", "practice", "excerpt", "locator"] {
        assert!(lesson.contains(field(&record, key)), "missing {key}");
    }
    for key in ["check", "rubric", "criterion"] {
        assert!(!lesson.contains(field(&record, key)), "exposed {key}");
    }
    assert!(!lesson.contains(field(&data["first_pack"], "practice")));
    assert!(!lesson.contains(field(&data["return_pack"], "practice")));
    // Each lookup validates catalog identities, even when the requested opening itself is sound.
    for conflicting in [102, 101, 201, 999] {
        let mut bad = record.clone();
        bad["unit"] = json!(conflicting);
        data["published_packs"] = json!([record, bad]);
        let (raw, pin) = snapshot(&data);
        assert!(
            run(&raw, &pin, &select(101)).is_err(),
            "ignored conflicting or unknown {conflicting}"
        );
    }
    let mut broken = record.clone();
    broken["rubric"] = Value::Null;
    data["published_packs"] = json!([broken]);
    let (raw, pin) = snapshot(&data);
    assert!(run(&raw, &pin, &select(102)).is_err());
}
