//! Fictional private exercises and the learner's one saved choice for a later visit.
use serde_json::{json, Value};

// These are authored, fictional choices. No parser scores a learner's prose.
pub(super) fn example(name: &str) -> Result<Value, String> {
    const WARM_LINE: &str =
        "Thanks for all your help! I told the others you will stay with me until seven.";
    let (situation, line, observable, experiment) = match name {
        "warm" => (
            "The other volunteer said they need to leave at six and has not agreed to stay.",
            WARM_LINE,
            "The thanks expresses appreciation. The speaker has promised someone else's time after hearing they need to leave.",
            "Keep the thanks. Try a second version in which the other person can choose whether to stay.",
        ),
        "agreed" => (
            "The other volunteer explicitly asked the speaker to tell the organizer they can stay until seven.",
            WARM_LINE,
            "The speaker passes on the choice that the other volunteer asked them to communicate.",
            "How could you leave room for the volunteer to update the plan if something changes?",
        ),
        "brief" => (
            "Each volunteer is free to choose when to leave.",
            "I'm leaving at six. Stay if you want.",
            "The speaker names their own departure and explicitly leaves the other person's departure open.",
            "Read it aloud. If you'd like it to sound warmer, change its tone while leaving both options open.",
        ),
        "ambiguous" => (
            "The meeting is still in progress. A colleague makes a suggestion.",
            "Let's finish tomorrow.",
            "Someone could hear a settled plan or an invitation to decide together. What did the speaker intend here?",
            "Try one version that sets only the speaker's limit and one version that invites a reply. You can also keep the original.",
        ),
        _ => return Err("choose warm, agreed, brief, or ambiguous".into()),
    };
    Ok(json!({
        "contribution": "private_example",
        "display": {"situation":situation,"example":line,"observable":observable,"experiment":experiment},
        "authority":"private_local"
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PracticeAim {
    StateLimit,
    InviteChoice,
}

impl PracticeAim {
    fn read(s: &str) -> Result<Option<Self>, String> {
        match s {
            "limit" => Ok(Some(Self::StateLimit)),
            "invitation" => Ok(Some(Self::InviteChoice)),
            "clear" => Ok(None),
            _ => Err("choose limit, invitation, or clear".into()),
        }
    }
    fn return_prompt(self) -> &'static str {
        match self {
            Self::StateLimit=>"You chose to try stating a limit. A volunteer shift could run late. What can you promise about your own time? You can skip.",
            Self::InviteChoice=>"You chose to try inviting a choice. A volunteer shift could run late. What could you ask without promising anyone else's time? You can skip."
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct SavedExercise {
    next: Option<(u32, PracticeAim)>,
}

impl SavedExercise {
    pub(super) fn choose(&mut self, day: u32, name: &str) -> Result<Value, String> {
        // An explicit choice can replace or clear the prospective exercise.
        let aim = PracticeAim::read(name)?;
        self.next = aim.map(|aim| (day, aim));
        let display = if aim.is_some() {
            "Saved your choice of exercise for a later meeting. You can replace it or clear it."
        } else {
            "There is no saved exercise to offer."
        };
        Ok(json!({"contribution":"learner_choice","display":display,"authority":"private_local"}))
    }

    pub(super) fn visit(&mut self, current_day: u32, day: u32) -> Result<Value, String> {
        if day < current_day || self.next.is_some_and(|(saved, _)| day <= saved) {
            return Err("choose a later meeting".into());
        }
        let display = self
            .next
            .take()
            .map(|(_, aim)| aim.return_prompt())
            .unwrap_or("There is no saved exercise to offer.");
        Ok(
            json!({"contribution":"private_practice_visit","display":display,"authority":"private_local"}),
        )
    }
}
