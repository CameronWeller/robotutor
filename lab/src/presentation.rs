//! Bind a numeric answer to the exact question that was delivered. The caller
//! selects a single authoritative lane from authenticated learner and unit IDs.
//! Durable adapters compare and write the opening identity and lane revision
//! together with the delivery. Re-presentation after a resume can replace an
//! expired opening exactly once at the current revision.

use crate::curriculum::{OutcomeId, Span};
use crate::history::Scope;
use crate::material::{FractionCard, RuleSource};
use crate::protocol::{
    transition, AdmissionError, Assistance, Command, Effect, Event, Gate, Judgment, Lane, Phase,
    Question, Receipt, State,
};
use crate::supply::{Enrollment, ExposureBook, Form, Requirement, Unavailable};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    /// Immutable identity in the unit release. Versions with different cards
    /// or rubrics require distinct identities in the catalog.
    pub revision: u64,
    pub question: Question,
    pub card: FractionCard,
}

/// A reviewed form and the operation requested by the instructional policy.
pub struct CatalogQuestion<'a> {
    pub form: &'a Form,
    pub question: Question,
    pub requirement: Requirement,
    pub source: &'a RuleSource<'a>,
}

/// The learner can refer to an opening without receiving the answer key.
/// Possession of a handle does not authorize access to the learner's lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    scope: Scope,
    outcome: OutcomeId,
    item_revision: u64,
    opening_record: u64,
    lane_revision: u64,
}

#[derive(Debug)]
pub struct Delivery<H = Handle> {
    pub handle: H,
    pub prompt: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub scope: Scope,
    pub outcome: OutcomeId,
    pub presentation_record: u64,
    pub response_record: u64,
    /// An expired opening was shown again after a resume. The evidence adapter
    /// can require a fresh item for a delayed or independent checkpoint.
    pub presented_again: bool,
    /// Set by the assignment opening transaction. A raw lane has no exposure attestation.
    /// A resumed question carries a practice-only check regardless of its original purpose.
    pub exposure_check: Option<Requirement>,
    pub item: Item,
    pub span: Span,
    pub judgment: Judgment,
    pub assistance: Assistance,
    pub effect: Effect,
}

impl Assessment {
    /// A new handle alone leaves freshness unknown.
    pub fn fresh_task(&self) -> bool {
        matches!(
            self.exposure_check,
            Some(Requirement::FreshTask | Requirement::FreshFamily)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    Supply(Unavailable),
    Admission(AdmissionError),
    StaleOpening,
    SourceUnavailable,
    UseItemBinding,
    Protocol(Effect),
}

struct Opening {
    handle: Handle,
    item: Item,
    span: Span,
    presented_again: bool,
    exposure_check: Option<Requirement>,
}

pub struct BoundLane {
    class: Option<u64>,
    scope: Scope,
    outcome: OutcomeId,
    lane: Lane,
    opening: Option<Opening>,
    assessment: Option<Assessment>,
    feedback_revision: Option<u64>,
}

impl BoundLane {
    pub fn new(scope: Scope, outcome: OutcomeId) -> Self {
        Self {
            class: None,
            scope,
            outcome,
            lane: Lane::default(),
            opening: None,
            assessment: None,
            feedback_revision: None,
        }
    }

    pub fn for_enrollment(seat: Enrollment, outcome: OutcomeId) -> Self {
        Self {
            class: Some(seat.class),
            ..Self::new(seat.scope, outcome)
        }
    }

    /// Transfer the constant-size protocol position only. Delivery and feedback belong to the
    /// old publication. The audience transaction seals the predecessor once the new opening wins.
    pub(crate) fn continue_with_next_item(&self) -> Self {
        Self {
            class: self.class,
            lane: self.lane.clone(),
            ..Self::new(self.scope, self.outcome)
        }
    }

    /// One seat-wide freshness check, lane compare-and-write and possible exposure before bytes escape.
    /// The caller resolves present membership/holds first. Dropping a permit consumes no exposure.
    pub fn present_from_catalog(
        &mut self,
        book: &mut ExposureBook,
        request: CatalogQuestion<'_>,
        after: u64,
        record: u64,
    ) -> Result<Delivery, BindError> {
        let class = self.class.ok_or(BindError::Supply(Unavailable::Scope))?;
        let CatalogQuestion {
            form,
            question,
            requirement,
            source,
        } = request;
        let item = Item {
            revision: form.revision,
            question,
            card: form.card,
        };
        let required = if self.resumes_opening(after, item, form.source) {
            Requirement::Practice
        } else if requirement == Requirement::Practice
            && !matches!(question, Question::Practice | Question::Clarification)
        {
            Requirement::FreshTask
        } else {
            requirement
        };
        let seat = Enrollment {
            scope: self.scope,
            class,
        };
        let permit = book
            .prepare_opening(seat, self.outcome, required, form, source)
            .map_err(BindError::Supply)?;
        let delivery = self.present_with_exposure(after, record, item, source, Some(required))?;
        permit.commit();
        Ok(delivery)
    }

    pub fn state(&self) -> State {
        self.lane.state()
    }

    pub fn revision(&self) -> u64 {
        self.lane.revision()
    }

    /// Only the opening at this exact lane revision can yield a public prompt.
    pub(crate) fn open_material(&self) -> Option<(Item, Span)> {
        let open = self.opening.as_ref()?;
        (open.handle.lane_revision == self.revision()
            && !self.state().paused
            && self.state().phase == Phase::AwaitingAnswer(open.item.question))
        .then_some((open.item, open.span))
    }

    /// Publication requires the committed feedback event for the current assessment.
    pub(crate) fn released_feedback(&self) -> Option<&Assessment> {
        (self.feedback_revision == Some(self.revision())).then_some(())?;
        self.assessment.as_ref()
    }

    /// The actor supplies the original command revision on retries. Only the
    /// item-bound paths may open questions or admit answers.
    pub fn control(&mut self, command: Command) -> Result<Receipt, BindError> {
        if matches!(command.event, Event::Ask(..) | Event::Answer(..)) {
            return Err(BindError::UseItemBinding);
        }
        // An invalid speculative control action must not age an active handle.
        if self.revision() == command.after {
            let effect = transition(self.state(), command.event).1;
            if matches!(effect, Effect::Rejected | Effect::Withheld) {
                return Err(BindError::Protocol(effect));
            }
        }
        let receipt = self.lane.apply(command).map_err(BindError::Admission)?;
        if matches!(receipt, Receipt::Applied(Effect::GaveFeedback(_))) {
            self.feedback_revision = Some(self.revision());
        }
        Ok(receipt)
    }

    pub fn present(
        &mut self,
        after: u64,
        record: u64,
        item: Item,
        source: &RuleSource<'_>,
    ) -> Result<Delivery, BindError> {
        self.present_with_exposure(after, record, item, source, None)
    }

    fn resumes_opening(&self, after: u64, item: Item, span: Span) -> bool {
        self.opening.as_ref().is_some_and(|open| {
            !self.state().paused
                && self.state().phase == Phase::AwaitingAnswer(item.question)
                && open.item == item
                && open.span == span
                && open.handle.lane_revision < after
        })
    }

    fn present_with_exposure(
        &mut self,
        after: u64,
        record: u64,
        item: Item,
        source: &RuleSource<'_>,
        exposure_check: Option<Requirement>,
    ) -> Result<Delivery, BindError> {
        if self.revision() != after {
            return Err(BindError::Admission(AdmissionError::VersionConflict));
        }
        let (prompt, span) = item
            .card
            .question(source)
            .ok_or(BindError::SourceUnavailable)?;
        // Pause/resume preserves the question but expires its submit handle.
        // The learner must actually see it again to acquire a new one. This
        // delivery consumes the opening slot of the already committed revision.
        let resumed_opening = self.resumes_opening(after, item, span);
        if !resumed_opening {
            let event = Event::Ask(item.question, Gate::Supported);
            let effect = transition(self.state(), event).1;
            if effect != Effect::Asked(item.question) {
                return Err(BindError::Protocol(effect));
            }
            self.lane
                .apply(Command {
                    after,
                    record,
                    event,
                })
                .map_err(BindError::Admission)?;
        }
        let handle = Handle {
            scope: self.scope,
            outcome: self.outcome,
            item_revision: item.revision,
            opening_record: record,
            lane_revision: self.revision(),
        };
        self.opening = Some(Opening {
            handle,
            item,
            span,
            presented_again: resumed_opening,
            exposure_check,
        });
        Ok(Delivery {
            handle,
            prompt,
            span,
        })
    }

    /// A changed lane requires a fresh learner-facing presentation. Dead
    /// receipts cannot be relabeled with today's revision. A source outage
    /// leaves the opening and lane revision intact for an authorized retry.
    pub fn submit(
        &mut self,
        handle: &Handle,
        record: u64,
        answer: &str,
        assistance: Assistance,
        source: &RuleSource<'_>,
    ) -> Result<Assessment, BindError> {
        let open = self.opening.as_ref().ok_or(BindError::StaleOpening)?;
        if open.handle != *handle
            || self.revision() != handle.lane_revision
            || self.state().phase != Phase::AwaitingAnswer(open.item.question)
        {
            return Err(BindError::StaleOpening);
        }
        if open.span != source.pin {
            return Err(BindError::SourceUnavailable);
        }
        let judgment = open
            .item
            .card
            .assess(answer, source)
            .ok_or(BindError::SourceUnavailable)?;
        let event = Event::Answer(judgment, assistance);
        let receipt = self
            .lane
            .apply(Command {
                after: handle.lane_revision,
                record,
                event,
            })
            .map_err(BindError::Admission)?;
        let Receipt::Applied(effect @ Effect::Analyzed(_)) = receipt else {
            return Err(BindError::StaleOpening);
        };
        let assessment = Assessment {
            scope: self.scope,
            outcome: self.outcome,
            presentation_record: handle.opening_record,
            response_record: record,
            presented_again: open.presented_again,
            exposure_check: open.exposure_check,
            item: open.item,
            span: open.span,
            judgment,
            assistance,
            effect,
        };
        self.opening = None;
        self.assessment = Some(assessment);
        Ok(assessment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{document_id, FractionTask, UNIT_RULE};
    use crate::protocol::Next;
    use std::collections::BTreeSet;

    const SCOPE: Scope = Scope {
        learner: 1,
        revision: 3,
    };

    fn source<'a>(bytes: &'a [u8], revoked: &'a BTreeSet<[u8; 32]>) -> RuleSource<'a> {
        RuleSource {
            bytes: Some(bytes),
            pin: Span {
                document: document_id(bytes),
                start: 0,
                end: UNIT_RULE.len() as u32,
            },
            revoked,
        }
    }

    fn probe() -> Item {
        Item {
            revision: 20,
            question: Question::RequestedProbe,
            card: FractionCard::propose(FractionTask::Sum, 1, 1, 2).unwrap(),
        }
    }

    fn control(lane: &mut BoundLane, event: Event) {
        let after = lane.revision();
        lane.control(Command {
            after,
            record: 101 + after,
            event,
        })
        .unwrap();
    }

    #[test]
    fn return_to_an_identical_question_requires_the_new_opening() {
        let live = BTreeSet::new();
        let view = source(UNIT_RULE.as_bytes(), &live);
        let mut lane = BoundLane::new(SCOPE, OutcomeId(7));
        let old = lane.present(0, 90, probe(), &view).unwrap();
        control(&mut lane, Event::Teach(Gate::Supported));
        let item = Item {
            revision: 21,
            question: Question::Practice,
            ..probe()
        };
        let new = lane.present(lane.revision(), 92, item, &view).unwrap();
        assert_eq!(new.prompt, old.prompt);
        assert_eq!(
            lane.submit(&old.handle, 93, "1", Assistance::NoneReported, &view),
            Err(BindError::StaleOpening)
        );
        let assessed = lane
            .submit(&new.handle, 94, "1", Assistance::NoneReported, &view)
            .unwrap();
        assert_eq!(assessed.item, item);
        assert!(!assessed.presented_again);
        assert!(!assessed.fresh_task());
        assert_eq!(assessed.span, view.pin);
        assert_eq!(assessed.effect, Effect::Analyzed(Next::ChangedExample));
        assert_eq!(
            lane.submit(&new.handle, 94, "1", Assistance::NoneReported, &view),
            Err(BindError::StaleOpening)
        );
    }

    #[test]
    fn scope_mismatch_or_source_substitution_cannot_grade_an_open_question() {
        let live = BTreeSet::new();
        let view = source(UNIT_RULE.as_bytes(), &live);
        let bytes = format!("{UNIT_RULE}\n{UNIT_RULE}");
        let other_view = source(bytes.as_bytes(), &live);
        let revoked = BTreeSet::from([view.pin.document]);
        let removed = RuleSource {
            revoked: &revoked,
            ..view
        };
        let mut lane = BoundLane::new(SCOPE, OutcomeId(7));
        let open = lane.present(0, 99, probe(), &view).unwrap();
        for (scope, goal) in [
            (
                Scope {
                    learner: 2,
                    ..SCOPE
                },
                7,
            ),
            (
                Scope {
                    revision: 4,
                    ..SCOPE
                },
                7,
            ),
            (SCOPE, 8),
        ] {
            let mut other = BoundLane::new(scope, OutcomeId(goal));
            other.present(0, 99, probe(), &view).unwrap();
            assert_eq!(
                other.submit(&open.handle, 100, "1", Assistance::NoneReported, &view),
                Err(BindError::StaleOpening)
            );
        }
        for bad in [other_view, removed] {
            assert_eq!(
                lane.submit(&open.handle, 100, "1", Assistance::NoneReported, &bad),
                Err(BindError::SourceUnavailable)
            );
        }
        assert_eq!(lane.revision(), 1);
        assert!(lane
            .submit(&open.handle, 100, "1", Assistance::NoneReported, &view)
            .is_ok());
    }

    #[test]
    fn resumed_learner_can_receive_the_original_item_again() {
        let live = BTreeSet::new();
        let view = source(UNIT_RULE.as_bytes(), &live);
        let mut lane = BoundLane::new(SCOPE, OutcomeId(7));
        let first = lane.present(0, 90, probe(), &view).unwrap();
        control(&mut lane, Event::Pause);
        control(&mut lane, Event::Resume);
        assert_eq!(
            lane.submit(&first.handle, 93, "1", Assistance::Unknown, &view),
            Err(BindError::StaleOpening)
        );
        let new = lane.present(lane.revision(), 94, probe(), &view).unwrap();
        assert_eq!(first.prompt, new.prompt);
        assert_eq!(
            lane.submit(&first.handle, 95, "1", Assistance::Unknown, &view),
            Err(BindError::StaleOpening)
        );
        let assessed = lane
            .submit(&new.handle, 96, "1", Assistance::Unknown, &view)
            .unwrap();
        assert_eq!(assessed.effect, Effect::Analyzed(Next::IndependentCheck));
        assert!(assessed.presented_again);
    }
}
