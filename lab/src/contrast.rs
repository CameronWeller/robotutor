//! Editorial selection of a follow-up that can change what is taught.
//! The caller supplies current, displayable items for one goal and one ordered
//! set of explanations still compatible with the learner's first response.
//! A prediction is about an observable answer, never an assessment of a person.
use std::collections::{BTreeMap, BTreeSet};

/// One prediction per live explanation. Several explanations may lead to the
/// same teaching move or predict the same answer. A missing prediction means
/// that explanation could produce any answer on this item.
pub struct Candidate<Item, Answer> {
    pub item: Item,
    pub seconds: u32,
    pub predicts: Vec<Option<Answer>>,
}

/// A selected contrast borrows its evidence table. The original response,
/// prediction alignment and curriculum edition belong to the caller.
pub struct Contrast<'a, Item, Answer, Lesson> {
    pub candidate: &'a Candidate<Item, Answer>,
    lessons: &'a [Lesson],
    pub worst_remaining_lessons: usize,
}

impl<Item, Answer: Eq, Lesson: Ord> Contrast<'_, Item, Answer, Lesson> {
    /// Unique compatible instruction may be offered. An unexpected response or
    /// a remaining ambiguity returns None for a direct explanation or choice.
    pub fn suggested_lesson(&self, answer: &Answer) -> Option<&Lesson> {
        if !self
            .candidate
            .predicts
            .iter()
            .any(|value| value.as_ref() == Some(answer))
        {
            return None;
        }
        let mut matches = self
            .candidate
            .predicts
            .iter()
            .zip(self.lessons)
            .filter(|(p, _)| p.as_ref().is_none_or(|v| v == answer))
            .map(|(_, lesson)| lesson);
        let first = matches.next()?;
        matches.all(|lesson| lesson == first).then_some(first)
    }
}

/// Across plausible answers, minimize the largest number of remaining teaching
/// moves, then prefer the quicker item. Input order breaks exact ties. The
/// caller budgets question time *after* reserving time for useful feedback.
/// Equal weight here is a design choice about ambiguity, not a learner prior.
pub fn select<'a, Item, Answer: Ord, Lesson: Ord>(
    lessons: &'a [Lesson],
    candidates: &'a [Candidate<Item, Answer>],
    available_seconds: u32,
) -> Result<Option<Contrast<'a, Item, Answer, Lesson>>, &'static str> {
    let initial = lessons.iter().collect::<BTreeSet<_>>().len();
    let mut best = None;
    let mut best_rank = (initial, 0);
    for candidate in candidates {
        if candidate.predicts.len() != lessons.len() || candidate.seconds == 0 {
            return Err("a timed candidate needs one aligned prediction per explanation");
        }
        if candidate.seconds > available_seconds {
            continue;
        }
        let mut outcomes = BTreeMap::<&Answer, BTreeSet<&Lesson>>::new();
        let mut unknown = BTreeSet::new();
        for (answer, lesson) in candidate.predicts.iter().zip(lessons) {
            match answer {
                Some(value) => {
                    outcomes.entry(value).or_default().insert(lesson);
                }
                None => {
                    unknown.insert(lesson);
                }
            }
        }
        let worst = outcomes
            .values()
            .map(|bucket| bucket.union(&unknown).count())
            .max()
            .unwrap_or(unknown.len());
        let rank = (worst, candidate.seconds);
        if rank < best_rank {
            best_rank = rank;
            best = Some(Contrast {
                candidate,
                lessons,
                worst_remaining_lessons: worst,
            });
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_hypotheses_with_the_same_teaching_move_do_not_justify_a_probe() {
        let candidates = [Candidate {
            item: (),
            seconds: 5,
            predicts: vec![Some(1), Some(2)],
        }];
        assert!(select(&["count", "count"], &candidates, 5)
            .unwrap()
            .is_none());
        assert!(select(&["count"], &candidates, 5).is_err());
        assert!(select(&["count", "draw"], &candidates, 4)
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_unknown_prediction_survives_every_bucket() {
        let candidates = [Candidate {
            item: (),
            seconds: 5,
            predicts: vec![Some(1), Some(2), None],
        }];
        let policy = select(&["count", "draw", "locate"], &candidates, 5)
            .unwrap()
            .unwrap();
        assert_eq!(policy.worst_remaining_lessons, 2);
        assert_eq!(policy.suggested_lesson(&1), None);
        assert_eq!(policy.suggested_lesson(&2), None);
        assert_eq!(policy.suggested_lesson(&99), None);
    }
}
