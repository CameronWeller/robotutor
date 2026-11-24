use serde_json::Value;
use tutor_lab::curriculum::goal_rehearsal::{run, Branch, Options, View};

const CASES: [(&str, &str); 5] = [
    (
        "chemical-engineering",
        include_str!("../fixtures/goals/chemical-engineering.json"),
    ),
    ("violin", include_str!("../fixtures/goals/violin.json")),
    (
        "better-person",
        include_str!("../fixtures/goals/better-person.json"),
    ),
    ("worship", include_str!("../fixtures/goals/worship.json")),
    ("writer", include_str!("../fixtures/goals/writer.json")),
];

fn cases(mut check: impl FnMut(&str, &str, &str, &Value)) {
    let reviews: Value =
        serde_json::from_str(include_str!("../fixtures/goals/reviews.json")).unwrap();
    for (slug, raw) in CASES {
        let data: Value = serde_json::from_str(raw).unwrap();
        let expected = reviews[slug]
            .as_str()
            .expect("independently recorded fixture pin");
        check(slug, raw, expected, &data);
    }
}

fn passage<'a>(record: &'a Value, field: &str) -> &'a str {
    record[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing text field {field}"))
}

fn learner() -> Options {
    Options {
        view: View::Learner,
        ..Options::default()
    }
}

#[test]
fn every_learner_reads_a_guided_exchange_before_independent_and_delayed_feedback() {
    cases(|slug, raw, pin, data| {
        let transcript = run(raw, pin, &learner()).unwrap();
        let first = &data["first_pack"];
        let transfer = &data["return_pack"];
        let mut remaining = transcript.as_str();
        for step in [
            passage(first, "lesson"),
            passage(first, "worked"),
            passage(first, "practice"),
            passage(first, "practice_response"),
            passage(first, "practice_feedback"),
            passage(first, "check"),
            passage(&first["attempts"]["demonstrated"], "response"),
            passage(&first["attempts"]["demonstrated"], "feedback"),
            passage(transfer, "check"),
            passage(&transfer["attempts"]["demonstrated"], "response"),
            passage(&transfer["attempts"]["demonstrated"], "feedback"),
        ] {
            let (_, tail) = remaining
                .split_once(step)
                .unwrap_or_else(|| panic!("{slug} lost or reordered encounter text: {step}"));
            remaining = tail;
        }
        let before_return = transcript
            .split_once(passage(&first["attempts"]["demonstrated"], "feedback"))
            .unwrap()
            .1
            .split_once(passage(transfer, "check"))
            .unwrap()
            .0;
        assert_eq!(
            before_return.contains(passage(transfer, "excerpt")),
            transfer["show_source_during_check"].as_bool().unwrap(),
            "{slug} ignored the return task's source display conditions"
        );
        for record in [first, transfer] {
            assert!(
                !transcript.contains(passage(record, "rubric")),
                "{slug} exposed private rubric"
            );
            for branch in ["revisit", "unobserved"] {
                assert!(
                    !transcript.contains(passage(&record["attempts"][branch], "feedback")),
                    "{slug} exposed feedback for another path"
                );
            }
        }
        for (other_slug, other_raw) in CASES {
            if slug != other_slug {
                let other: Value = serde_json::from_str(other_raw).unwrap();
                assert!(
                    !transcript.contains(passage(&other["return_pack"], "check")),
                    "{slug} leaked a different lesson"
                );
            }
        }
        let audit = run(raw, pin, &Options::default()).unwrap();
        assert_ne!(
            audit, transcript,
            "{slug} used the same audience projection"
        );
    });
}

#[test]
fn private_or_unmet_first_checks_end_the_current_path_and_returns_have_their_own_feedback() {
    cases(|slug, raw, pin, data| {
        let first = &data["first_pack"];
        let transfer = &data["return_pack"];
        for (branch, label) in [
            (Branch::Unobserved, "unobserved"),
            (Branch::Revisit, "revisit"),
        ] {
            let transcript = run(
                raw,
                pin,
                &Options {
                    first: branch,
                    ..learner()
                },
            )
            .unwrap();
            let first_reply = &first["attempts"][label];
            assert!(
                transcript.contains(passage(first_reply, "response")),
                "{slug} {label}"
            );
            assert!(
                transcript.contains(passage(first_reply, "feedback")),
                "{slug} {label}"
            );
            assert!(
                !transcript.contains(passage(transfer, "check")),
                "{slug} released a premature return"
            );
        }
        for (branch, label) in [
            (Branch::Unobserved, "unobserved"),
            (Branch::Revisit, "revisit"),
        ] {
            let transcript = run(
                raw,
                pin,
                &Options {
                    returning: branch,
                    ..learner()
                },
            )
            .unwrap();
            let turn = &transfer["attempts"][label];
            assert!(
                transcript.contains(passage(transfer, "check")),
                "{slug} {label}"
            );
            assert!(
                transcript.contains(passage(turn, "response")),
                "{slug} {label}"
            );
            assert!(
                transcript.contains(passage(turn, "feedback")),
                "{slug} {label}"
            );
            assert!(
                !transcript.contains(passage(&transfer["attempts"]["demonstrated"], "feedback")),
                "{slug} implied a successful return"
            );
        }
    });
}

#[test]
fn withdrawal_setting_change_and_missing_confirmation_withhold_later_material() {
    cases(|slug, raw, pin, data| {
        let first = &data["first_pack"];
        let transfer = &data["return_pack"];
        let withdrawn = Options {
            withdraw_return: true,
            ..learner()
        };
        let transcript = run(raw, pin, &withdrawn).unwrap();
        assert!(
            transcript.contains(passage(&first["attempts"]["demonstrated"], "feedback")),
            "{slug}"
        );
        assert!(
            !transcript.contains(passage(transfer, "check")),
            "{slug} exposed a withdrawn check"
        );
        assert!(
            !transcript.contains(passage(&transfer["attempts"]["demonstrated"], "feedback")),
            "{slug}"
        );
        for opts in [
            Options {
                view: View::Intake,
                ..learner()
            },
            Options {
                changed_setting: Some("A new learning setting".into()),
                ..learner()
            },
        ] {
            let transcript = run(raw, pin, &opts).unwrap();
            assert!(
                !transcript.contains(passage(first, "lesson")),
                "{slug} served unconfirmed material"
            );
            assert!(!transcript.contains(passage(transfer, "check")), "{slug}");
        }
    });
}

#[test]
fn early_or_recently_rehearsed_success_has_a_shorter_gap_than_an_unassisted_return() {
    cases(|slug, raw, pin, data| {
        let check = passage(&data["return_pack"], "check");
        let success = passage(&data["return_pack"]["attempts"]["demonstrated"], "feedback");
        for options in [
            Options {
                return_day: 3,
                ..Options::default()
            },
            Options {
                practice_day: Some(27),
                ..Options::default()
            },
        ] {
            let audit = run(raw, pin, &options).unwrap();
            assert!(
                audit.contains(check) && audit.contains(success),
                "{slug} withheld an available early lesson"
            );
            assert!(
                audit.contains("InsufficientSpacing"),
                "{slug} lost the reason to defer durable evidence"
            );
            assert!(
                !audit.contains("SupportsDurability"),
                "{slug} awarded durable evidence after recent exposure"
            );
        }
    });
}

#[test]
fn supplied_responses_use_the_actual_evidence_channel_and_never_inherit_fixture_reviews() {
    cases(|slug, raw, pin, data| {
        let first = &data["first_pack"];
        let demonstrated = &first["attempts"]["demonstrated"];
        let transfer = passage(&data["return_pack"], "check");
        let numeric = data["evidence_mode"] == "quantity";
        // The same words as the script arrive from a separate, unauthenticated response.
        let options = Options {
            first_response: Some(passage(demonstrated, "response").to_owned()),
            ..learner()
        };
        let transcript = run(raw, pin, &options).unwrap();
        assert_eq!(
            transcript.contains(transfer),
            numeric,
            "{slug} assessed through the wrong channel"
        );
        assert!(
            !transcript.contains(passage(demonstrated, "feedback")),
            "{slug} reused the scripted review"
        );
        if numeric {
            let wrong = Options {
                first_response: Some("0 kg".into()),
                ..learner()
            };
            let transcript = run(raw, pin, &wrong).unwrap();
            assert!(
                !transcript.contains(transfer),
                "{slug} ignored the numeric response"
            );
        }
    });
}

#[test]
fn author_views_disclose_only_their_commission_and_the_fixture_pin_covers_the_exact_bytes() {
    cases(|slug, raw, pin, data| {
        for (view, key) in [
            (View::AuthorFirst, "first_pack"),
            (View::AuthorReturn, "return_pack"),
        ] {
            let body = run(raw, pin, &Options { view, ..learner() }).unwrap();
            let wire: Value = serde_json::from_str(&body).unwrap();
            let brief: Value =
                serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
            let record = &data[key];
            assert_eq!(brief["objective"]["id"], record["unit"], "{slug} {key}");
            assert_eq!(
                brief["excerpts"][0]["text"], record["excerpt"],
                "{slug} {key}"
            );
            assert_eq!(brief["excerpts"].as_array().unwrap().len(), 1);
            assert!(
                !body.contains(passage(record, "rubric")),
                "{slug} leaked an answer to author"
            );
            assert!(brief.get("attempts").is_none());
        }
        let tampered = raw.replacen(
            passage(&data["return_pack"]["attempts"]["demonstrated"], "feedback"),
            "Altered result in the fixture",
            1,
        );
        assert_ne!(
            tampered, raw,
            "{slug} failed to construct an edited fixture"
        );
        assert!(
            run(&tampered, pin, &learner()).is_err(),
            "{slug} accepted altered outcome"
        );
        assert!(
            run(&format!("{raw}\n"), pin, &learner()).is_err(),
            "{slug} accepted changed raw bytes"
        );
    });
}

#[test]
fn preview_stops_before_the_answer_and_unknown_practice_limits_the_delayed_claim() {
    cases(|slug, raw, pin, data| {
        let preview = run(
            raw,
            pin,
            &Options {
                view: View::Opening,
                ..Options::default()
            },
        )
        .unwrap();
        assert!(
            preview.ends_with(passage(&data["first_pack"], "check")),
            "{slug}"
        );
        let complete = run(raw, pin, &learner()).unwrap();
        assert!(complete.starts_with(&preview), "{slug}");
        let unknown = run(
            raw,
            pin,
            &Options {
                unknown_history: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert!(
            unknown.contains(passage(&data["return_pack"], "check")),
            "{slug}"
        );
        assert!(unknown.contains("UnknownExposure"), "{slug}");
        assert!(!unknown.contains("SupportsDurability"), "{slug}");
    });
}
