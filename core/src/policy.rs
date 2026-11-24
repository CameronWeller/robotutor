//! Deterministic move selection. Same ledger + counters + seed ⇒ same move, whichever
//! language model renders it (the swap test holds by construction).

use crate::atlas::{Checker, Family, Goal, Regime};
use crate::ledger::{days, Belief, Cue, FamilyId, Hyp, NodeId, Projection};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Kind {
    Teach,
    Probe,
    Practice,
    Challenge,
    Review,
    Transfer,
    Repair,
    Close,
}

impl Kind {
    /// Moves that present an item and wait for an answer.
    pub fn asks(self) -> bool {
        !matches!(self, Kind::Teach | Kind::Repair | Kind::Close)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Move {
    pub kind: Kind,
    pub node: NodeId,
    pub family: Option<FamilyId>,
    /// The misconception a Repair addresses, if any.
    pub misconception: Option<usize>,
    pub why: &'static str,
    pub score: f64,
    /// Probability this move had among near-ties. Logged for offline policy evaluation (P40).
    pub propensity: f64,
    pub audit: bool,
}

/// Per-session counters, owned by the orchestrator. All integral, so states are hashable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Counters {
    pub assessed: u32,
    pub since_audit: u32,
    pub reviews: u32,
    pub recent: Vec<NodeId>,
    pub challenges: u32,
    pub since_challenge: u32,
    /// Node whose last answer was correct but weak evidence (cued, variant or repeat).
    pub challengeable: Option<NodeId>,
    /// Nodes where pressure stopped paying this session.
    pub backoff: BTreeSet<NodeId>,
    pub taught: BTreeSet<NodeId>,
    pub logged_practice: BTreeSet<NodeId>,
    /// Nodes that lost support through backward feedback and must be re-checked.
    pub recheck: BTreeSet<NodeId>,
    /// 0..=1000.
    pub fatigue: u32,
    pub minutes: u32,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub lambda: f64,
    pub review_share: f64,
    pub max_same_node: usize,
    pub challenge_gap: u32,
    pub audit_every: u32,
    pub fatigue_close: u32,
    pub fatigue_no_pressure: u32,
    pub session_minutes: u32,
    pub tie_eps: f64,
    pub ready_mean: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            lambda: 1.0,
            review_share: 0.2,
            max_same_node: 3,
            challenge_gap: 3,
            audit_every: 10,
            fatigue_close: 800,
            fatigue_no_pressure: 600,
            session_minutes: 25,
            tie_eps: 0.05,
            ready_mean: 0.65,
        }
    }
}

pub struct Ctx<'a> {
    pub goal: &'a Goal,
    pub proj: &'a Projection,
    pub exposure: &'a BTreeMap<FamilyId, u32>,
    pub quarantined: &'a BTreeSet<FamilyId>,
    pub c: &'a Counters,
    pub now: u64,
    pub p: &'a Params,
}

/// Deterministic generator (splitmix64), so every run is replayable from its seed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn prior() -> Belief {
    Belief { alpha: 1.0, beta: 1.0, ..Default::default() }
}

impl Ctx<'_> {
    fn families(&self, node: NodeId) -> impl Iterator<Item = &Family> {
        self.goal.families_for(node).filter(|f| !self.quarantined.contains(&f.id))
    }
    fn seen(&self, f: &Family) -> u32 {
        self.exposure.get(&f.id).copied().unwrap_or(0)
    }
    /// Least-exposed family satisfying `want`. Fresh families first, then variants.
    fn pick(&self, node: NodeId, want: impl Fn(&Family) -> bool) -> Option<&Family> {
        self.families(node)
            .filter(|f| want(f))
            .min_by_key(|f| (self.seen(f) >= f.variants, self.seen(f), f.id))
    }
    /// Sequencing uses a working-knowledge bar, lower than the mastery claim (pass 6):
    /// a claim must be strict because people act on it; routing must not stall on it.
    fn ready(&self, node: NodeId) -> bool {
        self.goal.node(node).prereqs.iter().all(|&q| {
            self.proj.node_shown(q)
                || self.proj.node(q).is_some_and(|b| b.evidence >= 2 && b.mean() >= self.p.ready_mean)
        })
    }
    fn claimed(&self, node: NodeId) -> bool {
        self.proj.node(node).is_some_and(|b| b.claimed)
    }
}

pub fn choose(cx: &Ctx, rng: &mut Rng) -> Move {
    let p = cx.p;
    let c = cx.c;
    let mv = |kind, node, family: Option<&Family>, why, score| Move {
        kind,
        node,
        family: family.map(|f| f.id),
        misconception: None,
        why,
        score,
        propensity: 1.0,
        audit: false,
    };
    let close = |why| mv(Kind::Close, 0, None, why, 0.0);
    if c.fatigue >= p.fatigue_close {
        return close("fatigue");
    }
    if c.minutes >= p.session_minutes {
        return close("time");
    }

    let fixated = |n: NodeId| {
        c.recent.len() >= p.max_same_node && c.recent.iter().rev().take(p.max_same_node).all(|&r| r == n)
    };
    // Unbiased audit draw (P60): uniformly over nodes with any evidence or claim, ignoring
    // scores. (ponytail: excludes a node that would break the fixation guard; tiny bias.)
    if c.since_audit >= p.audit_every && cx.goal.regime != Regime::Formative {
        let pool: Vec<NodeId> = cx
            .goal
            .topo()
            .into_iter()
            .filter(|&n| !fixated(n))
            .filter(|&n| cx.proj.node(n).is_some_and(|b| b.evidence > 0 || b.claimed))
            .filter(|&n| cx.pick(n, |f| f.cue == Cue::None && f.checker != Checker::SelfReport).is_some())
            .collect();
        if !pool.is_empty() {
            let n = pool[rng.below(pool.len())];
            let f = cx.pick(n, |f| f.cue == Cue::None && f.checker != Checker::SelfReport);
            let mut m = mv(Kind::Probe, n, f, "audit", 0.0);
            m.audit = true;
            m.propensity = 1.0 / pool.len() as f64;
            return m;
        }
    }

    let pressure_ok = c.fatigue < p.fatigue_no_pressure;
    let mut cands: Vec<Move> = vec![];
    let any_due = cx.goal.topo().into_iter().any(|n| {
        cx.proj.node(n).is_some_and(|b| b.due(days(cx.now))) || c.recheck.contains(&n)
    });
    let review_floor =
        any_due && c.assessed >= 4 && (c.reviews as f64) < p.review_share * c.assessed as f64;

    for n in cx.goal.topo() {
        if fixated(n) {
            continue; // fixation guard (P14)
        }
        let b = cx.proj.node(n).cloned().unwrap_or_else(prior);
        let info = 12.0 * b.variance(); // 1.0 at the uniform prior
        let gap = 1.0 - b.mean();
        let ready = cx.ready(n);
        let shown = cx.proj.node_shown(n);
        let assessable = |f: &Family| f.checker != Checker::SelfReport;
        let fatigue_cost = c.fatigue as f64 / 1000.0 * 0.5;

        // Re-checks after backward feedback, and spaced reviews.
        if c.recheck.contains(&n) || b.due(days(cx.now)) {
            if let Some(f) = cx.pick(n, |f| f.cue == Cue::None && assessable(f)) {
                let r = b.recall_at(days(cx.now)).unwrap_or(0.5);
                let why = if c.recheck.contains(&n) { "recheck after correction" } else { "spaced review due" };
                cands.push(mv(Kind::Review, n, Some(f), why, 1.0 + (0.85 - r).max(0.0) * 4.0));
            }
        }
        if review_floor {
            continue;
        }
        // Misconceptions: suspected ⇒ discriminate; confirmed ⇒ repair, then re-probe.
        for (&(hn, m), h) in cx.proj.hyps.range((n, 0)..(n + 1, 0)) {
            debug_assert_eq!(hn, n);
            match h {
                Hyp::Suspected => {
                    if let Some(f) = cx.pick(n, |f| f.probes_misconception == Some(m)) {
                        cands.push(mv(Kind::Probe, n, Some(f), "test a suspected misconception", 0.6 * info + 0.6));
                    }
                }
                Hyp::Confirmed if !c.taught.contains(&n) => {
                    let mut r = mv(Kind::Repair, n, None, "repair a confirmed misconception", 1.2);
                    r.misconception = Some(m);
                    cands.push(r);
                }
                Hyp::Confirmed => {
                    if let Some(f) = cx.pick(n, |f| f.probes_misconception == Some(m)) {
                        cands.push(mv(Kind::Probe, n, Some(f), "check the repair held", 0.9));
                    }
                }
            }
        }
        if shown {
            if !b.transfer && pressure_ok {
                if let Some(f) = cx.pick(n, |f| f.novel_context && assessable(f)) {
                    cands.push(mv(Kind::Transfer, n, Some(f), "try it somewhere new", 0.5 - fatigue_cost));
                }
            }
            continue;
        }
        // Formative check-ins: log the practice once per session. Never scored.
        if !c.logged_practice.contains(&n) && ready {
            if let Some(f) = cx.pick(n, |f| f.checker == Checker::SelfReport) {
                cands.push(mv(Kind::Practice, n, Some(f), "practice check-in", 0.8));
            }
        }
        // The oppositional move: a correct but weak answer gets challenged (P1, P41).
        if c.challengeable == Some(n) && pressure_ok && !c.backoff.contains(&n)
            && c.since_challenge >= p.challenge_gap
        {
            if let Some(f) = cx.pick(n, |f| f.cue == Cue::None && assessable(f)) {
                cands.push(mv(Kind::Challenge, n, Some(f), "a right answer that could be luck or recall", 0.9));
            }
        }
        if !(ready || cx.claimed(n)) {
            continue; // prerequisite blindness guard (P14): probe claims, teach only on a foundation
        }
        if let Some(f) = cx.pick(n, |f| f.cue == Cue::None && assessable(f)) {
            let why = if b.evidence == 0 { "find out what you already know" } else { "measure progress" };
            cands.push(mv(Kind::Probe, n, Some(f), why, 0.6 * info - fatigue_cost));
        }
        if ready && !c.taught.contains(&n) && b.taught < 3 && !(b.evidence >= 2 && b.mean() >= 0.65) {
            cands.push(mv(Kind::Teach, n, None, "new idea on a ready foundation", p.lambda * 0.5 * gap));
        }
        if b.taught > 0 || c.taught.contains(&n) {
            if let Some(f) = cx.pick(n, |f| assessable(f)) {
                cands.push(mv(Kind::Practice, n, Some(f), "practice what was just taught",
                    p.lambda * 0.4 * gap + 0.3 * info));
            }
        }
    }
    if cands.is_empty() {
        return close("nothing useful left today");
    }
    let best = cands.iter().map(|m| m.score).fold(f64::MIN, f64::max);
    let ties: Vec<&Move> = cands.iter().filter(|m| m.score >= best - p.tie_eps).collect();
    let mut m = ties[rng.below(ties.len())].clone();
    m.propensity = 1.0 / ties.len() as f64;
    m
}
