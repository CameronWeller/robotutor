//! Exhaustive model checking of the orchestrator with `stateright`.
//! Every interleaving of learner, grader, pause/resume, retraction and time inputs on a small
//! goal, up to a bounded ledger length, is explored breadth-first. The invariants are the
//! safety halves of the LTLf spec plus backward-feedback completeness.

use robotutor_core::atlas::Goal;
use robotutor_core::ledger::{project, weight, Agreement, Assist, Body, FamilyId};
use robotutor_core::policy::Params;
use robotutor_core::session::{Input, Output, Phase, Tutor};
use stateright::{Checker, Model, Property};
use std::collections::BTreeSet;

const GOAL: &str = r#"{
  "name": "tiny", "statement": "tiny", "regime": "Assessable", "targets": [2],
  "nodes": [
    {"id": 1, "name": "a", "misconceptions": ["m"], "sources": ["s"]},
    {"id": 2, "name": "b", "prereqs": [1], "sources": ["s"]}
  ],
  "families": [
    {"id": 10, "node": 1, "facet": "Apply", "checker": "Exact", "cue": "None", "variants": 2, "probes_misconception": 0, "stem": "q", "key": "1"},
    {"id": 11, "node": 1, "facet": "Apply", "checker": "Rubric", "cue": "Partial", "variants": 2, "stem": "q", "key": "k"},
    {"id": 20, "node": 2, "facet": "Apply", "checker": "Exact", "cue": "None", "variants": 2, "stem": "q", "key": "2"}
  ]
}"#;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct S {
    t: Tutor,
    presented: Option<FamilyId>,
    spoke_while_paused: bool,
    owed_dropped: bool,
}

struct M {
    goal: Goal,
    p: Params,
    max_events: usize,
}

fn dead(t: &Tutor) -> BTreeSet<u64> {
    t.ledger
        .iter()
        .filter_map(|e| match &e.body {
            Body::Supersede { target, .. } => Some(*target),
            _ => None,
        })
        .collect()
}

impl Model for M {
    type State = S;
    type Action = Input;

    fn init_states(&self) -> Vec<S> {
        vec![S { t: Tutor::new(1), presented: None, spoke_while_paused: false, owed_dropped: false }]
    }

    fn actions(&self, _: &S, a: &mut Vec<Input>) {
        a.extend([
            Input::Open,
            Input::Next,
            Input::Answer { assist: Assist::None },
            Input::Answer { assist: Assist::Unknown },
            Input::Graded { correct: Some(true), agreement: Agreement::Agree, hyps: vec![] },
            Input::Graded { correct: Some(false), agreement: Agreement::Checker, hyps: vec![0] },
            Input::Graded { correct: Some(true), agreement: Agreement::Disagree, hyps: vec![] },
            Input::GraderTimeout,
            Input::Detour,
            Input::Pause,
            Input::Resume,
            Input::Retract(10),
            Input::Retract(20),
            Input::Wait(4 * 1440),
            Input::Close,
        ]);
    }

    fn next_state(&self, s: &S, a: Input) -> Option<S> {
        let mut t = s.t.clone();
        let was_paused = t.paused;
        let had_owed = t.owed.is_some();
        let outs = t.step(&self.goal, &self.p, &a);
        if outs.iter().any(|o| matches!(o, Output::Rejected(_))) {
            return None;
        }
        let fed = outs.iter().any(|o| matches!(o, Output::Feedback { .. }));
        let spoke = outs
            .iter()
            .any(|o| matches!(o, Output::Present { .. } | Output::Teach { .. } | Output::Feedback { .. }));
        Some(S {
            presented: outs.iter().find_map(|o| match o {
                Output::Present { item, .. } => Some(item.family),
                _ => None,
            }),
            spoke_while_paused: was_paused && t.paused && spoke,
            owed_dropped: had_owed && t.owed.is_none() && !fed,
            t,
        })
    }

    fn within_boundary(&self, s: &S) -> bool {
        // Time is unbounded otherwise: allow at most two multi-day waits.
        s.t.ledger.len() <= self.max_events && s.t.now <= 2 * 4 * 1440
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("L1 never present a retracted family", |_, s: &S| {
                s.presented.is_none_or(|f| !s.t.quarantined.contains(&f))
            }),
            Property::always("open item is never from a retracted family", |_, s: &S| match &s.t.phase {
                Phase::Awaiting(i) => !s.t.quarantined.contains(&i.family),
                _ => true,
            }),
            Property::always("L4 silent while paused", |_, s: &S| !s.spoke_while_paused),
            Property::always("L5 feedback debt is never dropped", |_, s: &S| !s.owed_dropped),
            Property::always("retraction is complete", |_, s: &S| {
                let d = dead(&s.t);
                s.t.ledger.iter().all(|e| match &e.body {
                    Body::Attempt(a) => !(s.t.quarantined.contains(&a.family) && weight(a) > 0.0 && !d.contains(&e.id)),
                    _ => true,
                })
            }),
            Property::always("L2/L6 helped answers carry no weight", |_, s: &S| {
                s.t.ledger.iter().all(|e| match &e.body {
                    Body::Attempt(a) => a.assist == Assist::None || weight(a) == 0.0,
                    _ => true,
                })
            }),
            Property::always("lost support is queued for re-check", |m: &M, s: &S| {
                let raw: Vec<_> =
                    s.t.ledger.iter().filter(|e| !matches!(e.body, Body::Supersede { .. })).cloned().collect();
                let before = project(&raw);
                let after = project(&s.t.ledger);
                m.goal
                    .nodes
                    .iter()
                    .all(|n| !(before.node_shown(n.id) && !after.node_shown(n.id)) || s.t.c.recheck.contains(&n.id))
            }),
            Property::sometimes("an answer earns evidence", |_, s: &S| {
                s.t.ledger.iter().any(|e| matches!(&e.body, Body::Attempt(a) if weight(a) > 0.0))
            }),
            Property::sometimes("a retraction supersedes evidence", |_, s: &S| {
                s.t.ledger.iter().any(|e| matches!(e.body, Body::Supersede { .. }))
            }),
            Property::sometimes("feedback is owed while paused", |_, s: &S| s.t.paused && s.t.owed.is_some()),
            Property::sometimes("a side question taints an open item", |_, s: &S| {
                matches!(&s.t.phase, Phase::Awaiting(i) if i.assisted)
            }),
        ]
    }
}

#[test]
fn orchestrator_invariants_hold_on_short_interleavings() {
    check_interleavings(Some(10));
}

#[test]
#[ignore = "exhaustive seven-event search requires a high-memory workstation"]
fn orchestrator_invariants_hold_on_every_interleaving() {
    check_interleavings(None);
}

fn check_interleavings(depth: Option<usize>) {
    let goal = Goal::load(GOAL).unwrap();
    let m = M { goal, p: Params::default(), max_events: 7 };
    // Many pause, resume, close and reopen inputs can occur without growing the ledger.
    let mut checker = m.checker();
    if let Some(limit) = depth {
        checker = checker.target_max_depth(limit);
    }
    let c = checker.spawn_bfs().join();
    println!("states: {}, unique: {}", c.state_count(), c.unique_state_count());
    c.assert_properties();
}
