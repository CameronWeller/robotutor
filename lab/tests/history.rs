use std::collections::BTreeSet;
use tutor_lab::{
    curriculum::{Checkpoint, OutcomeId},
    history::{reconcile, Attempt, CheckpointProposal, HistoryError, Scope, Withdrawn},
    protocol::{Assistance, Judgment, Question},
};

const SCOPE: Scope = Scope {
    learner: 3,
    revision: 19,
};
const OUTCOME: OutcomeId = OutcomeId(7);

fn attempt(id: u64, day: u32, question: Question) -> Attempt {
    Attempt {
        scope: SCOPE,
        id,
        sequence: id,
        day,
        outcome: OUTCOME,
        item: id + 100,
        question,
        assistance: Assistance::NoneReported,
        judgment: Judgment::Correct,
        fresh: true,
        sources: BTreeSet::from([[id as u8; 32]]),
    }
}

fn pair(first: u64, confirmation: u64, due: u32) -> CheckpointProposal {
    CheckpointProposal {
        scope: SCOPE,
        outcome: OUTCOME,
        first,
        confirmation,
        due,
    }
}

fn fixture() -> (Vec<Attempt>, Vec<CheckpointProposal>) {
    (
        vec![
            attempt(1, 0, Question::Practice),
            attempt(2, 0, Question::ChangedExample),
            attempt(3, 8, Question::DelayedReview),
            attempt(4, 22, Question::DelayedReview),
        ],
        vec![pair(1, 2, 7), pair(2, 3, 21), pair(3, 4, 36)],
    )
}

fn project(
    attempts: &[Attempt],
    proposals: &[CheckpointProposal],
    withdrawn: &Withdrawn,
) -> Option<Checkpoint> {
    reconcile(SCOPE, 30, attempts, proposals, withdrawn)
        .unwrap()
        .checkpoints
        .get(&OUTCOME)
        .copied()
}

#[test]
fn every_supported_pair_is_retracted_including_shared_dependencies() {
    let (attempts, proposals) = fixture();
    assert_eq!(
        project(&attempts, &proposals, &Withdrawn::new(SCOPE)),
        Some(Checkpoint {
            observed: 22,
            due: 36
        })
    );
    for mut withdrawn in [
        {
            let mut w = Withdrawn::new(SCOPE);
            w.attempts.insert(3);
            w
        },
        {
            let mut w = Withdrawn::new(SCOPE);
            w.items.insert(103);
            w
        },
        {
            let mut w = Withdrawn::new(SCOPE);
            w.sources.insert([3; 32]);
            w
        },
    ] {
        assert_eq!(
            project(&attempts, &proposals, &withdrawn),
            Some(Checkpoint {
                observed: 0,
                due: 7
            })
        );
        withdrawn.attempts.insert(2);
        assert_eq!(project(&attempts, &proposals, &withdrawn), None);
    }
}

#[test]
fn help_repetition_and_an_unsubstantiated_key_never_schedule_a_return() {
    let (mut attempts, _) = fixture();
    let proposals = [pair(1, 2, 7)];
    for mutation in [
        (Assistance::Reported, true, true),
        (Assistance::Unknown, true, true),
        (Assistance::NoneReported, false, true),
        (Assistance::NoneReported, true, false),
    ] {
        let (assistance, fresh, has_support) = mutation;
        let mut changed = attempts.clone();
        changed[1].assistance = assistance;
        changed[1].fresh = fresh;
        if !has_support {
            changed[1].sources.clear();
        }
        assert_eq!(project(&changed, &proposals, &Withdrawn::new(SCOPE)), None);
    }
    attempts[1].item = attempts[0].item;
    assert_eq!(project(&attempts, &proposals, &Withdrawn::new(SCOPE)), None);
    assert_eq!(
        project(
            &attempts,
            &[pair(3, 2, 7), pair(1, 5, 7)],
            &Withdrawn::new(SCOPE)
        ),
        None
    );
    attempts[1].item += 1;
    attempts[1].question = Question::DelayedReview;
    assert_eq!(project(&attempts, &proposals, &Withdrawn::new(SCOPE)), None);
}

#[test]
fn scope_and_divergent_id_reuse_fail_closed_even_if_withdrawn() {
    let (mut attempts, mut proposals) = fixture();
    let other_learner = Scope {
        learner: 9,
        ..SCOPE
    };
    let other_release = Scope {
        revision: 20,
        ..SCOPE
    };
    for other in [other_learner, other_release] {
        attempts[0].scope = other;
        assert!(matches!(
            reconcile(SCOPE, 30, &attempts, &proposals, &Withdrawn::new(SCOPE)),
            Err(HistoryError::ScopeMismatch)
        ));
        attempts[0].scope = SCOPE;
        proposals[0].scope = other;
        assert!(matches!(
            reconcile(SCOPE, 30, &attempts, &proposals, &Withdrawn::new(SCOPE)),
            Err(HistoryError::ScopeMismatch)
        ));
        proposals[0].scope = SCOPE;
        assert!(matches!(
            reconcile(SCOPE, 30, &attempts, &proposals, &Withdrawn::new(other)),
            Err(HistoryError::ScopeMismatch)
        ));
    }
    let copy = attempts[0].clone();
    attempts.push(copy.clone());
    assert_eq!(
        project(&attempts, &proposals, &Withdrawn::new(SCOPE))
            .unwrap()
            .due,
        36
    );
    attempts.push(Attempt {
        judgment: Judgment::Incorrect,
        ..copy
    });
    let mut withdrawn = Withdrawn::new(SCOPE);
    withdrawn.attempts.insert(1);
    assert!(matches!(
        reconcile(SCOPE, 30, &attempts, &proposals, &withdrawn),
        Err(HistoryError::ConflictingAttempt(1))
    ));
}

#[test]
fn returns_and_corrections_are_order_independent_and_date_bounded() {
    let (mut attempts, mut proposals) = fixture();
    assert_eq!(
        reconcile(SCOPE, 7, &attempts, &proposals, &Withdrawn::new(SCOPE))
            .unwrap()
            .checkpoints[&OUTCOME]
            .due,
        7
    );
    let mut miss = attempt(5, 22, Question::DelayedReview);
    miss.judgment = Judgment::Incorrect;
    attempts.push(miss);
    attempts.reverse();
    proposals.reverse();
    let mut withdrawn = Withdrawn::new(SCOPE);
    assert_eq!(project(&attempts, &proposals, &withdrawn), None);
    withdrawn.attempts.insert(5);
    assert_eq!(project(&attempts, &proposals, &withdrawn).unwrap().due, 36);
    proposals.push(pair(3, 4, 35));
    assert!(matches!(
        reconcile(SCOPE, 30, &attempts, &proposals, &withdrawn),
        Err(HistoryError::ConflictingDue { .. })
    ));
}

#[test]
fn a_regrade_cannot_move_an_old_performance_after_a_later_miss() {
    let (mut attempts, mut proposals) = fixture();
    let mut miss = attempt(5, 22, Question::DelayedReview);
    miss.judgment = Judgment::Incorrect;
    attempts.push(miss);
    let replacement = Attempt {
        id: 6,
        ..attempts[3].clone()
    };
    attempts.push(replacement);
    proposals.push(pair(3, 6, 36));
    let mut withdrawn = Withdrawn::new(SCOPE);
    assert!(matches!(
        reconcile(SCOPE, 30, &attempts, &proposals, &withdrawn),
        Err(HistoryError::OverlappingRevisions {
            outcome: OUTCOME,
            sequence: 4
        })
    ));
    withdrawn.attempts.insert(4);
    assert_eq!(project(&attempts, &proposals, &withdrawn), None);
    withdrawn.attempts.insert(5);
    assert_eq!(project(&attempts, &proposals, &withdrawn).unwrap().due, 36);
}

#[test]
fn a_shared_source_retracts_only_the_objectives_whose_support_depends_on_it() {
    let (mut attempts, mut proposals) = fixture();
    let second_outcome = OutcomeId(8);
    let mut first = attempt(10, 10, Question::Practice);
    first.outcome = second_outcome;
    let mut check = attempt(11, 11, Question::ChangedExample);
    check.outcome = second_outcome;
    check.sources.insert([3; 32]);
    attempts.extend([first, check]);
    proposals.push(CheckpointProposal {
        outcome: second_outcome,
        ..pair(10, 11, 25)
    });
    let mut affected = Withdrawn::new(SCOPE);
    affected.sources.insert([3; 32]);
    affected.postponed.insert(second_outcome);
    let progress = reconcile(SCOPE, 30, &attempts, &proposals, &affected).unwrap();
    assert_eq!(progress.checkpoints.len(), 1);
    assert_eq!(progress.checkpoints[&OUTCOME].due, 7);
    assert_eq!(progress.postponed, BTreeSet::from([second_outcome]));
    affected.sources.clear();
    affected.attempts.insert(1);
    let progress = reconcile(SCOPE, 30, &attempts, &proposals, &affected).unwrap();
    assert_eq!(progress.checkpoints[&OUTCOME].due, 36);
    assert_eq!(progress.checkpoints[&second_outcome].due, 25);
}

#[test]
fn requalification_requires_two_supported_performances_after_a_substantive_miss() {
    let mut attempts = vec![
        attempt(1, 0, Question::Practice),
        attempt(2, 0, Question::ChangedExample),
        attempt(3, 1, Question::IndependentCheck),
        attempt(4, 14, Question::DelayedReview),
    ];
    attempts[1].judgment = Judgment::Incorrect;
    let withdrawn = Withdrawn::new(SCOPE);
    assert_eq!(project(&attempts, &[pair(1, 4, 28)], &withdrawn), None);
    assert_eq!(
        project(&attempts, &[pair(3, 4, 28)], &withdrawn)
            .unwrap()
            .due,
        28
    );
}
