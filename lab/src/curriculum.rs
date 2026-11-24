//! Deterministic planning over one reviewed unit and bounded discovery in a local source index.
//! Only the planner can propose a learner-facing target. Crawl results are acquisition candidates.

pub mod author_input;
pub mod availability;
pub mod creation;
pub mod discovery;
pub mod goal_evidence;
pub mod goal_rehearsal;
pub mod live_author;
pub mod live_turn;
pub mod program;
pub mod provenance;
pub mod rehearsal_release;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OutcomeId(pub u32);

/// Digest of the exact source bytes, supplied by the trusted importer.
pub type Document = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    pub document: Document,
    /// Half-open byte range in the immutable source.
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub id: OutcomeId,
    /// Every edge here is a required prerequisite for this unit.
    pub requires: BTreeSet<OutcomeId>,
    /// All spans are required. Includes support for asserted dependencies.
    pub support: BTreeSet<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitError {
    Duplicate(OutcomeId),
    UnknownPrerequisite {
        outcome: OutcomeId,
        prerequisite: OutcomeId,
    },
    MissingSupport(OutcomeId),
    EmptySpan(OutcomeId),
    Cycle,
}

#[derive(Debug)]
pub struct ReviewedUnit {
    revision: u64,
    outcomes: BTreeMap<OutcomeId, Outcome>,
    order: Vec<OutcomeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub observed: u32,
    /// First day on which an independent return check is due.
    pub due: u32,
}

/// One learner's externally reconciled evidence, pinned to one immutable unit revision.
pub struct Progress {
    pub revision: u64,
    pub checkpoints: BTreeMap<OutcomeId, Checkpoint>,
    /// Active activity holds, resolved from learner preferences, teacher decisions or open reviews.
    pub postponed: BTreeSet<OutcomeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Review,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Teach(OutcomeId),
    Recheck(OutcomeId),
    CurrentUntil(u32),
    RepairSources(OutcomeId),
    Postponed(OutcomeId),
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    WrongRevision,
    UnknownGoal(OutcomeId),
    InvalidCheckpoint(OutcomeId),
}

/// A worked example admitted by the same review boundary as the unit.
pub struct ReviewedExample {
    pub revision: u64,
    pub unit_revision: u64,
    pub outcome: OutcomeId,
    pub support: BTreeSet<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidanceOpportunity {
    Requested,
    Scheduled(Slot),
}

/// Permission to offer the identified worked example at this snapshot. Has no scoring effect.
#[derive(Debug, PartialEq, Eq)]
pub struct GuidedLesson {
    pub outcome: OutcomeId,
    pub example_revision: u64,
    pub overdue_prerequisites: BTreeSet<OutcomeId>,
}

impl ReviewedUnit {
    /// Structural check after release approval, source-byte validation and access checks at import.
    pub fn build(revision: u64, outcomes: Vec<Outcome>) -> Result<Self, UnitError> {
        let mut by_id = BTreeMap::new();
        for outcome in outcomes {
            let id = outcome.id;
            if outcome.support.is_empty() {
                return Err(UnitError::MissingSupport(id));
            }
            if outcome.support.iter().any(|s| s.start >= s.end) {
                return Err(UnitError::EmptySpan(id));
            }
            if by_id.insert(id, outcome).is_some() {
                return Err(UnitError::Duplicate(id));
            }
        }

        let mut remaining = BTreeMap::new();
        let mut children: BTreeMap<OutcomeId, Vec<OutcomeId>> = BTreeMap::new();
        let mut ready = VecDeque::new();
        for (&id, node) in &by_id {
            remaining.insert(id, node.requires.len());
            if node.requires.is_empty() {
                ready.push_back(id);
            }
            for &parent in &node.requires {
                if !by_id.contains_key(&parent) {
                    return Err(UnitError::UnknownPrerequisite {
                        outcome: id,
                        prerequisite: parent,
                    });
                }
                children.entry(parent).or_default().push(id);
            }
        }

        let mut order = Vec::with_capacity(by_id.len());
        while let Some(id) = ready.pop_front() {
            order.push(id);
            for child in children.get(&id).into_iter().flatten() {
                let count = remaining.get_mut(child).expect("known child");
                *count -= 1;
                if *count == 0 {
                    ready.push_back(*child);
                }
            }
        }
        if order.len() != by_id.len() {
            return Err(UnitError::Cycle);
        }
        Ok(Self {
            revision,
            outcomes: by_id,
            order,
        })
    }

    fn scope_at(
        &self,
        goal: OutcomeId,
        progress: &Progress,
        day: u32,
    ) -> Result<Vec<OutcomeId>, PlanError> {
        if progress.revision != self.revision {
            return Err(PlanError::WrongRevision);
        }
        if !self.outcomes.contains_key(&goal) {
            return Err(PlanError::UnknownGoal(goal));
        }
        let mut scope = BTreeSet::new();
        let mut work = vec![goal];
        while let Some(id) = work.pop() {
            if scope.insert(id) {
                work.extend(&self.outcomes[&id].requires);
            }
        }

        let ordered: Vec<_> = self
            .order
            .iter()
            .filter(|id| scope.contains(id))
            .copied()
            .collect();
        for id in &ordered {
            if let Some(c) = progress.checkpoints.get(id) {
                if c.observed > day || c.observed >= c.due {
                    return Err(PlanError::InvalidCheckpoint(*id));
                }
            }
        }
        Ok(ordered)
    }

    /// Select a target within one goal. The caller chooses the slot from the lesson time budget.
    pub fn next(
        &self,
        goal: OutcomeId,
        progress: &Progress,
        day: u32,
        revoked: &BTreeSet<Document>,
        preferred: Slot,
    ) -> Result<Decision, PlanError> {
        let mut current = BTreeSet::new();
        let mut earliest_due = u32::MAX;
        let mut recheck = None;
        let mut teach = None;
        let mut repair = None;
        let mut postponed = None;
        for id in self.scope_at(goal, progress, day)? {
            let outcome = &self.outcomes[&id];
            let checkpoint = progress.checkpoints.get(&id);
            if !outcome.requires.is_subset(&current) {
                continue;
            }
            if outcome
                .support
                .iter()
                .any(|span| revoked.contains(&span.document))
            {
                repair.get_or_insert(id);
                continue;
            }

            match checkpoint {
                Some(c) if day < c.due => {
                    current.insert(id);
                    earliest_due = earliest_due.min(c.due);
                }
                _ if progress.postponed.contains(&id) => {
                    postponed.get_or_insert(id);
                }
                Some(c) => {
                    let overdue = (c.due, id);
                    recheck = Some(recheck.map_or(overdue, |old| overdue.min(old)));
                }
                None => {
                    teach.get_or_insert(id);
                }
            }
        }

        let forward = teach.map(Decision::Teach);
        let review = recheck.map(|(_, id)| Decision::Recheck(id));
        let selection = match preferred {
            Slot::Forward => forward.or(review),
            Slot::Review => review.or(forward),
        };
        Ok(selection
            .or_else(|| repair.map(Decision::RepairSources))
            .or_else(|| postponed.map(Decision::Postponed))
            .unwrap_or(Decision::CurrentUntil(earliest_due)))
    }

    /// Offer a worked synthesis when earlier observations have never overlapped in time,
    /// or their review dates have passed. All strict ancestors require surviving evidence.
    pub fn guided_synthesis(
        &self,
        example: &ReviewedExample,
        progress: &Progress,
        day: u32,
        revoked: &BTreeSet<Document>,
        opportunity: GuidanceOpportunity,
    ) -> Result<Option<GuidedLesson>, PlanError> {
        if example.unit_revision != self.revision {
            return Err(PlanError::WrongRevision);
        }
        let scope = self.scope_at(example.outcome, progress, day)?;
        if opportunity == GuidanceOpportunity::Scheduled(Slot::Review)
            || example.support.is_empty()
            || example
                .support
                .iter()
                .any(|s| s.start >= s.end || revoked.contains(&s.document))
        {
            return Ok(None);
        }

        let mut overdue_prerequisites = BTreeSet::new();
        for id in scope {
            if progress.postponed.contains(&id)
                || self.outcomes[&id]
                    .support
                    .iter()
                    .any(|s| revoked.contains(&s.document))
            {
                return Ok(None);
            }
            if id != example.outcome {
                let Some(checkpoint) = progress.checkpoints.get(&id) else {
                    return Ok(None);
                };
                if day >= checkpoint.due {
                    overdue_prerequisites.insert(id);
                }
            }
        }
        Ok((!overdue_prerequisites.is_empty()).then_some(GuidedLesson {
            outcome: example.outcome,
            example_revision: example.revision,
            overdue_prerequisites,
        }))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CrawlBudget {
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_depth: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discoveries {
    /// Breadth-first, deterministic candidates. This output grants no serving authority.
    pub documents: Vec<Document>,
    pub edges_inspected: usize,
    /// At least one admissible discovery or uninspected adjacency remains outside this run.
    pub limited: bool,
}

/// Walk a pinned, locally indexed collection. Count rejected and duplicate links against work.
pub fn crawl(
    seed: Document,
    links: &BTreeMap<Document, BTreeSet<Document>>,
    permitted: &BTreeSet<Document>,
    budget: CrawlBudget,
) -> Discoveries {
    let mut found = Discoveries::default();
    if !permitted.contains(&seed) {
        return found;
    }
    if budget.max_nodes == 0 {
        found.limited = true;
        return found;
    }

    let mut seen = BTreeSet::from([seed]);
    let mut queue = VecDeque::from([(seed, 0)]);
    found.documents.push(seed);
    while let Some((document, depth)) = queue.pop_front() {
        if let Some(neighbors) = links.get(&document) {
            if depth == budget.max_depth {
                found.limited |= !neighbors.is_empty();
                continue;
            }
            for next in neighbors {
                if found.edges_inspected == budget.max_edges {
                    found.limited = true;
                    return found;
                }
                found.edges_inspected += 1;
                if !permitted.contains(next) || seen.contains(next) {
                    continue;
                }
                if found.documents.len() == budget.max_nodes {
                    found.limited = true;
                    return found;
                }
                seen.insert(*next);
                found.documents.push(*next);
                queue.push_back((*next, depth + 1));
            }
        }
    }
    found
}
