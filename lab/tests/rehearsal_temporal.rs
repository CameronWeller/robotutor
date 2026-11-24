//! Cross-session safety on observed learner output, checked independently of
//! the course reducer. A reopened process sees only the persisted events.
use serde_json::{json, Value};
use std::collections::BTreeSet;
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    protocol::Assistance,
    rehearsal::{Action, Session},
};

#[derive(Clone)]
struct Observation {
    event: Value,
    receipt: Value,
}

fn history(actions: Vec<Action>) -> Vec<Observation> {
    let mut saved: Vec<Observation> = Vec::new();
    for action in actions {
        let mut restored = Session::new().unwrap();
        for step in &saved {
            restored.replay(&step.event).unwrap();
        }
        let (event, receipt) = restored.apply(action).unwrap();
        saved.push(Observation { event, receipt });
    }
    saved
}

struct Check {
    opening_authorized: bool,
    credit_supported: bool,
    contacts_persist: bool,
}

fn monitor() -> Monitor<Check> {
    let mut predicates = Predicates::builder();
    let allowed = predicates.new_predicate(ClosurePredicate::new(
        |s: &Check| s.opening_authorized,
        "show course item only under current authority",
    ));
    let supported = predicates.new_predicate(ClosurePredicate::new(
        |s: &Check| s.credit_supported,
        "award only for a fresh, unassisted answer to the current opening",
    ));
    let contact = predicates.new_predicate(ClosurePredicate::new(
        |s: &Check| s.contacts_persist,
        "confirmed display stays in the history",
    ));
    let props = Property::always(
        Property::atomic(allowed) & Property::atomic(supported) & Property::atomic(contact),
    );
    let predicates = predicates.build();
    let machine = props.to_monitoring_automaton(&predicates);
    Monitor::new(predicates, machine)
}

// The authority changes and help disclosures below come from the submitted
// actions. They don't rely on the dispatcher's own verdict about its authority.
fn matches_fixture_key(task: Option<u64>, response: &Value) -> bool {
    // In these B histories the learner is asked for what remains from 5/16
    // after removing 3/16, or from 8/16 after removing 3/16.
    let expected = match task {
        Some(1) => 2,
        Some(2) => 5,
        _ => return false,
    };
    let Some((n, d)) = response.as_str().and_then(|s| s.trim().split_once('/')) else {
        return false;
    };
    match (n.parse::<u32>(), d.parse::<u32>()) {
        (Ok(n), Ok(d)) if d != 0 => u64::from(n) * 16 == u64::from(d) * expected,
        _ => false,
    }
}

fn violation(trace: &[Observation]) -> Option<usize> {
    let mut checker = monitor();
    let (mut active_source, mut grant) = (true, true);
    let mut previous_tasks = BTreeSet::new();
    let mut known_questions = BTreeSet::new();
    let mut open = None;
    for (index, step) in trace.iter().enumerate() {
        let op = step.event[0].as_str().unwrap();
        let out = &step.receipt;
        match op {
            "withdraw" => active_source = false,
            "replace" => active_source = true,
            "revoke" => grant = false,
            "reissue" => grant = true,
            _ => {}
        }
        if matches!(
            op,
            "withdraw" | "replace" | "revoke" | "reissue" | "work" | "unknown" | "recover"
        ) || (op == "course-aid" && step.event[1] != "format")
        {
            open = None;
        }
        let task = out["task"].as_u64();
        let display = out["display"].as_str();
        let course_open = out["contribution"] == "course_opening"
            || display.is_some_and(|s| known_questions.contains(s));
        let opening_authorized = !course_open
            || (grant
                && active_source
                && out["opened"] == true
                && display.is_some_and(|s| !s.is_empty())
                && task.is_some());
        if course_open {
            if let Some(s) = display {
                known_questions.insert(s.to_owned());
            }
            open = task.filter(|id| !previous_tasks.contains(id));
            if let Some(id) = task {
                previous_tasks.insert(id);
            }
        }
        let credit_supported = out["credit"].is_null()
            || (op == "answer"
                && step.event[2] == "none"
                && open.is_some()
                && open == task
                && grant
                && active_source
                && matches_fixture_key(task, &step.event[1])
                && out["judgment"] == "Correct");
        if op == "answer" {
            open = None;
        }
        let recorded: BTreeSet<_> = out["contacted_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        let check = Check {
            opening_authorized,
            credit_supported,
            contacts_persist: previous_tasks.is_subset(&recorded),
        };
        if !checker.next_state(&check) {
            return Some(index);
        }
    }
    None
}

#[test]
fn conversational_input_and_format_help_keep_the_original_numeric_opportunity() {
    use Action::*;
    for aid in ["format", "worked", "source"] {
        let private = "I pasted the private newsletter profile here by mistake.";
        let mut trace = history(vec![
            Open(30),
            Reply(private.into(), Assistance::NoneReported),
            CourseAid(aid.into()),
            Reply("1/8".into(), Assistance::NoneReported),
        ]);
        assert_eq!(trace[1].event, json!(["reply-options"]));
        assert!(!trace[1].receipt.to_string().contains(private));
        assert_eq!(
            trace[1].receipt["current_opening"],
            trace[0].receipt["current_opening"]
        );
        assert_eq!(trace[3].receipt["judgment"], "Correct");
        assert_eq!(trace[3].receipt["credit"].is_string(), aid == "format");
        assert_eq!(violation(&trace), None);
        if aid != "format" {
            trace[3].receipt["credit"] = json!("independent_return");
            assert_eq!(violation(&trace), Some(3));
        }
    }
}

#[test]
fn a_changed_authority_or_private_detour_cannot_supply_return_credit() {
    use Action::*;
    let answer = || Answer("2/16".into(), Assistance::NoneReported);
    let direct = history(vec![Open(30), answer()]);
    assert_eq!(
        direct.last().unwrap().receipt["credit"],
        "independent_return"
    );
    assert_eq!(direct[1].receipt["pending"], false);
    assert_eq!(violation(&direct), None);
    let mut fabricated = direct.clone();
    fabricated[1].event[1] = json!("0/16");
    assert_eq!(violation(&fabricated), Some(1));
    for change in [
        WithdrawSource,
        ReplaceSource,
        RevokeGrant,
        ReissueGrant,
        UnknownContact,
    ] {
        let actual = history(vec![Open(30), change, answer()]);
        assert!(actual[2].receipt["credit"].is_null());
        assert_eq!(violation(&actual), None);
        let mut forged = actual;
        forged[2].receipt["task"] = json!(1);
        forged[2].receipt["judgment"] = json!("Correct");
        forged[2].receipt["credit"] = json!("independent_return");
        assert_eq!(violation(&forged), Some(2));
    }
    let mut detour = history(vec![
        Open(30),
        Work("A private sentence for the newsletter.".into()),
        Resume(30),
        answer(),
    ]);
    assert_eq!(detour[1].event, json!(["work"]));
    assert!(detour[3].receipt["credit"].is_null());
    assert_eq!(violation(&detour), None);
    detour[3].receipt["credit"] = json!("independent_return");
    assert_eq!(violation(&detour), Some(3));
}

#[test]
fn exposure_survives_revocation_and_no_display_can_be_invented() {
    let mut trace = history(vec![
        Action::Open(30),
        Action::WithdrawSource,
        Action::Resume(31),
    ]);
    assert_eq!(violation(&trace), None);
    let genuine = trace[1].receipt.clone();
    trace[1].receipt["contacted_tasks"] = json!([]);
    assert_eq!(violation(&trace), Some(1));
    trace[1].receipt = genuine;
    let claimed_denial = trace[2].receipt.clone();
    trace[2].receipt["display"] = trace[0].receipt["display"].clone();
    assert_eq!(violation(&trace), Some(2));
    trace[2].receipt = claimed_denial;
    trace[2].receipt["opened"] = json!(true);
    trace[2].receipt["contribution"] = json!("course_opening");
    trace[2].receipt["task"] = json!(1);
    assert_eq!(violation(&trace), Some(2));
}

#[test]
fn recovery_and_live_submission_agree_at_every_committed_boundary() {
    let submitted = history(vec![
        Action::UseVariedInstances,
        Action::Open(30),
        Action::Work("A private sentence about the neighborhood.".into()),
        Action::WithdrawSource,
        Action::Resume(730),
        Action::ReplaceSource,
        Action::UnknownContact,
        Action::RecoverContact,
        Action::Resume(730),
    ]);
    let mut recovered = Session::new().unwrap();
    let mut live = Session::new().unwrap();
    for step in submitted {
        live.replay(&step.event).unwrap();
        recovered = recovered.restore_next(&step.event).unwrap();
        assert_eq!(live.status().unwrap(), recovered.status().unwrap());
    }
    let invalid = json!(["open", 5]);
    let old = live.status().unwrap();
    assert!(recovered.restore_next(&invalid).is_err());
    assert!(live.replay(&invalid).is_err());
    assert_eq!(live.status().unwrap(), old);
}
