use tutor_lab::curriculum::OutcomeId;
use tutor_lab::protocol::Question;
use tutor_lab::routing::profiles::{select, Contract, Miss, Qualification, Selection, Window};
use tutor_lab::routing::{Intent, Model, Role};

const FORM: Contract<'static> = Contract {
    release: 4,
    outcome: OutcomeId(8),
    intent: Intent::PersonalizeForm(Question::ChangedExample),
    locale: "es-MX",
    modality: "accessible-text/v2",
    schema: "fraction-parameters/v1",
};
const WINDOW: Window = Window {
    now_ms: 12,
    deadline_ms: 100,
    available_micros: 130,
    benchmark_epoch: 7,
    grant_epoch: 3,
};
const APPROVED: Qualification<'static> = Qualification {
    contract: FORM,
    role: Role::Reasoner,
    profile: 2,
    benchmark_epoch: 7,
    grant_epoch: 3,
    from_ms: 10,
    until_ms: 101,
};

fn model(revision: u64) -> Model {
    Model {
        revision,
        maximum_micros: if revision == 1 { 40 } else { 120 },
        admission_ms: 88,
        slots: 1,
        max_input_tokens: 4096,
        max_output_tokens: 64,
    }
}

#[test]
fn typed_private_form_requires_its_own_qualification_before_price_comparison() {
    let prose = Qualification {
        contract: Contract {
            intent: Intent::Ask(Question::ChangedExample),
            schema: "question-text/v1",
            ..FORM
        },
        profile: 1,
        ..APPROVED
    };
    let options = [prose, APPROVED];
    let live = |role, rev| {
        assert_eq!((role, rev), (Role::Reasoner, 2));
        Some(model(rev))
    };
    assert_eq!(
        select(
            FORM,
            Window {
                available_micros: 100,
                ..WINDOW
            },
            &options,
            live
        ),
        Err(Miss::Budget)
    );
    assert_eq!(
        select(FORM, WINDOW, &options, live),
        Ok(Selection {
            role: Role::Reasoner,
            model: model(2),
            deadline_ms: WINDOW.deadline_ms
        })
    );
    assert_eq!(
        select(
            FORM,
            Window {
                deadline_ms: 99,
                ..WINDOW
            },
            &options,
            live
        ),
        Err(Miss::Deadline)
    );
    assert_eq!(
        select(
            FORM,
            Window {
                deadline_ms: 99,
                available_micros: 100,
                ..WINDOW
            },
            &options,
            live
        ),
        Err(Miss::BudgetAndDeadline)
    );
}

#[test]
fn scope_language_schema_role_and_epochs_are_exact_before_a_live_lookup() {
    let never = |_, _| panic!("unqualified profile reached live lookup");
    for contract in [
        Contract { release: 5, ..FORM },
        Contract {
            outcome: OutcomeId(9),
            ..FORM
        },
        Contract {
            locale: "es",
            ..FORM
        },
        Contract {
            modality: "voice/v2",
            ..FORM
        },
        Contract {
            schema: "fraction-parameters/v2",
            ..FORM
        },
        Contract {
            locale: " ",
            ..FORM
        },
    ] {
        assert_eq!(
            select(contract, WINDOW, &[APPROVED], never),
            Err(Miss::Unqualified)
        );
    }
    for q in [
        Qualification {
            role: Role::Language,
            ..APPROVED
        },
        Qualification {
            from_ms: 13,
            ..APPROVED
        },
        Qualification {
            until_ms: 12,
            ..APPROVED
        },
        Qualification {
            benchmark_epoch: 6,
            ..APPROVED
        },
        Qualification {
            grant_epoch: 2,
            ..APPROVED
        },
    ] {
        assert_eq!(select(FORM, WINDOW, &[q], never), Err(Miss::Unqualified));
    }
    for intent in [Intent::GradeNumeric, Intent::Dispute] {
        assert_eq!(
            select(Contract { intent, ..FORM }, WINDOW, &[APPROVED], never),
            Err(Miss::LocalOnly)
        );
    }
}

#[test]
fn no_live_grant_changed_model_revision_or_elapsed_qualification_refuses_selection() {
    let run = |m| select(FORM, WINDOW, &[APPROVED], |_, _| m);
    assert_eq!(run(None), Err(Miss::Unavailable));
    assert_eq!(run(Some(model(3))), Err(Miss::Unavailable));
    assert_eq!(
        run(Some(Model {
            slots: 0,
            ..model(2)
        })),
        Err(Miss::Unavailable)
    );
    assert_eq!(
        run(Some(Model {
            max_input_tokens: 0,
            ..model(2)
        })),
        Err(Miss::Unavailable)
    );
    let ends_on_planned_completion = Qualification {
        until_ms: 100,
        ..APPROVED
    };
    assert_eq!(
        select(FORM, WINDOW, &[ends_on_planned_completion], |_, rev| Some(
            model(rev)
        )),
        Err(Miss::Deadline)
    );
}

#[test]
fn price_then_latency_then_revision_break_ties_independently_of_catalog_order() {
    let options = [3, 2, 1, 4].map(|profile| Qualification {
        profile,
        ..APPROVED
    });
    let live = |_, rev| {
        Some(Model {
            admission_ms: if rev == 3 { 70 } else { 50 },
            ..model(rev)
        })
    };
    assert_eq!(
        select(FORM, WINDOW, &options, live).unwrap().model.revision,
        1
    );
    // Cheap profile lost permission. Equally priced 2 and 4 beat slower 3.
    let now = |role, rev| (rev != 1).then(|| live(role, rev).unwrap());
    assert_eq!(
        select(FORM, WINDOW, &options, now).unwrap().model.revision,
        2
    );
    let mut reversed = options;
    reversed.reverse();
    assert_eq!(
        select(FORM, WINDOW, &reversed, now).unwrap().model.revision,
        2
    );
}

#[test]
fn duplicate_qualified_rows_use_the_later_effective_deadline_in_either_order() {
    let short = Qualification {
        until_ms: 30,
        ..APPROVED
    };
    let long = Qualification {
        until_ms: 40,
        ..APPROVED
    };
    let live = |_, rev| {
        Some(Model {
            admission_ms: 2,
            ..model(rev)
        })
    };
    for rows in [[short, long], [long, short]] {
        assert_eq!(select(FORM, WINDOW, &rows, live).unwrap().deadline_ms, 40);
        let short_turn = Window {
            deadline_ms: 25,
            ..WINDOW
        };
        assert_eq!(
            select(FORM, short_turn, &rows, live).unwrap().deadline_ms,
            25
        );
    }
}
