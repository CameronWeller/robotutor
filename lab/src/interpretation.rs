//! A quoted passage locates evidence for a reviewer. Only the reviewer's own
//! criterion-specific assessment and feedback may be shown to the learner.
use crate::{
    audience::Roster,
    context::{Authority, Kind, Packet, Purpose, Record, RecordRef},
    curriculum::{Document, OutcomeId},
    material::document_id,
    protocol::Assistance,
    routing::{Draft, Intent, Request, Role, Router, Ticket},
};
use std::collections::{btree_map::Entry, BTreeMap};

/// A criterion of an immutable presented item. The broker pins the observed
/// assistance before dispatch. Other criteria on the item can have other conditions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Criterion {
    pub presentation: u64,
    pub id: u64,
    pub rubric: RecordRef,
    pub response: RecordRef,
    pub assistance: Assistance,
}

/// A durable router operation within this installation's budget partition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewRevision {
    pub epoch: u64,
    pub operation: u64,
}

/// The trusted reviewer-assignment broker binds the predecessor, when present,
/// before commissioning a replacement assessment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewOrder {
    pub criterion: Criterion,
    pub replaces: Option<ReviewRevision>,
}

impl ReviewOrder {
    pub fn binding(&self) -> Document {
        let criterion = self.criterion;
        let help = match criterion.assistance {
            Assistance::NoneReported => "none_reported",
            Assistance::Reported => "reported",
            Assistance::Unknown => "unknown",
        };
        let manifest = serde_json::json!([
            "open-criterion/2",
            criterion.presentation,
            criterion.id,
            [criterion.rubric.id, criterion.rubric.revision],
            [criterion.response.id, criterion.response.revision],
            help,
            self.replaces.map(|r| [r.epoch, r.operation])
        ]);
        document_id(manifest.to_string().as_bytes())
    }
}

pub struct Candidate<'a> {
    pub ticket: Ticket,
    pub bytes: &'a [u8],
    pub rubric: &'a Record,
    pub response: &'a Record,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Finding {
    Meets,
    NotYet,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Suggestion {
    pub proposed: Finding,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unauthorized,
    Stale,
    WrongContract,
    MissingReceipt,
    InvalidOutput,
    InvalidFeedback,
    AlreadyReviewed,
    CaseConflict,
}

/// Trusted assessment authority grants a particular person responsibility for
/// this exact job and criterion. Current teacher membership is checked on use.
pub struct ReviewerPermit {
    pub teacher: u64,
    pub request: Request,
}

#[derive(Debug, Eq, PartialEq)]
struct SignedFeedback {
    teacher: u64,
    finding: Finding,
    text: String,
}

/// Presentations are allocated within a class, learner, unit release and objective.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CaseKey {
    pub class: u64,
    pub learner: u64,
    pub unit: u64,
    pub outcome: OutcomeId,
    pub presentation: u64,
    pub criterion: u64,
}

/// One canonical collection per installation and router budget partition.
/// A durable adapter needs a unique case row, an atomic predecessor comparison
/// and immutable revision receipts in the same transaction.
#[derive(Default)]
pub struct ReviewBook {
    cases: BTreeMap<CaseKey, CaseHistory>,
}

struct CaseHistory {
    active: OpenReview,
    prior: Vec<OpenReview>,
}

pub struct OpenReview {
    draft: Draft,
    criterion: Criterion,
    request: Request,
    class: u64,
    suggestion: Option<Suggestion>,
    signed: Option<SignedFeedback>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LearnerFeedback<'a> {
    Pending,
    UnderReview,
    Reviewed {
        criterion: u64,
        presentation: u64,
        unit_revision: u64,
        assistance: Assistance,
        finding: Finding,
        text: &'a str,
    },
}

fn read_suggestion(bytes: &[u8], response: &str) -> Result<Option<Suggestion>, Error> {
    // The only permitted outputs are JSON null or [finding, quote].
    let decoded: Option<(String, String)> =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidOutput)?;
    let Some((outcome, quote)) = decoded else {
        return Ok(None);
    };
    let proposed = match outcome.as_str() {
        "met" => Finding::Meets,
        "not_yet" => Finding::NotYet,
        _ => return Err(Error::InvalidOutput),
    };
    // UTF-8 string search yields byte boundaries. Comparing the first and
    // last match also detects overlapping repetitions such as "aa" in "aaa".
    if quote.trim().is_empty() {
        return Err(Error::InvalidOutput);
    }
    let start = response.find(&quote).ok_or(Error::InvalidOutput)?;
    if response.rfind(&quote) != Some(start) {
        return Err(Error::InvalidOutput);
    }
    let end = start + quote.len();
    Ok(Some(Suggestion {
        proposed,
        start,
        end,
    }))
}

/// Verify before looking up or changing the case. A replay cannot bypass the
/// current grant or substitute materials for the original response.
fn prepare(
    order: ReviewOrder,
    candidate: Candidate<'_>,
    router: &Router,
    packet: &Packet,
    current: &Authority,
    now_ms: u64,
) -> Result<OpenReview, Error> {
    let criterion = order.criterion;
    let ticket = candidate.ticket;
    let request = router.request_for(ticket).ok_or(Error::MissingReceipt)?;
    if now_ms >= request.deadline_ms || packet.dispatch(current).is_err() {
        return Err(Error::Stale);
    }
    if ticket.role != Role::Assessor
        || request.need.intent != Intent::AssessOpen
        || packet.purpose() != Purpose::Evaluate
        || request.need.packet != packet.digest()
        || request.binding != order.binding()
        || candidate.rubric.key != criterion.rubric
        || candidate.rubric.kind != Kind::Rubric
        || candidate.response.key != criterion.response
        || candidate.response.kind != Kind::Response
        || !packet.includes_record(candidate.rubric)
        || !packet.includes_record(candidate.response)
    {
        return Err(Error::WrongContract);
    }
    let draft = router.draft(ticket).ok_or(Error::MissingReceipt)?;
    if candidate.bytes.len() > 2048 || document_id(candidate.bytes) != draft.artifact {
        return Err(Error::InvalidOutput);
    }
    let suggestion = read_suggestion(candidate.bytes, &candidate.response.text)?;
    Ok(OpenReview {
        draft,
        criterion,
        request,
        class: current.snapshot.class,
        suggestion,
        signed: None,
    })
}

impl ReviewBook {
    /// Replays return the existing disposition. A replacement can only advance
    /// its named active predecessor. Earlier revisions stay out of current reads.
    pub fn admit(
        &mut self,
        order: ReviewOrder,
        candidate: Candidate<'_>,
        router: &Router,
        packet: &Packet,
        current: &Authority,
        now_ms: u64,
    ) -> Result<&mut OpenReview, Error> {
        let proposed = prepare(order, candidate, router, packet, current, now_ms)?;
        let history = match self.cases.entry(proposed.key()) {
            Entry::Vacant(slot) if order.replaces.is_none() => slot.insert(CaseHistory {
                active: proposed,
                prior: Vec::new(),
            }),
            Entry::Vacant(_) => return Err(Error::CaseConflict),
            Entry::Occupied(slot) => {
                let history = slot.into_mut();
                let active = &history.active;
                if active.request != proposed.request || active.draft != proposed.draft {
                    if order.replaces != Some(active.revision())
                        || proposed.revision() == active.revision()
                    {
                        return Err(Error::CaseConflict);
                    }
                    history
                        .prior
                        .push(std::mem::replace(&mut history.active, proposed));
                }
                history
            }
        };
        Ok(&mut history.active)
    }

    /// Read or adjudicate an existing case after its provider deadline. Actor,
    /// permit, roster and current source checks still apply on every access.
    pub fn current(&mut self, key: CaseKey) -> Option<&mut OpenReview> {
        self.cases.get_mut(&key).map(|history| &mut history.active)
    }
}

impl OpenReview {
    pub fn key(&self) -> CaseKey {
        let need = self.request.need;
        CaseKey {
            class: self.class,
            learner: need.scope.learner,
            unit: need.scope.revision,
            outcome: need.outcome,
            presentation: self.criterion.presentation,
            criterion: self.criterion.id,
        }
    }

    pub fn revision(&self) -> ReviewRevision {
        ReviewRevision {
            epoch: self.draft.ticket.epoch,
            operation: self.request.id,
        }
    }

    fn current(&self, packet: &Packet, authority: &Authority) -> bool {
        packet.digest() == self.request.need.packet && packet.dispatch(authority).is_ok()
    }

    /// Return just the proposed finding and its position. The teacher inspects
    /// the separately governed original answer and rubric to judge its meaning.
    pub fn inspect(
        &self,
        actor: u64,
        permit: &ReviewerPermit,
        roster: &Roster,
        packet: &Packet,
        authority: &Authority,
    ) -> Result<Option<Suggestion>, Error> {
        if permit.teacher != actor
            || permit.request != self.request
            || roster.class != self.class
            || !roster.teachers.contains(&actor)
            || !roster.learners.contains(&self.request.need.scope.learner)
        {
            return Err(Error::Unauthorized);
        }
        if !self.current(packet, authority) {
            return Err(Error::Stale);
        }
        Ok(self.suggestion)
    }

    /// The designated reviewer may plan instruction from a signed criterion
    /// finding. An unsigned suggestion contributes no finding to that decision.
    pub fn signed_finding(
        &self,
        actor: u64,
        permit: &ReviewerPermit,
        roster: &Roster,
        packet: &Packet,
        authority: &Authority,
    ) -> Result<Option<Finding>, Error> {
        self.inspect(actor, permit, roster, packet, authority)?;
        Ok(self.signed.as_ref().map(|s| s.finding))
    }

    /// The reviewer supplies their own feedback even when agreeing with the
    /// suggestion. A different adjudication requires a superseding review.
    pub fn adjudicate(
        &mut self,
        actor: u64,
        permit: &ReviewerPermit,
        roster: &Roster,
        packet: &Packet,
        authority: &Authority,
        decision: (Finding, &str),
    ) -> Result<(), Error> {
        self.inspect(actor, permit, roster, packet, authority)?;
        let (finding, feedback) = decision;
        if feedback.trim().is_empty() || feedback.len() > 2048 {
            return Err(Error::InvalidFeedback);
        }
        let signed = SignedFeedback {
            teacher: permit.teacher,
            finding,
            text: feedback.to_owned(),
        };
        if self
            .signed
            .as_ref()
            .is_some_and(|previous| previous != &signed)
        {
            return Err(Error::AlreadyReviewed);
        }
        self.signed = Some(signed);
        Ok(())
    }

    pub fn learner_view(
        &self,
        actor: u64,
        roster: &Roster,
        packet: &Packet,
        authority: &Authority,
    ) -> Result<LearnerFeedback<'_>, Error> {
        if actor != self.request.need.scope.learner
            || roster.class != self.class
            || !roster.learners.contains(&actor)
        {
            return Err(Error::Unauthorized);
        }
        if !self.current(packet, authority) {
            return Ok(LearnerFeedback::UnderReview);
        }
        Ok(match &self.signed {
            None => LearnerFeedback::Pending,
            Some(signed) => LearnerFeedback::Reviewed {
                criterion: self.criterion.id,
                presentation: self.criterion.presentation,
                unit_revision: self.request.need.scope.revision,
                assistance: self.criterion.assistance,
                finding: signed.finding,
                text: &signed.text,
            },
        })
    }
}
