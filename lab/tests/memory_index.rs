use std::collections::BTreeSet;
use tutor_lab::context::{
    index::{IndexError, MemoryIndex},
    *,
};
use tutor_lab::curriculum::OutcomeId;

fn row(base: &Record, id: u64, kind: Kind, text: &str) -> Record {
    Record {
        key: RecordRef { id, revision: 1 },
        kind,
        text: text.into(),
        ..base.clone()
    }
}

fn start() -> (Request, MemoryIndex, Vec<Record>) {
    let (mut req, _, records) = example(Purpose::Probe);
    req.input_limit = 4096;
    let mut index = MemoryIndex::new(req.snapshot.scope, req.snapshot.class);
    for r in &records {
        index
            .put(index.epoch(), r.clone(), BTreeSet::new())
            .unwrap();
    }
    req.snapshot.epoch = index.epoch();
    (req, index, records)
}

fn packet(req: &mut Request, index: &MemoryIndex) -> (Packet, Authority) {
    req.snapshot.epoch = index.epoch();
    let page = index.page(req).unwrap();
    let authority = Authority::new(req, &page.records).unwrap();
    let pkt = compile(req, &authority, &page.records, &|m| {
        Count::Measured(m.wire().len() as u32)
    })
    .unwrap();
    (pkt, authority)
}

#[test]
fn retrieval_seeks_past_10000_old_irrelevant_or_denied_rows() {
    let (mut req, mut index, base) = start();
    for n in 0..10000 {
        let mut saved = row(&base[0], 100 + n, Kind::Episode, "Previous semester");
        saved.term -= 1;
        saved.turn = 900 + n;
        saved.carry = n % 4 == 0 || n % 4 == 1;
        if n % 4 == 1 {
            saved.outcome = OutcomeId(55);
        }
        if n % 4 == 2 {
            saved.grants = [(Role::Teacher, Purpose::Probe)].into();
        }
        index.put(index.epoch(), saved, BTreeSet::new()).unwrap();
    }
    let recent = row(&base[0], 11, Kind::Episode, "Current term");
    index.put(index.epoch(), recent, BTreeSet::new()).unwrap();
    req.snapshot.epoch = index.epoch();
    let page = index.page(&req).unwrap();
    assert_eq!((page.point_lookups, page.episode_keys_examined), (2, 4));
    let ids: Vec<_> = page.records.iter().map(|r| r.key.id).collect();
    assert_eq!(ids, vec![1, 2, 11, 10096, 10092, 10088]);
    let (pkt, auth) = packet(&mut req, &index);
    assert_eq!(
        pkt.included().collect::<Vec<_>>(),
        page.records.iter().map(|r| r.key).collect::<Vec<_>>()
    );
    let body: serde_json::Value = serde_json::from_str(&pkt.dispatch(&auth).unwrap().user).unwrap();
    assert_eq!(body["episodes"][1]["term"], req.snapshot.term - 1);
}

#[test]
fn correction_forget_and_restore_fence_the_next_wire_and_inflight_work() {
    let (mut req, mut index, mut backup) = start();
    let help = row(&backup[0], 3, Kind::Help, "A worked solution was shown.");
    let episode = row(&backup[0], 4, Kind::Episode, "Used a worked solution.");
    let summary = row(&backup[0], 5, Kind::Episode, "Prior exercise was guided.");
    index
        .put(index.epoch(), help.clone(), BTreeSet::new())
        .unwrap();
    index
        .put(index.epoch(), episode.clone(), [help.key].into())
        .unwrap();
    index
        .put(index.epoch(), summary.clone(), [episode.key].into())
        .unwrap();
    req.memory[0] = Some(help.key);
    let (old, _) = packet(&mut req, &index);
    let old_epoch = index.epoch();
    backup.extend([help.clone(), episode, summary]);
    let mut corrected = help.clone();
    corrected.key.revision += 1;
    corrected.text = "A larger font was supplied. Content help remains unknown.".into();
    index
        .put(old_epoch, corrected.clone(), BTreeSet::new())
        .unwrap();
    assert!(matches!(index.page(&req), Err(Hold::StaleSnapshot)));
    req.memory[0] = Some(corrected.key);
    let (new, current) = packet(&mut req, &index);
    assert_eq!(old.dispatch(&current), Err(Hold::StaleSnapshot));
    let wire = new.dispatch(&current).unwrap().wire();
    assert!(wire.contains("Content help remains unknown."));
    assert!(!wire.contains("worked solution") && !wire.contains("Prior exercise was guided."));
    assert_eq!(index.restore(old_epoch, &backup), Err(IndexError::Stale));
    index.restore(index.epoch(), &backup).unwrap(); // archive has only the old help revision
    let (restored, auth) = packet(&mut req, &index);
    assert!(restored
        .dispatch(&auth)
        .unwrap()
        .user
        .contains("\"last_help\":null"));
    index.forget(index.epoch(), corrected.key).unwrap();
    backup.push(corrected.clone()); // even the last valid copy is now forbidden
    index.restore(index.epoch(), &backup).unwrap();
    corrected.key.revision += 1;
    assert_eq!(
        index.put(index.epoch(), corrected, BTreeSet::new()),
        Err(IndexError::Forgotten)
    );
    let (forgotten, auth) = packet(&mut req, &index);
    assert!(forgotten
        .dispatch(&auth)
        .unwrap()
        .user
        .contains("\"last_help\":null"));
}

#[test]
fn derived_context_cannot_widen_source_access_or_create_a_dependency_cycle() {
    let (req, mut index, base) = start();
    let source = row(&base[0], 7, Kind::Help, "Learner asked for a hint.");
    let derived = row(&base[0], 8, Kind::Episode, "Previous hint.");
    index
        .put(index.epoch(), source.clone(), BTreeSet::new())
        .unwrap();
    for broadened in [
        Record {
            carry: true,
            ..derived.clone()
        },
        Record {
            term: source.term + 1,
            ..derived.clone()
        },
        Record {
            learner: None,
            kind: Kind::Material,
            ..derived.clone()
        },
        Record {
            grants: [(Role::Teacher, Purpose::Probe)].into(),
            ..derived.clone()
        },
    ] {
        assert_eq!(
            index.put(index.epoch(), broadened, [source.key].into()),
            Err(IndexError::InvalidSupport)
        );
    }
    index
        .put(index.epoch(), derived.clone(), [source.key].into())
        .unwrap();
    let mut later = source.clone();
    later.key.revision += 1;
    assert_eq!(
        index.put(index.epoch(), later, [derived.key].into()),
        Err(IndexError::InvalidSupport)
    );
    let foreign = Record {
        learner: Some(req.snapshot.scope.learner + 1),
        ..source
    };
    assert_eq!(
        index.put(index.epoch(), foreign, BTreeSet::new()),
        Err(IndexError::WrongScope)
    );
    let before = index.epoch();
    index.forget(before, derived.key).unwrap();
    assert_eq!(
        index.put(before, derived, BTreeSet::new()),
        Err(IndexError::Stale)
    );
}

#[test]
fn partial_restore_never_returns_a_derivative_whose_origin_is_missing() {
    let (mut req, mut index, mut backup) = start();
    let origin = row(&backup[0], 10, Kind::Help, "Origin");
    let summary = row(&backup[0], 11, Kind::Episode, "Based on origin");
    index
        .put(index.epoch(), origin.clone(), BTreeSet::new())
        .unwrap();
    index
        .put(index.epoch(), summary.clone(), [origin.key].into())
        .unwrap();
    backup.push(summary);
    index.restore(index.epoch(), &backup).unwrap();
    let (p, a) = packet(&mut req, &index);
    assert!(!p.dispatch(&a).unwrap().user.contains("Based on origin"));
    backup.push(origin);
    index.restore(index.epoch(), &backup).unwrap();
    let (p, a) = packet(&mut req, &index);
    assert!(p.dispatch(&a).unwrap().user.contains("Based on origin"));
}
