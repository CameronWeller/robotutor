use serde_json::{json, Value};
use tutor_lab::curriculum::{
    program::{Frame, Proposal},
    rehearsal_release::publish,
};

fn fixture() -> (Frame, Proposal, Value) {
    let data: Value = serde_json::from_str(include_str!("../fixtures/law-survey.json")).unwrap();
    let frame = Frame::from_json(&data).unwrap();
    let plan = Proposal::from_json(&data["proposal"]).unwrap();
    (frame, plan, data["first_pack"].clone())
}

#[test]
fn source_withdrawal_blocks_broker_reads_and_the_author_request_hides_the_reviewed_answers() {
    let (frame, plan, record) = fixture();
    let release = publish(&frame, &plan, frame.review_digest(&plan), &record).unwrap();
    assert_eq!(u64::from(release.unit()), record["unit"].as_u64().unwrap());
    let envelope: Value = serde_json::from_str(release.request()).unwrap();
    let brief: Value =
        serde_json::from_str(envelope["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        brief["excerpts"],
        json!([{"cite":"S1","text":record["excerpt"]}])
    );
    for field in ["check", "rubric"] {
        assert!(!release.request().contains(record[field].as_str().unwrap()));
    }
    assert_eq!(release.material(true).unwrap().lesson, record["lesson"]);
    assert!(release
        .material(false)
        .unwrap_err()
        .contains("SourceUnavailable"));
    assert_eq!(release.material(true).unwrap().check, record["check"]);
}

#[test]
fn a_changed_attribution_changes_the_work_and_invalid_material_cannot_replay() {
    let (frame, plan, mut record) = fixture();
    let reviewed = frame.review_digest(&plan);
    let original = publish(&frame, &plan, reviewed, &record).unwrap();
    // Model can see identical words in two independently reviewed local source editions.
    // Their resulting job and allowed span identities must still differ.
    record["locator"] = json!("Reviewer's alternate citation");
    let changed = publish(&frame, &plan, reviewed, &record).unwrap();
    assert_ne!(
        original.material(true).unwrap().sources,
        changed.material(true).unwrap().sources
    );
    assert_ne!(original.request(), changed.request());
    assert!(publish(&frame, &plan, [0; 32], &record).is_err());
    record["worked"] = record["check"].clone();
    assert!(publish(&frame, &plan, reviewed, &record).is_err());
}

#[test]
fn both_encounters_across_the_goal_examples_use_their_own_pinned_evidence() {
    for case in [
        include_str!("../fixtures/goals/worship.json"),
        include_str!("../fixtures/goals/better-person.json"),
        include_str!("../fixtures/goals/chemical-engineering.json"),
        include_str!("../fixtures/goals/violin.json"),
        include_str!("../fixtures/goals/writer.json"),
    ] {
        let data: Value = serde_json::from_str(case).unwrap();
        let frame = Frame::from_json(&data).unwrap();
        let plan = Proposal::from_json(&data["proposal"]).unwrap();
        let reviewed = frame.review_digest(&plan);
        for encounter in ["first_pack", "return_pack"] {
            let record = &data[encounter];
            let release = publish(&frame, &plan, reviewed, record).unwrap();
            let request: Value = serde_json::from_str(release.request()).unwrap();
            let brief: Value =
                serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(brief["objective"]["id"], record["unit"]);
            assert_eq!(
                brief["excerpts"],
                json!([{"cite":"S1","text":record["excerpt"]}])
            );
            assert_eq!(brief["scope"]["setting"], data["scope"]["setting"]);
            assert!(brief.get("attempts").is_none() && brief.get("quantity").is_none());
            assert!(!release
                .request()
                .contains(record["rubric"].as_str().unwrap()));
            assert_eq!(release.material(true).unwrap().rubric, record["rubric"]);
            assert!(release.material(false).is_err());
        }
    }
}
