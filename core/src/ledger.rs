//! The evidence ledger and its projection.
//!
//! The ledger is append-only. Every belief about a learner is a pure fold over it, so a
//! correction (`Supersede`) changes the past by adding a row, and re-projection is the
//! backward-feedback mechanism. Constants live here and are versioned with the release.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub type NodeId = u32;
pub type FamilyId = u32;
pub type EventId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Facet {
    Recall,
    Apply,
    Explain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Assist {
    None,
    Hint,
    Worked,
    /// The learner said they used outside help.
    Declared,
    /// Nobody knows. Treated like help: it never earns credit (P5–P7).
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Novelty {
    Fresh,
    Variant,
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Agreement {
    /// A deterministic checker decided.
    Checker,
    /// Two independent graders agreed.
    Agree,
    /// Only one grader answered (timeout or single-grader route).
    Single,
    Disagree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Cue {
    None,
    Partial,
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attempt {
    pub node: NodeId,
    pub facet: Facet,
    pub family: FamilyId,
    /// `None` = the graders could not tell. It carries no weight.
    pub correct: Option<bool>,
    pub agreement: Agreement,
    pub assist: Assist,
    pub novelty: Novelty,
    pub cue: Cue,
    pub novel_context: bool,
    /// Drawn by the unbiased audit slot rather than chosen by the policy (P60).
    pub audit: bool,
    /// The misconception (index into the node's list) this family discriminates, if any.
    pub probes: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Body {
    Attempt(Attempt),
    /// A claim (intake, credential, "I already know this"). Sets where testing starts.
    /// It never moves the posterior (P22).
    Claim { node: NodeId },
    Taught { node: NodeId },
    /// An item from this family was shown. Exposure is a fact, so it lives in the ledger.
    Presented { family: FamilyId },
    /// An interpreter (LLM) suspects this misconception. A hypothesis, not a fact: it only
    /// makes a discriminating probe more valuable. Probes on it confirm or refute it.
    Hypothesis { node: NodeId, misconception: usize },
    /// Backward feedback: the target no longer counts. The reason is kept for audit.
    Supersede { target: EventId, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    /// Minutes since the learner started. Integral so states can be hashed and model-checked.
    pub t: u64,
    pub body: Body,
}

// ---- release constants (one version for all of them) ----
pub const RELEASE: &str = "core/0";
pub const TARGET_RECALL: f64 = 0.85;
pub const S0_DAYS: f64 = 3.0;
pub const SHOWN_MEAN: f64 = 0.8;
pub const EVIDENCE_DECAY: f64 = 0.85;

pub fn weight(a: &Attempt) -> f64 {
    let q = match a.agreement {
        Agreement::Checker | Agreement::Agree => 1.0,
        Agreement::Single => 0.5,
        Agreement::Disagree => 0.0,
    };
    let assist = if a.assist == Assist::None { 1.0 } else { 0.0 };
    let n = match a.novelty {
        Novelty::Fresh => 1.0,
        Novelty::Variant => 0.6,
        Novelty::Repeat => 0.2,
    };
    let c = match a.cue {
        Cue::None => 1.0,
        Cue::Partial => 0.5,
        Cue::Full => 0.0,
    };
    if a.correct.is_none() {
        return 0.0;
    }
    q * assist * n * c
}

pub fn days(minutes: u64) -> f64 {
    minutes as f64 / 1440.0
}

/// Retrievability under a power-law forgetting curve.
pub fn recall(stability: f64, elapsed_days: f64) -> f64 {
    1.0 / (1.0 + elapsed_days.max(0.0) / (9.0 * stability))
}

/// Days until recall falls to `TARGET_RECALL`: 9S(1/τ − 1).
pub fn interval(stability: f64) -> f64 {
    9.0 * stability * (1.0 / TARGET_RECALL - 1.0)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Belief {
    pub alpha: f64,
    pub beta: f64,
    /// Memory stability in days. 0 = no retrieval memory established yet.
    pub stability: f64,
    pub last_retrieval: Option<f64>,
    pub clean_successes: u32,
    pub families_passed: BTreeSet<FamilyId>,
    pub shown_at: Option<f64>,
    pub durable: bool,
    pub transfer: bool,
    pub claimed: bool,
    pub taught: u32,
    pub evidence: u32,
}

impl Belief {
    fn new() -> Self {
        Belief { alpha: 1.0, beta: 1.0, ..Default::default() }
    }
    pub fn mean(&self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }
    pub fn variance(&self) -> f64 {
        let s = self.alpha + self.beta;
        self.alpha * self.beta / (s * s * (s + 1.0))
    }
    pub fn recall_at(&self, now: f64) -> Option<f64> {
        Some(recall(self.stability, now - self.last_retrieval?)).filter(|_| self.stability > 0.0)
    }
    pub fn shown(&self) -> bool {
        self.shown_at.is_some()
    }
    /// Review is due when memory, not mastery, says so: any node with an established
    /// retrieval memory whose predicted recall fell below target (pass 3).
    pub fn due(&self, now: f64) -> bool {
        self.recall_at(now).is_some_and(|r| r < TARGET_RECALL)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Hyp {
    Suspected,
    Confirmed,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Projection {
    pub beliefs: BTreeMap<(NodeId, Facet), Belief>,
    /// Open misconception hypotheses. Refuted ones are removed.
    pub hyps: BTreeMap<(NodeId, usize), Hyp>,
}

impl Projection {
    pub fn get(&self, node: NodeId, facet: Facet) -> Option<&Belief> {
        self.beliefs.get(&(node, facet))
    }
    /// Node-level view: the facet with the most evidence stands for the node.
    pub fn node(&self, node: NodeId) -> Option<&Belief> {
        [Facet::Apply, Facet::Explain, Facet::Recall]
            .iter()
            .filter_map(|f| self.beliefs.get(&(node, *f)))
            .max_by_key(|b| (b.shown(), b.evidence, b.taught))
    }
    pub fn node_shown(&self, node: NodeId) -> bool {
        [Facet::Apply, Facet::Explain, Facet::Recall]
            .iter()
            .any(|f| self.beliefs.get(&(node, *f)).is_some_and(|b| b.shown()))
    }
}

/// Fold the ledger into beliefs. Superseded events are skipped entirely.
pub fn project(events: &[Event]) -> Projection {
    let dead: BTreeSet<EventId> = events
        .iter()
        .filter_map(|e| match &e.body {
            Body::Supersede { target, .. } => Some(*target),
            _ => None,
        })
        .collect();
    let mut p = Projection::default();
    for e in events.iter().filter(|e| !dead.contains(&e.id)) {
        match &e.body {
            // Node-level facts (claims, teaching) live on the Apply facet.
            Body::Claim { node } => {
                p.beliefs.entry((*node, Facet::Apply)).or_insert_with(Belief::new).claimed = true;
            }
            Body::Taught { node } => {
                p.beliefs.entry((*node, Facet::Apply)).or_insert_with(Belief::new).taught += 1;
            }
            Body::Hypothesis { node, misconception } => {
                p.hyps.entry((*node, *misconception)).or_insert(Hyp::Suspected);
            }
            Body::Attempt(a) => {
                apply(p.beliefs.entry((a.node, a.facet)).or_insert_with(Belief::new), a, days(e.t));
                // A discriminating probe that someone could judge settles the hypothesis.
                if let (Some(m), Some(ok)) = (a.probes, a.correct) {
                    if ok && weight(a) > 0.0 {
                        p.hyps.remove(&(a.node, m));
                    } else if !ok && a.assist == Assist::None && a.agreement != Agreement::Disagree {
                        p.hyps.insert((a.node, m), Hyp::Confirmed);
                    }
                }
            }
            Body::Supersede { .. } | Body::Presented { .. } => {}
        }
    }
    p
}

fn apply(b: &mut Belief, a: &Attempt, t: f64) {
    let w = weight(a);
    let y = if a.correct == Some(true) { 1.0 } else { 0.0 };
    if w > 0.0 {
        // The learner is not stationary: old evidence fades toward the prior, so early misses
        // from before they learned stop counting against them (pass 5). A fractional
        // observation fades the past fractionally: factor γ^w. Window ≈ 1/(1−γ).
        let d = EVIDENCE_DECAY.powf(w);
        b.alpha = 1.0 + d * (b.alpha - 1.0);
        b.beta = 1.0 + d * (b.beta - 1.0);
    }
    b.alpha += w * y;
    b.beta += w * (1.0 - y);
    if w > 0.0 {
        b.evidence += 1;
    }
    // Memory changes only with an unassisted, uncued retrieval that someone could judge.
    let retrieval = a.assist == Assist::None && a.cue != Cue::Full && a.correct.is_some()
        && a.agreement != Agreement::Disagree;
    if !retrieval {
        return;
    }
    let gap = b.last_retrieval.map(|l| t - l);
    if a.correct == Some(true) {
        let clean = a.novelty != Novelty::Repeat && a.cue == Cue::None;
        if b.stability == 0.0 {
            b.stability = S0_DAYS;
        } else {
            let r = recall(b.stability, gap.unwrap_or(0.0));
            b.stability = (b.stability * (1.2 * (1.0 - r)).exp()).clamp(1.0, 3650.0);
        }
        if clean {
            b.clean_successes += 1;
            b.families_passed.insert(a.family);
            // Durable: a clean success at least 3 days after the previous retrieval, once the
            // node was already shown. (ponytail: fixed 3-day floor; tie to the goal horizon
            // when real retention data exists.)
            if b.shown_at.is_some() && gap.is_some_and(|g| g >= 3.0) {
                b.durable = true;
            }
            if b.shown_at.is_some() && a.novel_context {
                b.transfer = true;
            }
        }
        if b.shown_at.is_none()
            && b.mean() >= SHOWN_MEAN
            && b.clean_successes >= 2
            && b.families_passed.len() >= 2
        {
            b.shown_at = Some(t);
        }
    } else if b.stability > 0.0 {
        b.stability = (b.stability * (-1.8f64).exp()).max(1.0);
    }
    b.last_retrieval = Some(t);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(family: FamilyId, correct: bool) -> Attempt {
        Attempt {
            node: 1,
            facet: Facet::Apply,
            family,
            correct: Some(correct),
            agreement: Agreement::Checker,
            assist: Assist::None,
            novelty: Novelty::Fresh,
            cue: Cue::None,
            novel_context: false,
            audit: false,
            probes: None,
        }
    }
    fn ev(id: EventId, t_days: f64, body: Body) -> Event {
        Event { id, t: (t_days * 1440.0) as u64, body }
    }

    #[test]
    fn shown_needs_two_clean_successes_on_two_families_and_supersede_removes_it() {
        let mut log = vec![ev(1, 0.0, Body::Attempt(att(10, true)))];
        assert!(!project(&log).beliefs[&(1, Facet::Apply)].shown());
        log.push(ev(2, 0.1, Body::Attempt(att(10, true))));
        assert!(!project(&log).beliefs[&(1, Facet::Apply)].shown(), "same family twice is not enough");
        log.push(ev(3, 0.2, Body::Attempt(att(11, true))));
        assert!(!project(&log).beliefs[&(1, Facet::Apply)].shown(), "decayed mean still below 0.8");
        log.push(ev(5, 0.3, Body::Attempt(att(11, true))));
        assert!(project(&log).beliefs[&(1, Facet::Apply)].shown());
        // The second family turns out to be flawed: backward feedback removes the credit.
        log.push(ev(4, 5.0, Body::Supersede { target: 3, reason: "family 11 quarantined".into() }));
        log.push(ev(6, 5.0, Body::Supersede { target: 5, reason: "family 11 quarantined".into() }));
        assert!(!project(&log).beliefs[&(1, Facet::Apply)].shown());
    }

    #[test]
    fn assisted_claimed_and_disputed_work_earns_nothing() {
        let mut a = att(10, true);
        a.assist = Assist::Unknown;
        let mut d = att(11, true);
        d.agreement = Agreement::Disagree;
        let log = vec![
            ev(1, 0.0, Body::Claim { node: 1 }),
            ev(2, 0.0, Body::Attempt(a)),
            ev(3, 0.0, Body::Attempt(d)),
        ];
        let b = &project(&log).beliefs[&(1, Facet::Apply)];
        assert_eq!((b.alpha, b.beta), (1.0, 1.0));
        assert!(b.claimed);
    }

    #[test]
    fn durable_requires_a_real_gap_after_shown() {
        let log = vec![
            ev(1, 0.0, Body::Attempt(att(10, true))),
            ev(2, 0.0, Body::Attempt(att(11, true))),
            ev(3, 0.1, Body::Attempt(att(14, true))),
            ev(5, 0.2, Body::Attempt(att(15, true))),
            ev(6, 0.5, Body::Attempt(att(12, true))),
        ];
        assert!(!project(&log).beliefs[&(1, Facet::Apply)].durable, "half a day is not a delay");
        let mut log2 = log.clone();
        log2.push(ev(4, 9.0, Body::Attempt(att(13, true))));
        assert!(project(&log2).beliefs[&(1, Facet::Apply)].durable);
    }

    #[test]
    fn a_probe_confirms_or_refutes_a_hypothesis() {
        let hyp = ev(1, 0.0, Body::Hypothesis { node: 1, misconception: 0 });
        let mut miss = att(20, false);
        miss.probes = Some(0);
        let mut pass = att(21, true);
        pass.probes = Some(0);
        let p = project(&[hyp.clone(), ev(2, 0.0, Body::Attempt(miss))]);
        assert_eq!(p.hyps.get(&(1, 0)), Some(&Hyp::Confirmed));
        let p = project(&[hyp, ev(2, 0.0, Body::Attempt(pass))]);
        assert!(p.hyps.is_empty());
    }
}
