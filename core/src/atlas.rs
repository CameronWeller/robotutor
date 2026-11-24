//! The shared knowledge plane: nodes, item families and goal contracts in one release.
//! IDs are minted by code (never by a model). A release is loaded from JSON and validated.

use crate::ledger::{Cue, Facet, FamilyId, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Regime {
    /// Text-assessable capabilities: graded mastery claims allowed.
    Assessable,
    /// Performance (instrument, writing craft): claims only on sub-skills with a validated
    /// checker or rubric. Everything else is witness or self-report, labelled as such.
    Performance,
    /// Character, devotion, wellbeing: never a mastery claim. The tutor coaches practice and
    /// reflection and uses the learner's chosen sources.
    Formative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Checker {
    /// Deterministic answer key (numeric hull, choice, exact form).
    Exact,
    /// Open answer judged by two independent graders against a rubric.
    Rubric,
    /// Machine measurement of a performance (e.g. intonation in cents), deterministic DSP.
    Sensor,
    /// The learner's own log or reflection. Never credit.
    SelfReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Status {
    Verified,
    Quarantined,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    #[serde(default)]
    pub prereqs: Vec<NodeId>,
    #[serde(default)]
    pub misconceptions: Vec<String>,
    /// Passage references that support teaching this node. Empty ⇒ unsupported, never taught.
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Family {
    pub id: FamilyId,
    pub node: NodeId,
    pub facet: Facet,
    pub checker: Checker,
    pub cue: Cue,
    #[serde(default)]
    pub novel_context: bool,
    /// Index into the node's misconception list this family discriminates, if any.
    #[serde(default)]
    pub probes_misconception: Option<usize>,
    /// Logit difficulty in roughly −2..2.
    #[serde(default)]
    pub difficulty: f64,
    /// Instances available before the family repeats.
    #[serde(default = "one")]
    pub variants: u32,
    pub stem: String,
    #[serde(default)]
    pub key: String,
    #[serde(default = "verified")]
    pub status: Status,
}
fn one() -> u32 {
    1
}
fn verified() -> Status {
    Status::Verified
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Goal {
    pub name: String,
    pub statement: String,
    pub regime: Regime,
    /// Capabilities the learner is aiming for; prerequisites are reached through the graph.
    pub targets: Vec<NodeId>,
    pub nodes: Vec<Node>,
    pub families: Vec<Family>,
}

impl Goal {
    pub fn load(json: &str) -> Result<Goal, String> {
        let g: Goal = serde_json::from_str(json).map_err(|e| e.to_string())?;
        g.validate()?;
        Ok(g)
    }

    pub fn node(&self, id: NodeId) -> &Node {
        self.nodes.iter().find(|n| n.id == id).expect("validated release")
    }

    pub fn families_for(&self, node: NodeId) -> impl Iterator<Item = &Family> {
        self.families.iter().filter(move |f| f.node == node && f.status == Status::Verified)
    }

    /// Nodes in prerequisite order (prerequisites first). Ties by id, so the order is stable.
    pub fn topo(&self) -> Vec<NodeId> {
        let mut done = BTreeSet::new();
        let mut out = vec![];
        while out.len() < self.nodes.len() {
            let before = out.len();
            for n in &self.nodes {
                if !done.contains(&n.id) && n.prereqs.iter().all(|p| done.contains(p)) {
                    done.insert(n.id);
                    out.push(n.id);
                }
            }
            assert!(out.len() > before, "validated release has no cycles");
        }
        out
    }

    /// Structural admission rules for a release. A model may propose any of this content;
    /// only a release that passes is served.
    pub fn validate(&self) -> Result<(), String> {
        let ids: BTreeMap<NodeId, &Node> = self.nodes.iter().map(|n| (n.id, n)).collect();
        if ids.len() != self.nodes.len() {
            return Err("duplicate node id".into());
        }
        for n in &self.nodes {
            if n.prereqs.len() > 7 {
                return Err(format!("node {} has more than 7 direct prerequisites", n.id));
            }
            if let Some(p) = n.prereqs.iter().find(|p| !ids.contains_key(p)) {
                return Err(format!("node {} names missing prerequisite {p}", n.id));
            }
        }
        // cycle check by Kahn's algorithm
        let mut indeg: BTreeMap<NodeId, usize> = ids.keys().map(|&k| (k, 0)).collect();
        for n in &self.nodes {
            *indeg.get_mut(&n.id).unwrap() = n.prereqs.len();
        }
        let mut ready: Vec<NodeId> = indeg.iter().filter(|(_, &d)| d == 0).map(|(&k, _)| k).collect();
        let mut seen = 0;
        while let Some(k) = ready.pop() {
            seen += 1;
            for n in self.nodes.iter().filter(|n| n.prereqs.contains(&k)) {
                let d = indeg.get_mut(&n.id).unwrap();
                *d -= 1;
                if *d == 0 {
                    ready.push(n.id);
                }
            }
        }
        if seen != self.nodes.len() {
            return Err("prerequisite cycle".into());
        }
        let fids: BTreeSet<FamilyId> = self.families.iter().map(|f| f.id).collect();
        if fids.len() != self.families.len() {
            return Err("duplicate family id".into());
        }
        for f in &self.families {
            let n = ids.get(&f.node).ok_or(format!("family {} names missing node", f.id))?;
            if let Some(m) = f.probes_misconception {
                if m >= n.misconceptions.len() {
                    return Err(format!("family {} probes unknown misconception", f.id));
                }
            }
            if self.regime == Regime::Formative && f.checker != Checker::SelfReport
                && f.checker != Checker::Rubric
            {
                return Err(format!("formative family {} must be reflective", f.id));
            }
            if f.checker == Checker::Exact && f.key.is_empty() {
                return Err(format!("exact family {} has no key", f.id));
            }
        }
        for t in &self.targets {
            if !ids.contains_key(t) {
                return Err(format!("target {t} missing"));
            }
        }
        Ok(())
    }
}

/// Built-in fixtures: the five learner goals used for design rehearsal.
pub fn fixtures() -> Vec<Goal> {
    [
        include_str!("../fixtures/chemical-engineering.json"),
        include_str!("../fixtures/writer.json"),
        include_str!("../fixtures/violin.json"),
        include_str!("../fixtures/better-person.json"),
        include_str!("../fixtures/worship.json"),
    ]
    .iter()
    .map(|j| Goal::load(j).unwrap_or_else(|e| panic!("fixture: {e}")))
    .collect()
}
