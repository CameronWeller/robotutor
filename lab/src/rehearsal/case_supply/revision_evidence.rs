//! Continue a publisher-authorized hall case with private, simulated subject reviews.
use super::Commission;
use crate::{
    curriculum::{
        creation::Pack,
        goal_evidence::{
            delayed, Evidence, Observation, Presentation, Review, ReviewedWriting, Spacing,
            Submission, Task,
        },
        OutcomeId,
    },
    history::{Scope, Withdrawn},
    interpretation::Finding,
    protocol::{Assistance, Judgment},
    rehearsal::goal_choice::published_psalm_lesson,
    retention::{Cell, CheckResult, Moment},
};
use serde_json::Value;

const CELL: Cell = Cell {
    scope: Scope {
        learner: 42,
        revision: 1,
    },
    outcome: OutcomeId(101),
};
const PETITION: &str = "Stop the rain so we can sing outdoors.";
const REVISION:&str="Thank you for the dry hall we have been given. ‘Give thanks’ led the singer to name the shelter already received.";
const NEXT:&str="Thank you for the lamp that showed the way home. I used ‘Give thanks’ to mark what the worker received.";

fn at(day: u32, seq: u64, repeated: bool) -> Presentation {
    Presentation {
        attempt: seq,
        at: Moment { day, sequence: seq },
        fresh: !repeated,
        assistance: if repeated {
            Assistance::Reported
        } else {
            Assistance::NoneReported
        },
    }
}

fn approved_task<'a>(data: &'a Value, row: &'a Value, pack: &Pack) -> Task<'a> {
    Task::from_pack(
        CELL,
        pack.digest(),
        "written_artifact",
        data["criterion_claim"].as_str().unwrap(),
        row,
    )
    .unwrap()
    .with_published_sources(pack)
    .unwrap()
}

fn signed<'a>(
    task: &Task<'_>,
    when: Presentation,
    prose: &'a str,
    finding: Finding,
    quote: &str,
) -> (Submission<'a>, Review, Evidence) {
    let captured = task.capture(when, Observation::Written(prose));
    let review = task.attest(when, &captured, 22, finding, quote).unwrap();
    let evidence = task.assess(when, &captured, Some(&review));
    (captured, review, evidence)
}

fn delayed_result(before: &Evidence, after: &Evidence, w: &Withdrawn) -> CheckResult {
    delayed(
        before,
        after,
        Some(&[]),
        w,
        Spacing {
            stability_days: 3.0,
            threshold: 0.85,
            target_day: None,
        },
    )
    .unwrap()
}

#[test]
fn a_reviewed_change_survives_shared_unit_numbers_but_not_an_unreviewed_edit() {
    use crate::curriculum::goal_evidence::delayed_on_reviewed_change;
    use crate::retention::Exposure;
    use std::collections::BTreeSet;

    let source = published_psalm_lesson().unwrap();
    let lamp_pack = source.material(true).unwrap();
    let mut order = Commission::default();
    order.request();
    order.review(lamp_pack, "hall").unwrap();
    let release = order.reviewed.as_ref().unwrap();
    let hall_pack = release
        .batch
        .available(release.work, &lamp_pack.sources)
        .unwrap();
    let data: Value =
        serde_json::from_str(include_str!("../../../fixtures/goals/worship.json")).unwrap();
    let row = &data["first_pack"];

    // Construct a recorded performance from the pack actually examined. The
    // scripted review judges a specific piece of writing in this comparison.
    let examine = |pack: &Pack,
                   when: Presentation,
                   line: &str,
                   quote: &str,
                   published: bool,
                   finding: Finding| {
        let mut wire = row.clone();
        wire["check"] = Value::from(pack.check.as_str());
        wire["rubric"] = Value::from(pack.rubric.as_str());
        let task = Task::from_pack(
            CELL,
            pack.digest(),
            "written_artifact",
            data["criterion_claim"].as_str().unwrap(),
            &wire,
        )
        .unwrap();
        let task = if published {
            task.with_published_sources(pack).unwrap()
        } else {
            task
        };
        let case = task.published_case();
        let (_, _, report) = signed(&task, when, line, finding, quote);
        (case, report)
    };
    let assess = |pack: &Pack, when, line: &str, quote: &str, published| {
        examine(pack, when, line, quote, published, Finding::Meets)
    };
    let before = at(30, 1, false);
    let later = at(58, 2, false);
    let (hall_id, hall) = assess(hall_pack, before, REVISION, "dry hall", true);
    let (lamp_id, lamp) = assess(lamp_pack, later, NEXT, "lamp", true);
    let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    // The offline editor signed these two exact publications and this shared criterion.
    assert_eq!(
        hex(hall_id.unwrap().pack),
        "ed3e285d495fca40ba9cbd14a668971957aba2f830a22b2b8026c6ba5735bb13"
    );
    assert_eq!(
        hex(lamp_id.unwrap().pack),
        "ddeb8523294bc679951c3edfc993f2903e2af3c833ff5e7f6a3ecfefff69aac5"
    );
    assert_eq!(
        hex(hall_id.unwrap().criterion),
        "294c87287da53ccdab240694475f7843757ab3a634c2d6ee9de8ea0e3aaa3e45"
    );
    assert_eq!(lamp_id.unwrap().criterion, hall_id.unwrap().criterion);
    let reviewed = BTreeSet::from([(hall_id.unwrap(), lamp_id.unwrap())]);
    let w = Withdrawn::new(CELL.scope);
    let check = |a: &Evidence,
                 b: &Evidence,
                 log: Option<&[Exposure]>,
                 w: &Withdrawn,
                 pairs: &BTreeSet<_>| {
        delayed_on_reviewed_change(
            a,
            b,
            log,
            w,
            Spacing {
                stability_days: 3.0,
                threshold: 0.85,
                target_day: None,
            },
            pairs,
        )
        .unwrap()
    };
    let empty = Some([].as_slice());
    assert_eq!(delayed_result(&hall, &lamp, &w), CheckResult::Ineligible);
    assert_eq!(
        check(&hall, &lamp, empty, &w, &reviewed),
        CheckResult::SupportsDurability
    );
    assert_eq!(
        check(&hall, &lamp, None, &w, &reviewed),
        CheckResult::UnknownExposure
    );
    let help = [Exposure {
        cell: CELL,
        at: Moment {
            day: 57,
            sequence: 1,
        },
    }];
    assert_eq!(
        check(&hall, &lamp, Some(&help), &w, &reviewed),
        CheckResult::InsufficientSpacing
    );
    assert_eq!(
        check(&hall, &lamp, empty, &w, &BTreeSet::new()),
        CheckResult::Ineligible
    );

    let (_, familiar) = assess(lamp_pack, at(58, 2, true), NEXT, "lamp", true);
    assert_eq!(
        check(&hall, &familiar, empty, &w, &reviewed),
        CheckResult::Ineligible
    );
    let (_, legacy) = assess(lamp_pack, later, NEXT, "lamp", false);
    assert_eq!(
        check(&hall, &legacy, empty, &w, &reviewed),
        CheckResult::Ineligible
    );
    let (_, earlier_lamp) = assess(lamp_pack, before, NEXT, "lamp", true);
    let (_, later_hall) = assess(hall_pack, later, REVISION, "dry hall", true);
    assert_eq!(
        check(&earlier_lamp, &later_hall, empty, &w, &reviewed),
        CheckResult::Ineligible
    );
    // A second editorial decision can support the opposite direction, including
    // a negative finding. A commission cannot guarantee an independent success.
    let reverse = BTreeSet::from([(lamp_id.unwrap(), hall_id.unwrap())]);
    let (_, unsuccessful_hall) = examine(
        hall_pack,
        later,
        PETITION,
        "Stop the rain",
        true,
        Finding::NotYet,
    );
    assert_eq!(
        check(&earlier_lamp, &later_hall, empty, &w, &reverse),
        CheckResult::SupportsDurability
    );
    assert_eq!(
        check(&earlier_lamp, &unsuccessful_hall, empty, &w, &reverse),
        CheckResult::OpposesDurability
    );
    // A worked sample on the same objective is still a teaching contact.
    // Counting from delivery makes a same-day check too early. This case assumes
    // that the upcoming check itself was never shown during the sample.
    let sample = [Exposure {
        cell: CELL,
        at: Moment {
            day: 58,
            sequence: 1,
        },
    }];
    let (_, returned) = assess(lamp_pack, at(63, 3, false), NEXT, "lamp", true);
    let seen = Presentation {
        fresh: false,
        ..at(63, 3, false)
    };
    let (_, displayed_previously) = assess(lamp_pack, seen, NEXT, "lamp", true);
    for (evidence, expected) in [
        (&lamp, CheckResult::InsufficientSpacing),
        (&returned, CheckResult::SupportsDurability),
        (&displayed_previously, CheckResult::Ineligible),
    ] {
        assert_eq!(
            check(&hall, evidence, Some(&sample), &w, &reviewed),
            expected
        );
    }

    // A third case mentions BOTH gifts and lets the writer choose. Repeating
    // either earlier literal answer is enough under the scripted review.
    let mut either = lamp_pack.clone();
    either.check = "Rain cancels a choir's rehearsal. The group receives a dry hall and a departing worker receives a lamp for the trip. Choose either fictional person. Give thanks for one gift and connect it to the verse.".into();
    let mut renamed = lamp_pack.clone();
    renamed.check = super::draft("disguised").unwrap().0.into();
    let mut edited = lamp_pack.clone();
    edited
        .rubric
        .push_str(" Require the learner to report personal devotion.");
    let mut reprint = lamp_pack.clone();
    reprint
        .worked
        .push_str(" Reprinted in a different edition.");
    for (name, pack, prior, line, quote) in [
        ("either after hall", &either, &hall, REVISION, "dry hall"),
        ("either after lamp", &either, &earlier_lamp, NEXT, "lamp"),
        ("renamed lamp", &renamed, &earlier_lamp, NEXT, "lamp"),
        ("unreviewed rubric", &edited, &hall, NEXT, "lamp"),
        ("reprinted lamp", &reprint, &earlier_lamp, NEXT, "lamp"),
    ] {
        let (_, admitted) = assess(pack, later, line, quote, true);
        let result = check(prior, &admitted, empty, &w, &reviewed);
        println!("{name}: {result:?}");
        assert_eq!(result, CheckResult::Ineligible);
    }
    let mut revoked = w;
    revoked.sources.insert(hall_pack.digest());
    assert_eq!(
        check(&hall, &lamp, empty, &revoked, &reviewed),
        CheckResult::Ineligible
    );
}

#[test]
fn hall_revision_and_later_unseen_lamp_preserve_the_observation_conditions() {
    let source = published_psalm_lesson().unwrap();
    let lamp_pack = source.material(true).unwrap();
    let mut order = Commission::default();
    assert!(order.request());
    assert_eq!(
        order.review(lamp_pack, "hall").unwrap()["editor_disposition"],
        "approved"
    );
    let hall = order.reviewed.as_ref().unwrap();
    let hall_pack = hall.batch.available(hall.work, &lamp_pack.sources).unwrap();
    let data: Value =
        serde_json::from_str(include_str!("../../../fixtures/goals/worship.json")).unwrap();
    let lamp_row = &data["first_pack"];
    let mut hall_row = lamp_row.clone();
    hall_row["check"] = Value::String(hall_pack.check.clone());
    let hall = approved_task(&data, &hall_row, hall_pack);
    let lamp = approved_task(&data, lamp_row, lamp_pack);

    let before = at(30, 1, false);
    let after = at(30, 2, true);
    let later = at(58, 3, false);
    let w = Withdrawn::new(CELL.scope);
    let copied = lamp_pack
        .worked
        .split('‘')
        .nth(1)
        .unwrap()
        .split('’')
        .next()
        .unwrap();
    assert!(copied.contains("place beside my neighbors"));
    let (old, old_review, initial) =
        signed(&hall, before, PETITION, Finding::NotYet, "Stop the rain");
    let (copy, copy_review, copied_result) = signed(
        &hall,
        after,
        copied,
        Finding::NotYet,
        "place beside my neighbors",
    );
    let (changed, revision_review, revised_result) = signed(
        &hall,
        after,
        REVISION,
        Finding::Meets,
        "dry hall we have been given",
    );
    let (_, _, new_result) = signed(
        &lamp,
        later,
        NEXT,
        Finding::Meets,
        "lamp that showed the way home",
    );
    for (name, observation) in [
        ("original", &initial),
        ("copy", &copied_result),
        ("revision", &revised_result),
        ("new", &new_result),
    ] {
        println!(
            "{name}: {:?}, independently eligible={}",
            observation.judgment(),
            observation.opens_transfer(&w)
        );
    }
    assert_eq!(initial.judgment(), Judgment::Incorrect);
    assert_eq!(copied_result.judgment(), Judgment::Incorrect);
    assert_eq!(revised_result.judgment(), Judgment::Correct);
    assert!(!revised_result.opens_transfer(&w));
    assert!(new_result.opens_transfer(&w));
    assert_eq!(
        delayed_result(&revised_result, &new_result, &w),
        CheckResult::Ineligible
    );
    assert_eq!(
        hall.assess(after, &copy, Some(&revision_review)).judgment(),
        Judgment::Missing
    );
    // A counterfactual learner already met the criterion independently in the
    // hall. Even a genuinely different second check under the same objective
    // still carries unit 101 as its item ID in both published records.
    let (_, _, independent_hall) = signed(
        &hall,
        before,
        REVISION,
        Finding::Meets,
        "dry hall we have been given",
    );
    println!(
        "distinct checked tasks on the same unit: {:?}",
        delayed_result(&independent_hall, &new_result, &w)
    );
    assert!(independent_hall.opens_transfer(&w));
    assert_eq!(
        delayed_result(&independent_hall, &new_result, &w),
        CheckResult::Ineligible
    );
    let first = ReviewedWriting {
        at: before,
        submission: &old,
        review: &old_review,
    };
    let copied = ReviewedWriting {
        at: after,
        submission: &copy,
        review: &copy_review,
    };
    let revised = ReviewedWriting {
        at: after,
        submission: &changed,
        review: &revision_review,
    };
    let copy_comparison = hall.compare_writing(first, copied, &w).unwrap();
    let revision_comparison = hall.compare_writing(first, revised, &w).unwrap();
    assert!(copy_comparison.wording_changed && revision_comparison.wording_changed);
    assert_eq!(copy_comparison.later, Finding::NotYet);
    assert_eq!(
        &PETITION[revision_comparison.earlier_quote.clone()],
        "Stop the rain"
    );
    assert_eq!(
        &REVISION[revision_comparison.later_quote.clone()],
        "dry hall we have been given"
    );
    assert_eq!(revision_comparison.earlier, Finding::NotYet);
    assert_eq!(revision_comparison.later, Finding::Meets);
    assert_eq!(revision_comparison.earlier_help, Assistance::NoneReported);
    assert_eq!(revision_comparison.later_help, Assistance::Reported);

    // Exact same bytes, new submission ID and a conflicting signature.
    let (same, reconsidered, _) = signed(&hall, after, PETITION, Finding::Meets, "Stop the rain");
    let same = ReviewedWriting {
        at: after,
        submission: &same,
        review: &reconsidered,
    };
    let disagreement = hall.compare_writing(first, same, &w).unwrap();
    assert!(!disagreement.wording_changed);
    assert_ne!(disagreement.earlier, disagreement.later);
    assert!(hall
        .compare_writing(
            first,
            ReviewedWriting {
                review: &copy_review,
                ..revised
            },
            &w
        )
        .is_none());
    assert!(lamp.compare_writing(first, revised, &w).is_none());

    // A withdrawn assessment revision loses authority; its replacement names
    // the original performance time, and still carries the original help.
    let mut corrected = Withdrawn::new(CELL.scope);
    corrected.attempts.insert(before.attempt);
    assert!(hall.compare_writing(first, revised, &corrected).is_none());
    let corrected_at = Presentation {
        attempt: 10,
        ..before
    };
    let (corrected_sub, corrected_review, _) = signed(
        &hall,
        corrected_at,
        PETITION,
        Finding::Unresolved,
        "Stop the rain",
    );
    let replacement = ReviewedWriting {
        at: corrected_at,
        submission: &corrected_sub,
        review: &corrected_review,
    };
    let corrected_read = hall
        .compare_writing(replacement, revised, &corrected)
        .unwrap();
    assert_eq!(corrected_read.earlier, Finding::Unresolved);
    assert_eq!(corrected_read.later, Finding::Meets);
    assert_eq!(corrected_read.later_help, Assistance::Reported);
    assert_eq!(initial.judgment(), Judgment::Incorrect);

    corrected.sources.insert(hall_pack.digest());
    assert!(hall
        .compare_writing(replacement, revised, &corrected)
        .is_none());
    assert!(new_result.opens_transfer(&corrected)); // distinct release, same underlying verse
    corrected
        .sources
        .insert(hall_pack.sources.iter().next().unwrap().document);
    assert!(!new_result.opens_transfer(&corrected));
    let (_, _, already_seen) = signed(
        &lamp,
        Presentation {
            fresh: false,
            ..later
        },
        NEXT,
        Finding::Meets,
        "lamp that showed the way home",
    );
    assert!(!already_seen.opens_transfer(&w));
    println!("copied={copy_comparison:?}; revised={revision_comparison:?}; unchanged={disagreement:?}; regraded={corrected_read:?}");
}
