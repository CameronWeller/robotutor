//! Admission and metering of one teaching turn. This module owns no provider,
//! question bank, grader or publishing credentials. Monetary values are integer
//! microdollars. Each router represents one shared budget and idempotency epoch.

pub mod profiles;

use crate::context::{Authority, Count, Hold, Messages, Packet, Purpose, Snapshot};
use crate::curriculum::{Document, OutcomeId};
use crate::history::Scope;
use crate::protocol::Question;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Explain,
    Ask(Question),
    /// Draft parameters within a published template for the learner's selected question.
    PersonalizeForm(Question),
    AuthorForm,
    GradeNumeric,
    AssessOpen,
    Dispute,
}

impl Intent {
    fn purpose(self) -> Purpose {
        match self {
            Self::Explain => Purpose::Teach,
            Self::AuthorForm => Purpose::Author,
            Self::PersonalizeForm(_) => Purpose::PrivateForm,
            Self::Ask(_) => Purpose::Probe,
            Self::GradeNumeric | Self::AssessOpen | Self::Dispute => Purpose::Evaluate,
        }
    }

    fn role(self) -> Option<Role> {
        match self {
            Self::AuthorForm
            | Self::Explain
            | Self::Ask(Question::Practice | Question::Clarification)
            | Self::PersonalizeForm(Question::Practice | Question::Clarification) => {
                Some(Role::Language)
            }
            Self::Ask(_) | Self::PersonalizeForm(_) => Some(Role::Reasoner),
            Self::AssessOpen => Some(Role::Assessor),
            Self::GradeNumeric | Self::Dispute => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Need {
    pub scope: Scope,
    pub outcome: OutcomeId,
    pub intent: Intent,
    /// Identity of the exact authorized messages, turn and source revisions.
    pub packet: Document,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// Unique, durable operation ID within this router's budget partition.
    pub id: u64,
    pub need: Need,
    /// Digest of the broker's immutable operation contract (such as the template, rubric and intended audience).
    pub binding: Document,
    pub deadline_ms: u64,
    pub max_micros: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Language,
    Reasoner,
    Assessor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// Immutable model, price, capability evaluation and tokenizer revision.
    pub revision: u64,
    /// Upper bound for one attempt with this packet's supported input/output caps.
    pub maximum_micros: u64,
    /// Required time left before dispatch, including a margin for downstream review.
    pub admission_ms: u64,
    pub slots: u16,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reviewed {
    pub valid_until_ms: u64,
    /// A trusted release broker checked rights, task fit, freshness and the exact
    /// approved artifact for this request. An artifact can be shared across grants.
    pub need: Need,
    pub artifact: Document,
    pub binding: Document,
}

/// A scoped lesson or presentation broker authenticates the local move against
/// this snapshot. For this path, Need.packet identifies the immutable local
/// turn contract. It requires no model messages or token counter.
pub struct LocalGrant {
    pub valid_until_ms: u64,
    pub binding: Document,
    pub snapshot: Snapshot,
    pub need: Need,
}

/// Fresh observations from trusted adapters. Permissions and profiles are scoped
/// to the current tenant, use, processing agreement, modality and retention rules.
#[derive(Default)]
pub struct Options {
    pub reviewed: Option<Reviewed>,
    pub exact_checker: bool,
    pub human_available: bool,
    pub permitted: std::collections::BTreeSet<Role>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub epoch: u64,
    pub request_id: u64,
    pub need: Need,
    pub role: Role,
    pub model_revision: u64,
    pub attempt: u8,
    pub maximum_micros: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draft {
    pub ticket: Ticket,
    /// SHA-256 of the exact candidate bytes retained in quarantine.
    pub artifact: Document,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Unauthorized,
    ChangedRequest,
    ChangedRelease,
    Deadline,
    NoChecker,
    Dispute,
    Unavailable,
    NoCapacity,
    Budget,
    AttemptLimit,
    FinalFailure,
    ReceiptCapacity,
    ProfileMismatch,
    TokenLimit,
    BillingHold,
    NoPendingCall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    UseReviewed(Document),
    CheckLocally,
    /// Only this response grants the adapter permission to initiate one call.
    Dispatch(Ticket),
    InFlight(Ticket),
    ReviewDraft(Draft),
    Human(Reason),
    Abstain(Reason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Returned {
    Draft(Document),
    /// Definitively closed and billed by the provider; another attempt is permitted.
    Retryable,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub result: Returned,
    pub charged_micros: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettlementError {
    UnknownTicket,
    ConflictingReceipt,
    ChargeAboveReservation,
}

struct Attempt {
    ticket: Ticket,
    completion: Option<Completion>,
    running: bool,
}

struct Entry {
    request: Request,
    attempts: Vec<Attempt>,
}

pub struct Router {
    epoch: u64,
    budget: u64,
    max_attempts: u8,
    max_requests: usize,
    models: BTreeMap<Role, BTreeMap<u64, Model>>,
    entries: BTreeMap<u64, Entry>,
    occupied: BTreeMap<(Role, u64), u16>,
    reserved: u64,
    spent: u64,
    billing_hold: bool,
}

fn handoff(options: &Options, why: Reason) -> Action {
    if options.human_available {
        Action::Human(why)
    } else {
        Action::Abstain(why)
    }
}

fn local_choice(request: Request, options: &Options, now_ms: u64) -> Option<Action> {
    let need = request.need;
    match need.intent {
        Intent::GradeNumeric if options.exact_checker => Some(Action::CheckLocally),
        Intent::GradeNumeric => Some(handoff(options, Reason::NoChecker)),
        Intent::Dispute => Some(handoff(options, Reason::Dispute)),
        Intent::AssessOpen => None,
        _ => options.reviewed.map(|grant| {
            if now_ms >= grant.valid_until_ms {
                Action::Abstain(Reason::Unauthorized)
            } else if grant.need == need && grant.binding == request.binding {
                Action::UseReviewed(grant.artifact)
            } else {
                Action::Abstain(Reason::ChangedRelease)
            }
        }),
    }
}

/// A local action requires current authorization and its own final artifact or
/// checker admission. It has no remote side effects or provider receipt limit.
pub fn route_local(
    request: Request,
    authority: &Authority,
    grant: LocalGrant,
    options: &Options,
    now_ms: u64,
) -> Action {
    let snapshot = authority.snapshot;
    if now_ms >= grant.valid_until_ms
        || grant.need != request.need
        || grant.binding != request.binding
        || grant.snapshot != snapshot
        || snapshot.scope != request.need.scope
        || snapshot.outcome != request.need.outcome
        || snapshot.purpose != request.need.intent.purpose()
    {
        return Action::Abstain(Reason::Unauthorized);
    }
    if now_ms >= request.deadline_ms {
        return Action::Abstain(Reason::Deadline);
    }
    local_choice(request, options, now_ms).unwrap_or_else(|| handoff(options, Reason::Unavailable))
}

fn messages_for_need<'a>(
    need: Need,
    packet: &'a Packet,
    authority: &Authority,
) -> Result<&'a Messages, Reason> {
    if packet.scope() != need.scope
        || packet.outcome() != need.outcome
        || packet.purpose() != need.intent.purpose()
        || packet.digest() != need.packet
    {
        return Err(Reason::Unauthorized);
    }
    packet.dispatch(authority).map_err(|hold| match hold {
        Hold::Unmeasured => Reason::TokenLimit,
        _ => Reason::Unauthorized,
    })
}

impl Router {
    pub fn new(
        epoch: u64,
        budget_micros: u64,
        max_attempts: u8,
        max_requests: usize,
        models: BTreeMap<Role, Model>,
    ) -> Self {
        Self {
            epoch,
            budget: budget_micros,
            max_attempts,
            max_requests,
            models: models
                .into_iter()
                .map(|(role, model)| (role, BTreeMap::from([(model.revision, model)])))
                .collect(),
            entries: BTreeMap::new(),
            occupied: BTreeMap::new(),
            reserved: 0,
            spent: 0,
            billing_hold: false,
        }
    }

    pub fn reserved_micros(&self) -> u64 {
        self.reserved
    }
    pub fn spent_micros(&self) -> u64 {
        self.spent
    }

    pub fn remaining_micros(&self) -> u64 {
        self.budget - self.spent - self.reserved
    }

    /// Momentary preflight view. Only admission can reserve the hinted capacity.
    pub fn available_profile(&self, role: Role, revision: u64) -> Option<Model> {
        let model = *self.models.get(&role)?.get(&revision)?;
        let used = self.occupied.get(&(role, revision)).copied().unwrap_or(0);
        (!self.billing_hold && used < model.slots).then_some(model)
    }

    /// Install another immutable profile in the same accounting partition.
    /// Role/profile slots represent disjoint, operator-allocated provider capacity.
    pub fn register_profile(&mut self, role: Role, model: Model) -> Result<(), Reason> {
        let configured = self.models.entry(role).or_default();
        if configured
            .get(&model.revision)
            .is_some_and(|old| *old != model)
        {
            return Err(Reason::ProfileMismatch);
        }
        configured.insert(model.revision, model);
        Ok(())
    }

    /// Every call, including replays and draft reads, checks today's authority.
    /// The adapter dispatches the same packet and caps named by the ticket, after
    /// persisting the reservation and outbox atomically.
    pub fn route(
        &mut self,
        request: Request,
        packet: &Packet,
        authority: &Authority,
        options: &Options,
        now_ms: u64,
    ) -> Action {
        let messages = match messages_for_need(request.need, packet, authority) {
            Ok(messages) => messages,
            Err(reason) => return Action::Abstain(reason),
        };
        let Count::Measured(input) = packet.count() else {
            return Action::Abstain(Reason::TokenLimit);
        };
        self.admit(
            request,
            options,
            now_ms,
            authority.snapshot.profile,
            input,
            messages.max_output_tokens,
        )
    }

    fn opening(&self, ticket: Ticket) -> Option<(&Entry, &Attempt)> {
        let entry = self.entries.get(&ticket.request_id)?;
        let at = usize::from(ticket.attempt.checked_sub(1)?);
        let attempt = entry.attempts.get(at)?;
        (attempt.ticket == ticket).then_some((entry, attempt))
    }

    /// The immutable operation originally admitted for this ticket. Publication
    /// checks the digest and deadline here rather than trusting the completion.
    pub fn request_for(&self, ticket: Ticket) -> Option<Request> {
        self.opening(ticket).map(|(entry, _)| entry.request)
    }

    /// Final check by the worker holding the outbox claim. This read cannot
    /// acquire a transport lease; repeat sends still require provider idempotency.
    pub fn messages_for<'a>(
        &self,
        ticket: Ticket,
        packet: &'a Packet,
        current: &Authority,
        options: &Options,
        now_ms: u64,
    ) -> Result<&'a Messages, Reason> {
        let (entry, attempt) = self.opening(ticket).ok_or(Reason::NoPendingCall)?;
        if !attempt.running || attempt.completion.is_some() {
            return Err(Reason::NoPendingCall);
        }
        if self.billing_hold {
            return Err(Reason::BillingHold);
        }
        if !options.permitted.contains(&ticket.role) {
            return Err(Reason::Unauthorized);
        }
        if local_choice(entry.request, options, now_ms).is_some() {
            return Err(Reason::ChangedRelease);
        }
        let remaining = entry
            .request
            .deadline_ms
            .checked_sub(now_ms)
            .ok_or(Reason::Deadline)?;
        let model = self
            .models
            .get(&ticket.role)
            .and_then(|profiles| profiles.get(&ticket.model_revision))
            .ok_or(Reason::Unavailable)?;
        if remaining == 0 || remaining < model.admission_ms {
            return Err(Reason::Deadline);
        }
        messages_for_need(entry.request.need, packet, current)
    }

    /// Inspect a completed candidate by its exact immutable billing receipt.
    /// This operation cannot dispatch an attempt or publish bytes.
    pub fn draft(&self, ticket: Ticket) -> Option<Draft> {
        let (_, attempt) = self.opening(ticket)?;
        match attempt.completion?.result {
            Returned::Draft(artifact) => Some(Draft { ticket, artifact }),
            _ => None,
        }
    }

    fn admit(
        &mut self,
        request: Request,
        options: &Options,
        now_ms: u64,
        profile: u64,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Action {
        if let Some(previous) = self.entries.get(&request.id) {
            if previous.request != request {
                return Action::Abstain(Reason::ChangedRequest);
            }
        }
        if now_ms >= request.deadline_ms {
            return Action::Abstain(Reason::Deadline);
        }
        let attempts = self
            .entries
            .get(&request.id)
            .map_or(&[][..], |entry| entry.attempts.as_slice());
        let need = request.need;
        // An assessment always goes through its verifier, even if the same
        // question and response look identical to those in an approved lesson.
        if let Some(action) = local_choice(request, options, now_ms) {
            return action;
        }
        let role = need
            .intent
            .role()
            .expect("local and disputed work was handled above");
        if !options.permitted.contains(&role) {
            return handoff(options, Reason::Unauthorized);
        }
        if let Some(previous) = attempts.last() {
            match previous.completion.map(|receipt| receipt.result) {
                None => return Action::InFlight(previous.ticket),
                Some(Returned::Draft(artifact)) => {
                    return Action::ReviewDraft(Draft {
                        ticket: previous.ticket,
                        artifact,
                    })
                }
                Some(Returned::Failed) => return handoff(options, Reason::FinalFailure),
                Some(Returned::Retryable) => {}
            }
        }
        if self.billing_hold {
            return handoff(options, Reason::BillingHold);
        }
        if attempts.is_empty() && self.entries.len() >= self.max_requests {
            return handoff(options, Reason::ReceiptCapacity);
        }
        if attempts.len() >= usize::from(self.max_attempts) {
            return handoff(options, Reason::AttemptLimit);
        }
        let Some(profiles) = self.models.get(&role) else {
            return handoff(options, Reason::Unavailable);
        };
        let Some(model) = profiles.get(&profile) else {
            return handoff(options, Reason::ProfileMismatch);
        };
        if input_tokens > model.max_input_tokens || output_tokens > model.max_output_tokens {
            return handoff(options, Reason::TokenLimit);
        }
        if model.admission_ms > request.deadline_ms - now_ms {
            return handoff(options, Reason::Deadline);
        }
        let occupied = self.occupied.entry((role, profile)).or_default();
        if *occupied >= model.slots {
            return handoff(options, Reason::NoCapacity);
        }
        let already_charged: u64 = attempts
            .iter()
            .filter_map(|a| a.completion)
            .map(|c| c.charged_micros)
            .sum();
        if model.maximum_micros > self.budget - self.spent - self.reserved
            || model.maximum_micros > request.max_micros - already_charged
        {
            return handoff(options, Reason::Budget);
        }
        let ticket = Ticket {
            epoch: self.epoch,
            request_id: request.id,
            need,
            role,
            model_revision: model.revision,
            attempt: (attempts.len() + 1) as u8,
            maximum_micros: model.maximum_micros,
        };
        *occupied += 1;
        self.reserved += model.maximum_micros;
        self.entries
            .entry(request.id)
            .or_insert_with(|| Entry {
                request,
                attempts: Vec::new(),
            })
            .attempts
            .push(Attempt {
                ticket,
                completion: None,
                running: true,
            });
        Action::Dispatch(ticket)
    }

    /// An authoritative transport receipt confirms the call has stopped running.
    /// The slot is reusable but the monetary reservation and retry fence remain
    /// until final settlement. A client-side timeout supplies no such receipt.
    pub fn confirm_stopped(&mut self, ticket: Ticket) -> Result<bool, SettlementError> {
        let attempt = find_attempt(&mut self.entries, ticket)?;
        Ok(release_slot(attempt, &mut self.occupied))
    }

    /// Accept an authoritative, final usage receipt. A timeout with unknown
    /// delivery/billing leaves the ticket pending. The adapter reconciles it
    /// against the provider idempotency key before making any retry eligible.
    /// Returns false on an exact duplicate. Late receipts are still billable.
    pub fn settle(
        &mut self,
        ticket: Ticket,
        completion: Completion,
    ) -> Result<bool, SettlementError> {
        let attempt = find_attempt(&mut self.entries, ticket)?;
        if let Some(old) = attempt.completion {
            return if old == completion {
                Ok(false)
            } else {
                Err(SettlementError::ConflictingReceipt)
            };
        }
        if completion.charged_micros > ticket.maximum_micros {
            self.billing_hold = true;
            return Err(SettlementError::ChargeAboveReservation);
        }
        attempt.completion = Some(completion);
        self.reserved -= ticket.maximum_micros;
        self.spent += completion.charged_micros;
        release_slot(attempt, &mut self.occupied);
        Ok(true)
    }
}

fn find_attempt(
    entries: &mut BTreeMap<u64, Entry>,
    ticket: Ticket,
) -> Result<&mut Attempt, SettlementError> {
    ticket
        .attempt
        .checked_sub(1)
        .and_then(|index| {
            entries
                .get_mut(&ticket.request_id)?
                .attempts
                .get_mut(usize::from(index))
        })
        .filter(|attempt| attempt.ticket == ticket)
        .ok_or(SettlementError::UnknownTicket)
}

fn release_slot(attempt: &mut Attempt, occupied: &mut BTreeMap<(Role, u64), u16>) -> bool {
    if !attempt.running {
        return false;
    }
    attempt.running = false;
    *occupied
        .get_mut(&(attempt.ticket.role, attempt.ticket.model_revision))
        .expect("running attempt holds a slot") -= 1;
    true
}
