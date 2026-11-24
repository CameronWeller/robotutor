use serde_json::Value;
use tutor_lab::curriculum::{
    creation::MAX_ROWS,
    program::{Frame, Issue, Proposal},
};

fn survey() -> (Frame, Proposal, u32) {
    let law: Value = serde_json::from_str(include_str!("../fixtures/law-survey.json")).unwrap();
    let frame = Frame::from_json(&law).unwrap();
    let proposal = Proposal::from_json(&law["proposal"]).unwrap();
    let calendar = frame.calendar.unwrap();
    (frame, proposal, calendar.forward_minutes())
}

#[test]
fn law_creation_follows_the_forecast_even_when_a_unit_spans_sessions() {
    let (frame, proposal, budget) = survey();
    let window = |elapsed, width| {
        frame
            .creation_window(&proposal, budget, elapsed, width, MAX_ROWS)
            .unwrap()
    };
    assert_eq!(window(0, 160), [101, 102]);
    assert_eq!(window(0, 40), [101]);
    assert_eq!(window(39, 2), [101, 102]);
    // At the start of the third forward session the philosophy unit still has 80 minutes.
    assert_eq!(window(80, 40), [102]);
    assert_eq!(window(159, 1), [102]);
    assert_eq!(window(159, 2), [102, 103]);
    assert_eq!(window(160, 1), [103]);
    // A 40-minute full forecast defers the entire second unit, regardless of lookahead.
    assert_eq!(
        frame
            .creation_window(&proposal, 40, 0, 160, MAX_ROWS)
            .unwrap(),
        [101]
    );
}

#[test]
fn empty_terminal_and_overflowing_intervals_respect_forecast_boundaries() {
    let (frame, proposal, budget) = survey();
    let schedule = frame.sequence(&proposal, budget).unwrap();
    let final_minute = schedule.used_minutes - 1;
    let window = |elapsed, width| {
        frame
            .creation_window(&proposal, budget, elapsed, width, MAX_ROWS)
            .unwrap()
    };
    assert!(window(80, 0).is_empty());
    assert!(window(schedule.used_minutes, 1).is_empty());
    assert!(window(u32::MAX, u32::MAX).is_empty());
    assert_eq!(
        window(final_minute, u32::MAX),
        [*schedule.scheduled.last().unwrap()]
    );
    assert!(frame
        .creation_window(&proposal, 0, 0, u32::MAX, MAX_ROWS)
        .unwrap()
        .is_empty());
}

#[test]
fn selection_respects_the_callers_budget_and_the_authoring_batch_limit() {
    let (frame, proposal, budget) = survey();
    let scheduled = frame.sequence(&proposal, budget).unwrap().scheduled;
    assert!(scheduled.len() > MAX_ROWS);
    for (requested, expected) in [(0, 0), (1, 1), (3, 3), (usize::MAX, MAX_ROWS)] {
        let ids = frame
            .creation_window(&proposal, budget, 0, budget, requested)
            .unwrap();
        assert_eq!(ids, scheduled[..expected]);
    }
}

#[test]
fn invalid_outline_is_reported_even_for_an_empty_window() {
    let (frame, proposal, budget) = survey();
    let mut broken = proposal.wire();
    broken["units"][1]["id"] = broken["units"][0]["id"].clone();
    let broken = Proposal::from_json(&broken).unwrap();
    let issues = frame.creation_window(&broken, budget, 0, 0, 0).unwrap_err();
    assert!(issues.contains(&Issue::DuplicateUnit(101)));
}

#[test]
fn both_forecast_entry_points_protect_the_reserved_hours() {
    let (frame, proposal, forward) = survey();
    let calendar = frame.calendar.unwrap();
    assert_eq!(calendar.forward_within(1080), Some(840));
    assert_eq!(calendar.forward_within(1), Some(0));
    assert_eq!(calendar.forward_within(4321), None);
    let excess = forward + 1;
    let expected = vec![Issue::CalendarExceeded {
        requested: excess,
        available: forward,
    }];
    assert_eq!(frame.sequence(&proposal, excess).unwrap_err(), expected);
    assert_eq!(
        frame
            .creation_window(&proposal, excess, 0, 160, 16)
            .unwrap_err(),
        expected
    );
}
