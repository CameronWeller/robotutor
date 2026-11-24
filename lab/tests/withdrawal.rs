//! A bounded integration check. Every ordering is replayed from the same reviewed release.
use std::collections::{BTreeMap, BTreeSet};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    curriculum::{Decision, Outcome, OutcomeId, ReviewedUnit, Slot, Span},
    history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{Assistance, Judgment, Question},
};

const BASIC: OutcomeId = OutcomeId(1);
const SIBLING: OutcomeId = OutcomeId(2);
const CHILD: OutcomeId = OutcomeId(3);
const GOAL: OutcomeId = OutcomeId(4);
const SCOPE: Scope = Scope {
    learner: 1,
    revision: 1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    WithdrawRights,
    RestoreRights,
    ChallengeOriginal,
    ReplaceOriginal,
    LongGap,
}

#[derive(Debug)]
struct Observation {
    rights: bool,
    unresolved: bool,
    overdue: bool,
    checkpoint: bool,
    can_show: bool,
    judgment: Option<Judgment>,
    child_plan: Decision,
    shared_goal: Decision,
    sibling_available: bool,
}

impl Observation {
    fn agrees(&self) -> bool {
        let expected = if !self.rights {
            Decision::RepairSources(BASIC)
        } else if self.unresolved {
            Decision::Postponed(BASIC)
        } else if self.overdue {
            Decision::Recheck(BASIC)
        } else {
            Decision::Teach(CHILD)
        };
        self.checkpoint == (self.rights && !self.unresolved)
            && self.can_show == self.rights
            && self.judgment == self.rights.then_some(Judgment::Correct)
            && self.child_plan == expected
            && self.shared_goal == Decision::Teach(SIBLING)
            && self.sibling_available
    }
}

#[test]
fn backward_corrections_reach_delivery_and_the_next_forward_slot() {
    use Change::*;
    let changes = [
        WithdrawRights,
        RestoreRights,
        ChallengeOriginal,
        ReplaceOriginal,
        LongGap,
    ];
    let basic_bytes = UNIT_RULE.as_bytes();
    let second_bytes = format!("Branch two\n{UNIT_RULE}");
    let basic = Span {
        document: document_id(basic_bytes),
        start: 0,
        end: basic_bytes.len() as u32,
    };
    let other = Span {
        document: document_id(second_bytes.as_bytes()),
        start: 11,
        end: second_bytes.len() as u32,
    };
    let unit = ReviewedUnit::build(
        1,
        [
            (BASIC, vec![], basic),
            (SIBLING, vec![], other),
            (CHILD, vec![BASIC], other),
            (GOAL, vec![SIBLING, CHILD], other),
        ]
        .map(|(id, needs, pin)| Outcome {
            id,
            requires: needs.into_iter().collect(),
            support: BTreeSet::from([pin]),
        })
        .into(),
    )
    .unwrap();
    let card = FractionCard::propose(FractionTask::Sum, 1, 2, 3).unwrap();
    let first = Attempt {
        scope: SCOPE,
        id: 1,
        sequence: 1,
        outcome: BASIC,
        item: 1,
        day: 0,
        question: Question::Practice,
        judgment: Judgment::Correct,
        assistance: Assistance::NoneReported,
        fresh: true,
        sources: BTreeSet::from([basic.document]),
    };
    let original = Attempt {
        id: 2,
        sequence: 2,
        item: 2,
        question: Question::ChangedExample,
        ..first.clone()
    };
    let proposal = CheckpointProposal {
        scope: SCOPE,
        outcome: BASIC,
        first: 1,
        confirmation: 2,
        due: 28,
    };

    // G(agreement) needs no history beyond the current revision view. Each
    // observation's expected conditions come independently from the event prefix.
    let predicates = || {
        let mut builder = Predicates::builder();
        let atom = builder.new_predicate(ClosurePredicate::new(
            Observation::agrees,
            "policy agreement",
        ));
        (builder.build(), Property::always(Property::atomic(atom)))
    };
    let (set, property) = predicates();
    let automaton = property.to_monitoring_automaton(&set);
    let mut count = BTreeMap::<&str, usize>::new();
    for index in 0..5usize.pow(4) {
        let mut number = index;
        let trace = [(); 4].map(|_| {
            let change = changes[number % 5];
            number /= 5;
            change
        });
        let mut monitor = Monitor::new(predicates().0, automaton.clone());
        let mut withdrawn = Withdrawn::new(SCOPE);
        let mut assessments = vec![first.clone(), original.clone()];
        let mut pairs = vec![proposal];
        let mut day = 1;
        for (i, &change) in trace.iter().enumerate() {
            match change {
                WithdrawRights => {
                    withdrawn.sources.insert(basic.document);
                }
                RestoreRights => {
                    withdrawn.sources.remove(&basic.document);
                }
                ChallengeOriginal if assessments.len() == 2 => {
                    withdrawn.attempts.insert(2);
                    withdrawn.postponed.insert(BASIC);
                }
                ReplaceOriginal if assessments.len() == 2 => {
                    withdrawn.attempts.insert(2);
                    withdrawn.postponed.remove(&BASIC);
                    assessments.push(Attempt {
                        id: 3,
                        ..original.clone()
                    });
                    pairs.push(CheckpointProposal {
                        confirmation: 3,
                        ..proposal
                    });
                }
                LongGap => day = 1826,
                _ => {}
            }
            let progress = reconcile(SCOPE, day, &assessments, &pairs, &withdrawn).unwrap();
            let plan = |goal| {
                unit.next(goal, &progress, day, &withdrawn.sources, Slot::Forward)
                    .unwrap()
            };
            let source = RuleSource {
                bytes: Some(basic_bytes),
                pin: basic,
                revoked: &withdrawn.sources,
            };
            let sibling_source = RuleSource {
                bytes: Some(second_bytes.as_bytes()),
                pin: other,
                ..source
            };
            let prefix = &trace[..=i];
            let observed = Observation {
                rights: prefix
                    .iter()
                    .rev()
                    .find(|e| matches!(e, WithdrawRights | RestoreRights))
                    != Some(&WithdrawRights),
                unresolved: prefix.contains(&ChallengeOriginal)
                    && !prefix.contains(&ReplaceOriginal),
                overdue: prefix.contains(&LongGap),
                checkpoint: progress.checkpoints.contains_key(&BASIC),
                child_plan: plan(CHILD),
                shared_goal: plan(GOAL),
                can_show: card.question(&source).is_some(),
                judgment: card.assess("1", &source),
                sibling_available: card.question(&sibling_source).is_some(),
            };
            assert!(monitor.next_state(&observed), "{prefix:?}: {observed:?}");
            *count.entry("observations").or_default() += 1;
            for (name, covered) in [
                ("revoked", !observed.rights),
                (
                    "unresolved after rights restore",
                    change == RestoreRights
                        && observed.unresolved
                        && prefix.contains(&WithdrawRights),
                ),
                (
                    "replacement while revoked",
                    change == ReplaceOriginal && !observed.rights,
                ),
                (
                    "due on long return",
                    observed.child_plan == Decision::Recheck(BASIC),
                ),
                ("child ready", observed.child_plan == Decision::Teach(CHILD)),
            ] {
                if covered {
                    *count.entry(name).or_default() += 1;
                }
            }
        }
    }
    assert_eq!(count.len(), 6, "missing scenario witness: {count:?}");
    assert_eq!(count["observations"], 2500);
    println!("{count:?}");
}
