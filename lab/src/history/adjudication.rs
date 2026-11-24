//! Bind a signed criterion review to the historical assessment it can replace.
use super::{Attempt, CheckpointProposal};
use crate::audience::Roster;
use crate::context::{Authority, Packet};
use crate::interpretation::{
    CaseKey, Criterion, Error, Finding, OpenReview, ReviewOrder, ReviewRevision, ReviewerPermit,
};
use crate::protocol::Judgment;

/// Historical receipt assembled at issue/submission time by the owning application.
/// The item and criterion are separate identities. A later reviewer cannot change either.
pub struct DisputedAssessment {
    pub original: Attempt,
    pub class: u64,
    pub criterion: Criterion,
}

pub struct ReviewAccess<'a> {
    pub actor: u64,
    pub permit: &'a ReviewerPermit,
    pub roster: &'a Roster,
    pub packet: &'a Packet,
    pub authority: &'a Authority,
}

pub struct Replacement {
    predecessor: u64,
    attempt: Attempt,
}

impl DisputedAssessment {
    pub fn key(&self) -> CaseKey {
        CaseKey {
            class: self.class,
            learner: self.original.scope.learner,
            unit: self.original.scope.revision,
            outcome: self.original.outcome,
            presentation: self.criterion.presentation,
            criterion: self.criterion.id,
        }
    }

    /// The caller loads the active case and predecessor under one snapshot. It allocates
    /// an immutable assessment ID for the signed revision in the historical ID namespace.
    /// A pending or unresolved case keeps its original withdrawn with no replacement.
    pub fn replacement(
        &self,
        new_id: u64,
        predecessor: Option<ReviewRevision>,
        review: &OpenReview,
        access: &ReviewAccess<'_>,
    ) -> Result<Option<Replacement>, Error> {
        let order = ReviewOrder {
            criterion: self.criterion,
            replaces: predecessor,
        };
        if new_id == self.original.id
            || review.key() != self.key()
            || self.criterion.assistance != self.original.assistance
            || order.binding() != access.permit.request.binding
        {
            return Err(Error::WrongContract);
        }
        let judgment = match review.signed_finding(
            access.actor,
            access.permit,
            access.roster,
            access.packet,
            access.authority,
        )? {
            Some(Finding::Meets) => Judgment::Correct,
            Some(Finding::NotYet) => Judgment::Incorrect,
            Some(Finding::Unresolved) | None => return Ok(None),
        };
        Ok(Some(Replacement {
            predecessor: self.original.id,
            attempt: Attempt {
                id: new_id,
                judgment,
                ..self.original.clone()
            },
        }))
    }
}

impl Replacement {
    pub fn attempt(&self) -> &Attempt {
        &self.attempt
    }

    /// Preserve the original schedule attached to a support pair. The reconciler still
    /// checks both performances, distinct items, source eligibility and later misses.
    pub fn rebind(&self, proposal: CheckpointProposal) -> Option<CheckpointProposal> {
        if proposal.scope != self.attempt.scope || proposal.outcome != self.attempt.outcome {
            return None;
        }
        let mut bound = proposal;
        if bound.first == self.predecessor {
            bound.first = self.attempt.id;
        } else if bound.confirmation == self.predecessor {
            bound.confirmation = self.attempt.id;
        } else {
            return None;
        }
        Some(bound)
    }
}
