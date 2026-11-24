//! A learner's private, in-memory workshop for their draft or work in progress.
//! A note points to the submitted text and invites one experiment or observation.
pub mod course;
pub mod draft_model;
pub mod live;
pub mod passage;

use std::{fmt, ops::Range, str::FromStr};

pub const MAX_DRAFT_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Clarity,
    Rhythm,
    Voice,
    NextStep,
}

impl fmt::Display for Focus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Clarity => "clarity",
            Self::Rhythm => "rhythm",
            Self::Voice => "voice",
            Self::NextStep => "next-step",
        })
    }
}

impl FromStr for Focus {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "clarity" => Ok(Self::Clarity),
            "rhythm" => Ok(Self::Rhythm),
            "voice" => Ok(Self::Voice),
            "next-step" | "next step" => Ok(Self::NextStep),
            _ => Err("choose clarity, rhythm, voice, or next-step"),
        }
    }
}

pub struct Workshop {
    draft: String,
    focus: Focus,
    revision: u64,
    revision_focus: Option<Focus>,
    comparison: Option<RevisionComparison>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionComparison {
    pub from_revision: u64,
    pub to_revision: u64,
    pub before: String,
    pub after: String,
}

/// A replacement is attributable when exactly one aligned passage differs and
/// each side occurs once in its respective version. A changed line also covers
/// an edit that splits one sentence in two. Keep only the bounded passage pair.
fn changed_passage<'a>(old: &'a str, new: &'a str) -> Option<(&'a str, &'a str)> {
    fn one<'a>(
        mut left: impl Iterator<Item = &'a str>,
        mut right: impl Iterator<Item = &'a str>,
    ) -> Option<(&'a str, &'a str)> {
        let mut pair = None;
        loop {
            match (left.next(), right.next()) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), Some(b)) if pair.is_none() => pair = Some((a, b)),
                (None, None) => return pair,
                _ => return None,
            }
        }
    }
    let eligible = |&(a, b): &(&str, &str)| {
        [a, b]
            .iter()
            .all(|s| crate::coaching::usable_line(s, draft_model::MAX_QUOTE_BYTES))
            && old.find(a) == old.rfind(a)
            && new.find(b) == new.rfind(b)
    };
    one(
        passages(old).into_iter().map(|p| p.text),
        passages(new).into_iter().map(|p| p.text),
    )
    .filter(eligible)
    .or_else(|| one(old.lines().map(str::trim), new.lines().map(str::trim)).filter(eligible))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Feedback {
    /// Half-open UTF-8 byte offsets into the exact draft at this revision.
    pub span: Range<usize>,
    pub revision: u64,
    pub observation: String,
    pub experiment: String,
}

fn validate(draft: &str) -> Result<(), &'static str> {
    if draft.len() > MAX_DRAFT_BYTES {
        return Err("draft exceeds 16384 UTF-8 bytes");
    }
    if draft.trim().is_empty() {
        return Err("supply a draft with some visible text");
    }
    Ok(())
}

impl Workshop {
    pub fn new(draft: &str, focus: Focus) -> Result<Self, &'static str> {
        validate(draft)?;
        Ok(Self {
            draft: draft.to_owned(),
            focus,
            revision: 1,
            revision_focus: Some(focus),
            comparison: None,
        })
    }

    pub fn draft(&self) -> &str {
        &self.draft
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn set_focus(&mut self, focus: Focus) {
        if self.focus != focus {
            self.comparison = None;
            self.revision_focus = None;
        }
        self.focus = focus;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn revise(&mut self, draft: &str) -> Result<(), &'static str> {
        self.replace(draft, false)
    }

    /// Explicit opt-in to retain one small before/after pair for this edit only.
    pub fn revise_for_comparison(&mut self, draft: &str) -> Result<(), &'static str> {
        self.replace(draft, true)
    }

    pub fn comparison(&self) -> Option<&RevisionComparison> {
        self.comparison.as_ref()
    }

    fn replace(&mut self, draft: &str, compare: bool) -> Result<(), &'static str> {
        validate(draft)?;
        if self.draft != draft {
            let next = self
                .revision
                .checked_add(1)
                .ok_or("draft revision exhausted")?;
            self.comparison = if compare && self.revision_focus == Some(self.focus) {
                changed_passage(&self.draft, draft).map(|(before, after)| RevisionComparison {
                    before: before.into(),
                    after: after.into(),
                    from_revision: self.revision,
                    to_revision: next,
                })
            } else {
                None
            };
            self.revision = next;
            self.revision_focus = Some(self.focus);
            self.draft = draft.to_owned();
        } else {
            self.comparison = None;
        }
        Ok(())
    }

    pub fn feedback(&self) -> Feedback {
        let passages = passages(&self.draft);
        let (quote, observation, experiment) = match self.focus {
            Focus::Clarity => clarity(&passages),
            Focus::Rhythm => rhythm(&passages),
            Focus::Voice => voice(&passages),
            Focus::NextStep => next_step(&passages),
        };
        // All quote choices are borrowed slices of this draft. Repeated phrases
        // keep their actual occurrence; searching for their text could pick another.
        let start = quote.as_ptr() as usize - self.draft.as_ptr() as usize;
        Feedback {
            span: start..start + quote.len(),
            revision: self.revision,
            observation,
            experiment,
        }
    }
}

struct Passage<'a> {
    text: &'a str,
    /// Space-separated groups with surrounding punctuation removed. These are
    /// anchors for editing, not a language-independent word or sentence count.
    terms: Vec<&'a str>,
}

fn passages(draft: &str) -> Vec<Passage<'_>> {
    let mut parts = Vec::new();
    let mut previous_stop = false;
    let mut start = 0;
    // Keep punctuation inside a printed number or an unspaced initialism intact.
    for (i, c) in draft.char_indices() {
        let line_end = matches!(c, '\n' | '\r');
        let wide_stop = matches!(c, '。' | '！' | '？');
        if line_end || (previous_stop && c.is_whitespace()) || wide_stop {
            let end = if wide_stop { i + c.len_utf8() } else { i };
            push_passage(&draft[start..end], &mut parts);
            start = i + c.len_utf8();
            previous_stop = false;
        } else if !matches!(c, '"' | '”' | '’' | ')' | ']') {
            previous_stop = matches!(c, '.' | '!' | '?' | '…');
        }
    }
    push_passage(&draft[start..], &mut parts);
    parts
}

fn push_passage<'a>(text: &'a str, parts: &mut Vec<Passage<'a>>) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let terms = text
        .split_whitespace()
        .map(|term| {
            term.trim_matches(|c: char| c.is_ascii_punctuation() || "…‘’“”—–。、！？".contains(c))
        })
        .filter(|term| !term.is_empty())
        .collect();
    parts.push(Passage { text, terms });
}

type Note<'a> = (&'a str, String, String);

fn member(word: &str, choices: &[&str]) -> bool {
    choices.iter().any(|other| word.eq_ignore_ascii_case(other))
}

fn next_step<'a>(parts: &[Passage<'a>]) -> Note<'a> {
    for p in parts {
        let end = p
            .text
            .trim_end_matches(['"', '\'', '”', '’', ')', ']', '」', '』', '!']);
        let labeled = p.text.split_once(':').is_some_and(|(label, body)| {
            !body.trim().is_empty() && member(label.trim(), &["question", "open question"])
        });
        if end.ends_with(['?', '？']) || labeled {
            return (p.text,
                "You pose a question here.".into(),
                "Choose one source you can consult or one observation you can make to settle this question. Write down what result would change your next decision, then compare it with what you find.".into());
        }
    }
    for p in parts {
        if let Some(word) = p.terms.iter().find(|w| {
            member(
                w,
                &[
                    "assume",
                    "assumes",
                    "assumed",
                    "assuming",
                    "assumption",
                    "assumptions",
                ],
            )
        }) {
            return (p.text,
                format!("The word “{word}” flags a possible assumption to examine."),
                "Pick one assumption in this passage. What would you expect to observe if it holds? Check one source or example and note whether the result changes your plan.".into());
        }
    }
    let p = parts
        .iter()
        .rev()
        .find(|p| !p.terms.is_empty())
        .unwrap_or(&parts[0]);
    (p.text,
        "Use this passage as the starting point for your next move.".into(),
        "Name one small action you could take on this part of the work. Before trying it, say what you would need to see or produce to decide what comes next. Then record what actually happened.".into())
}

fn clarity<'a>(parts: &[Passage<'a>]) -> Note<'a> {
    for p in parts {
        if let Some([a, b, c]) = p.terms.windows(3).find(|t| {
            t[0].eq_ignore_ascii_case("in")
                && t[1].eq_ignore_ascii_case("order")
                && t[2].eq_ignore_ascii_case("to")
        }) {
            return (p.text,
                format!("“{a} {b} {c}” gives the reader a lead-in before the purpose of the action."),
                "Try reading that phrase as just “to”. Listen for whether the longer lead-in helped the argument or the sound of the line, and keep the version you prefer.".into(),
            );
        }
        for pair in p.terms.windows(2) {
            if member(pair[0], &["this", "that", "it", "they"])
                && member(
                    pair[1],
                    &[
                        "is", "was", "were", "are", "will", "would", "could", "made", "means",
                        "meant",
                    ],
                )
            {
                return (p.text, format!("The phrase “{} {}” begins with “{}”. What should the reader picture here?",pair[0],pair[1],pair[0]),
                    format!("If “{}” stands for a person, thing, or earlier event, try naming it in a copy. Read it with the previous line, if there is one. Which version gives the reader the amount of certainty you want?",pair[0]));
            }
        }
        if let Some(word) = p
            .terms
            .iter()
            .find(|w| member(w, &["soon", "later", "somewhere", "eventually"]))
        {
            return (p.text,format!("“{word}” leaves the exact time or place open here."),
                format!("Try a version with one visible place or time cue in place of “{word}”. Compare the sense of direction or suspense, then keep what suits the passage."));
        }
    }
    let p = parts
        .iter()
        .max_by_key(|p| p.terms.len())
        .expect("nonblank draft");
    if let Some([_, word, _]) = p
        .terms
        .windows(3)
        .find(|w| member(w[1], &["but", "although", "because", "and", "while"]))
    {
        return (p.text,format!("“{word}” connects two parts of this passage."),
            format!("Mark the two thoughts on either side of “{word}”. Try putting the one your reader needs first in a sentence of its own. Read both versions for what the connection gains or loses."));
    }
    match p.terms.as_slice() {
        [first, .., last] => (p.text, format!("This passage opens with “{first}” and lands on “{last}”."),
            format!("Cover everything after “{first}” and say what a reader needs to know first. Try moving that detail earlier, then check whether the landing on “{last}” still works.")),
        _ => (p.text,"This passage presents a single visual or verbal unit.".into(),
            "Set one concrete detail beside it in a copy. Which reading gives the audience the amount of context you intend?".into()),
    }
}

fn rhythm<'a>(parts: &[Passage<'a>]) -> Note<'a> {
    for pair in parts.windows(2) {
        if let (Some(a), Some(b)) = (pair[0].terms.first(), pair[1].terms.first()) {
            if a.eq_ignore_ascii_case(b) {
                return (pair[1].text,format!("This passage and the preceding one both begin with “{b}”."),
                    format!("Read the two openings aloud as a refrain. Try starting the second somewhere other than “{b}”. Which version gives the second beat the emphasis you hear?"));
            }
        }
    }
    if let Some(pair) = parts
        .windows(2)
        .max_by_key(|p| p[0].terms.len().abs_diff(p[1].terms.len()))
    {
        let (before, here) = (pair[0].terms.len(), pair[1].terms.len());
        return (pair[1].text,format!("I count {before} space-separated groups in the preceding passage and {here} here."),
            "Read the join aloud once. In a copy, move your pause earlier or later by one phrase. Which landing gives the following line the timing you want?".into());
    }
    let p = &parts[0];
    if let Some(pause) = p
        .text
        .chars()
        .find(|c| matches!(c, ',' | ';' | ':' | '—' | '、' | '，'))
    {
        return (p.text,format!("The “{pause}” gives you a possible pause inside this passage."),
            format!("Read through “{pause}” without pausing, then give it a full beat. Try the punctuation or line break that best matches your reading."));
    }
    if p.terms.len() >= 2 {
        let anchor = p.terms[p.terms.len() / 2];
        return (p.text,format!("“{anchor}” falls near the middle of this passage."),
            format!("Read it once in a single breath, then try a break just before “{anchor}”. Listen for which words take the stress in each reading."));
    }
    (p.text,"You can read this short unit on its own.".into(),
        "Try repeating the passage on a second line, once close to the first and once after a blank line. Read both versions aloud and keep the pacing you want.".into())
}

fn voice<'a>(parts: &[Passage<'a>]) -> Note<'a> {
    for p in parts {
        for word in &p.terms {
            if word.contains(['\'', '’']) {
                return (p.text,format!("The spelling of “{word}” is one of the audible choices on this line."),
                    format!("Say the line to an imagined listener. Try another way you would actually say “{word}”, then read both aloud and keep the version that sounds like your speaker."));
            }
            if member(word, &["I", "me", "my", "we", "our", "you", "your"]) {
                return (p.text,format!("“{word}” places a speaker or listener directly inside this passage."),
                    format!("Read it as written. In a copy, start with an image or action from the scene and bring “{word}” in later. Which distance from the reader fits the voice you want?"));
            }
        }
    }
    let p = parts
        .iter()
        .find(|p| p.terms.len() >= 2)
        .unwrap_or(&parts[0]);
    // Preserve the author's exact spelling; length only chooses an audible word to try.
    let word = p
        .terms
        .iter()
        .max_by_key(|w| w.chars().count())
        .copied()
        .unwrap_or(p.text);
    (p.text,format!("Here the page gives the reader “{word}” in your chosen wording."),
        format!("Try saying this line to someone you know, then to a stranger. Write down one alternative to “{word}” that you actually used. Read it beside the original and keep the wording that sounds like you."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critique_quotes_the_actual_multiline_passage_and_changes_with_the_focus() {
        let draft = "We waited.\r\n\r\nWe waited.  This was a long night.\n";
        let mut w = Workshop::new(draft, Focus::Clarity).unwrap();
        let f = w.feedback();
        assert_eq!(&draft[f.span], "This was a long night.");
        assert!(f.experiment.contains("“This”"));
        w.set_focus(Focus::Rhythm);
        let f = w.feedback();
        assert_eq!(&draft[f.span.clone()], "We waited.");
        assert_eq!(f.span.start, draft.find("\r\n\r\n").unwrap() + 4);
        assert!(f.observation.contains("preceding"));
        w.set_focus(Focus::Voice);
        let f = w.feedback();
        assert_eq!(f.span.start, 0);
        assert!(f.experiment.contains("“We”"));
        assert_eq!(w.draft(), draft);
    }

    #[test]
    fn invalid_and_identical_edits_preserve_revision_and_changed_text_is_reanalysed() {
        let mut w = Workshop::new("Come later.", Focus::Clarity).unwrap();
        assert!(w.feedback().observation.contains("“later”"));
        assert!(w.revise("  \t\n").is_err());
        assert!(w.revise(&"a".repeat(MAX_DRAFT_BYTES + 1)).is_err());
        w.revise("Come later.").unwrap();
        assert_eq!((w.draft(), w.revision()), ("Come later.", 1));
        w.revise("Come after the bell.").unwrap();
        let f = w.feedback();
        assert_eq!(f.revision, 2);
        assert_eq!(&w.draft()[f.span], "Come after the bell.");
        assert!(!f.observation.contains("later"));
        w.revise("Come later.").unwrap();
        assert_eq!(w.feedback().revision, 3);
    }

    #[test]
    fn project_notes_identify_what_the_learner_can_check_next() {
        let plan =
            "Community supper layout\r\nAssuming each table seats six, we need eight tables.";
        let mut w = Workshop::new(plan, "next step".parse().unwrap()).unwrap();
        assert_eq!(w.focus().to_string(), "next-step");
        let f = w.feedback();
        assert_eq!(
            &plan[f.span],
            "Assuming each table seats six, we need eight tables."
        );
        assert!(f.experiment.contains("Check one source or example"));

        let question = "How many tables does the hall actually have?";
        w.revise(&format!("{plan}\n{question}")).unwrap();
        let f = w.feedback();
        assert_eq!(&w.draft()[f.span], question);
        assert!(f.experiment.contains("one observation"));

        w.revise("Hall inventory checked.\nDraw the two table layouts.")
            .unwrap();
        let f = w.feedback();
        assert_eq!(&w.draft()[f.span], "Draw the two table layouts.");
        assert!(f.experiment.contains("one small action"));
    }

    #[test]
    fn comparison_needs_permission_one_locatable_replacement_and_consecutive_same_focus() {
        let mut w = Workshop::new("Keep café open.\nRain came.", Focus::Rhythm).unwrap();
        w.revise("Keep café open.\nSnow came.").unwrap();
        assert!(w.comparison().is_none());
        w.revise_for_comparison("Keep café open.\nLe soleil est revenu.")
            .unwrap();
        let change = w.comparison().unwrap();
        assert_eq!(
            (
                change.before.as_str(),
                change.after.as_str(),
                change.from_revision,
                change.to_revision
            ),
            ("Snow came.", "Le soleil est revenu.", 2, 3)
        );
        w.set_focus(Focus::Voice);
        w.set_focus(Focus::Rhythm);
        assert!(w.comparison().is_none());
        w.revise_for_comparison("Keep café open.\nSnow came back.")
            .unwrap();
        assert!(w.comparison().is_none());
        w.revise_for_comparison("Keep café open.\nSnow again.")
            .unwrap();
        assert!(w.comparison().is_some());
        for (old, new) in [
            ("Wait.\nWait.", "Wait.\nGo."),
            ("Yesterday.\nRain.", "Today.\nSnow."),
            ("The square.", "The square.\nWe waited."),
            ("Moon.", "Moon."),
        ] {
            let mut w = Workshop::new(old, Focus::Clarity).unwrap();
            w.revise_for_comparison(new).unwrap();
            assert!(w.comparison().is_none(), "{old}");
        }
    }

    #[test]
    fn exact_offsets_accommodate_combining_characters_wide_stops_and_symbol_only_drafts() {
        let original = "  鳥。 \r\nThe cafe\u{301} closed.\nThey were still singing. ";
        let w = Workshop::new(original, Focus::Clarity).unwrap();
        let f = w.feedback();
        assert_eq!(&original[f.span], "They were still singing.");
        for draft in [
            "🌙",
            "。",
            "e\u{301}",
            "1.5, then 2.5.",
            "  （雪）　\n",
            "...?!",
        ] {
            for focus in [Focus::Clarity, Focus::Rhythm, Focus::Voice, Focus::NextStep] {
                let f = Workshop::new(draft, focus).unwrap().feedback();
                assert!(draft
                    .get(f.span)
                    .is_some_and(|quoted| !quoted.trim().is_empty()));
                assert!(!f.experiment.is_empty());
            }
        }
        let first = passages("Use 1.5 units. Then 2.5.").remove(0);
        assert_eq!(first.text, "Use 1.5 units.");
    }
}
