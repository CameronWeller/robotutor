//! Project authorized learner and class views from a published activity and live delivery receipts.
//! An adapter supplies an authenticated actor and a current roster, release and rights snapshot.
use std::collections::{BTreeMap, BTreeSet};

use crate::curriculum::Span;
use crate::history::Scope;
use crate::material::{RuleSource, UNIT_RULE};
use crate::presentation::{
    Assessment, BindError, BoundLane, CatalogQuestion, Delivery, Handle, Item,
};
use crate::protocol::{Assistance, Command, Effect, Judgment, Next, Phase, Question, Receipt};
use crate::supply::{Enrollment, ExposureBook, Form, Requirement};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AssignmentKey {
    pub class: u64,
    pub activity: u64,
}

/// The class directory supplies these installation-scoped principal IDs on each read.
pub struct Roster {
    pub class: u64,
    pub teachers: BTreeSet<u64>,
    pub learners: BTreeSet<u64>,
}

/// Admitted catalog and scheduling metadata only. The publisher alone changes a publication.
pub struct Assignment {
    pub key: AssignmentKey,
    pub publication: u64,
    pub form: Form,
    pub question: Question,
    /// Learner to current assignment-grant instance. The directory/assignment adapter rotates
    /// this individual value when either covered grant is newly issued; retired IDs never recur.
    pub assigned: BTreeMap<u64, u64>,
    pub held: BTreeSet<u64>,
    pub corrected: bool,
    /// Current return opportunities committed by the scheduler, keyed by assigned learner.
    pub returns: BTreeMap<u64, u32>,
}

impl Assignment {
    fn item(&self) -> Item {
        Item {
            revision: self.form.revision,
            question: self.question,
            card: self.form.card,
        }
    }
}

/// A caller obtains these current snapshots under the authenticated actor's grant.
#[derive(Clone, Copy)]
pub struct LearnerAccess<'a> {
    pub actor: u64,
    pub roster: &'a Roster,
    pub assignment: &'a Assignment,
    pub source: &'a RuleSource<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessStop {
    Denied,
    Updated,
    UnderReview,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipationError {
    Access(AccessStop),
    Binding(BindError),
}

impl LearnerAccess<'_> {
    fn authorize(self, learner: u64, class: u64) -> Result<u64, AccessStop> {
        let assignment = self.assignment;
        if self.roster.class != class
            || assignment.key.class != class
            || assignment.form.class != class
            || self.actor != learner
            || assignment
                .form
                .owner
                .is_some_and(|owner| owner != self.actor)
            || !self.roster.learners.contains(&self.actor)
        {
            return Err(AccessStop::Denied);
        }
        let grant = assignment
            .assigned
            .get(&self.actor)
            .copied()
            .ok_or(AccessStop::Denied)?;
        if assignment.corrected || !live_source(assignment, self.source) {
            return Err(AccessStop::Updated);
        }
        if assignment.held.contains(&self.actor) {
            return Err(AccessStop::UnderReview);
        }
        Ok(grant)
    }

    fn check(self, participation: &Participation) -> Result<u64, AccessStop> {
        if self.assignment.key != participation.key {
            return Err(AccessStop::Denied);
        }
        let grant = self.authorize(participation.scope.learner, participation.key.class)?;
        if participation.grant != Some(grant) {
            return Err(AccessStop::Denied);
        }

        if !participation.matches(self.assignment) {
            return Err(AccessStop::Updated);
        }
        if participation.lane.state().phase == Phase::Disputed {
            return Err(AccessStop::UnderReview);
        }
        if participation.lane.state().paused {
            return Err(AccessStop::Paused);
        }
        Ok(grant)
    }
}

/// Bind the browser's opaque question reference to this immutable audience publication and grant.
/// Caller-provided record numbers may repeat in a newly allocated participation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssignedHandle {
    key: AssignmentKey,
    publication: u64,
    grant: u64,
    opening: Handle,
}

/// One learner and objective. A published handoff can carry its protocol debt into the next activity.
pub struct Participation {
    key: AssignmentKey,
    grant: Option<u64>,
    publication: u64,
    scope: Scope,
    form: Form,
    question: Question,
    lane: BoundLane,
    continued: bool,
}

impl Participation {
    pub fn start(assignment: &Assignment, learner: u64) -> Self {
        let scope = Scope {
            learner,
            revision: assignment.form.release,
        };
        Self {
            key: assignment.key,
            grant: assignment.assigned.get(&learner).copied(),
            publication: assignment.publication,
            scope,
            form: assignment.form,
            question: assignment.question,
            continued: false,
            lane: BoundLane::for_enrollment(
                Enrollment {
                    scope,
                    class: assignment.key.class,
                },
                assignment.form.outcome,
            ),
        }
    }

    pub fn revision(&self) -> u64 {
        self.lane.revision()
    }

    /// Trusted policy or reviewer commands. Item delivery and answer admission cannot enter here.
    pub fn control(&mut self, command: Command) -> Result<Receipt, BindError> {
        self.ensure_active()?;
        self.lane.control(command)
    }

    /// Check the authoritative seat and semantic form again at publication. The exclusive book
    /// borrow fences simultaneous lanes in this model. A failed lane CAS drops the permit unspent.
    pub fn present(
        &mut self,
        access: LearnerAccess<'_>,
        book: &mut ExposureBook,
        requested: Requirement,
        after: u64,
        record: u64,
    ) -> Result<Delivery<AssignedHandle>, ParticipationError> {
        let grant = access.check(self).map_err(ParticipationError::Access)?;
        self.ensure_active().map_err(ParticipationError::Binding)?;
        let delivery = self
            .lane
            .present_from_catalog(
                book,
                CatalogQuestion {
                    form: &self.form,
                    question: self.question,
                    requirement: requested,
                    source: access.source,
                },
                after,
                record,
            )
            .map_err(ParticipationError::Binding)?;
        Ok(Delivery {
            handle: AssignedHandle {
                key: self.key,
                publication: self.publication,
                grant,
                opening: delivery.handle,
            },
            prompt: delivery.prompt,
            span: delivery.span,
        })
    }

    /// Carry the protocol position into a distinct assigned activity. Both grants must be
    /// current in one authoritative read. The predecessor remains a sealed, readable class row.
    pub fn present_successor(
        &mut self,
        current: LearnerAccess<'_>,
        successor: LearnerAccess<'_>,
        book: &mut ExposureBook,
        requested: Requirement,
        after: u64,
        record: u64,
    ) -> Result<(Self, Delivery<AssignedHandle>), ParticipationError> {
        current.check(self).map_err(ParticipationError::Access)?;
        self.ensure_active().map_err(ParticipationError::Binding)?;
        let assignment = successor.assignment;
        if assignment.key == self.key
            || assignment.key.class != self.key.class
            || assignment.form.release != self.scope.revision
            || assignment.form.outcome != self.form.outcome
        {
            return Err(ParticipationError::Access(AccessStop::Denied));
        }
        if !matches!(
            self.lane.state().phase,
            Phase::Taught | Phase::Refreshed | Phase::FollowUp(_) | Phase::ReviewDue
        ) {
            return Err(ParticipationError::Binding(BindError::Protocol(
                Effect::Rejected,
            )));
        }
        let mut next = Self {
            key: assignment.key,
            publication: assignment.publication,
            scope: self.scope,
            grant: assignment.assigned.get(&self.scope.learner).copied(),
            form: assignment.form,
            question: assignment.question,
            lane: self.lane.continue_with_next_item(),
            continued: false,
        };
        let opening = next.present(successor, book, requested, after, record)?;
        self.continued = true;
        Ok((next, opening))
    }

    fn ensure_active(&self) -> Result<(), BindError> {
        if self.continued {
            Err(BindError::StaleOpening)
        } else {
            Ok(())
        }
    }

    pub fn submit(
        &mut self,
        access: LearnerAccess<'_>,
        handle: &AssignedHandle,
        record: u64,
        answer: &str,
        assistance: Assistance,
    ) -> Result<Assessment, ParticipationError> {
        let grant = access.check(self).map_err(ParticipationError::Access)?;
        self.ensure_active().map_err(ParticipationError::Binding)?;
        if handle.key != self.key || handle.publication != self.publication || handle.grant != grant
        {
            return Err(ParticipationError::Binding(BindError::StaleOpening));
        }
        self.lane
            .submit(&handle.opening, record, answer, assistance, access.source)
            .map_err(ParticipationError::Binding)
    }

    fn matches(&self, assignment: &Assignment) -> bool {
        self.publication == assignment.publication
            && self.form == assignment.form
            && self.question == assignment.question
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ViewError {
    AccessDenied,
    DuplicateParticipation,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LearnerView {
    Ready,
    Lesson {
        text: &'static str,
        next: &'static str,
        source: Span,
    },
    Question {
        prompt: String,
        why: &'static str,
        source: Span,
    },
    Feedback {
        result: &'static str,
        worked_answer: Option<String>,
        why: &'static str,
        next: &'static str,
        return_day: Option<u32>,
        source: Span,
    },
    AwaitingFeedback,
    Continue(&'static str),
    Paused,
    UnderReview,
    Updated,
}

impl LearnerView {
    /// Fixed status copy carries no answer, peer data or internal adjudication notes.
    pub fn status_message(&self) -> Option<&'static str> {
        match self {
            Self::Ready => Some("Choose a lesson or ask to try a question."),
            Self::Paused => Some("Your activity is paused. Resume when you are ready."),
            Self::UnderReview => Some("This activity is under review. You can ask your teacher about it or continue with another topic."),
            Self::Updated => Some("This version is unavailable. Ask for the current activity."),
            Self::Continue(message) => Some(message),
            Self::AwaitingFeedback => Some("Your response was received. Feedback is pending."),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TeacherAction {
    InviteToStart,
    ContinueActivity,
    FollowUpStarted,
    OfferExample,
    ClarifyResponse,
    OfferIndependentCheck,
    ArrangeReturn,
    ReviewWithLearner,
    RespectPause,
    ReplaceMaterial,
}

impl TeacherAction {
    pub fn label(self) -> &'static str {
        use TeacherAction::*;
        match self {
            InviteToStart => "Invite to start",
            ContinueActivity => "Continue the activity",
            FollowUpStarted => "Follow-up activity started",
            OfferExample => "Offer a worked example",
            ClarifyResponse => "Ask what the learner meant",
            OfferIndependentCheck => "Offer a fresh check",
            ArrangeReturn => "Arrange a return visit",
            ReviewWithLearner => "Review with the learner",
            RespectPause => "Leave the activity paused",
            ReplaceMaterial => "Replace the activity material",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultState {
    MatchedWithoutReportedHelp,
    MatchedWithHelpOrUnknown,
    DifferentAmount,
    Uninterpreted,
    NoCurrentResult,
    UnderReview,
    MaterialWithdrawn,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TeacherView {
    pub assigned: usize,
    pub unassigned_enrolled: usize,
    pub counts: BTreeMap<TeacherAction, usize>,
    /// One bucket per currently assigned learner at this snapshot.
    pub results: BTreeMap<ResultState, usize>,
    /// Current enrollees with an individual hold or an explicit human escalation.
    pub follow_up: Vec<u64>,
}

fn live_source(assignment: &Assignment, source: &RuleSource<'_>) -> bool {
    source.pin == assignment.form.source && assignment.form.card.question(source).is_some()
}

fn guidance(assessment: &Assessment) -> Option<(&'static str, &'static str, TeacherAction)> {
    let Effect::Analyzed(next) = assessment.effect else {
        return None;
    };
    use TeacherAction::*;
    Some(match next {
        Next::Clarify => (
            "The response leaves more than one possible reading.",
            "Tell me which amount you meant.",
            ClarifyResponse,
        ),
        Next::Explain => (
            "A worked example can show where the equal parts enter the calculation.",
            "Work through an example together.",
            OfferExample,
        ),
        Next::IndependentCheck => (
            "An independent example can show what you can do with this rule.",
            "Try a fresh example when you are ready.",
            OfferIndependentCheck,
        ),
        Next::ChangedExample => (
            "A different example checks whether the same reasoning carries over.",
            "Try a changed example.",
            OfferIndependentCheck,
        ),
        Next::WaitReview => (
            "A later attempt can show which parts you remember.",
            "Return for a fresh question.",
            ArrangeReturn,
        ),
        Next::HumanReview => (
            "Your response needs a person's judgment.",
            "Review the item with your teacher.",
            ReviewWithLearner,
        ),
        Next::OfferBreak => (
            "You can pause here and choose how to continue.",
            "Pause or ask for a worked example.",
            RespectPause,
        ),
    })
}

pub fn learner_view(
    actor: u64,
    roster: &Roster,
    assignment: &Assignment,
    participation: &Participation,
    source: &RuleSource<'_>,
) -> Result<LearnerView, ViewError> {
    let access = LearnerAccess {
        actor,
        roster,
        assignment,
        source,
    };
    if let Err(stop) = access.check(participation) {
        return match stop {
            AccessStop::Denied => Err(ViewError::AccessDenied),
            AccessStop::Updated => Ok(LearnerView::Updated),
            AccessStop::UnderReview => Ok(LearnerView::UnderReview),
            AccessStop::Paused => Ok(LearnerView::Paused),
        };
    }
    let lane = &participation.lane;
    if let Some((item, span)) = lane.open_material() {
        if item != assignment.item() || span != assignment.form.source {
            return Ok(LearnerView::Updated);
        }
        let Some((prompt, source)) = item.card.question(source) else {
            return Ok(LearnerView::Updated);
        };
        return Ok(LearnerView::Question {
            prompt,
            source,
            why: "This question checks how you use equal parts of the same whole.",
        });
    }
    if let Some(assessment) = lane.released_feedback() {
        if assessment.item != assignment.item() || assessment.span != assignment.form.source {
            return Ok(LearnerView::Updated);
        }
        let Some((why, next, _)) = guidance(assessment) else {
            return Ok(LearnerView::UnderReview);
        };
        let result = match assessment.judgment {
            Judgment::Correct => "Your numerical answer matches.",
            Judgment::Incorrect => "Your numerical answer differs from the expected amount.",
            Judgment::Ambiguous => "I need a clearer reading of your answer.",
            Judgment::Missing => "No answer was recorded.",
        };
        let worked_answer = match assessment.judgment {
            Judgment::Correct | Judgment::Incorrect => {
                assessment.item.card.worked(source).map(|(text, _)| text)
            }
            Judgment::Ambiguous | Judgment::Missing => None,
        };
        return Ok(LearnerView::Feedback {
            result,
            worked_answer,
            why,
            next: if participation.continued {
                "You continued in the next activity."
            } else {
                next
            },
            return_day: assignment.returns.get(&actor).copied(),
            source: assessment.span,
        });
    }
    if participation.continued {
        return Ok(LearnerView::Continue("You continued in the next activity."));
    }
    Ok(match lane.state().phase {
        Phase::Taught | Phase::Refreshed => LearnerView::Lesson {
            text: UNIT_RULE,
            next: "Choose whether to try a question or keep working through the rule.",
            source: assignment.form.source,
        },
        Phase::AwaitingFeedback(_) => LearnerView::AwaitingFeedback,
        Phase::AwaitingAnswer(_) => LearnerView::Continue("Reopen the question to continue."),
        Phase::ReviewDue => {
            LearnerView::Continue("Your return check is ready. You can also request a refresher.")
        }
        Phase::FollowUp(_) => LearnerView::Continue("Resume the next activity in this lesson."),
        _ => LearnerView::Ready,
    })
}

/// Missing assigned rows count as invitations. Foreign classes, unassigned and former members are excluded.
pub fn teacher_view(
    actor: u64,
    roster: &Roster,
    assignment: &Assignment,
    participations: &[&Participation],
    source: &RuleSource<'_>,
) -> Result<TeacherView, ViewError> {
    if roster.class != assignment.key.class
        || assignment.form.class != assignment.key.class
        || !roster.teachers.contains(&actor)
        || assignment.form.owner.is_some()
    {
        return Err(ViewError::AccessDenied);
    }
    let assigned: BTreeSet<_> = assignment
        .assigned
        .keys()
        .filter(|id| roster.learners.contains(id))
        .copied()
        .collect();
    let mut rows = BTreeMap::new();
    for p in participations {
        if p.key == assignment.key
            && assigned.contains(&p.scope.learner)
            && p.publication == assignment.publication
            && p.grant == assignment.assigned.get(&p.scope.learner).copied()
            && rows.insert(p.scope.learner, *p).is_some()
        {
            return Err(ViewError::DuplicateParticipation);
        }
    }
    let source_ready = live_source(assignment, source);
    let mut view = TeacherView {
        assigned: assigned.len(),
        unassigned_enrolled: roster.learners.len() - assigned.len(),
        counts: BTreeMap::new(),
        results: BTreeMap::new(),
        follow_up: vec![],
    };
    for id in assigned {
        use TeacherAction::*;
        let mut result = ResultState::NoCurrentResult;
        let action = if assignment.corrected || !source_ready {
            ReplaceMaterial
        } else if assignment.held.contains(&id) {
            ReviewWithLearner
        } else if let Some(p) = rows.get(&id) {
            let lane = &p.lane;
            if !p.matches(assignment)
                || lane
                    .open_material()
                    .is_some_and(|pair| pair != (assignment.item(), assignment.form.source))
            {
                ReplaceMaterial
            } else if lane.state().phase == Phase::Disputed {
                ReviewWithLearner
            } else if lane.state().paused {
                RespectPause
            } else if let Some(a) = lane.released_feedback() {
                if a.item != assignment.item() || a.span != assignment.form.source {
                    ReplaceMaterial
                } else {
                    result = match (a.judgment, a.assistance) {
                        (Judgment::Correct, Assistance::NoneReported) => {
                            ResultState::MatchedWithoutReportedHelp
                        }
                        (Judgment::Correct, _) => ResultState::MatchedWithHelpOrUnknown,
                        (Judgment::Incorrect, _) => ResultState::DifferentAmount,
                        (Judgment::Ambiguous | Judgment::Missing, _) => ResultState::Uninterpreted,
                    };
                    if p.continued {
                        FollowUpStarted
                    } else {
                        guidance(a).map_or(ReviewWithLearner, |(_, _, action)| action)
                    }
                }
            } else if p.continued {
                FollowUpStarted
            } else if lane.revision() == 0 {
                InviteToStart
            } else {
                ContinueActivity
            }
        } else {
            InviteToStart
        };
        result = match action {
            ReplaceMaterial => ResultState::MaterialWithdrawn,
            ReviewWithLearner => ResultState::UnderReview,
            _ => result,
        };
        *view.results.entry(result).or_default() += 1;
        *view.counts.entry(action).or_default() += 1;
        if action == ReviewWithLearner {
            view.follow_up.push(id);
        }
    }
    Ok(view)
}
