//! One learner, one versioned objective, one outstanding turn.
//!
//! The state is a projection of ordered facts. Neither a choice of next move nor a
//! time interval is evidence of mastery. Adapters attest inputs at the boundary:
//! the current artifact is approved for this learner and use, an answer is bound
//! to its open item, and a due date has actually elapsed. The policy is pure.
//!
//! A scheduler token binds the objective, obligation revision, latest exposure
//! revision and earliest date. Reload these together before issuing
//! `ReviewDue`. A refresher supersedes the old token. If immediate practice is
//! chosen, a due signal racing that practice stays queued until accepted or
//! explicitly superseded by its outcome. Conflicts, rejections and retries
//! never settle an obligation. The clock and durable queue are adapter contracts.

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    /// Release checks on this exact revision passed at the time of use.
    Supported,
    Unavailable,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Question {
    Practice,
    RequestedProbe,
    Clarification,
    IndependentCheck,
    ChangedExample,
    DelayedReview,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Judgment {
    Correct,
    Incorrect,
    Ambiguous,
    Missing,
}

/// Unknown remains a legitimate observation. Only content-bearing assistance
/// belongs here. Input accommodation can preserve what an item tests.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Assistance {
    NoneReported,
    Reported,
    Unknown,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Next {
    Clarify,
    Explain,
    IndependentCheck,
    ChangedExample,
    WaitReview,
    HumanReview,
    OfferBreak,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Phase {
    #[default]
    Ready,
    Taught,
    /// The learner took a refresher in place of a cold check. A new clock can
    /// open a later review even if the learner stopped after the explanation.
    Refreshed,
    AwaitingAnswer(Question),
    AwaitingFeedback(Next),
    FollowUp(Next),
    ReviewDue,
    Disputed,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct State {
    pub phase: Phase,
    pub paused: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Teach(Gate),
    /// Explicit learner request before a pending or due cold check.
    RequestRefresher(Gate),
    Ask(Question, Gate),
    Answer(Judgment, Assistance),
    Feedback(Gate),
    /// A scheduler attests the due date for this objective and obligation.
    ReviewDue,
    /// The evidence reconciler rejected the support for this return. The
    /// scheduler supplies the original obligation revision and cancels its timer.
    CheckpointUnsupported,
    /// A learner, teacher or source owner has challenged the evidence.
    Challenge,
    /// An authorized reviewer released the hold. Resume with a clean projection.
    Resolve,
    Pause,
    Resume,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Taught,
    /// Atomically record new exposure, supersede the previous due token, and
    /// schedule the next cold window from that exposure before rendering.
    RetaughtBeforeReview,
    Asked(Question),
    Analyzed(Next),
    GaveFeedback(Next),
    Withheld,
    Rejected,
    ReviewOpened,
    RecheckRequired,
    Invalidated,
    Reopened,
    Paused,
    Resumed,
}

impl Event {
    /// The complete finite input alphabet used for exhaustive protocol checking.
    pub fn all() -> Vec<Event> {
        let mut events = vec![
            Self::ReviewDue,
            Self::CheckpointUnsupported,
            Self::Challenge,
            Self::Resolve,
            Self::Pause,
            Self::Resume,
        ];
        for gate in [Gate::Supported, Gate::Unavailable] {
            events.push(Self::Teach(gate));
            events.push(Self::RequestRefresher(gate));
            events.push(Self::Feedback(gate));
            for question in [
                Question::Practice,
                Question::RequestedProbe,
                Question::Clarification,
                Question::IndependentCheck,
                Question::ChangedExample,
                Question::DelayedReview,
            ] {
                events.push(Self::Ask(question, gate));
            }
        }
        for judgment in [
            Judgment::Correct,
            Judgment::Incorrect,
            Judgment::Ambiguous,
            Judgment::Missing,
        ] {
            for assistance in [
                Assistance::NoneReported,
                Assistance::Reported,
                Assistance::Unknown,
            ] {
                events.push(Self::Answer(judgment, assistance));
            }
        }
        events
    }
}

fn next_move(question: Question, judgment: Judgment, assistance: Assistance) -> Next {
    match judgment {
        Judgment::Missing => Next::OfferBreak,
        Judgment::Ambiguous if question == Question::Clarification => Next::HumanReview,
        Judgment::Ambiguous => Next::Clarify,
        Judgment::Incorrect => Next::Explain,
        Judgment::Correct => match (question, assistance) {
            (Question::IndependentCheck, Assistance::Reported | Assistance::Unknown) => {
                Next::Explain
            }
            // Clarification repairs interpretation of the previous response.
            // Open a fresh scored item before seeking a changed-example pair.
            (Question::Clarification, _) => Next::IndependentCheck,
            (_, Assistance::Reported | Assistance::Unknown) => Next::IndependentCheck,
            (Question::ChangedExample | Question::DelayedReview, Assistance::NoneReported) => {
                Next::WaitReview
            }
            (_, Assistance::NoneReported) => Next::ChangedExample,
        },
    }
}

fn allows_teaching(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Ready
            | Phase::Taught
            | Phase::AwaitingAnswer(
                Question::Practice
                    | Question::RequestedProbe
                    | Question::Clarification
                    | Question::IndependentCheck
                    | Question::ChangedExample
            )
            | Phase::FollowUp(
                Next::Clarify
                    | Next::Explain
                    | Next::IndependentCheck
                    | Next::ChangedExample
                    | Next::OfferBreak
            )
    )
}

fn allows_question(phase: Phase, question: Question) -> bool {
    matches!(
        (phase, question),
        (
            Phase::Ready | Phase::Taught | Phase::Refreshed | Phase::FollowUp(Next::OfferBreak),
            Question::RequestedProbe
        ) | (Phase::Taught | Phase::Refreshed, Question::Practice)
            | (Phase::FollowUp(Next::Clarify), Question::Clarification)
            | (
                Phase::FollowUp(Next::IndependentCheck),
                Question::IndependentCheck
            )
            | (
                Phase::FollowUp(Next::ChangedExample),
                Question::ChangedExample
            )
            | (Phase::ReviewDue, Question::DelayedReview)
    )
}

/// Applying a withheld or out-of-order request preserves the whole projection.
/// A productive answer creates feedback debt. Even a request for a different
/// lesson cannot consume that debt. A challenge invalidates dependent actions.
pub fn transition(mut state: State, event: Event) -> (State, Effect) {
    let effect = match event {
        Event::Challenge if state.phase != Phase::Disputed => {
            state.phase = Phase::Disputed;
            Effect::Invalidated
        }
        Event::Resolve
            if matches!(
                state.phase,
                Phase::Disputed | Phase::FollowUp(Next::HumanReview)
            ) =>
        {
            state.phase = Phase::Ready;
            Effect::Reopened
        }
        Event::Pause if !state.paused => {
            state.paused = true;
            Effect::Paused
        }
        Event::Resume if state.paused => {
            state.paused = false;
            Effect::Resumed
        }
        Event::ReviewDue
            if matches!(
                state.phase,
                Phase::FollowUp(Next::WaitReview) | Phase::Refreshed
            ) =>
        {
            state.phase = Phase::ReviewDue;
            Effect::ReviewOpened
        }
        Event::CheckpointUnsupported
            if matches!(
                state.phase,
                Phase::FollowUp(Next::WaitReview)
                    | Phase::ReviewDue
                    | Phase::Refreshed
                    | Phase::AwaitingAnswer(Question::DelayedReview)
            ) =>
        {
            state.phase = Phase::FollowUp(Next::IndependentCheck);
            Effect::RecheckRequired
        }
        _ if state.paused => Effect::Rejected,
        Event::RequestRefresher(gate)
            if matches!(
                state.phase,
                Phase::FollowUp(Next::WaitReview)
                    | Phase::ReviewDue
                    | Phase::Refreshed
                    | Phase::AwaitingAnswer(Question::DelayedReview)
            ) =>
        {
            if gate == Gate::Unavailable {
                Effect::Withheld
            } else {
                state.phase = Phase::Refreshed;
                Effect::RetaughtBeforeReview
            }
        }
        Event::Teach(gate) if allows_teaching(state.phase) => {
            if gate == Gate::Unavailable {
                Effect::Withheld
            } else {
                state.phase = Phase::Taught;
                Effect::Taught
            }
        }
        Event::Ask(question, gate) if allows_question(state.phase, question) => {
            if gate == Gate::Unavailable {
                Effect::Withheld
            } else {
                state.phase = Phase::AwaitingAnswer(question);
                Effect::Asked(question)
            }
        }
        Event::Answer(judgment, assistance) => match state.phase {
            Phase::AwaitingAnswer(question) => {
                let next = next_move(question, judgment, assistance);
                state.phase = Phase::AwaitingFeedback(next);
                Effect::Analyzed(next)
            }
            _ => Effect::Rejected,
        },
        Event::Feedback(gate) => match state.phase {
            Phase::AwaitingFeedback(next) if gate == Gate::Supported => {
                state.phase = Phase::FollowUp(next);
                Effect::GaveFeedback(next)
            }
            Phase::AwaitingFeedback(_) => Effect::Withheld,
            _ => Effect::Rejected,
        },
        _ => Effect::Rejected,
    };
    (state, effect)
}

/// An adapter binds the reference to the immutable, scoped input record
/// (including the presented artifact, source revision and assessment conditions).
/// `after` is the revision observed when deciding to issue this command.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub after: u64,
    pub record: u64,
    pub event: Event,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Receipt {
    Applied(Effect),
    /// The previous call already committed. Do not deliver its effect again.
    Replayed(Effect),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    VersionConflict,
    RevisionExhausted,
}

/// Constant-space reference for optimistic admission. One exact retry is served
/// from memory; older attempts receive a version conflict without side effects.
/// A production transaction must persist record identity, the new projection
/// and an outbox receipt together before exposing the effect.
#[derive(Clone, Debug, Default)]
pub struct Lane {
    revision: u64,
    state: State,
    last: Option<(Command, Effect)>,
}

impl Lane {
    pub fn state(&self) -> State {
        self.state
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply(&mut self, command: Command) -> Result<Receipt, AdmissionError> {
        if let Some((last, effect)) = self.last {
            if command == last {
                return Ok(Receipt::Replayed(effect));
            }
        }
        if command.after != self.revision {
            return Err(AdmissionError::VersionConflict);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(AdmissionError::RevisionExhausted)?;
        let (state, effect) = transition(self.state, command.event);
        self.revision = revision;
        self.state = state;
        self.last = Some((command, effect));
        Ok(Receipt::Applied(effect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: Gate = Gate::Supported;

    fn walk(start: State, script: &[(Event, Effect)]) -> State {
        script.iter().fold(start, |state, &(event, expected)| {
            let (next, effect) = transition(state, event);
            assert_eq!(effect, expected, "from {state:?} on {event:?}");
            next
        })
    }

    #[test]
    fn assisted_success_needs_a_fresh_check_and_a_changed_example_before_review() {
        use Assistance::*;
        use Judgment::Correct;
        let state = walk(
            State::default(),
            &[
                (Event::Ask(Question::Practice, OK), Effect::Rejected),
                (Event::Teach(OK), Effect::Taught),
                (
                    Event::Ask(Question::Practice, OK),
                    Effect::Asked(Question::Practice),
                ),
                (
                    Event::Answer(Correct, Reported),
                    Effect::Analyzed(Next::IndependentCheck),
                ),
                (
                    Event::Feedback(OK),
                    Effect::GaveFeedback(Next::IndependentCheck),
                ),
                (
                    Event::Ask(Question::IndependentCheck, OK),
                    Effect::Asked(Question::IndependentCheck),
                ),
                (
                    Event::Answer(Correct, NoneReported),
                    Effect::Analyzed(Next::ChangedExample),
                ),
                (
                    Event::Feedback(OK),
                    Effect::GaveFeedback(Next::ChangedExample),
                ),
                (
                    Event::Ask(Question::ChangedExample, OK),
                    Effect::Asked(Question::ChangedExample),
                ),
                (
                    Event::Answer(Correct, NoneReported),
                    Effect::Analyzed(Next::WaitReview),
                ),
                (Event::Feedback(OK), Effect::GaveFeedback(Next::WaitReview)),
                (Event::Ask(Question::DelayedReview, OK), Effect::Rejected),
                (Event::Pause, Effect::Paused),
                (Event::ReviewDue, Effect::ReviewOpened),
                (Event::Ask(Question::DelayedReview, OK), Effect::Rejected),
                (Event::Resume, Effect::Resumed),
                (
                    Event::Ask(Question::DelayedReview, OK),
                    Effect::Asked(Question::DelayedReview),
                ),
                (
                    Event::Answer(Judgment::Incorrect, Unknown),
                    Effect::Analyzed(Next::Explain),
                ),
                (Event::Feedback(OK), Effect::GaveFeedback(Next::Explain)),
            ],
        );
        assert_eq!(state.phase, Phase::FollowUp(Next::Explain));
    }

    #[test]
    fn repeated_ambiguity_ends_automatic_questioning() {
        let ambiguous = Event::Answer(Judgment::Ambiguous, Assistance::Unknown);
        let state = walk(
            State::default(),
            &[
                (
                    Event::Ask(Question::RequestedProbe, OK),
                    Effect::Asked(Question::RequestedProbe),
                ),
                (ambiguous, Effect::Analyzed(Next::Clarify)),
                (Event::Feedback(Gate::Unavailable), Effect::Withheld),
                (Event::Teach(OK), Effect::Rejected),
                (Event::Feedback(OK), Effect::GaveFeedback(Next::Clarify)),
                (
                    Event::Ask(Question::Clarification, OK),
                    Effect::Asked(Question::Clarification),
                ),
                (ambiguous, Effect::Analyzed(Next::HumanReview)),
                (Event::Feedback(OK), Effect::GaveFeedback(Next::HumanReview)),
                (Event::Ask(Question::Clarification, OK), Effect::Rejected),
                (Event::Teach(OK), Effect::Rejected),
                (Event::Resolve, Effect::Reopened),
            ],
        );
        assert_eq!(state, State::default());
    }

    #[test]
    fn clarifying_the_wording_requires_a_fresh_first_observation() {
        let state = State {
            phase: Phase::AwaitingAnswer(Question::Clarification),
            paused: false,
        };
        let next = walk(
            state,
            &[
                (
                    Event::Answer(Judgment::Correct, Assistance::NoneReported),
                    Effect::Analyzed(Next::IndependentCheck),
                ),
                (
                    Event::Feedback(OK),
                    Effect::GaveFeedback(Next::IndependentCheck),
                ),
                (Event::Ask(Question::ChangedExample, OK), Effect::Rejected),
                (
                    Event::Ask(Question::IndependentCheck, OK),
                    Effect::Asked(Question::IndependentCheck),
                ),
            ],
        );
        assert_eq!(
            next.phase,
            Phase::AwaitingAnswer(Question::IndependentCheck)
        );
    }

    #[test]
    fn a_reconciled_lapse_reopens_the_first_check_before_another_return() {
        let start = State {
            phase: Phase::AwaitingAnswer(Question::DelayedReview),
            paused: true,
        };
        let next = walk(
            start,
            &[
                (Event::CheckpointUnsupported, Effect::RecheckRequired),
                (Event::ReviewDue, Effect::Rejected),
                (Event::Resume, Effect::Resumed),
                (
                    Event::Answer(Judgment::Correct, Assistance::NoneReported),
                    Effect::Rejected,
                ),
                (
                    Event::Ask(Question::IndependentCheck, OK),
                    Effect::Asked(Question::IndependentCheck),
                ),
                (
                    Event::Answer(Judgment::Correct, Assistance::NoneReported),
                    Effect::Analyzed(Next::ChangedExample),
                ),
            ],
        );
        assert_eq!(next.phase, Phase::AwaitingFeedback(Next::ChangedExample));
    }

    #[test]
    fn stale_and_duplicate_inputs_never_publish_a_second_effect() {
        let mut lane = Lane::default();
        let probe = Command {
            after: 0,
            record: 12,
            event: Event::Ask(Question::RequestedProbe, OK),
        };
        let help = Command {
            after: 1,
            record: 13,
            event: Event::Teach(OK),
        };
        let stale_answer = Command {
            after: 1,
            record: 14,
            event: Event::Answer(Judgment::Correct, Assistance::NoneReported),
        };
        let next_item = Command {
            after: 2,
            record: 15,
            event: Event::Ask(Question::Practice, OK),
        };
        assert_eq!(
            lane.apply(probe),
            Ok(Receipt::Applied(Effect::Asked(Question::RequestedProbe)))
        );
        assert_eq!(lane.apply(help), Ok(Receipt::Applied(Effect::Taught)));
        assert_eq!(lane.apply(help), Ok(Receipt::Replayed(Effect::Taught)));
        assert_eq!(
            lane.apply(Command { record: 99, ..help }),
            Err(AdmissionError::VersionConflict)
        );
        assert_eq!(
            lane.apply(next_item),
            Ok(Receipt::Applied(Effect::Asked(Question::Practice)))
        );
        assert_eq!(
            lane.apply(stale_answer),
            Err(AdmissionError::VersionConflict)
        );
        assert_eq!(lane.apply(probe), Err(AdmissionError::VersionConflict));
        assert_eq!(lane.revision(), 3);
        assert_eq!(
            lane.state().phase,
            Phase::AwaitingAnswer(Question::Practice)
        );
    }

    #[test]
    fn correction_during_a_pause_invalidates_pending_feedback() {
        let start = State {
            phase: Phase::AwaitingFeedback(Next::WaitReview),
            paused: true,
        };
        let state = walk(
            start,
            &[
                (Event::Challenge, Effect::Invalidated),
                (Event::Feedback(OK), Effect::Rejected),
                (Event::Resolve, Effect::Reopened),
                (Event::Teach(OK), Effect::Rejected),
                (Event::Resume, Effect::Resumed),
                (Event::Feedback(OK), Effect::Rejected),
                (Event::ReviewDue, Effect::Rejected),
                (Event::Teach(OK), Effect::Taught),
            ],
        );
        assert_eq!(state.phase, Phase::Taught);
    }

    #[test]
    fn asking_for_instruction_can_replace_a_due_check_without_losing_the_next_return() {
        let start = State {
            phase: Phase::FollowUp(Next::WaitReview),
            paused: false,
        };
        let refreshed = walk(
            start,
            &[
                (Event::ReviewDue, Effect::ReviewOpened),
                (Event::Teach(OK), Effect::Rejected),
                (Event::RequestRefresher(Gate::Unavailable), Effect::Withheld),
                (
                    Event::Ask(Question::DelayedReview, OK),
                    Effect::Asked(Question::DelayedReview),
                ),
                (Event::RequestRefresher(OK), Effect::RetaughtBeforeReview),
                (
                    Event::Answer(Judgment::Correct, Assistance::NoneReported),
                    Effect::Rejected,
                ),
            ],
        );
        assert_eq!(refreshed.phase, Phase::Refreshed);
        walk(
            refreshed,
            &[(
                Event::Ask(Question::Practice, OK),
                Effect::Asked(Question::Practice),
            )],
        );
        walk(
            refreshed,
            &[
                (Event::ReviewDue, Effect::ReviewOpened),
                (
                    Event::Ask(Question::DelayedReview, OK),
                    Effect::Asked(Question::DelayedReview),
                ),
            ],
        );
    }
}
