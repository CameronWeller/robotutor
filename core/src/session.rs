//! The orchestrator: a deterministic state machine between the learner, the policy and the
//! model calls. It never calls a model. It emits requests (`Present`, `Grade`, `Teach`) and
//! receives results as inputs, so the same machine runs in tests, the model checker,
//! simulations and live sessions.

use crate::atlas::{Checker, Goal};
use crate::ledger::*;
use crate::policy::{choose, Counters, Ctx, Kind, Params, Rng};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Item {
    pub family: FamilyId,
    pub node: NodeId,
    pub facet: Facet,
    pub kind: Kind,
    pub checker: Checker,
    pub novelty: Novelty,
    pub cue: Cue,
    pub novel_context: bool,
    pub probes: Option<usize>,
    pub audit: bool,
    /// Raised to `Hint` when the learner asked a side question while this item was open.
    pub assisted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Phase {
    Idle,
    Awaiting(Item),
    Grading(Item, Assist),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Input {
    Open,
    Next,
    Answer { assist: Assist },
    Graded { correct: Option<bool>, agreement: Agreement, hyps: Vec<usize> },
    GraderTimeout,
    /// The learner asks their own question (answered by retrieval, never with an open key).
    Detour,
    Pause,
    Resume,
    /// Backward feedback: a family is found flawed after release.
    Retract(FamilyId),
    /// Time passes (minutes).
    Wait(u64),
    Close,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Output {
    Teach { node: NodeId, repair: Option<usize>, why: &'static str },
    Present { item: Item, why: &'static str, propensity: f64 },
    Grade(Item),
    Feedback { item: Item, correct: Option<bool> },
    Credit { node: NodeId, family: FamilyId },
    Withdrawn(Item),
    DetourAnswer { during: Option<FamilyId> },
    Superseded { event: EventId },
    Recheck(NodeId),
    Closed(&'static str),
    Rejected(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Tutor {
    pub ledger: Vec<Event>,
    pub phase: Phase,
    pub open: bool,
    pub paused: bool,
    pub c: Counters,
    /// Feedback owed for a graded answer. Survives pause and close (P42).
    pub owed: Option<(Item, Option<bool>)>,
    pub quarantined: BTreeSet<FamilyId>,
    pub now: u64,
    pub rng: u64,
    /// Why the last move was chosen (shown to the learner on request).
    pub last_why: &'static str,
}

impl Tutor {
    pub fn new(seed: u64) -> Self {
        Tutor {
            ledger: vec![],
            phase: Phase::Idle,
            open: false,
            paused: false,
            c: Counters::default(),
            owed: None,
            quarantined: BTreeSet::new(),
            now: 0,
            rng: seed,
            last_why: "",
        }
    }

    fn push(&mut self, body: Body) -> EventId {
        let id = self.ledger.len() as EventId + 1;
        self.ledger.push(Event { id, t: self.now, body });
        id
    }

    pub fn projection(&self) -> Projection {
        project(&self.ledger)
    }

    pub fn exposure(&self) -> BTreeMap<FamilyId, u32> {
        let mut m = BTreeMap::new();
        for e in &self.ledger {
            if let Body::Presented { family } = e.body {
                *m.entry(family).or_insert(0) += 1;
            }
        }
        m
    }

    /// One deterministic transition. Inputs that do not fit the phase are rejected and
    /// leave the state unchanged.
    pub fn step(&mut self, goal: &Goal, p: &Params, input: &Input) -> Vec<Output> {
        let mut out = vec![];
        match (input, self.phase.clone()) {
            (Input::Open, _) if !self.open => {
                self.open = true;
                let recheck = std::mem::take(&mut self.c.recheck);
                self.c = Counters { recheck, ..Default::default() };
                self.discharge(&mut out);
            }
            (Input::Next, Phase::Idle) if self.open && !self.paused && self.owed.is_none() => {
                self.next(goal, p, &mut out);
            }
            (Input::Answer { assist }, Phase::Awaiting(item)) if !self.paused => {
                let assist = if item.assisted && *assist == Assist::None { Assist::Hint } else { *assist };
                if item.checker == Checker::SelfReport {
                    // A log or reflection: acknowledged, recorded, never judged.
                    self.record(goal, &item, assist, None, Agreement::Single, &[], &mut out);
                } else {
                    self.phase = Phase::Grading(item.clone(), assist);
                    out.push(Output::Grade(item));
                }
            }
            (Input::Graded { correct, agreement, hyps }, Phase::Grading(item, assist)) => {
                self.record(goal, &item, assist, *correct, *agreement, hyps, &mut out);
            }
            (Input::GraderTimeout, Phase::Grading(item, assist)) => {
                self.record(goal, &item, assist, None, Agreement::Single, &[], &mut out);
            }
            (Input::Detour, phase) if self.open && !self.paused => {
                let during = match phase {
                    Phase::Awaiting(mut item) => {
                        item.assisted = true;
                        let f = item.family;
                        self.phase = Phase::Awaiting(item);
                        Some(f)
                    }
                    _ => None,
                };
                out.push(Output::DetourAnswer { during });
            }
            (Input::Pause, _) if self.open && !self.paused => self.paused = true,
            (Input::Resume, _) if self.paused => {
                self.paused = false;
                self.discharge(&mut out);
            }
            (Input::Retract(f), phase) if !self.quarantined.contains(f) => {
                self.retract(*f, phase, &mut out);
            }
            (Input::Wait(m), _) => {
                self.now += m;
                if self.open {
                    self.c.minutes += (*m).min(u32::MAX as u64) as u32;
                }
            }
            (Input::Close, phase) if self.open => {
                if let Phase::Awaiting(item) = phase {
                    out.push(Output::Withdrawn(item));
                    self.phase = Phase::Idle;
                }
                self.open = false;
                self.paused = false;
                out.push(Output::Closed("learner"));
            }
            _ => out.push(Output::Rejected("input does not fit the current phase")),
        }
        out
    }

    fn discharge(&mut self, out: &mut Vec<Output>) {
        if self.open && !self.paused {
            if let Some((item, correct)) = self.owed.take() {
                out.push(Output::Feedback { item, correct });
            }
        }
    }

    fn next(&mut self, goal: &Goal, p: &Params, out: &mut Vec<Output>) {
        let proj = self.projection();
        let exposure = self.exposure();
        let mut rng = Rng(self.rng);
        let m = choose(
            &Ctx { goal, proj: &proj, exposure: &exposure, quarantined: &self.quarantined, c: &self.c, now: self.now, p },
            &mut rng,
        );
        self.rng = rng.0;
        self.last_why = m.why;
        match m.kind {
            Kind::Close => {
                self.open = false;
                out.push(Output::Closed(m.why));
            }
            Kind::Teach | Kind::Repair => {
                self.push(Body::Taught { node: m.node });
                self.c.taught.insert(m.node);
                self.c.fatigue += 40;
                out.push(Output::Teach { node: m.node, repair: m.misconception, why: m.why });
            }
            _ => {
                let fam = goal.families.iter().find(|f| Some(f.id) == m.family).expect("policy picks a family");
                let seen = exposure.get(&fam.id).copied().unwrap_or(0);
                let novelty = if seen == 0 {
                    Novelty::Fresh
                } else if seen < fam.variants {
                    Novelty::Variant
                } else {
                    Novelty::Repeat
                };
                let item = Item {
                    family: fam.id,
                    node: fam.node,
                    facet: fam.facet,
                    kind: m.kind,
                    checker: fam.checker,
                    novelty,
                    cue: fam.cue,
                    novel_context: fam.novel_context,
                    probes: fam.probes_misconception,
                    audit: m.audit,
                    assisted: false,
                };
                self.push(Body::Presented { family: fam.id });
                if fam.checker == Checker::SelfReport {
                    self.c.logged_practice.insert(fam.node);
                }
                self.c.recent.push(fam.node);
                if self.c.recent.len() > 8 {
                    self.c.recent.remove(0);
                }
                self.phase = Phase::Awaiting(item.clone());
                out.push(Output::Present { item, why: m.why, propensity: m.propensity });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        goal: &Goal,
        item: &Item,
        assist: Assist,
        correct: Option<bool>,
        agreement: Agreement,
        hyps: &[usize],
        out: &mut Vec<Output>,
    ) {
        let attempt = Attempt {
            node: item.node,
            facet: item.facet,
            family: item.family,
            correct,
            agreement,
            assist,
            novelty: item.novelty,
            cue: item.cue,
            novel_context: item.novel_context,
            audit: item.audit,
            probes: item.probes,
        };
        let w = weight(&attempt);
        let id = self.push(Body::Attempt(attempt));
        if self.quarantined.contains(&item.family) {
            // Retracted while being graded: keep the row, remove its weight.
            self.push(Body::Supersede { target: id, reason: "family retracted during grading".into() });
            out.push(Output::Superseded { event: id });
        } else if w > 0.0 {
            out.push(Output::Credit { node: item.node, family: item.family });
        }
        let n_mis = goal.node(item.node).misconceptions.len();
        for &m in hyps.iter().filter(|&&m| m < n_mis) {
            self.push(Body::Hypothesis { node: item.node, misconception: m });
        }
        let c = &mut self.c;
        if item.checker != Checker::SelfReport {
            c.assessed += 1;
            c.since_audit = if item.audit { 0 } else { c.since_audit + 1 };
        }
        c.since_challenge += 1;
        if item.kind == Kind::Review {
            c.reviews += 1;
            c.recheck.remove(&item.node);
        }
        if item.kind == Kind::Challenge {
            // Stopping rule (P41): one challenge per node per session.
            c.challenges += 1;
            c.since_challenge = 0;
            c.backoff.insert(item.node);
        }
        let weak = item.cue != Cue::None || item.novelty != Novelty::Fresh || item.kind == Kind::Practice;
        c.challengeable = (correct == Some(true) && weak).then_some(item.node);
        c.fatigue += if correct == Some(false) { 100 } else { 60 };
        self.phase = Phase::Idle;
        self.owed = Some((item.clone(), correct));
        self.discharge(out);
    }

    fn retract(&mut self, f: FamilyId, phase: Phase, out: &mut Vec<Output>) {
        let before = self.projection();
        self.quarantined.insert(f);
        if let Phase::Awaiting(item) = phase {
            if item.family == f {
                self.phase = Phase::Idle;
                out.push(Output::Withdrawn(item));
            }
        }
        let dead: BTreeSet<EventId> = self
            .ledger
            .iter()
            .filter_map(|e| match &e.body {
                Body::Supersede { target, .. } => Some(*target),
                _ => None,
            })
            .collect();
        let hits: Vec<EventId> = self
            .ledger
            .iter()
            .filter(|e| matches!(&e.body, Body::Attempt(a) if a.family == f) && !dead.contains(&e.id))
            .map(|e| e.id)
            .collect();
        for id in hits {
            self.push(Body::Supersede { target: id, reason: format!("family {f} retracted") });
            out.push(Output::Superseded { event: id });
        }
        let after = self.projection();
        let lost: Vec<NodeId> = before
            .beliefs
            .keys()
            .map(|(n, _)| *n)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|&n| before.node_shown(n) && !after.node_shown(n))
            .collect();
        for n in lost {
            self.c.recheck.insert(n);
            out.push(Output::Recheck(n));
        }
    }
}
