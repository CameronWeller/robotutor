//! Rebuild one learner's scheduling checkpoints after an item or judgment is corrected.
//! The caller supplies admitted observations and the complete dependency lists of
//! their item, rubric and checker. This module makes no ownership or grading decisions.
pub mod adjudication;

use crate::curriculum::{Checkpoint, Document, OutcomeId, Progress};
use crate::protocol::{Assistance, Judgment, Question};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub learner: u64,
    /// Identifies one immutable unit release within this installation.
    pub revision: u64,
}

/// Each ID identifies one immutable assessment revision in this scope. Regrading
/// withdraws its ID and admits a new revision for the same performance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub scope: Scope,
    pub id: u64,
    /// Stable order of the learner performance, preserved through regrading.
    pub sequence: u64,
    pub outcome: OutcomeId,
    /// Immutable item revision, including its rubric.
    pub item: u64,
    pub day: u32,
    pub question: Question,
    pub judgment: Judgment,
    /// Resolved from observed product help and outside-help disclosure.
    pub assistance: Assistance,
    /// This exact task was fresh under the recorded exposure policy.
    pub fresh: bool,
    pub sources: BTreeSet<Document>,
}

/// The scheduling policy names two supporting assessments and supplies a return
/// date. The date stays tied to the confirmation that originally produced it.
#[derive(Clone, Copy, Debug)]
pub struct CheckpointProposal {
    pub scope: Scope,
    pub outcome: OutcomeId,
    pub first: u64,
    pub confirmation: u64,
    pub due: u32,
}

#[derive(Clone, Debug)]
pub struct Withdrawn {
    pub scope: Scope,
    /// Active activity holds pass through to the planner independently of credit.
    pub postponed: BTreeSet<OutcomeId>,
    pub attempts: BTreeSet<u64>,
    pub items: BTreeSet<u64>,
    pub sources: BTreeSet<Document>,
}

impl Withdrawn {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            postponed: BTreeSet::new(),
            attempts: BTreeSet::new(),
            items: BTreeSet::new(),
            sources: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryError {
    ScopeMismatch,
    ConflictingAttempt(u64),
    OverlappingRevisions { outcome: OutcomeId, sequence: u64 },
    ConflictingDue { first: u64, confirmation: u64 },
}

/// Projects the latest supported confirmation for each outcome. Rebuilding uses
/// today's withdrawals even when inspecting an older day. The result can be
/// passed directly to ReviewedUnit::next for this learner and release.
pub fn reconcile(
    scope: Scope,
    day: u32,
    attempts: &[Attempt],
    proposals: &[CheckpointProposal],
    withdrawn: &Withdrawn,
) -> Result<Progress, HistoryError> {
    if withdrawn.scope != scope {
        return Err(HistoryError::ScopeMismatch);
    }

    let mut observed = BTreeMap::new();
    let mut last_miss = BTreeMap::new();
    let mut active = BTreeMap::new();
    for attempt in attempts {
        if attempt.scope != scope {
            return Err(HistoryError::ScopeMismatch);
        }
        if let Some(previous) = observed.insert(attempt.id, attempt) {
            if previous != attempt {
                return Err(HistoryError::ConflictingAttempt(attempt.id));
            }
        }
        if !withdrawn.attempts.contains(&attempt.id) {
            let key = (attempt.outcome, attempt.sequence);
            if active
                .insert(key, attempt.id)
                .is_some_and(|id| id != attempt.id)
            {
                return Err(HistoryError::OverlappingRevisions {
                    outcome: attempt.outcome,
                    sequence: attempt.sequence,
                });
            }
        }
        if attempt.judgment == Judgment::Incorrect && eligible(attempt, day, withdrawn) {
            let stamp = (attempt.day, attempt.sequence);
            let latest = last_miss.entry(attempt.outcome).or_insert(stamp);
            *latest = (*latest).max(stamp);
        }
    }

    let mut dates = BTreeMap::new();
    let mut latest: BTreeMap<OutcomeId, ((u32, u64), Checkpoint)> = BTreeMap::new();
    for proposal in proposals {
        if proposal.scope != scope {
            return Err(HistoryError::ScopeMismatch);
        }
        let (Some(first), Some(check)) = (
            observed.get(&proposal.first),
            observed.get(&proposal.confirmation),
        ) else {
            continue;
        };
        let first_stamp = (first.day, first.sequence);
        let stamp = (check.day, check.sequence);
        let valid_return = match check.question {
            Question::ChangedExample => first_stamp < stamp,
            Question::DelayedReview => first.day < check.day,
            _ => false,
        };
        if !valid_return
            || first.item == check.item
            || first.outcome != proposal.outcome
            || check.outcome != proposal.outcome
            || proposal.due <= check.day
            || last_miss
                .get(&proposal.outcome)
                .is_some_and(|miss| *miss >= first_stamp)
            || ![first, check].iter().all(|attempt| {
                attempt.judgment == Judgment::Correct && eligible(attempt, day, withdrawn)
            })
        {
            continue;
        }
        if let Some(previous) = dates.insert((first.id, check.id), proposal.due) {
            if previous != proposal.due {
                return Err(HistoryError::ConflictingDue {
                    first: first.id,
                    confirmation: check.id,
                });
            }
        }
        let checkpoint = Checkpoint {
            observed: check.day,
            due: proposal.due,
        };
        let entry = latest
            .entry(proposal.outcome)
            .or_insert((stamp, checkpoint));
        // Distinct supporting pairs can finish on the same confirmation.
        // Keep the earlier review if those proposals have different due dates.
        if stamp > entry.0 || (stamp == entry.0 && checkpoint.due < entry.1.due) {
            *entry = (stamp, checkpoint);
        }
    }
    Ok(Progress {
        revision: scope.revision,
        postponed: withdrawn.postponed.clone(),
        checkpoints: latest
            .into_iter()
            .map(|(outcome, (_, checkpoint))| (outcome, checkpoint))
            .collect(),
    })
}

pub(crate) fn eligible(attempt: &Attempt, day: u32, withdrawn: &Withdrawn) -> bool {
    attempt.day <= day
        && attempt.fresh
        && attempt.question != Question::Clarification
        && attempt.assistance == Assistance::NoneReported
        && !attempt.sources.is_empty()
        && !withdrawn.attempts.contains(&attempt.id)
        && !withdrawn.items.contains(&attempt.item)
        && attempt.sources.is_disjoint(&withdrawn.sources)
}
