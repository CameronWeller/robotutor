use serde_json::{json, Value};
use tutor_lab::curriculum::{
    discovery::{compare_maps, discover, initial_packet, map_first_reply, rehearse},
    program::Frame,
};

fn scenario() -> Value {
    serde_json::from_str(include_str!("../fixtures/law-discovery.json")).unwrap()
}
fn frame() -> Frame {
    Frame::from_json(&serde_json::from_str(include_str!("../fixtures/law-survey.json")).unwrap())
        .unwrap()
}

#[test]
fn a_first_year_outline_fails_the_civic_surveys_held_questions() {
    let v = scenario();
    let result = rehearse(&v, &frame()).unwrap();
    assert_eq!(result.source_count, 5);
    assert_eq!(result.initial_missing, [2, 9, 11, 12, 13, 14, 15, 16, 17]);
    assert_eq!(
        result
            .initial_challenges
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [2, 15]
    );
    assert!(result.ready_for_editorial_review());
    assert_eq!(result.provisional, [13, 14, 17]);

    let mut partial = v;
    partial["additions"]
        .as_array_mut()
        .unwrap()
        .retain(|row| row["facet"] != 2);
    let result = rehearse(&partial, &frame()).unwrap();
    assert_eq!(result.repaired_missing, [2]);
    assert!(!result.ready_for_editorial_review());
}

#[test]
fn proposer_cannot_supply_quotes_invent_citations_or_reassign_evidence() {
    let brief = scenario();
    let mut untrusted = brief.clone();
    untrusted["material"]["sources"][0]["labels"][0]["key"] = json!("self-certified");
    untrusted["first_map"][0]["evidence"] = json!(["self-certified"]);
    assert!(compare_maps(&brief, &frame(), &untrusted)
        .unwrap_err()
        .contains("unknown source label"));
    let mut v = scenario();
    v["first_map"][0]["quote"] = json!("invented");
    assert!(rehearse(&v, &frame()).is_err());
    for citation in ["invented", "hls-tort", "life-course"] {
        let mut v = scenario();
        v["first_map"][0]["evidence"] = json!([citation]);
        assert!(rehearse(&v, &frame()).is_err());
    }
    let mut v = scenario();
    v["additions"][0]["name"] = json!("Entire philosophy syllabus");
    assert!(rehearse(&v, &frame()).is_err());
    let mut v = scenario();
    v["additions"][0] = json!({"facet":2,"name":"Philosophy of law","owner_aim":"life-course"});
    assert!(rehearse(&v, &frame()).is_err());
}

#[test]
fn challenge_is_independent_of_first_packet_and_material_scope_is_bound() {
    let mut v = scenario();
    v["scope"]["answer_key"] = json!("would leak an answer");
    let packet = initial_packet(&v).unwrap();
    assert_eq!(packet["source_cards"].as_array().unwrap().len(), 3);
    assert_eq!(packet["owner_aims"].as_array().unwrap().len(), 3);
    for excluded in [
        "cherokee-federal",
        "sep-valid",
        "facets",
        "first_map",
        "additions",
        "answer_key",
        "challenges",
    ] {
        assert!(!packet.to_string().contains(excluded));
    }
    let philosophy = v["additions"].as_array_mut().unwrap().remove(0);
    v["first_map"].as_array_mut().unwrap().push(philosophy);
    assert!(rehearse(&v, &frame())
        .unwrap_err()
        .contains("at this stage"));

    let mut v = scenario();
    v["material"]["challenges"][0]["evidence"] = json!("hls-regulation");
    assert!(rehearse(&v, &frame()).is_err());
    let mut v = scenario();
    v["material"]["sources"][3]["publisher"] = v["material"]["sources"][0]["publisher"].clone();
    assert!(rehearse(&v, &frame()).is_err());
    let mut v = scenario();
    v["scope"]["jurisdiction"] = json!("England and Wales");
    assert!(rehearse(&v, &frame()).is_err());
}

#[test]
fn new_source_obligation_cannot_disappear_between_discovery_and_planning() {
    let mut v = scenario();
    v["material"]["sources"][4]["labels"][0]["facets"] = json!([15, 19]);
    let discovery = discover(&v, &v).unwrap();
    assert_eq!(discovery.candidate.len(), 18);
    assert_eq!(discovery.repaired_missing, [19]);
    let err = rehearse(&v, &frame()).unwrap_err();
    assert!(err.contains("disagree on areas [19]"));
    let mut v = scenario();
    v["material"]["challenges"] = json!([]);
    assert!(rehearse(&v, &frame()).is_err());
}

#[test]
fn voluntary_aim_needs_a_reason_and_evidence_needs_a_finite_label() {
    let mut v = scenario();
    v["material"]["owner_aims"][0]["reason"] = json!("");
    assert!(rehearse(&v, &frame()).is_err());
    let mut v = scenario();
    v["material"]["sources"][0]["labels"][0]["text"] = json!("x".repeat(181));
    assert!(rehearse(&v, &frame()).is_err());
}

#[test]
fn a_proposed_course_name_can_be_split_by_an_editor_without_model_supplied_ids() {
    let brief = scenario();
    let reply = json!({"areas":[
        {"name":"Bargain, Exchange, and Liability", "basis":"gt-exchange"},
        {"name":"Contracts", "basis":"hls-contract"},
        {"name":"Agreement", "basis":"hls-contract"},
        {"name":"Other economic institutions", "basis":"hls-business"},
        {"name":"Customary resolution", "basis":null}
    ]});
    let mapped = map_first_reply(&brief, &reply, |name, key| match (name, key) {
        ("Bargain, Exchange, and Liability", "gt-exchange") => {
            vec![(5, "Torts".into()), (6, "Contracts".into())]
        }
        (_, "hls-contract") => vec![(6, "Contracts".into())],
        _ => vec![],
    })
    .unwrap();
    assert_eq!(mapped.pending, reply["areas"].as_array().unwrap()[3..]);
    assert!(mapped.conflicts.is_empty());
    assert_eq!(
        mapped.first_map[1],
        json!({"facet":6,"name":"Contracts","evidence":["gt-exchange","hls-contract"]})
    );
    let audited = compare_maps(
        &brief,
        &frame(),
        &json!({"first_map":mapped.first_map,"additions":[]}),
    )
    .unwrap();
    assert_eq!(
        audited.candidate.keys().copied().collect::<Vec<_>>(),
        [5, 6]
    );
    assert!(audited.initial_missing.contains(&2));

    let invalid = map_first_reply(&brief, &reply, |_, _| vec![(2, "Philosophy of law".into())]);
    assert!(invalid
        .unwrap_err()
        .contains("invalid source label gt-exchange"));
    for key in ["sep-valid", "invented-source"] {
        let raw = json!({"areas":[{"name":"Law and morals","basis":key}]});
        assert!(map_first_reply(&brief, &raw, |_, _| vec![]).is_err());
    }
    let raw = json!({"areas":[{"name":"Professional duty","basis":"justice-access"}]});
    let mapped = map_first_reply(&brief, &raw, |_, _| {
        vec![(17, "Professional ethics and access to justice".into())]
    })
    .unwrap();
    assert_eq!(mapped.first_map[0]["owner_aim"], "justice-access");
}

#[test]
fn conflicting_editorial_choices_are_held_by_area() {
    let mut brief = scenario();
    // Simulate a reviewed topic lead for the same area as an existing owner aim.
    brief["material"]["sources"][0]["labels"][0]["facets"] = json!([1, 17, 18]);
    let raw = json!({"areas":[
        {"name":"Professional research", "basis":"hls-method"},
        {"name":"Access to representation", "basis":"justice-access"},
        {"name":"Exchange and liability", "basis":"gt-exchange"},
        {"name":"Contracts", "basis":"hls-contract"}
    ]});
    let result = map_first_reply(&brief, &raw, |_, key| match key {
        "hls-method" | "justice-access" => vec![(17, "Ethics".into())],
        "gt-exchange" => vec![(5, "Torts".into()), (6, "Commercial agreements".into())],
        _ => vec![(6, "Contracts".into())],
    })
    .unwrap();
    assert_eq!(
        result.first_map,
        [json!({"facet":5,"name":"Torts","evidence":["gt-exchange"]})]
    );
    assert_eq!(
        result.conflicts,
        [
            json!({"facet":6,"conflicting_names":true,"conflicting_basis":false,"choices":{
            "Commercial agreements":["gt-exchange"],"Contracts":["hls-contract"]}}),
            json!({"facet":17,"conflicting_names":false,"conflicting_basis":true,"choices":{
            "Ethics":["hls-method","justice-access"]}})
        ]
    );
}
