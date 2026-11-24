use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::curriculum::*;

fn node(id: u8, parents: &[u8]) -> Outcome {
    Outcome {
        id: OutcomeId(id.into()),
        requires: parents.iter().map(|p| OutcomeId(u32::from(*p))).collect(),
        support: BTreeSet::from([Span {
            document: [id; 32],
            start: 0,
            end: 10,
        }]),
    }
}

#[test]
fn invalid_unit_is_rejected_as_a_whole() {
    let candidates = vec![
        (vec![node(1, &[2]), node(2, &[1])], UnitError::Cycle),
        (vec![node(1, &[1])], UnitError::Cycle),
        (
            vec![node(1, &[]), node(1, &[])],
            UnitError::Duplicate(OutcomeId(1)),
        ),
        (
            vec![node(2, &[1])],
            UnitError::UnknownPrerequisite {
                outcome: OutcomeId(2),
                prerequisite: OutcomeId(1),
            },
        ),
    ];
    for (nodes, expected) in candidates {
        assert_eq!(ReviewedUnit::build(9, nodes).unwrap_err(), expected);
    }
    let mut missing = node(1, &[]);
    missing.support.clear();
    assert_eq!(
        ReviewedUnit::build(9, vec![missing]).unwrap_err(),
        UnitError::MissingSupport(OutcomeId(1))
    );
    let mut empty = node(1, &[]);
    empty.support = BTreeSet::from([Span {
        document: [1; 32],
        start: 2,
        end: 2,
    }]);
    assert_eq!(
        ReviewedUnit::build(9, vec![empty]).unwrap_err(),
        UnitError::EmptySpan(OutcomeId(1))
    );
}

#[test]
fn delayed_return_preserves_both_review_and_forward_opportunities() {
    let unit = ReviewedUnit::build(9, vec![node(3, &[1, 2]), node(1, &[]), node(2, &[])]).unwrap();
    let mut learner = Progress {
        revision: 9,
        checkpoints: BTreeMap::new(),
        postponed: BTreeSet::new(),
    };
    learner.checkpoints.insert(
        OutcomeId(1),
        Checkpoint {
            observed: 0,
            due: 7,
        },
    );
    let current = BTreeSet::new();
    let goal = OutcomeId(3);
    assert_eq!(
        unit.next(goal, &learner, 3650, &current, Slot::Forward),
        Ok(Decision::Teach(OutcomeId(2)))
    );
    assert_eq!(
        unit.next(goal, &learner, 3650, &current, Slot::Review),
        Ok(Decision::Recheck(OutcomeId(1)))
    );
    let revoked = BTreeSet::from([[1; 32], [2; 32]]);
    assert_eq!(
        unit.next(goal, &learner, 3650, &revoked, Slot::Review),
        Ok(Decision::RepairSources(OutcomeId(1)))
    );
    assert_eq!(
        unit.next(OutcomeId(4), &learner, 3650, &current, Slot::Forward),
        Err(PlanError::UnknownGoal(OutcomeId(4)))
    );
    learner.revision = 10;
    assert_eq!(
        unit.next(goal, &learner, 3650, &current, Slot::Review),
        Err(PlanError::WrongRevision)
    );
    learner.revision = 9;
    for checkpoint in [
        Checkpoint {
            observed: 3,
            due: 3,
        },
        Checkpoint {
            observed: 4000,
            due: 4001,
        },
    ] {
        learner.checkpoints.insert(OutcomeId(1), checkpoint);
        assert_eq!(
            unit.next(goal, &learner, 3650, &current, Slot::Review),
            Err(PlanError::InvalidCheckpoint(OutcomeId(1)))
        );
    }
}

#[test]
fn decisions_respect_all_ancestors_on_every_three_node_dag() {
    // Enumerate every subgraph of the three possible forward edges in topological order.
    // The oracle spells out transitive reachability independently of the planner's traversal.
    let goal = OutcomeId(2);
    let mut checked = 0;
    for edges in 0..8 {
        let a_to_b = edges & 1 != 0;
        let b_to_c = edges & 2 != 0;
        let a_to_c = edges & 4 != 0;
        let mut c_parents = vec![];
        if b_to_c {
            c_parents.push(1);
        }
        if a_to_c {
            c_parents.push(0);
        }
        let unit = ReviewedUnit::build(
            7,
            vec![
                node(0, &[]),
                node(1, if a_to_b { &[0] } else { &[] }),
                node(2, &c_parents),
            ],
        )
        .unwrap();
        let in_scope = [a_to_c || (a_to_b && b_to_c), b_to_c, true];
        for progress_code in 0..27 {
            let status = [
                progress_code % 3,
                (progress_code / 3) % 3,
                progress_code / 9,
            ];
            let mut learner = Progress {
                revision: 7,
                checkpoints: BTreeMap::new(),
                postponed: BTreeSet::new(),
            };
            for (index, state) in status.iter().enumerate() {
                if *state != 0 {
                    let due = if *state == 1 { 1 } else { 3 + index as u32 };
                    learner
                        .checkpoints
                        .insert(OutcomeId(index as u32), Checkpoint { observed: 0, due });
                }
            }
            for mask in 0..8 {
                let revoked = (0..3_u8)
                    .filter(|id| mask & (1 << id) != 0)
                    .map(|id| [id; 32])
                    .collect();
                let valid = [mask & 1 == 0, mask & 2 == 0, mask & 4 == 0];
                let current_a = status[0] == 2 && valid[0];
                let ready_b = !a_to_b || current_a;
                let current_b = ready_b && status[1] == 2 && valid[1];
                let ready = [
                    true,
                    ready_b,
                    (!b_to_c || current_b) && (!a_to_c || current_a),
                ];
                let eligible = |i: usize| in_scope[i] && ready[i] && valid[i];
                for holds in 0..8 {
                    learner.postponed = (0..3)
                        .filter(|i| holds & (1 << i) != 0)
                        .map(OutcomeId)
                        .collect();
                    let active = |i: usize| eligible(i) && holds & (1 << i) == 0;
                    let any = |state| (0..3).any(|i| active(i) && status[i] == state);
                    for slot in [Slot::Forward, Slot::Review] {
                        match unit.next(goal, &learner, 1, &revoked, slot).unwrap() {
                            Decision::Teach(id) => {
                                assert!(active(id.0 as usize) && status[id.0 as usize] == 0);
                                assert!(slot == Slot::Forward || !any(1));
                            }
                            Decision::Recheck(id) => {
                                assert!(active(id.0 as usize) && status[id.0 as usize] == 1);
                                assert!(slot == Slot::Review || !any(0));
                            }
                            Decision::RepairSources(id) => {
                                let i = id.0 as usize;
                                assert!(in_scope[i] && ready[i] && !valid[i] && !any(1) && !any(0));
                            }
                            Decision::Postponed(id) => {
                                let i = id.0 as usize;
                                assert!(eligible(i) && !active(i) && status[i] != 2);
                                assert!(!any(0) && !any(1));
                                assert!(!(0..3).any(|j| in_scope[j] && ready[j] && !valid[j]));
                            }
                            Decision::CurrentUntil(day) => {
                                assert!(
                                    (0..3).all(|i| !in_scope[i] || (eligible(i) && status[i] == 2))
                                );
                                assert_eq!(day, 3 + (0..3).find(|i| in_scope[*i]).unwrap() as u32);
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 27_648);
}

#[test]
fn crawl_charges_edges_before_filters_and_never_releases_candidates() {
    let a = [0; 32];
    let b = [1; 32];
    let forbidden = [2; 32];
    let c = [3; 32];
    let d = [4; 32];
    let links = BTreeMap::from([
        (a, BTreeSet::from([a, b, forbidden, c])),
        (b, BTreeSet::from([a, d])),
    ]);
    let allowed = BTreeSet::from([a, b, c, d]);
    let default = CrawlBudget {
        max_nodes: 4,
        max_edges: 6,
        max_depth: 3,
    };
    assert_eq!(
        crawl(a, &links, &allowed, default),
        Discoveries {
            documents: vec![a, b, c, d],
            edges_inspected: 6,
            limited: false
        }
    );
    let edge_cut = crawl(
        a,
        &links,
        &allowed,
        CrawlBudget {
            max_edges: 3,
            ..default
        },
    );
    assert_eq!(
        edge_cut,
        Discoveries {
            documents: vec![a, b],
            edges_inspected: 3,
            limited: true
        }
    );
    let shallow = crawl(
        a,
        &links,
        &allowed,
        CrawlBudget {
            max_depth: 0,
            ..default
        },
    );
    assert_eq!(
        shallow,
        Discoveries {
            documents: vec![a],
            edges_inspected: 0,
            limited: true
        }
    );
    let small = crawl(
        a,
        &links,
        &allowed,
        CrawlBudget {
            max_nodes: 1,
            ..default
        },
    );
    assert_eq!(
        small,
        Discoveries {
            documents: vec![a],
            edges_inspected: 2,
            limited: true
        }
    );
    let zero = crawl(
        a,
        &links,
        &allowed,
        CrawlBudget {
            max_nodes: 0,
            ..default
        },
    );
    assert!(zero.limited && zero.documents.is_empty());
    assert_eq!(
        crawl(forbidden, &links, &allowed, default),
        Discoveries::default()
    );
}

#[test]
fn postponed_preparation_yields_to_an_independent_branch() {
    let unit = ReviewedUnit::build(1, vec![node(1, &[]), node(2, &[]), node(3, &[1, 2])]).unwrap();
    let mut learner = Progress {
        revision: 1,
        checkpoints: BTreeMap::new(),
        postponed: BTreeSet::from([OutcomeId(1)]),
    };
    let revoked = BTreeSet::new();
    let goal = OutcomeId(3);
    assert_eq!(
        unit.next(goal, &learner, 1, &revoked, Slot::Review),
        Ok(Decision::Teach(OutcomeId(2)))
    );
    learner.postponed.insert(OutcomeId(2));
    assert_eq!(
        unit.next(goal, &learner, 1, &revoked, Slot::Forward),
        Ok(Decision::Postponed(OutcomeId(1)))
    );
    learner.checkpoints.insert(
        OutcomeId(2),
        Checkpoint {
            observed: 1,
            due: 100,
        },
    );
    learner.postponed.remove(&OutcomeId(1));
    assert_eq!(
        unit.next(goal, &learner, 2, &revoked, Slot::Forward),
        Ok(Decision::Teach(OutcomeId(1)))
    );
}

fn example_for(outcome: OutcomeId) -> ReviewedExample {
    ReviewedExample {
        revision: 25,
        unit_revision: 1,
        outcome,
        support: BTreeSet::from([Span {
            document: [99; 32],
            start: 0,
            end: 5,
        }]),
    }
}

#[test]
fn sparse_returns_offer_guided_synthesis_without_consuming_an_overdue_check() {
    let goal = OutcomeId(3);
    let unit = ReviewedUnit::build(1, vec![node(1, &[]), node(2, &[]), node(3, &[1, 2])]).unwrap();
    let mut learner = Progress {
        revision: 1,
        checkpoints: BTreeMap::new(),
        postponed: BTreeSet::new(),
    };
    let example = example_for(goal);
    let revoked = BTreeSet::new();
    for visit in 0..6 {
        let day = 14 * visit;
        let just_demonstrated = OutcomeId(1 + visit % 2);
        learner.checkpoints.insert(
            just_demonstrated,
            Checkpoint {
                observed: day,
                due: day + 7,
            },
        );
        let offer = unit
            .guided_synthesis(
                &example,
                &learner,
                day,
                &revoked,
                GuidanceOpportunity::Scheduled(Slot::Forward),
            )
            .unwrap();
        if visit == 0 {
            assert_eq!(offer, None);
        } else {
            let due = OutcomeId(3 - just_demonstrated.0);
            assert_eq!(
                offer,
                Some(GuidedLesson {
                    outcome: goal,
                    example_revision: 25,
                    overdue_prerequisites: BTreeSet::from([due])
                })
            );
            assert_eq!(
                unit.next(goal, &learner, day, &revoked, Slot::Forward),
                Ok(Decision::Recheck(due))
            );
            assert_eq!(
                unit.guided_synthesis(
                    &example,
                    &learner,
                    day,
                    &revoked,
                    GuidanceOpportunity::Scheduled(Slot::Review)
                ),
                Ok(None)
            );
        }
    }
    // A learner returning ten years later can request a worked lesson. Both dated obligations survive.
    assert_eq!(
        unit.guided_synthesis(
            &example,
            &learner,
            3650,
            &revoked,
            GuidanceOpportunity::Requested
        )
        .unwrap()
        .unwrap()
        .overdue_prerequisites,
        BTreeSet::from([OutcomeId(1), OutcomeId(2)])
    );
    assert_eq!(
        unit.next(goal, &learner, 3650, &revoked, Slot::Review),
        Ok(Decision::Recheck(OutcomeId(1)))
    );
    assert!(!learner.checkpoints.contains_key(&goal));
}

#[test]
fn guided_example_requires_supported_ancestors_a_live_example_and_no_holds() {
    let goal = OutcomeId(3);
    let unit = ReviewedUnit::build(1, vec![node(1, &[]), node(2, &[1]), node(3, &[2])]).unwrap();
    let mut learner = Progress {
        revision: 1,
        checkpoints: BTreeMap::new(),
        postponed: BTreeSet::new(),
    };
    let mut example = example_for(goal);
    let ask = GuidanceOpportunity::Requested;
    let none = BTreeSet::new();
    let checkpoint = Checkpoint {
        observed: 0,
        due: 7,
    };
    learner.checkpoints.insert(OutcomeId(2), checkpoint);
    // No surviving checkpoint for a transitive prerequisite after withdrawal.
    assert_eq!(
        unit.guided_synthesis(&example, &learner, 20, &none, ask),
        Ok(None)
    );
    learner.checkpoints.insert(OutcomeId(1), checkpoint);
    // When every prerequisite is current, ordinary instruction is already available.
    assert_eq!(
        unit.guided_synthesis(&example, &learner, 6, &none, ask),
        Ok(None)
    );
    assert_eq!(
        unit.next(goal, &learner, 6, &none, Slot::Forward),
        Ok(Decision::Teach(goal))
    );
    // Revoking the example, the goal, its parent, or a transitive source independently blocks the lesson.
    for document in [1, 2, 3, 99] {
        let revoked = BTreeSet::from([[document; 32]]);
        assert_eq!(
            unit.guided_synthesis(&example, &learner, 7, &revoked, ask),
            Ok(None)
        );
    }
    for id in [OutcomeId(1), OutcomeId(2), goal] {
        learner.postponed.insert(id);
        assert_eq!(
            unit.guided_synthesis(&example, &learner, 7, &none, ask),
            Ok(None)
        );
        learner.postponed.remove(&id);
    }
    assert!(unit
        .guided_synthesis(&example, &learner, 7, &none, ask)
        .unwrap()
        .is_some());
    example.unit_revision = 2;
    assert_eq!(
        unit.guided_synthesis(&example, &learner, 7, &none, ask),
        Err(PlanError::WrongRevision)
    );
    example.unit_revision = 1;
    example.support.clear();
    assert_eq!(
        unit.guided_synthesis(&example, &learner, 7, &none, ask),
        Ok(None)
    );
}
