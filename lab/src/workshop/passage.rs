//! Private comparison of a learner-selected excerpt and their explanation.
//! The only machine check is whether the selected words occur in the supplied text.
use crate::coaching::usable_line;
use std::ops::Range;

#[derive(Clone, Copy)]
pub enum Purpose {
    ExplainInstruction,
    InterpretLanguage,
}

pub struct PassagePractice {
    purpose: Purpose,
    passage: String,
    selected: Option<Range<usize>>,
}

impl PassagePractice {
    pub fn new(purpose: Purpose, passage: &str) -> Result<Self, &'static str> {
        if !usable_line(passage, 3072) {
            return Err(
                "Supply one short passage on a single line (up to 3072 bytes), or type skip.",
            );
        }
        Ok(Self {
            purpose,
            passage: passage.into(),
            selected: None,
        })
    }

    pub fn opening(&self) -> String {
        let demonstration = match self.purpose {
            Purpose::ExplainInstruction => "Worked example (invented): “Use the side door during repairs.” A reader who hears “Always use the side door” loses the time limit. Look for who acts, what they do, and any condition that changes when the instruction applies.",
            Purpose::InterpretLanguage => "Worked example (invented): “A second cup stood untouched.” The cup is in the text. An absent guest is one possible explanation. Ask which words support that reading and what else those words could mean.",
        };
        format!("Private reading practice\n{demonstration}\nYour supplied wording · {}\nWhich short phrase would you keep when explaining this to someone else? Type quote followed by the exact words, unsure for a hint, or skip.", self.passage)
    }

    pub fn select(&mut self, words: &str) -> Result<String, &'static str> {
        self.selected = None;
        if !usable_line(words, super::draft_model::MAX_QUOTE_BYTES) {
            return Err("Choose a short phrase of up to 384 bytes, or type unsure.");
        }
        let start = self.passage.find(words).ok_or("Those exact words do not appear together. Copy a phrase from your supplied wording, or type unsure.")?;
        self.selected = Some(start..start + words.len());
        let invitation = match self.purpose {
            Purpose::ExplainInstruction => "Explain the instruction to a first-time reader in one sentence. Keep the action and any condition the passage gives.",
            Purpose::InterpretLanguage => "Write one possible reading for someone seeing this for the first time. Mark an inference with words such as “may suggest”.",
        };
        Ok(format!("You selected “{words}” from the passage. {invitation} Type draft followed by your sentence, or skip."))
    }

    pub fn hint(&self) -> &'static str {
        match self.purpose {
            Purpose::ExplainInstruction => "Look for a word that changes who, when, or what. In the invented example, removing “during repairs” changes when the instruction applies. If your passage gives no condition, you can quote the action itself. Type quote followed by your choice, or skip.",
            Purpose::InterpretLanguage => "Start with a detail you could point to in the passage. In the invented example, “untouched” describes the cup and leaves the reason open. Type quote followed by a detail in your own passage, or skip.",
        }
    }

    pub fn compare(&self, draft: &str) -> Result<String, &'static str> {
        let selected = self.selected.as_ref().ok_or(
            "Choose supporting words first. Type quote followed by an exact phrase, or unsure.",
        )?;
        if !usable_line(draft, 3072) {
            return Err(
                "Type draft followed by one sentence on a single line (up to 3072 bytes), or skip.",
            );
        }
        let quote = &self.passage[selected.clone()];
        let question = match self.purpose {
            Purpose::ExplainInstruction => "Could someone follow your sentence in a situation where the original instruction would no longer apply? Check the action and any condition against the whole passage. Which point would you verify with the issuer before using this as real guidance?",
            Purpose::InterpretLanguage => "Which part of your sentence can a second reader point to, and which part is your interpretation? Try a different reading of the same detail and see whether the rest of the passage supports it.",
        };
        Ok(format!("Your selected words · {quote}\nYour draft · {draft}\n{question}\nType draft followed by a revision to compare again, quote to choose different words, or skip to leave the exercise."))
    }
}
