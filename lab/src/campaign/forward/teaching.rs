//! Reference decisions from the real fraction checker and dialogue reducer. Each episode
//! follows one learner through instruction, feedback and a later visit. Classroom seats
//! share a source interruption but retain individual answers, help and dispute histories.
//! The clock, answers, assistance reports and reviewer interventions are synthetic. These
//! observations exercise control flow; they do not measure teaching quality or retention.
use super::{Observation, Rng};
use crate::curriculum::Span;
use crate::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use crate::protocol::{
    transition, Assistance, Effect, Event, Gate, Judgment, Next, Phase, Question, State,
};
use crate::temporal::check_trace;
use std::collections::BTreeSet;

struct Episode<'a> {
    seat: u32,
    day: u32,
    group: bool,
    state: State,
    events: Vec<Event>,
    rows: &'a mut Vec<Observation>,
    pin: Span,
    withdrawn: BTreeSet<[u8; 32]>,
    prior_dispute: bool,
    prior_hints: bool,
}

impl Episode<'_> {
    fn source(&self) -> RuleSource<'_> {
        RuleSource {
            bytes: Some(UNIT_RULE.as_bytes()),
            pin: self.pin,
            revoked: &self.withdrawn,
        }
    }

    fn step(&mut self, event: Event) -> Effect {
        let before = self.state;
        let prior_question = match before.phase {
            Phase::AwaitingAnswer(q) => Some(q),
            _ => None,
        };
        let features = vec![
            ("answer submitted", matches!(event, Event::Answer(..))),
            ("feedback requested", matches!(event, Event::Feedback(_))),
            ("question requested", matches!(event, Event::Ask(..))),
            (
                "instruction requested",
                matches!(event, Event::Teach(_) | Event::RequestRefresher(_)),
            ),
            ("source withdrawn", !self.withdrawn.is_empty()),
            ("learner paused", before.paused),
            ("active dispute", before.phase == Phase::Disputed),
            (
                "feedback owed",
                matches!(before.phase, Phase::AwaitingFeedback(_)),
            ),
            (
                "clarification item open",
                prior_question == Some(Question::Clarification),
            ),
            (
                "independent item open",
                prior_question == Some(Question::IndependentCheck),
            ),
            (
                "changed item open",
                prior_question == Some(Question::ChangedExample),
            ),
            (
                "return item open",
                prior_question == Some(Question::DelayedReview),
            ),
            (
                "checker supports numeric answer",
                matches!(event, Event::Answer(Judgment::Correct, _)),
            ),
            (
                "checker found numeric discrepancy",
                matches!(event, Event::Answer(Judgment::Incorrect, _)),
            ),
            (
                "response needs interpretation",
                matches!(event, Event::Answer(Judgment::Ambiguous, _)),
            ),
            (
                "response absent",
                matches!(event, Event::Answer(Judgment::Missing, _)),
            ),
            (
                "content help reported",
                matches!(event, Event::Answer(_, Assistance::Reported)),
            ),
            (
                "help status unknown",
                matches!(event, Event::Answer(_, Assistance::Unknown)),
            ),
            ("earlier content help", self.prior_hints),
            ("earlier dispute", self.prior_dispute),
            ("later visit", self.day > 0),
            ("classroom seat", self.group),
        ];
        let (after, effect) = transition(before, event);
        // Sample the two decisions faced by the tutor: how to respond to the evidence
        // about an answer, and whether it may deliver an offered item or feedback now.
        // The arithmetic judgment is available to the tutor before this routing decision.
        let label = match effect {
            Effect::Analyzed(Next::Explain) => Some("offer an explanation"),
            Effect::Analyzed(Next::Clarify) => Some("ask what the learner meant"),
            Effect::Analyzed(Next::HumanReview) => Some("invite subject review"),
            Effect::Analyzed(Next::OfferBreak) => Some("offer a break"),
            Effect::Analyzed(Next::IndependentCheck) => Some("offer a fresh independent check"),
            Effect::Analyzed(Next::ChangedExample) => Some("offer a changed example"),
            Effect::Analyzed(Next::WaitReview) => Some("schedule a later return"),
            Effect::Taught
            | Effect::RetaughtBeforeReview
            | Effect::Asked(_)
            | Effect::GaveFeedback(_) => Some("deliver the requested material"),
            Effect::Withheld => Some("withhold pending source access"),
            Effect::Rejected => Some("defer this move"),
            _ => None,
        };
        if let Some(label) = label {
            let location = format!(
                "seat {} day {} event {} {:?}",
                self.seat,
                self.day,
                self.events.len() + 1,
                event
            );
            self.rows
                .push(Observation::new("teaching", location, features, label));
        }
        self.events.push(event);
        self.state = after;
        self.prior_dispute |= matches!(event, Event::Challenge);
        self.prior_hints |= matches!(event, Event::Answer(_, Assistance::Reported));
        effect
    }

    fn gate(&self, card: FractionCard) -> Gate {
        if card.question(&self.source()).is_some() {
            Gate::Supported
        } else {
            Gate::Unavailable
        }
    }

    fn teach(&mut self, card: FractionCard) {
        let gate = if card.worked(&self.source()).is_some() {
            Gate::Supported
        } else {
            Gate::Unavailable
        };
        self.step(Event::Teach(gate));
    }

    fn ask(&mut self, question: Question, card: FractionCard) {
        self.step(Event::Ask(question, self.gate(card)));
    }

    fn answer(&mut self, card: FractionCard, answer: &str, help: Assistance) -> Result<(), String> {
        let judgment = card
            .assess(answer, &self.source())
            .ok_or("fraction evidence unavailable")?;
        if !matches!(
            self.step(Event::Answer(judgment, help)),
            Effect::Analyzed(_)
        ) {
            return Err("answer did not belong to an open teaching item".into());
        }
        Ok(())
    }

    fn feedback(&mut self, card: FractionCard) -> Result<Next, String> {
        match self.step(Event::Feedback(self.gate(card))) {
            Effect::GaveFeedback(next) => Ok(next),
            other => Err(format!(
                "required teaching feedback was not delivered: {other:?}"
            )),
        }
    }

    fn try_item(
        &mut self,
        question: Question,
        item: &Item,
        answer: &str,
        help: Assistance,
    ) -> Result<Next, String> {
        self.ask(question, item.card);
        self.answer(item.card, answer, help)?;
        self.feedback(item.card)
    }
}

struct Item {
    card: FractionCard,
    correct: String,
}

fn item(task: FractionTask, first: u16, second: u16, parts: u16) -> Item {
    let count = if task == FractionTask::Sum {
        first + second
    } else {
        second
    };
    Item {
        card: FractionCard::propose(task, first, second, parts)
            .expect("counts fit the generated whole"),
        correct: format!("{count}/{parts}"),
    }
}

fn episode(
    lane: &mut Episode<'_>,
    rng: &mut Rng,
    profile: u32,
    class_interruption: bool,
) -> Result<(), String> {
    use Assistance::{NoneReported, Reported, Unknown};
    use Question::{ChangedExample, Clarification, DelayedReview, IndependentCheck, Practice};
    let n = 1 + rng.below(6) as u16;
    let first = item(FractionTask::Sum, n, 2, n + 6);
    let independent = item(FractionTask::Sum, n, 3, n + 7);
    let changed = item(FractionTask::MissingPart, 2, n, n + 5);
    let later = item(FractionTask::MissingPart, 3, n + 1, n + 8);
    let help = if rng.below(2) == 0 { Reported } else { Unknown };

    lane.teach(first.card);
    lane.ask(Practice, first.card);
    let response = match profile % 4 {
        0 => first.correct.as_str(),
        1 => "about half",
        2 => "1/1",
        _ => "",
    };
    lane.answer(first.card, response, help)?;
    // Each seat can have outstanding individual feedback when the common source
    // permission changes. Retry following restoration without analyzing a second answer.
    if (lane.group && class_interruption) || (!lane.group && profile.is_multiple_of(3)) {
        lane.withdrawn.insert(lane.pin.document);
        lane.step(Event::Feedback(lane.gate(first.card)));
        lane.withdrawn.clear();
    }
    let mut next = lane.feedback(first.card)?;
    if next == Next::Clarify {
        let reply = if profile.is_multiple_of(3) {
            "I meant the bigger piece"
        } else {
            &first.correct
        };
        next = lane.try_item(Clarification, &first, reply, NoneReported)?;
    }
    // An appeal holds instruction until a reviewer resolves the disputed record.
    if profile % 3 == 1 {
        lane.step(Event::Challenge);
        lane.ask(Practice, first.card);
        lane.step(Event::Resolve);
        lane.teach(first.card);
        next = lane.try_item(Practice, &first, &first.correct, Reported)?;
    } else if matches!(next, Next::OfferBreak | Next::Explain | Next::HumanReview) {
        if next == Next::HumanReview {
            lane.step(Event::Resolve);
        }
        if next == Next::OfferBreak {
            lane.step(Event::Pause);
            lane.ask(Practice, first.card);
            lane.step(Event::Resume);
        }
        lane.teach(first.card);
        next = lane.try_item(Practice, &first, &first.correct, Reported)?;
    }
    if next != Next::IndependentCheck {
        return Err("episode lost its independent check".into());
    }
    if profile % 4 == 2 {
        // A nominally independent item that received help needs fresh evidence.
        lane.try_item(IndependentCheck, &independent, &independent.correct, help)?;
        lane.teach(first.card);
        lane.try_item(Practice, &first, &first.correct, Reported)?;
        let fresh = item(FractionTask::Sum, 2, n + 3, n + 9);
        lane.try_item(IndependentCheck, &fresh, &fresh.correct, NoneReported)?;
    } else {
        lane.try_item(
            IndependentCheck,
            &independent,
            &independent.correct,
            NoneReported,
        )?;
    }
    lane.try_item(ChangedExample, &changed, &changed.correct, NoneReported)?;

    lane.day = 21 + rng.below(10);
    lane.step(Event::ReviewDue);
    if profile.is_multiple_of(5) {
        lane.step(Event::RequestRefresher(lane.gate(first.card)));
        lane.day += 21 + rng.below(10);
        lane.step(Event::ReviewDue);
    }
    let answer = match rng.below(4) {
        0 => "1/1",
        1 => "I used the other piece",
        _ => &later.correct,
    };
    lane.try_item(DelayedReview, &later, answer, NoneReported)?;
    lane.step(Event::Pause);
    check_trace(State::default(), &lane.events)
}

pub(super) fn cases(seed: u64) -> Result<Vec<Observation>, String> {
    let mut rng = Rng::new(seed);
    let rotation = rng.below(12);
    let class_interruption = rng.below(4) != 0;
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let mut rows = Vec::new();
    for seat in 0..12 {
        let mut lane = Episode {
            seat,
            day: 0,
            group: seat >= 6,
            state: State::default(),
            events: Vec::new(),
            rows: &mut rows,
            pin,
            withdrawn: BTreeSet::new(),
            prior_dispute: false,
            prior_hints: false,
        };
        episode(
            &mut lane,
            &mut rng,
            (seat + rotation) % 12,
            class_interruption,
        )
        .map_err(|why| format!("teaching seat {seat} at seed {seed}: {why}"))?;
    }
    Ok(rows)
}
