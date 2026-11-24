//! Read the existing class projection for a shared course and a learner-owned form.
use std::collections::BTreeSet;
use tutor_lab::audience::{
    teacher_view, Assignment, AssignmentKey, Roster, TeacherAction, ViewError,
};
use tutor_lab::curriculum::{OutcomeId, Span};
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::Question;
use tutor_lab::supply::Form;

pub(super) fn project() {
    let roster = Roster {
        class: 9,
        teachers: [1].into(),
        learners: [2, 3].into(),
    };
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let mut shared = Assignment {
        key: AssignmentKey {
            class: 9,
            activity: 7,
        },
        publication: 1,
        form: Form {
            class: 9,
            release: 1,
            outcome: OutcomeId(1),
            revision: 1,
            task: 1,
            family: 1,
            owner: None,
            card: FractionCard::propose(FractionTask::Sum, 1, 2, 4).expect("valid example"),
            source: pin,
        },
        question: Question::RequestedProbe,
        assigned: [(2, 1), (3, 2)].into(),
        held: Default::default(),
        corrected: false,
        returns: Default::default(),
    };
    let revoked = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &revoked,
    };
    let view =
        teacher_view(1, &roster, &shared, &[], &source).expect("teacher owns shared assignment");
    assert_eq!(view.counts.get(&TeacherAction::InviteToStart), Some(&2));
    assert!(view.follow_up.is_empty());
    shared.form.owner = Some(2);
    assert_eq!(
        teacher_view(1, &roster, &shared, &[], &source),
        Err(ViewError::AccessDenied)
    );
    println!("shared course: 2 invitations and 0 individual follow-ups; teacher preview of a private form: denied");
}
