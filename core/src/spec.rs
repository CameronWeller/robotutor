//! The LTLf specification: promises every trace must keep, whoever produced it
//! (unit test, model checker, long-horizon simulation or a live session with real models).
//! Property names match docs/core/ltl-spec.md.

use crate::atlas::{Checker, Goal, Regime};
use crate::ledger::{Agreement, Assist, Attempt, Body, FamilyId, NodeId};
use crate::ltl::*;
use crate::policy::{Kind, Params};
use crate::session::{Input, Output, Tutor};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct Step {
    pub input: Input,
    pub outs: Vec<Output>,
    pub open: bool,
    pub paused: bool,
    pub owed: bool,
    pub quarantined: BTreeSet<FamilyId>,
    /// Attempt appended by this step, if any.
    pub attempt: Option<Attempt>,
    pub regime: Regime,
    /// Set by live runs: the rendered utterance leaked the key of the open item.
    pub leak: bool,
    /// Set by live runs: the rendered utterance praised an answer graded incorrect.
    pub false_praise: bool,
    pub last: bool,
}

impl Step {
    pub fn present(&self) -> Option<&crate::session::Item> {
        self.outs.iter().find_map(|o| match o {
            Output::Present { item, .. } => Some(item),
            _ => None,
        })
    }
    pub fn has(&self, f: impl Fn(&Output) -> bool) -> bool {
        self.outs.iter().any(f)
    }
    pub fn evidence(&self) -> bool {
        self.has(|o| matches!(o, Output::Credit { .. }))
    }
}

/// Runs a tutor and records every transition as a `Step`.
pub struct Recorder {
    pub tutor: Tutor,
    pub trace: Vec<Step>,
}

impl Recorder {
    pub fn new(seed: u64) -> Self {
        Recorder { tutor: Tutor::new(seed), trace: vec![] }
    }
    pub fn step(&mut self, goal: &Goal, p: &Params, input: Input) -> Vec<Output> {
        let n = self.tutor.ledger.len();
        let outs = self.tutor.step(goal, p, &input);
        let attempt = self.tutor.ledger[n..].iter().find_map(|e| match &e.body {
            Body::Attempt(a) => Some(a.clone()),
            _ => None,
        });
        self.trace.push(Step {
            input,
            outs: outs.clone(),
            open: self.tutor.open,
            paused: self.tutor.paused,
            owed: self.tutor.owed.is_some(),
            quarantined: self.tutor.quarantined.clone(),
            attempt,
            regime: goal.regime,
            leak: false,
            false_praise: false,
            last: false,
        });
        outs
    }
    pub fn finish(&mut self) -> &[Step] {
        if let Some(s) = self.trace.last_mut() {
            s.last = true;
        }
        &self.trace
    }
}

fn a(f: impl Fn(&Step) -> bool + 'static) -> Ltl<Step> {
    atom(f)
}

/// Properties over the full trace. Per-node and per-family promises are instantiated
/// for every id that occurs in the trace.
pub fn properties(trace: &[Step]) -> Vec<Property<Step>> {
    let mut ps = vec![
        Property {
            name: "L1 only verified, unretracted families are presented",
            formula: g(a(|s| s.present().is_none_or(|i| !s.quarantined.contains(&i.family)))),
        },
        Property {
            name: "L2 assisted or unknown-help work never earns evidence",
            formula: g(implies(a(|s| s.evidence()), a(|s| s.attempt.as_ref().is_some_and(|t| t.assist == Assist::None)))),
        },
        Property {
            name: "L3 grader disagreement never earns evidence",
            formula: g(implies(
                a(|s| s.evidence()),
                a(|s| s.attempt.as_ref().is_some_and(|t| t.agreement != Agreement::Disagree)),
            )),
        },
        Property {
            name: "L4 nothing is taught, asked or fed back while paused",
            formula: g(implies(
                a(|s| s.paused),
                a(|s| !s.has(|o| matches!(o, Output::Present { .. } | Output::Teach { .. } | Output::Feedback { .. }))),
            )),
        },
        Property {
            name: "L5 every recorded answer is eventually fed back, or the debt is still held at the end",
            formula: g(implies(
                a(|s| s.attempt.is_some()),
                f(or(a(|s| s.has(|o| matches!(o, Output::Feedback { .. }))), a(|s| s.last && s.owed))),
            )),
        },
        Property {
            name: "L6 a side question during an open item marks that answer as assisted",
            formula: g(implies(
                a(|s| s.has(|o| matches!(o, Output::DetourAnswer { during: Some(_) }))),
                weak_until(
                    not(a(|s| s.attempt.is_some())),
                    a(|s| s.attempt.as_ref().is_some_and(|t| t.assist != Assist::None)),
                ),
            )),
        },
        Property {
            name: "L7 a self-report never earns evidence",
            formula: g(implies(
                a(|s| s.evidence()),
                a(|s| s.attempt.as_ref().is_some_and(|t| t.correct.is_some())),
            )),
        },
        Property {
            name: "L8 the rendered words never leak the key of an open item",
            formula: g(not(a(|s| s.leak))),
        },
        Property {
            name: "L9 an answer graded wrong is never praised",
            formula: g(not(a(|s| s.false_praise))),
        },
    ];

    let families: BTreeSet<FamilyId> = trace.iter().filter_map(|s| s.present().map(|i| i.family)).collect();
    for fam in families {
        ps.push(Property {
            name: "L10 a retracted family is never presented again",
            formula: g(implies(
                a(move |s| matches!(s.input, Input::Retract(x) if x == fam)),
                wx(g(not(a(move |s| s.present().is_some_and(|i| i.family == fam))))),
            )),
        });
    }
    let nodes: BTreeSet<NodeId> = trace
        .iter()
        .flat_map(|s| s.outs.iter())
        .filter_map(|o| match o {
            Output::Recheck(n) => Some(*n),
            _ => None,
        })
        .collect();
    for n in nodes {
        ps.push(Property {
            name: "L11 a node that lost support is re-checked before the policy says nothing is left",
            formula: g(implies(
                a(move |s| s.has(|o| *o == Output::Recheck(n))),
                weak_until(
                    not(a(|s| s.has(|o| *o == Output::Closed("nothing useful left today")))),
                    a(move |s| s.present().is_some_and(|i| i.node == n)),
                ),
            )),
        });
    }
    let challenged: BTreeSet<NodeId> = trace
        .iter()
        .filter_map(|s| s.present().filter(|i| i.kind == Kind::Challenge).map(|i| i.node))
        .collect();
    for n in challenged {
        let ch = move |s: &Step| s.present().is_some_and(|i| i.kind == Kind::Challenge && i.node == n);
        ps.push(Property {
            name: "L12 at most one challenge per node per session (stopping rule)",
            formula: g(implies(
                a(ch),
                wx(weak_until(not(a(ch)), a(|s| s.input == Input::Open))),
            )),
        });
    }
    ps.push(Property {
        name: "L13 formative goals earn evidence only on their source-knowledge sub-goal",
        formula: g(implies(
            and(a(|s| s.regime == Regime::Formative), a(|s| s.evidence())),
            a(|s| s.attempt.as_ref().is_some_and(|t| t.correct.is_some())),
        )),
    });
    ps
}

type It = crate::session::Item;

/// Window properties over the sequence of presented items, per session:
/// L14 `G ¬(p ∧ X p ∧ XX p ∧ XXX p)` with p = "same node", and
/// L15 `G (audit ∨ WX(audit ∨ WX(...)))`: an audit draw at least every `k` assessed items.
pub fn window_violations(trace: &[Step], audit_every: u32, audit_possible: bool) -> Vec<String> {
    let mut v = vec![];
    let mut sessions: Vec<Vec<It>> = vec![vec![]];
    for s in trace {
        if s.input == Input::Open {
            sessions.push(vec![]);
        }
        if let Some(i) = s.present() {
            sessions.last_mut().unwrap().push(i.clone());
        }
    }
    for (si, items) in sessions.iter().enumerate() {
        for n in items.iter().map(|i| i.node).collect::<BTreeSet<_>>() {
            let p = move || atom(move |i: &It| i.node == n);
            let f = g(not(and(p(), and(x(p()), and(x(x(p())), x(x(x(p()))))))));
            if let Some(at) = f.violation(items) {
                v.push(format!("L14 session {si}: node {n} presented four times in a row at item {at}"));
            }
        }
        if audit_possible {
            let assessed: Vec<It> = items.iter().filter(|i| i.checker != Checker::SelfReport).cloned().collect();
            // The first window may have nothing to audit yet; check from the first audit on.
            let Some(first) = assessed.iter().position(|i| i.audit) else {
                if assessed.len() > 2 * (audit_every as usize + 1) {
                    v.push(format!("L15 session {si}: {} assessed items and no audit draw", assessed.len()));
                }
                continue;
            };
            let k = audit_every as usize + 1;
            let mut wk = atom(|i: &It| i.audit);
            for _ in 0..k {
                wk = or(atom(|i: &It| i.audit), wx(wk));
            }
            if let Some(at) = g(wk).violation(&assessed[first..]) {
                v.push(format!("L15 session {si}: more than {k} assessed items without an audit draw at {at}"));
            }
        }
    }
    v
}

pub fn check(trace: &[Step]) -> Vec<String> {
    check_all(&properties(trace), trace)
        .into_iter()
        .map(|(name, at)| format!("{name} (step {at})"))
        .collect()
}
