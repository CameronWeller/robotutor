//! Offline enactment of the reviewed first Article I pack. Both branches start at the same
//! unseen check. Reviewer decisions are fixtures, supplied independently of the model draft.
use tutor_lab::{
    curriculum::creation::Pack,
    interpretation::Finding,
    protocol::{
        transition, Assistance, Effect, Event, Gate, Judgment, Next, Phase, Question, State,
    },
};

struct HumanReading {
    location: Finding,
    interpretation: Finding,
    feedback: &'static str,
}

struct Branch {
    label: &'static str,
    response: &'static str,
    proposal: Finding,
    quote: &'static str,
    reviewer: HumanReading,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Walkthrough,
    LearnerCorrect,
    LearnerSignature,
}

impl View {
    fn branches(self) -> &'static [Branch] {
        match self {
            Self::Walkthrough => &BRANCHES,
            Self::LearnerCorrect => &BRANCHES[..1],
            Self::LearnerSignature => &BRANCHES[1..],
        }
    }
}

static BRANCHES: [Branch; 2] = [
    Branch {
        label: "A",
        response: "The headline never says it was presented to the President. I found that step in Article I, section 7, clause 2.",
        proposal: Finding::Meets,
        quote: "never says it was presented to the President",
        reviewer: HumanReading {
            location: Finding::Meets,
            interpretation: Finding::Meets,
            feedback: "You pointed to the clause and identified the missing fact. The headline establishes passage by both chambers and leaves presentment open.",
        },
    },
    Branch {
        label: "B",
        response: "Article I, section 7, clause 2 mentions the President. A signature is always required, so the headline needs proof the President signed.",
        // A plausible evaluator error. It sees the right citation and accepts too much.
        proposal: Finding::Meets,
        quote: "A signature is always required",
        reviewer: HumanReading {
            location: Finding::Meets,
            interpretation: Finding::NotYet,
            feedback: "Your citation identifies the clause shown here. Its opening words require presentation. Which words in this excerpt support your additional claim about a signature? We can read the rest of the clause to investigate that question.",
        },
    },
];

fn step(state: &mut State, event: Event) -> Result<Effect, String> {
    let (after, effect) = transition(*state, event);
    if matches!(effect, Effect::Withheld | Effect::Rejected) {
        return Err(format!(
            "Cannot enact {event:?} from {:?} ({effect:?})",
            state.phase
        ));
    }
    *state = after;
    Ok(effect)
}

// A quoted learner phrase authenticates the observation. The independent reading determines
// the judgment on this check. The existing ReviewBook example exercises live admission gates.
fn review_check(mut state: State, branch: &Branch) -> Result<(State, Next), String> {
    if state.phase != Phase::AwaitingAnswer(Question::IndependentCheck) {
        return Err("This finding belongs to the new headline check".into());
    }
    if branch.quote.is_empty() || branch.response.matches(branch.quote).count() != 1 {
        return Err("The proposed quote must identify one exact passage in this attempt".into());
    }
    let judgment = match (branch.reviewer.location, branch.reviewer.interpretation) {
        (Finding::Meets, Finding::Meets) => Judgment::Correct,
        (Finding::Unresolved, _) | (_, Finding::Unresolved) => Judgment::Ambiguous,
        _ => Judgment::Incorrect,
    };
    step(
        &mut state,
        Event::Answer(judgment, Assistance::NoneReported),
    )?;
    let Effect::GaveFeedback(next) = step(&mut state, Event::Feedback(Gate::Supported))? else {
        return Err("The attempt still needs feedback".into());
    };
    Ok((state, next))
}

pub fn teach(
    pack: &Pack,
    source_url: &str,
    locator: &str,
    excerpt: &str,
    view: View,
) -> Result<(), String> {
    enact(pack, source_url, locator, excerpt, view, |text| {
        println!("{text}")
    })
}

fn enact(
    pack: &Pack,
    source_url: &str,
    locator: &str,
    excerpt: &str,
    view: View,
    mut show: impl FnMut(&str),
) -> Result<(), String> {
    let mut state = State::default();
    step(&mut state, Event::Teach(Gate::Supported))?;
    show(&format!(
        "Teacher\n{}\n{locator}\n“{excerpt}”\n{source_url}",
        pack.lesson
    ));
    show(&format!("Worked example\n{}", pack.worked));

    step(&mut state, Event::Ask(Question::Practice, Gate::Supported))?;
    show(&format!("Guided practice\n{}", pack.practice));
    show("Your response with the worked example at hand (scripted)\nThe House still has to pass it. The phrase names the House and the Senate.");
    step(
        &mut state,
        Event::Answer(Judgment::Correct, Assistance::Reported),
    )?;
    step(&mut state, Event::Feedback(Gate::Supported))?;
    show("You found both chambers in the text. Scheduling debate leaves the House vote open.");

    step(
        &mut state,
        Event::Ask(Question::IndependentCheck, Gate::Supported),
    )?;
    show(&format!("New headline check\n{}", pack.check));
    for branch in view.branches() {
        if view == View::Walkthrough {
            show(&format!(
                "Alternative {} · scripted learner, no help reported on this item",
                branch.label
            ));
        } else {
            show("Your response (scripted)");
        }
        show(branch.response);
    }
    for branch in view.branches() {
        let (mut after, next) = review_check(state, branch)?;
        if view == View::Walkthrough {
            show(&format!("Alternative {} · simulated assessor proposal, private and provisional\n{:?} on “{}”",
                branch.label, branch.proposal, branch.quote));
            show(&format!(
                "Independent human review (scripted) · identifying the locator supplied on screen {:?}, interpreting this excerpt {:?}. Independent research needs a separate task.",
                branch.reviewer.location, branch.reviewer.interpretation
            ));
        }
        show(branch.reviewer.feedback);
        match next {
            Next::ChangedExample => {
                show("Would you like to try reading a different example next time?")
            }
            Next::Explain => {
                step(&mut after, Event::Teach(Gate::Supported))?;
                show(&format!(
                    "Let’s reread the words about the President together.\n“{excerpt}”"
                ));
            }
            other => {
                return Err(format!(
                    "Unexpected follow-up for the scripted reading: {other:?}"
                ))
            }
        }
    }
    if view == View::Walkthrough {
        show("These observations concern this passage today. Transfer and delayed retention still need fresh tasks.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_learner_receives_only_their_own_attempt_and_reviewed_feedback() {
        let pack = Pack {
            lesson: "Assigned lesson".into(),
            worked: "Assigned example".into(),
            practice: "Assigned practice".into(),
            check: "Assigned check".into(),
            criterion: "Private criterion".into(),
            rubric: "Private rubric".into(),
            sources: Default::default(),
        };
        for (view, own, other) in [
            (View::LearnerCorrect, &BRANCHES[0], &BRANCHES[1]),
            (View::LearnerSignature, &BRANCHES[1], &BRANCHES[0]),
        ] {
            let mut output = String::new();
            enact(
                &pack,
                "Source URL",
                "Locator",
                "Source excerpt",
                view,
                |text| {
                    output.push_str(text);
                    output.push('\n');
                },
            )
            .unwrap();
            let ordered = [
                pack.lesson.as_str(),
                &pack.worked,
                &pack.practice,
                &pack.check,
                own.response,
                own.reviewer.feedback,
            ];
            let offsets = ordered.map(|text| output.find(text).unwrap());
            assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
            for private in [
                pack.rubric.as_str(),
                &pack.criterion,
                other.response,
                other.reviewer.feedback,
                "assessor",
                "Independent human review",
                "Meets",
                "NotYet",
            ] {
                assert!(!output.contains(private), "leaked {private}");
            }
        }
    }

    #[test]
    fn optimistic_proposal_cannot_advance_the_signature_response_or_score_practice() {
        let waiting = State {
            phase: Phase::AwaitingAnswer(Question::IndependentCheck),
            ..State::default()
        };
        assert_eq!(
            review_check(waiting, &BRANCHES[0]).unwrap().1,
            Next::ChangedExample
        );
        assert_eq!(
            review_check(waiting, &BRANCHES[1]).unwrap().1,
            Next::Explain
        );
        let practice = State {
            phase: Phase::AwaitingAnswer(Question::Practice),
            ..waiting
        };
        assert!(review_check(practice, &BRANCHES[1]).is_err());
    }
}
