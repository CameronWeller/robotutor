use serde_json::Value;
use tutor_lab::{
    protocol::Assistance,
    rehearsal::{Action, Session},
};

// A fresh Session is populated only by replaying the events which were
// returned before the caller disconnected.
fn reopened(events: &[Value]) -> Session {
    let mut session = Session::new().unwrap();
    for event in events {
        session.replay(event).unwrap();
    }
    session
}

#[test]
fn the_missed_return_and_voluntary_changed_work_are_distinct_on_reopen() {
    let mut course = Session::new().unwrap();
    let (open, _) = course.apply(Action::Open(30)).unwrap();
    let (answer, original) = course
        .apply(Action::Answer("1/16".into(), Assistance::NoneReported))
        .unwrap();
    assert_eq!(original["judgment"], "Incorrect");
    assert_eq!(original["independent"], true);
    let (offer, changed) = course.apply(Action::Diagnostic).unwrap();
    assert!(changed["display"].as_str().unwrap().contains("nine books"));
    assert_eq!(changed["original"]["learner"], 42);
    let (hint, _) = course.apply(Action::DiagnosticHelp).unwrap();
    let mut later = reopened(&[open, answer, offer, hint]);
    let (_, post) = later
        .apply(Action::DiagnosticAnswer(
            "5 books".into(),
            Assistance::NoneReported,
        ))
        .unwrap();
    assert_eq!(post["diagnostic"]["observation"]["judgment"], "Correct");
    assert_eq!(post["diagnostic"]["observation"]["assistance"], "Reported");
    assert_eq!(post["original"]["judgment"], original["judgment"]);
    assert_eq!(post["original_attempt"], changed["original_attempt"]);
    assert!(post["original_attempt"].is_u64());
    assert!(post["credit"].is_null());
    assert_eq!(post["planner"], original["planner"]);
    assert!(later
        .apply(Action::DiagnosticAnswer(
            "5 books".into(),
            Assistance::Reported
        ))
        .is_err());
}

#[test]
fn help_unknown_or_a_skipped_response_never_becomes_an_independent_course_finding() {
    let mut course = Session::new().unwrap();
    course.apply(Action::Open(30)).unwrap();
    course
        .apply(Action::Answer("1/16".into(), Assistance::Unknown))
        .unwrap();
    assert!(course.apply(Action::Diagnostic).is_err());

    let mut course = Session::new().unwrap();
    course.apply(Action::Open(30)).unwrap();
    course
        .apply(Action::Answer("1/16".into(), Assistance::NoneReported))
        .unwrap();
    course.apply(Action::Diagnostic).unwrap();
    let (_, skipped) = course.apply(Action::DiagnosticSkip).unwrap();
    assert!(skipped["diagnostic"]["observation"].is_null());
    assert!(skipped["credit"].is_null());
    assert!(course.apply(Action::DiagnosticHelp).is_err());
}

#[test]
fn invalidation_between_diagnostic_presentation_and_answer_refuses_the_old_task() {
    let mut original = Session::new().unwrap();
    let events = [
        Action::Open(30),
        Action::Answer("1/16".into(), Assistance::NoneReported),
        Action::Diagnostic,
        Action::WithdrawSource,
    ]
    .into_iter()
    .map(|a| original.apply(a).unwrap().0)
    .collect::<Vec<_>>();
    let mut after = reopened(&events);
    assert!(after
        .apply(Action::DiagnosticAnswer(
            "5 books".into(),
            Assistance::NoneReported
        ))
        .is_err());
    assert!(after.apply(Action::Diagnostic).is_err());
}
