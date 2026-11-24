use std::collections::BTreeSet;
use tutor_lab::{
    curriculum::OutcomeId,
    history::{Attempt, Scope, Withdrawn},
    protocol::{Assistance, Judgment, Question},
    retention::{read, Cell, CheckResult, Exposure, Moment, Prior, ReadError},
};

const CELL: Cell = Cell {
    scope: Scope {
        learner: 9,
        revision: 17,
    },
    outcome: OutcomeId(11),
};

fn at(day: u32) -> Moment {
    Moment {
        day,
        sequence: u64::from(day) + 1,
    }
}

fn prior() -> Prior {
    Prior {
        cell: CELL,
        verified: at(0),
        stability_days: 3.0,
        threshold: 0.85,
        had_durable_success: false,
    }
}

fn response(day: u32, judgment: Judgment) -> Attempt {
    Attempt {
        scope: CELL.scope,
        id: 100 + u64::from(day),
        sequence: at(day).sequence,
        day,
        outcome: CELL.outcome,
        item: 700,
        question: Question::DelayedReview,
        judgment,
        assistance: Assistance::NoneReported,
        fresh: true,
        sources: BTreeSet::from([[7; 32]]),
    }
}

#[test]
fn day_three_day_five_and_first_observation_match_the_reference_cases() {
    let active = Withdrawn::new(CELL.scope);
    for (day, expected, due) in [
        (3, CheckResult::InsufficientSpacing, false),
        (5, CheckResult::SupportsDurability, true),
    ] {
        let (f, result) = read(
            CELL,
            at(day),
            None,
            Some(prior()),
            Some(&[]),
            Some(&response(day, Judgment::Correct)),
            &active,
        )
        .unwrap();
        let f = f.unwrap();
        assert_eq!((result, f.refresh_due), (expected, due));
        assert!((f.required_gap_days - 4.764705882352942).abs() < 1e-10);
    }
    let out = read(
        CELL,
        at(0),
        None,
        None,
        Some(&[]),
        Some(&response(0, Judgment::Correct)),
        &active,
    )
    .unwrap();
    assert_eq!(out, (None, CheckResult::InsufficientSpacing));
}

#[test]
fn an_assisted_encounter_restarts_the_evidence_gap_while_the_model_clock_keeps_running() {
    let active = Withdrawn::new(CELL.scope);
    let check = response(30, Judgment::Correct);
    let current = Exposure {
        cell: CELL,
        at: at(30),
    };
    let guided = Exposure {
        cell: CELL,
        at: at(25),
    };
    let compare = |exposures: &[Exposure]| {
        let (forecast, check) = read(
            CELL,
            at(30),
            Some(395),
            Some(prior()),
            Some(exposures),
            Some(&check),
            &active,
        )
        .unwrap();
        (forecast.unwrap(), check)
    };
    let (cold, cold_check) = compare(&[current]);
    let (warm, warm_check) = compare(&[current, guided]);
    assert_eq!(cold_check, CheckResult::SupportsDurability);
    assert_eq!(warm_check, CheckResult::InsufficientSpacing);
    assert_eq!(
        (cold.days_since_exposure, warm.days_since_exposure),
        (Some(30), Some(5))
    );
    assert_eq!(cold.recall, warm.recall);
    assert!((cold.recall - 27.0 / 57.0).abs() < 1e-12);
    assert!(warm.refresh_due && cold.refresh_due);
    assert!((23.0..25.0).contains(&warm.required_gap_days));
    assert_eq!(
        read(
            CELL,
            at(30),
            Some(395),
            Some(prior()),
            Some(&[guided]),
            None,
            &active
        )
        .unwrap()
        .1,
        CheckResult::NoCheck
    );
}

#[test]
fn early_failures_remain_visible_and_assisted_or_retracted_grades_cannot_count() {
    let mut active = Withdrawn::new(CELL.scope);
    let mut miss = response(2, Judgment::Incorrect);
    let supported = Prior {
        had_durable_success: true,
        ..prior()
    };
    let classify = |p, attempt: &Attempt, active: &Withdrawn| {
        read(CELL, at(2), None, Some(p), Some(&[]), Some(attempt), active)
            .unwrap()
            .1
    };
    assert_eq!(
        classify(prior(), &miss, &active),
        CheckResult::ReviewFailure
    );
    assert_eq!(
        classify(supported, &miss, &active),
        CheckResult::OpposesDurability
    );
    for help in [Assistance::Reported, Assistance::Unknown] {
        miss.assistance = help;
        assert_eq!(classify(supported, &miss, &active), CheckResult::Ineligible);
    }
    miss.assistance = Assistance::NoneReported;
    active.sources.insert([7; 32]);
    assert_eq!(classify(supported, &miss, &active), CheckResult::Ineligible);
}

#[test]
fn reject_bad_parameters_future_inputs_and_cross_cell_records() {
    let active = Withdrawn::new(CELL.scope);
    let check = response(30, Judgment::Correct);
    let error = |p: Option<Prior>, exposures: &[Exposure], a: &Attempt| {
        read(CELL, at(30), None, p, Some(exposures), Some(a), &active).unwrap_err()
    };
    for stability_days in [0.0, -1.0, 3651.0, f64::NAN, f64::INFINITY] {
        let bad = Prior {
            stability_days,
            ..prior()
        };
        assert_eq!(error(Some(bad), &[], &check), ReadError::InvalidSnapshot);
    }
    for threshold in [0.0, 1.0, f64::NAN] {
        let bad = Prior {
            threshold,
            ..prior()
        };
        assert_eq!(error(Some(bad), &[], &check), ReadError::InvalidSnapshot);
    }
    let future = Exposure {
        cell: CELL,
        at: at(31),
    };
    assert_eq!(error(None, &[future], &check), ReadError::InvalidChronology);
    for verified in [at(30), at(31)] {
        let bad = Prior {
            verified,
            ..prior()
        };
        assert_eq!(error(Some(bad), &[], &check), ReadError::InvalidChronology);
    }
    let past_answer = response(29, Judgment::Correct);
    assert_eq!(
        error(Some(prior()), &[], &past_answer),
        ReadError::InvalidChronology
    );
    let other = Cell {
        outcome: OutcomeId(12),
        ..CELL
    };
    let foreign = Prior {
        cell: other,
        ..prior()
    };
    let elsewhere = Exposure {
        cell: other,
        at: at(1),
    };
    let wrong_answer = Attempt {
        outcome: other.outcome,
        ..check.clone()
    };
    assert_eq!(error(Some(foreign), &[], &check), ReadError::WrongCell);
    assert_eq!(error(None, &[elsewhere], &check), ReadError::WrongCell);
    assert_eq!(error(None, &[], &wrong_answer), ReadError::WrongCell);
    let wrong_scope = Withdrawn::new(Scope {
        learner: 10,
        ..CELL.scope
    });
    let read_wrong = read(other, at(30), None, None, Some(&[]), None, &wrong_scope);
    assert_eq!(read_wrong, Err(ReadError::WrongCell));
}

#[test]
fn missing_coverage_preserves_the_forecast_and_suspends_durable_credit() {
    let active = Withdrawn::new(CELL.scope);
    let check = response(30, Judgment::Correct);
    let assess = |prior, coverage| {
        read(
            CELL,
            at(30),
            Some(395),
            prior,
            coverage,
            Some(&check),
            &active,
        )
        .unwrap()
    };
    let (forecast, disposition) = assess(Some(prior()), None);
    let unknown = forecast.unwrap();
    assert_eq!(disposition, CheckResult::UnknownExposure);
    assert_eq!(unknown.days_since_exposure, None);
    let (cold, cold_check) = assess(Some(prior()), Some(&[]));
    assert_eq!(cold_check, CheckResult::SupportsDurability);
    assert_eq!(unknown.recall, cold.unwrap().recall);
    assert!(unknown.refresh_due);
    let guided = Exposure {
        cell: CELL,
        at: at(0),
    };
    assert_eq!(
        assess(None, Some(&[guided])),
        (None, CheckResult::InsufficientSpacing)
    );
}
