//! The feedback and optional follow-up for one reviewed response, scoped by its
//! caller to one learner, outcome and release. This is an isolated design model.
//! A page-open receipt says only that a reference was opened during this item.
use crate::protocol::{Assistance, Gate, Judgment};

/// The learner can share work for teaching and still decline a separate assessment.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AssessmentChoice {
    Submit,
    PrivatePractice,
    Skip,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AssessmentRoute {
    NotOffered,
    Submitted,
    PrivatePractice,
    Skipped,
}

pub fn assessment_route(release: Gate, choice: AssessmentChoice) -> AssessmentRoute {
    match (release, choice) {
        (Gate::Unavailable, _) => AssessmentRoute::NotOffered,
        (Gate::Supported, AssessmentChoice::Submit) => AssessmentRoute::Submitted,
        (Gate::Supported, AssessmentChoice::PrivatePractice) => AssessmentRoute::PrivatePractice,
        (Gate::Supported, AssessmentChoice::Skip) => AssessmentRoute::Skipped,
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Attempt {
    pub item: u64,
    pub judgment: Judgment,
    pub disclosure: Assistance,
    pub reference_opened: bool,
}

impl Attempt {
    pub fn new(
        item: u64,
        judgment: Judgment,
        disclosure: Assistance,
        reference_opened: bool,
    ) -> Self {
        Self {
            item,
            judgment,
            disclosure,
            reference_opened,
        }
    }

    /// The only assessment text the loop owns. For free-text work, the caller
    /// must obtain the judgment from a current rubric-bound assessment.
    pub fn feedback(self) -> String {
        let observation = match self.judgment {
            Judgment::Correct => "This response was marked correct.",
            Judgment::Incorrect => "This response was marked incorrect. The reason is still open.",
            Judgment::Ambiguous => "I need more information to interpret this response.",
            Judgment::Missing => "There is no response to assess.",
        };
        let disclosure = match self.disclosure {
            Assistance::Unknown => "I don't have an assistance report.",
            Assistance::Reported => "You reported using help.",
            Assistance::NoneReported => "No help was reported for this response.",
        };
        let access = if self.reference_opened {
            " A reference opened during this item."
        } else {
            ""
        };
        format!("{observation} {disclosure}{access}")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Offer {
    OptionalNewExample,
    ReasoningOrExplanation,
    ClarificationOrExplanation,
    PauseOrExplanation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Choice {
    NewExample,
    Explain,
    Discuss,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Stage {
    Feedback,
    LookingForExample,
    OpenExample(u64),
    RequestedExplanation,
    Discussion,
    Closed,
}

/// Minimal later-session view. Draft text and browsing destinations never
/// enter this loop. Absence of a later assessment is a meaningful result.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReturnView {
    pub original: Attempt,
    pub follow_up: Option<Attempt>,
    pub stage: Stage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    OutOfOrder,
    SameItem,
    Unavailable,
    WrongItem,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResponseLoop(ReturnView);

impl ResponseLoop {
    pub fn new(attempt: Attempt) -> Self {
        Self(ReturnView {
            original: attempt,
            follow_up: None,
            stage: Stage::Feedback,
        })
    }

    pub fn return_view(&self) -> ReturnView {
        self.0
    }

    pub fn offer(&self) -> Option<Offer> {
        if self.0.stage != Stage::Feedback {
            return None;
        }
        Some(match self.0.original.judgment {
            Judgment::Correct => Offer::OptionalNewExample,
            Judgment::Incorrect => Offer::ReasoningOrExplanation,
            Judgment::Ambiguous => Offer::ClarificationOrExplanation,
            Judgment::Missing => Offer::PauseOrExplanation,
        })
    }

    pub fn choose(&mut self, choice: Choice) -> Result<(), Refusal> {
        self.0.stage = match (self.0.stage, choice) {
            (_, Choice::Stop) => Stage::Closed,
            (
                Stage::Feedback | Stage::LookingForExample | Stage::OpenExample(_),
                Choice::Explain,
            ) => Stage::RequestedExplanation,
            (Stage::Feedback, Choice::NewExample)
                if self.offer() == Some(Offer::OptionalNewExample) =>
            {
                Stage::LookingForExample
            }
            (Stage::Feedback, Choice::Discuss)
                if matches!(
                    self.offer(),
                    Some(Offer::ClarificationOrExplanation | Offer::ReasoningOrExplanation)
                ) =>
            {
                Stage::Discussion
            }
            _ => return Err(Refusal::OutOfOrder),
        };
        Ok(())
    }

    /// The catalog must attest that this form currently serves the same outcome,
    /// is fresh for this learner, and has approved source and release support.
    /// A changed identifier alone cannot establish any of those conditions.
    pub fn open_example(&mut self, item: u64, catalog: Gate) -> Result<(), Refusal> {
        if self.0.stage != Stage::LookingForExample {
            return Err(Refusal::OutOfOrder);
        }
        if catalog == Gate::Unavailable {
            return Err(Refusal::Unavailable);
        }
        if item == self.0.original.item {
            return Err(Refusal::SameItem);
        }
        self.0.stage = Stage::OpenExample(item);
        Ok(())
    }

    /// The caller binds the review to the scoped opening and rechecks its
    /// support at submission. A stop or explanation closes the scoring path.
    pub fn record_check(&mut self, observation: Attempt, current: Gate) -> Result<(), Refusal> {
        let Stage::OpenExample(item) = self.0.stage else {
            return Err(Refusal::OutOfOrder);
        };
        if observation.item != item {
            return Err(Refusal::WrongItem);
        }
        if current == Gate::Unavailable {
            return Err(Refusal::Unavailable);
        }
        self.0.follow_up = Some(observation);
        self.0.stage = Stage::Closed;
        Ok(())
    }
}
