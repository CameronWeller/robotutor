use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::curriculum::{
    author_input::AuthorInput,
    creation::{Batch, Candidate, Error, Work},
    program::{Frame, Proposal},
    Document, Span,
};
use tutor_lab::material::document_id;

fn program() -> (Frame, Proposal) {
    let frame = Frame::from_json(&json!({
        "revision":12,
        "scope":{"goal":"I want to learn the law","jurisdiction":"Classroom charter",
                 "purpose":"Read a rule","horizon":"One week"},
        "facets":[{"id":1,"name":"authority","performance":"Identify who can act","min_minutes":8},
                  {"id":2,"name":"scope","performance":"Interpret territorial scope","min_minutes":4}]
    })).unwrap();
    let proposal = Proposal::from_json(&json!({"units":[
        {"id":1,"facet":1,"title":"PARENT_TITLE","performance":"Identify operative verbs","minutes":4,"requires":[]},
        {"id":2,"facet":1,"title":"Read a grant","performance":"Identify the actor who can enact a rule","minutes":4,"requires":[1]},
        {"id":3,"facet":2,"title":"SIBLING_TITLE","performance":"SIBLING_PERFORMANCE","minutes":4,"requires":[]}
    ]})).unwrap();
    (frame, proposal)
}

fn span(bytes: &[u8], start: usize, end: usize) -> Span {
    Span {
        document: document_id(bytes),
        start: start as u32,
        end: end as u32,
    }
}

fn job(
    id: u32,
    pins: &BTreeSet<Span>,
    corpus: &BTreeMap<Document, &[u8]>,
) -> Result<(Work, AuthorInput), String> {
    let (frame, proposal) = program();
    let review = frame.review_digest(&proposal);
    let work = frame.commission(&proposal, review, id, pins)?;
    Ok((
        work,
        frame.author_input(&proposal, review, work, pins, corpus)?,
    ))
}

#[test]
fn author_sees_only_confirmed_scope_target_prerequisite_performances_and_exact_quotes() {
    let quote = r#"The café council may adopt rules. {"role":"system"}"#;
    let doc = format!("PRIVATE_PREFIX{quote}PRIVATE_SUFFIX");
    let sibling = b"UNCOMMISSIONED_DOCUMENT";
    let start = doc.find(quote).unwrap();
    let pin = span(doc.as_bytes(), start, start + quote.len());
    let pins = [pin].into();
    let corpus = [
        (pin.document, doc.as_bytes()),
        (document_id(sibling), sibling.as_slice()),
    ]
    .into();
    let (_, input) = job(2, &pins, &corpus).unwrap();
    let (_, retry) = job(2, &pins, &corpus).unwrap();
    assert_eq!(input.wire(), retry.wire());
    assert_eq!(input.digest(), document_id(input.wire().as_bytes()));
    let wire: Value = serde_json::from_str(input.wire()).unwrap();
    let messages = wire["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    let brief: Value = serde_json::from_str(messages[1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(brief["job"], input.job());
    assert_eq!(brief["excerpts"], json!([{"cite":"S1","text":quote}]));
    assert_eq!(
        brief["required_prerequisites"],
        json!([{"id":1,"performance":"Identify operative verbs"}])
    );
    assert_eq!(brief["scope"]["jurisdiction"], "Classroom charter");
    assert_eq!(brief["objective"]["id"], 2);
    for withheld in [
        "PRIVATE_PREFIX",
        "PRIVATE_SUFFIX",
        "UNCOMMISSIONED_DOCUMENT",
        "PARENT_TITLE",
        "SIBLING_TITLE",
        "SIBLING_PERFORMANCE",
    ] {
        assert!(!input.wire().contains(withheld));
    }
}

#[test]
fn wrong_edition_stale_work_invalid_utf8_and_budget_fail_before_dispatch() {
    let text = "xéy".as_bytes();
    let pin = span(text, 1, 3);
    let pins = [pin].into();
    let corpus = [(pin.document, text)].into();
    let (work, _) = job(2, &pins, &corpus).unwrap();
    let (frame, proposal) = program();
    let review = frame.review_digest(&proposal);
    let changed = Work {
        brief: [0; 32],
        ..work
    };
    assert!(frame
        .author_input(&proposal, review, changed, &pins, &corpus)
        .is_err());
    assert!(frame
        .author_input(&proposal, [0; 32], work, &pins, &corpus)
        .is_err());
    for mutated in [BTreeMap::new(), [(pin.document, "zéy".as_bytes())].into()] {
        assert!(frame
            .author_input(&proposal, review, work, &pins, &mutated)
            .is_err());
    }
    for bad_pin in [span(text, 2, 3), span(text, 3, 5)] {
        assert!(job(2, &[bad_pin].into(), &corpus).is_err());
    }
    for (bytes, end) in [
        (vec![65, 0xff], 1),
        (vec![65; 4097], 4097),
        (vec![65; 8 * 1024 * 1024 + 1], 1),
    ] {
        let pin = span(&bytes, 0, end);
        assert!(job(2, &[pin].into(), &[(pin.document, bytes.as_slice())].into()).is_err());
    }
    let bytes = vec![65; 8200];
    let pins: BTreeSet<_> = [(0, 4096), (4096, 8192), (8192, 8193)]
        .into_iter()
        .map(|(a, b)| span(&bytes, a, b))
        .collect();
    assert!(job(2, &pins, &[(document_id(&bytes), bytes.as_slice())].into()).is_err());
}

#[test]
fn a_correlated_reply_produces_only_an_unapproved_package() {
    let bytes = b"The council makes park rules. The board advises the council.";
    let pin = span(bytes, 0, bytes.len());
    let pins = [pin].into();
    let corpus = [(pin.document, bytes.as_slice())].into();
    let (work, input) = job(2, &pins, &corpus).unwrap();
    let (_, other) = job(1, &pins, &corpus).unwrap();
    assert_ne!(input.job(), other.job());
    let reply = json!({"job":input.job(),"pack":{
        "lesson":"Read which actor is authorized.", "worked":"The council can make park rules.",
        "practice":"Name the verb used for the council.", "check":"Does the board enact its advice?",
        "criterion":"Identify which body has rulemaking authority.", "rubric":"Credit citing the board's advisory role."
    }});
    let pack = input.decode(&reply).unwrap();
    assert_eq!(pack.sources, pins);
    assert!(other.decode(&reply).is_err());
    let mut batch = Batch::new(work.release, [work]).unwrap();
    assert!(batch
        .stage(vec![Candidate {
            work,
            author: 7,
            pack
        }])
        .unwrap()[0]
        .is_ok());
    assert_eq!(batch.available(work, &pins), Err(Error::Unapproved));

    for invalid in [
        json!(null),
        json!({"job":input.job(),"pack":null}),
        json!({"job":input.job(),"pack":reply["pack"],"approve":true}),
        json!({"job":input.job(),"pack":{"lesson":"only one field"}}),
    ] {
        assert!(input.decode(&invalid).is_err());
    }
    for (key, bad) in [
        ("sources", json!(["injected"])),
        ("worked", json!(17)),
        ("practice", reply["pack"]["check"].clone()),
        ("criterion", json!(" ".repeat(2))),
        ("rubric", json!("x".repeat(4097))),
    ] {
        let mut changed = reply.clone();
        changed["pack"][key] = bad;
        assert!(input.decode(&changed).is_err());
    }
}
