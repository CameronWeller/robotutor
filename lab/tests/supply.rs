use std::collections::BTreeSet;
use tutor_lab::curriculum::{OutcomeId, Span};
use tutor_lab::history::Scope;
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::supply::{
    select, Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable,
};

#[test]
fn a_projected_solution_disqualifies_translations_and_related_transfer_forms() {
    let scope = Scope {
        learner: 42,
        revision: 2,
    };
    let seat = Enrollment { scope, class: 17 };
    let target = OutcomeId(1);
    let bytes = UNIT_RULE.as_bytes();
    let pin = Span {
        document: document_id(bytes),
        start: 0,
        end: bytes.len() as u32,
    };
    let active = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(bytes),
        pin,
        revoked: &active,
    };
    let first = Form {
        class: seat.class,
        release: 2,
        outcome: target,
        owner: None,
        revision: 11,
        task: 101,
        family: 1,
        source: pin,
        card: FractionCard::propose(FractionTask::Sum, 2, 3, 8).unwrap(),
    };
    let translation = Form {
        revision: 12,
        ..first
    };
    let variant = Form {
        revision: 13,
        task: 102,
        card: FractionCard::propose(FractionTask::Sum, 1, 5, 8).unwrap(),
        ..first
    };
    let changed = Form {
        revision: 14,
        task: 103,
        family: 2,
        card: FractionCard::propose(FractionTask::MissingPart, 1, 2, 6).unwrap(),
        ..first
    };
    let catalog = [translation, variant, changed];
    let mut book = ExposureBook::new(seat, true);
    book.record(seat, &first, Contact::Possible).unwrap();
    let choose = |requirement| {
        select(seat, target, requirement, &book, &catalog, &source)
            .unwrap()
            .revision
    };
    assert_eq!(choose(Requirement::Practice), 12);
    assert_eq!(choose(Requirement::FreshTask), 13);
    assert_eq!(choose(Requirement::FreshFamily), 14);

    // A new opening and intact source cannot resolve missing exposure history.
    let unknown = ExposureBook::new(seat, false);
    assert_eq!(
        select(
            seat,
            target,
            Requirement::FreshTask,
            &unknown,
            &catalog,
            &source
        ),
        Err(Unavailable::NoFreshForm)
    );
    let other = Scope {
        learner: 43,
        ..scope
    };
    assert_eq!(
        book.record(
            Enrollment {
                scope: other,
                ..seat
            },
            &first,
            Contact::Delivered
        ),
        Err(Unavailable::Scope)
    );
    let revoked = BTreeSet::from([pin.document]);
    let held = RuleSource {
        revoked: &revoked,
        ..source
    };
    assert_eq!(
        select(seat, target, Requirement::Practice, &book, &catalog, &held),
        Err(Unavailable::NoSupportedForm)
    );
}

#[test]
fn migrating_a_split_task_blocks_plausible_aliases_without_importing_old_answers() {
    use std::collections::BTreeMap;
    use tutor_lab::supply::ExposureCrosswalk;

    let first = Enrollment {
        class: 3,
        scope: Scope {
            learner: 42,
            revision: 20,
        },
    };
    let next = Enrollment {
        scope: Scope {
            revision: 21,
            ..first.scope
        },
        ..first
    };
    let target = OutcomeId(10);
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let active = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &active,
    };
    let old = Form {
        class: first.class,
        release: first.scope.revision,
        outcome: target,
        owner: None,
        revision: 90,
        task: 5,
        family: 2,
        source: pin,
        card: FractionCard::propose(FractionTask::Sum, 2, 3, 8).unwrap(),
    };
    let alias = Form {
        release: next.scope.revision,
        revision: 91,
        task: 10,
        family: 7,
        ..old
    };
    let unused = Form {
        revision: 92,
        task: 12,
        card: FractionCard::propose(FractionTask::Sum, 2, 4, 8).unwrap(),
        ..alias
    };
    let mut history = ExposureBook::new(first, true);
    history.record(first, &old, Contact::Delivered).unwrap();
    let mut crosswalk = ExposureCrosswalk {
        class: first.class,
        earlier_release: 20,
        later_release: 21,
        tasks: BTreeMap::from([(5, BTreeSet::from([10, 11]))]),
        families: BTreeMap::from([(2, BTreeSet::from([7]))]),
    };
    let book = history.carry_forward(next, &crosswalk).unwrap();
    let shelf = [alias, unused];
    assert_eq!(book.exposure(10), Some(Contact::Possible));
    assert_eq!(book.exposure(11), Some(Contact::Possible));
    assert_eq!(
        select(next, target, Requirement::FreshTask, &book, &shelf, &source),
        Ok(&unused)
    );
    assert_eq!(
        select(
            next,
            target,
            Requirement::FreshFamily,
            &book,
            &shelf,
            &source
        ),
        Err(Unavailable::NoFreshForm)
    );
    let other = Enrollment { class: 4, ..next };
    assert!(matches!(
        history.carry_forward(other, &crosswalk),
        Err(Unavailable::Scope)
    ));

    // A missing entry fences negative lookups. An explicitly reviewed retirement can clear it.
    crosswalk.tasks.remove(&5);
    let gap = history.carry_forward(next, &crosswalk).unwrap();
    assert_eq!(
        select(next, target, Requirement::FreshTask, &gap, &shelf, &source),
        Err(Unavailable::NoFreshForm)
    );
    assert_eq!(
        select(next, target, Requirement::Practice, &gap, &shelf, &source),
        Ok(&alias)
    );
    crosswalk.tasks.insert(5, BTreeSet::new());
    let retired = history.carry_forward(next, &crosswalk).unwrap();
    assert_eq!(
        select(
            next,
            target,
            Requirement::FreshTask,
            &retired,
            &shelf,
            &source
        ),
        Ok(&alias)
    );
}
