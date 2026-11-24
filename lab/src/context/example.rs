//! Synthetic, authorized arithmetic inputs for integrations and teaching experiments.
use super::*;

pub fn example(purpose: Purpose) -> (Request, Authority, Vec<Record>) {
    let snapshot = Snapshot {
        scope: Scope {
            learner: 42,
            revision: 1,
        },
        class: 7,
        term: 3,
        turn: 8,
        epoch: 2,
        receipt: 91,
        profile: 1,
        outcome: OutcomeId(10),
        purpose,
        role: if matches!(purpose, Purpose::Author | Purpose::Evaluate) {
            Role::Teacher
        } else {
            Role::Learner
        },
    };
    let brief = match purpose {
        Purpose::PrivateForm | Purpose::Author => {
            "Supply three integer parameters for a new equal-parts exercise in one whole."
        }
        Purpose::Teach => "Explain how counting equal parts combines two lengths.",
        Purpose::Probe => "Ask for a new sum of two equal-sized fractions of one metre.",
        Purpose::Evaluate => "Assess only the numeric value of three quarters plus one quarter.",
    };
    let mut facts = vec![
        (Kind::Brief, brief),
        (Kind::Material, crate::material::UNIT_RULE),
    ];
    if purpose == Purpose::Evaluate {
        facts.extend([
            (
                Kind::Rubric,
                "Accept any rational number equivalent to one.",
            ),
            (Kind::Response, "4/4"),
        ]);
    }
    let records: Vec<_> = facts
        .into_iter()
        .enumerate()
        .map(|(i, (kind, text))| Record {
            key: RecordRef {
                id: i as u64 + 1,
                revision: 1,
            },
            release: snapshot.scope.revision,
            class: snapshot.class,
            learner: if purpose == Purpose::Author || matches!(kind, Kind::Material | Kind::Rubric)
            {
                None
            } else {
                Some(snapshot.scope.learner)
            },
            grants: BTreeSet::from([(snapshot.role, purpose)]),
            outcome: snapshot.outcome,
            kind,
            term: snapshot.term,
            turn: snapshot.turn,
            carry: false,
            text: text.into(),
        })
        .collect();
    let request = Request {
        snapshot,
        required: records.iter().map(|r| r.key).collect(),
        memory: [None; 3],
        input_limit: 512,
        output_limit: 80,
    };
    let authority = Authority::new(&request, &records).expect("distinct fixture IDs");
    (request, authority, records)
}
