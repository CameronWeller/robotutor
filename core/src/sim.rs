//! Hidden-truth learners and the long-horizon simulation.
//!
//! A simulated learner has a ground-truth state the tutor cannot see: skill per node, shallow
//! (cue-dependent) knowledge, true memory stability, held misconceptions, and traits (undeclared
//! help, fatigue, attendance). That lets us measure what no real deployment can yet: whether
//! the tutor's beliefs match the truth, and whether its teaching moves the truth.
//!
//! The learning dynamics below are ASSUMPTIONS taken from robust findings (testing effect,
//! spacing, prerequisite dependence, shallow vs deep knowledge). They rank policies under
//! those assumptions; they do not prove classroom effect. Grader and interpreter noise are
//! parameters meant to be fitted from live model calls (see `live`).

use crate::atlas::{Checker, Family, Goal};
use crate::ledger::{days, Agreement, Assist, Cue, NodeId};
use crate::policy::{Kind, Params, Rng};
use crate::session::{Input, Item, Output};
use crate::spec::{self, Recorder};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Persona {
    pub name: &'static str,
    /// Initial skill per node (islands: high skill above a missing prerequisite).
    pub prior: BTreeMap<NodeId, f64>,
    pub misconceptions: BTreeSet<(NodeId, usize)>,
    /// Probability of using outside help on an item they expect to miss.
    pub help_use: f64,
    /// Probability of declaring that help.
    pub honesty: f64,
    pub fatigue_quit: u32,
    pub gap_days: f64,
    pub learn_rate: f64,
}

#[derive(Clone, Debug)]
pub struct Hidden {
    pub skill: BTreeMap<NodeId, f64>,
    pub shallow: BTreeMap<NodeId, f64>,
    pub stability: BTreeMap<NodeId, f64>,
    pub last: BTreeMap<NodeId, f64>,
    pub mis: BTreeSet<(NodeId, usize)>,
}

#[derive(Clone, Debug)]
pub struct Noise {
    /// Probability each rubric grader returns the true verdict.
    pub grader_acc: [f64; 2],
    /// Probability the two graders make the same mistake together (shared blind spot, P34).
    pub shared_error: f64,
    /// Interpreter: P(names the held misconception | wrong answer caused by it).
    pub interp_hit: f64,
    /// Interpreter: P(names some misconception when none caused the miss).
    pub interp_false: f64,
    pub sensor_acc: f64,
}

/// Learning amounts per exchange (assumptions, shared by every policy and by live runs).
/// Pass 2 widened them: errorful practice with feedback teaches at least as much as success.
pub const TEACH: f64 = 0.35;
pub const LEARN_OK: f64 = 0.08;
pub const LEARN_MISS: f64 = 0.10;

impl Default for Noise {
    fn default() -> Self {
        // Placeholders until live calibration (pass log records the fitted values).
        Noise { grader_acc: [0.9, 0.88], shared_error: 0.03, interp_hit: 0.7, interp_false: 0.1, sensor_acc: 0.97 }
    }
}

impl Hidden {
    pub fn new(goal: &Goal, p: &Persona) -> Self {
        let mut h = Hidden {
            skill: BTreeMap::new(),
            shallow: BTreeMap::new(),
            stability: BTreeMap::new(),
            last: BTreeMap::new(),
            mis: p.misconceptions.clone(),
        };
        for n in &goal.nodes {
            let s = p.prior.get(&n.id).copied().unwrap_or(0.05);
            h.skill.insert(n.id, s);
            h.shallow.insert(n.id, s);
            h.stability.insert(n.id, if s > 0.5 { 20.0 } else { 2.0 });
            h.last.insert(n.id, 0.0);
        }
        h
    }

    /// True memory after a retrieval attempt. Spacing effect: the lower recall was, the more a
    /// success strengthens memory; massed repetition adds little. A miss that came from a
    /// misconception or a slip (the learner knew it) is not a memory lapse (pass 4).
    pub fn retrieved(&mut self, n: NodeId, r: f64, success: bool, not_a_lapse: bool) {
        let st = self.stability.get_mut(&n).unwrap();
        if success {
            *st = (*st * (1.05 + 2.0 * (1.0 - r))).min(3650.0);
        } else if !not_a_lapse {
            *st = (*st * 0.7).max(1.0);
        }
    }

    pub fn recall(&self, n: NodeId, t: f64) -> f64 {
        crate::ledger::recall(self.stability[&n], t - self.last[&n])
    }

    /// Probability of a correct, unaided answer right now. The ground truth for metrics.
    pub fn p_correct(&self, goal: &Goal, f: &Family, t: f64, fatigue: f64) -> f64 {
        let n = f.node;
        if let Some(m) = f.probes_misconception {
            if self.mis.contains(&(n, m)) {
                return 0.12; // the misconception produces its characteristic wrong answer
            }
        }
        let r = self.recall(n, t);
        let prereq = goal.node(n).prereqs.iter().map(|q| self.skill[q] * self.recall(*q, t)).fold(1.0, f64::min);
        let deep = self.skill[&n] * r * (0.5 + 0.5 * prereq);
        let mut k = match (f.cue, f.facet) {
            (Cue::None, crate::ledger::Facet::Explain) => deep,
            (Cue::None, _) => deep.max(0.6 * self.shallow[&n] * r),
            _ => deep.max(self.shallow[&n] * r),
        };
        if f.novel_context {
            k *= 0.8;
        }
        let (guess, slip) = if f.checker == Checker::Exact { (0.05, 0.05) } else { (0.1, 0.08) };
        let z = 7.0 * (k - 0.5 - 0.12 * f.difficulty);
        let p = guess + (1.0 - guess - slip) / (1.0 + (-z).exp());
        (p - 0.15 * fatigue).clamp(0.01, 0.99)
    }

    fn touch(&mut self, n: NodeId, t: f64) {
        self.last.insert(n, t);
    }

    /// A lesson: deep skill grows if the foundation is there; cued knowledge grows anyway.
    pub fn teach(&mut self, goal: &Goal, n: NodeId, rate: f64, t: f64) {
        self.learn(goal, n, TEACH, rate, t);
        let s = self.shallow.get_mut(&n).unwrap();
        *s += 0.3 * (1.0 - *s);
    }

    pub fn learn(&mut self, goal: &Goal, n: NodeId, amount: f64, rate: f64, t: f64) {
        let ready = goal.node(n).prereqs.iter().map(|q| self.skill[q]).fold(1.0, f64::min);
        let gain = amount * rate * (0.2 + 0.8 * ready);
        let s = self.skill.get_mut(&n).unwrap();
        *s += gain * (1.0 - *s);
        self.touch(n, t);
    }
}

/// Unaided recall on every goal node at a horizon after the last session: the outcome measure.
pub fn outcome(goal: &Goal, h: &Hidden, t: f64) -> f64 {
    let fams: Vec<&Family> = goal
        .nodes
        .iter()
        .filter_map(|n| goal.families_for(n.id).find(|f| f.cue == Cue::None && f.checker != Checker::SelfReport))
        .collect();
    fams.iter().map(|f| h.p_correct(goal, f, t, 0.0)).sum::<f64>() / fams.len().max(1) as f64
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub policy: String,
    pub persona: String,
    pub outcome_d28: f64,
    pub start: f64,
    pub minutes: f64,
    pub items: u32,
    pub teach: u32,
    pub reviews: u32,
    pub challenges: u32,
    /// Mean |belief mean − true p| over nodes with evidence, at the end.
    pub belief_error: f64,
    /// Nodes flagged `shown` whose true unaided p < 0.6 (false credit).
    pub false_shown: u32,
    pub shown: u32,
    /// Brier score of the belief mean on audit draws (an unbiased check, P60).
    pub audit_brier: f64,
    pub audits: u32,
    pub llm_calls: u32,
    pub cost_usd: f64,
    pub violations: Vec<String>,
}

/// Cost model per call, from public OpenRouter prices (claude-haiku-5.5 / gpt-6-luna at
/// $0.10 in, $0.50 out per million tokens). Token counts are per-purpose packet budgets.
fn call_cost(tokens_in: f64, tokens_out: f64) -> f64 {
    (tokens_in * 0.10 + tokens_out * 0.50) / 1e6
}

/// Run one learner with the core tutor for `sessions` sessions.
pub fn run_core(goal: &Goal, persona: &Persona, noise: &Noise, params: &Params, sessions: u32, seed: u64) -> Report {
    let mut rng = Rng(seed ^ 0xA5A5);
    let mut h = Hidden::new(goal, persona);
    let start = outcome(goal, &h, 0.0);
    let mut rec = Recorder::new(seed);
    let mut rep = Report { policy: "core".into(), persona: persona.name.into(), start, ..Default::default() };
    let mut audit_sq = 0.0;
    let mut t_day = 0.0f64;

    for _ in 0..sessions {
        rec.step(goal, params, Input::Open);
        let mut fatigue = 0u32;
        loop {
            let outs = rec.step(goal, params, Input::Next);
            let mut item: Option<Item> = None;
            for o in &outs {
                match o {
                    Output::Teach { node, repair, .. } => {
                        rep.teach += 1;
                        rep.llm_calls += 1;
                        rep.cost_usd += call_cost(1500.0, 350.0);
                        if let Some(m) = repair {
                            if h.mis.contains(&(*node, *m)) && rng.unit() < 0.6 {
                                h.mis.remove(&(*node, *m));
                            }
                        }
                        h.teach(goal, *node, persona.learn_rate, t_day);
                    }
                    Output::Present { item: i, .. } => item = Some(i.clone()),
                    _ => {}
                }
            }
            if rec.tutor.open && item.is_none() {
                rec.step(goal, params, Input::Wait(3));
                continue;
            }
            let Some(item) = item else { break };
            rep.llm_calls += 1; // render the question
            rep.cost_usd += call_cost(1200.0, 120.0);
            let fam = goal.families.iter().find(|f| f.id == item.family).unwrap().clone();
            let fat = fatigue as f64 / 1000.0;
            let p_true = h.p_correct(goal, &fam, t_day, fat);
            if item.audit {
                let b = rec.tutor.projection().node(item.node).map(|b| b.mean()).unwrap_or(0.5);
                audit_sq += (b - p_true).powi(2);
                rep.audits += 1;
            }
            // Undeclared outside help (P6): used when the learner expects to miss.
            let mut assist = Assist::None;
            let mut truth = rng.unit() < p_true;
            if !truth && p_true < 0.5 && rng.unit() < persona.help_use {
                truth = rng.unit() < 0.9;
                if rng.unit() < persona.honesty {
                    assist = Assist::Declared;
                }
            }
            rec.step(goal, params, Input::Answer { assist });
            rep.items += 1;
            if fam.checker != Checker::SelfReport {
                let (correct, agreement) = grade(&fam, truth, noise, &mut rng);
                let mut hyps = vec![];
                if !truth {
                    let held: Vec<usize> = h.mis.iter().filter(|(n, _)| *n == item.node).map(|(_, m)| *m).collect();
                    if !held.is_empty() && rng.unit() < noise.interp_hit {
                        hyps.push(held[rng.below(held.len())]);
                    } else if held.is_empty() && rng.unit() < noise.interp_false {
                        let k = goal.node(item.node).misconceptions.len();
                        if k > 0 {
                            hyps.push(rng.below(k));
                        }
                    }
                }
                if fam.checker == Checker::Rubric {
                    rep.llm_calls += 2;
                    rep.cost_usd += 2.0 * call_cost(900.0, 150.0);
                    if !truth {
                        rep.llm_calls += 1;
                        rep.cost_usd += call_cost(700.0, 100.0);
                    }
                }
                rec.step(goal, params, Input::Graded { correct, agreement, hyps });
            }
            rep.llm_calls += 1; // render feedback
            rep.cost_usd += call_cost(1200.0, 150.0);
            // Hidden learning from this exchange.
            let n = item.node;
            let r = h.recall(n, t_day);
            match (item.kind, truth) {
                (_, true) if assist == Assist::None => {
                    h.retrieved(n, r, true, false);
                    h.learn(goal, n, LEARN_OK, persona.learn_rate, t_day);
                }
                (Kind::Challenge, false) => {
                    let sh = h.shallow[&n];
                    h.learn(goal, n, 0.15 * sh + LEARN_MISS, persona.learn_rate, t_day);
                }
                (_, false) => {
                    let by_mis = item.probes.is_some_and(|m| h.mis.contains(&(n, m)));
                    h.retrieved(n, r, false, by_mis || p_true > 0.5);
                    h.learn(goal, n, LEARN_MISS, persona.learn_rate, t_day);
                }
                _ => h.touch(n, t_day),
            }
            if item.kind == Kind::Practice {
                let s = h.shallow.get_mut(&n).unwrap();
                *s += 0.15 * (1.0 - *s);
                h.learn(goal, n, 0.08, persona.learn_rate, t_day);
            }
            fatigue += if truth { 60 } else { 100 };
            rec.step(goal, params, Input::Wait(2));
            if fatigue >= persona.fatigue_quit {
                rec.step(goal, params, Input::Close);
                break;
            }
            if !rec.tutor.open {
                break;
            }
        }
        if rec.tutor.open {
            rec.step(goal, params, Input::Close);
        }
        let gap = persona.gap_days * (0.5 + rng.unit());
        t_day += gap;
        rec.step(goal, params, Input::Wait((gap * 1440.0) as u64));
        debug_assert!((days(rec.tutor.now) - t_day).abs() < 0.01);
    }
    rep.minutes = rec.trace.iter().filter_map(|s| match s.input { Input::Wait(m) if m < 60 => Some(m as f64), _ => None }).sum::<f64>();
    rep.reviews = rec.trace.iter().filter(|s| s.present().is_some_and(|i| i.kind == Kind::Review)).count() as u32;
    rep.challenges = rec.trace.iter().filter(|s| s.present().is_some_and(|i| i.kind == Kind::Challenge)).count() as u32;
    finish(goal, &h, &rec.tutor.projection(), t_day, &mut rep);
    rep.audit_brier = if rep.audits > 0 { audit_sq / rep.audits as f64 } else { f64::NAN };
    let trace = rec.finish().to_vec();
    rep.violations = spec::check(&trace);
    rep.violations.extend(spec::window_violations(&trace, params.audit_every, true));
    rep
}

fn grade(f: &Family, truth: bool, noise: &Noise, rng: &mut Rng) -> (Option<bool>, Agreement) {
    match f.checker {
        Checker::Exact => (Some(truth), Agreement::Checker),
        Checker::Sensor => (Some(if rng.unit() < noise.sensor_acc { truth } else { !truth }), Agreement::Checker),
        Checker::Rubric => {
            if rng.unit() < noise.shared_error {
                return (Some(!truth), Agreement::Agree); // both wrong together: undetectable
            }
            let a = if rng.unit() < noise.grader_acc[0] { truth } else { !truth };
            let b = if rng.unit() < noise.grader_acc[1] { truth } else { !truth };
            if a == b {
                (Some(a), Agreement::Agree)
            } else {
                (Some(a), Agreement::Disagree)
            }
        }
        Checker::SelfReport => (None, Agreement::Single),
    }
}

fn finish(goal: &Goal, h: &Hidden, proj: &crate::ledger::Projection, t_end: f64, rep: &mut Report) {
    let mut err = 0.0;
    let mut k = 0;
    for n in &goal.nodes {
        let Some(f) = goal.families_for(n.id).find(|f| f.cue == Cue::None && f.checker != Checker::SelfReport) else {
            continue;
        };
        let p = h.p_correct(goal, f, t_end, 0.0);
        if let Some(b) = proj.node(n.id).filter(|b| b.evidence > 0) {
            err += (b.mean() - p).abs();
            k += 1;
        }
        if proj.node_shown(n.id) {
            rep.shown += 1;
            if p < 0.6 {
                rep.false_shown += 1;
            }
        }
    }
    rep.belief_error = if k > 0 { err / k as f64 } else { f64::NAN };
    rep.outcome_d28 = outcome(goal, h, t_end + 28.0);
    if std::env::var("ROBOTUTOR_DIAG").is_ok() {
        eprintln!("held misconceptions at end: {:?}; hypotheses: {:?}", h.mis, proj.hyps);
        for n in &goal.nodes {
            let f = goal.families_for(n.id).find(|f| f.cue == Cue::None && f.checker != Checker::SelfReport);
            let b = proj.node(n.id);
            eprintln!(
                "node {} skill {:.2} shallow {:.2} S {:.1} p_end {:.2} p_d28 {:.2} | belief {:.2} ev {} taught {}",
                n.id, h.skill[&n.id], h.shallow[&n.id], h.stability[&n.id],
                f.map(|f| h.p_correct(goal, f, t_end, 0.0)).unwrap_or(f64::NAN),
                f.map(|f| h.p_correct(goal, f, t_end + 28.0, 0.0)).unwrap_or(f64::NAN),
                b.map(|b| b.mean()).unwrap_or(f64::NAN), b.map(|b| b.evidence).unwrap_or(0), b.map(|b| b.taught).unwrap_or(0)
            );
        }
    }
}

/// Baseline: a reactive chat tutor (an approximation of a study-mode chatbot with no
/// persistent learner model). It stays on the current topic until two correct answers in a
/// row (any help counts), then moves on in a fixed order. It never schedules review.
pub fn run_reactive(goal: &Goal, persona: &Persona, noise: &Noise, sessions: u32, items_per_session: u32, seed: u64) -> Report {
    baseline(goal, persona, noise, sessions, items_per_session, seed, None)
}

/// Baseline: a fixed curriculum with mastery = three correct in a row and a fixed 7-day review.
pub fn run_fixed(goal: &Goal, persona: &Persona, noise: &Noise, sessions: u32, items_per_session: u32, seed: u64) -> Report {
    baseline(goal, persona, noise, sessions, items_per_session, seed, Some(7.0))
}

fn baseline(goal: &Goal, persona: &Persona, noise: &Noise, sessions: u32, per: u32, seed: u64, review: Option<f64>) -> Report {
    let mut rng = Rng(seed ^ 0x5A5A);
    let mut h = Hidden::new(goal, persona);
    let mut rep = Report {
        policy: if review.is_some() { "fixed+review".into() } else { "reactive-chat".into() },
        persona: persona.name.into(),
        start: outcome(goal, &h, 0.0),
        ..Default::default()
    };
    let order = goal.topo();
    let need = if review.is_some() { 3 } else { 2 };
    let mut cur = 0usize;
    let mut streak = 0;
    let mut mastered: BTreeMap<NodeId, f64> = BTreeMap::new();
    let mut taught: BTreeSet<NodeId> = BTreeSet::new();
    let mut t = 0.0;
    let mut exposure: BTreeMap<u32, u32> = BTreeMap::new();
    for _ in 0..sessions {
        let mut fatigue = 0u32;
        for _ in 0..per {
            // Fixed policy: review the oldest mastered node past its interval, else continue.
            let due = review.and_then(|iv| mastered.iter().find(|(_, &m)| t - m >= iv).map(|(&n, _)| n));
            let n = match due {
                Some(n) => n,
                None if cur < order.len() => order[cur],
                None => break,
            };
            if due.is_none() && !taught.contains(&n) {
                taught.insert(n);
                rep.teach += 1;
                rep.cost_usd += call_cost(3000.0 + 400.0 * rep.items as f64, 400.0); // history grows (P44)
                h.teach(goal, n, persona.learn_rate, t);
                continue;
            }
            let Some(fam) = goal
                .families_for(n)
                .filter(|f| f.checker != Checker::SelfReport)
                .min_by_key(|f| exposure.get(&f.id).copied().unwrap_or(0))
                .cloned()
            else {
                cur += 1; // nothing assessable here: a chat tutor just moves on
                continue;
            };
            *exposure.entry(fam.id).or_insert(0) += 1;
            let p = h.p_correct(goal, &fam, t, fatigue as f64 / 1000.0);
            let mut truth = rng.unit() < p;
            if !truth && p < 0.5 && rng.unit() < persona.help_use {
                truth = rng.unit() < 0.9;
            }
            let (verdict, _) = grade(&fam, truth, noise, &mut rng);
            let ok = verdict == Some(true);
            rep.items += 1;
            rep.llm_calls += 2;
            rep.cost_usd += call_cost(3000.0 + 400.0 * rep.items as f64, 300.0);
            let r = h.recall(n, t);
            if truth {
                h.retrieved(n, r, true, false);
                h.learn(goal, n, LEARN_OK, persona.learn_rate, t);
            } else {
                let by_mis = fam.probes_misconception.is_some_and(|m| h.mis.contains(&(n, m)));
                h.retrieved(n, r, false, by_mis || p > 0.5);
                h.learn(goal, n, LEARN_MISS, persona.learn_rate, t);
            }
            fatigue += if truth { 60 } else { 100 };
            if due.is_some() {
                rep.reviews += 1;
                mastered.insert(n, t);
            } else {
                streak = if ok { streak + 1 } else { 0 };
                if streak >= need {
                    mastered.insert(n, t);
                    cur += 1;
                    streak = 0;
                }
            }
            t += 2.0 / 1440.0;
            if fatigue >= persona.fatigue_quit {
                break;
            }
        }
        t += persona.gap_days * (0.5 + rng.unit());
    }
    rep.shown = mastered.len() as u32;
    rep.false_shown = mastered
        .keys()
        .filter(|&&n| {
            goal.families_for(n)
                .find(|f| f.cue == Cue::None && f.checker != Checker::SelfReport)
                .is_some_and(|f| h.p_correct(goal, f, t, 0.0) < 0.6)
        })
        .count() as u32;
    rep.minutes = rep.items as f64 * 2.0 + rep.teach as f64 * 3.0;
    rep.outcome_d28 = outcome(goal, &h, t + 28.0);
    rep.belief_error = f64::NAN;
    rep.audit_brier = f64::NAN;
    rep
}

/// Standard personas for rehearsal. Cameron-like: strong in places, impatient with repetition.
pub fn personas(goal: &Goal) -> Vec<Persona> {
    let first = goal.topo()[0];
    let last = *goal.topo().last().unwrap();
    let mis_on = |k: usize| -> BTreeSet<(NodeId, usize)> {
        goal.nodes.iter().filter(|n| !n.misconceptions.is_empty()).take(k).map(|n| (n.id, 0)).collect()
    };
    vec![
        Persona {
            name: "novice-steady",
            prior: BTreeMap::new(),
            misconceptions: mis_on(2),
            help_use: 0.1,
            honesty: 0.8,
            fatigue_quit: 1600,
            gap_days: 2.0,
            learn_rate: 1.0,
        },
        Persona {
            name: "island-expert",
            // Knows the advanced topic but not its foundation (P37).
            prior: [(last, 0.7), (first, 0.2)].into_iter().collect(),
            misconceptions: mis_on(1),
            help_use: 0.05,
            honesty: 0.9,
            fatigue_quit: 1400,
            gap_days: 4.0,
            learn_rate: 1.2,
        },
        Persona {
            name: "helper-reliant",
            // Uses outside help often and rarely says so (P5–P8).
            prior: BTreeMap::new(),
            misconceptions: mis_on(2),
            help_use: 0.6,
            honesty: 0.2,
            fatigue_quit: 1200,
            gap_days: 3.0,
            learn_rate: 0.8,
        },
        Persona {
            name: "irregular-busy",
            // Life interrupts: long gaps, short sessions (P42).
            prior: BTreeMap::new(),
            misconceptions: mis_on(1),
            help_use: 0.1,
            honesty: 0.8,
            fatigue_quit: 700,
            gap_days: 9.0,
            learn_rate: 1.0,
        },
    ]
}
