//! Preserve the assessment conditions of one displayed course question across a workshop.
//! The host owns its pinned exercise ID, scope and release. It revalidates all three
//! before accepting a return or redisplaying the exercise. This object records only whether the detour is active and whether
//! the original opening has lost its eligibility for independent credit.

#[derive(Clone, Debug)]
pub struct CourseTurn {
    parked: bool,
    practice_only: bool,
}

impl CourseTurn {
    /// Begin tracking one verified, fresh exercise that the learner has already seen.
    pub fn opened() -> Self {
        Self {
            parked: false,
            practice_only: false,
        }
    }

    /// Enter the workshop. The request alone supplies no answer, coaching or evidence.
    pub fn park(&mut self) -> Result<(), &'static str> {
        if self.parked {
            return Err("the course exercise is already parked");
        }
        self.parked = true;
        Ok(())
    }

    /// Record delivered workshop coaching whose overlap with the exercise is possible.
    pub fn coached(&mut self) {
        if self.parked {
            self.practice_only = true;
        }
    }

    /// Return to the same visible opening. No new prompt or coaching is delivered by this action.
    pub fn resume(&mut self) -> Result<(), &'static str> {
        if !self.parked {
            return Err("there is no parked course exercise to resume");
        }
        self.parked = false;
        Ok(())
    }

    /// Record another delivery of this same exercise, including a restored display on a new device.
    pub fn redisplayed(&mut self) {
        self.practice_only = true;
    }

    pub fn is_parked(&self) -> bool {
        self.parked
    }

    /// A false value on a parked turn is not authorization to accept an answer.
    pub fn practice_only(&self) -> bool {
        self.practice_only
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telo::monitor::Monitor;
    use telo::predicate::{ClosurePredicate, Predicates};
    use telo::Property;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Action {
        Park,
        Coach,
        Resume,
        Redisplay,
    }

    #[test]
    fn cancelling_keeps_a_still_visible_exercise_eligible_until_it_is_redisplayed() {
        let mut turn = CourseTurn::opened();
        turn.park().unwrap();
        assert!(!turn.practice_only());
        turn.resume().unwrap();
        assert!(!turn.is_parked());
        assert!(!turn.practice_only());
        turn.redisplayed();
        assert!(turn.practice_only());
    }

    #[test]
    fn all_detour_histories_preserve_the_exposure_boundary() {
        // One observation per command. G(agrees) checks every intermediate prefix.
        // The oracle derives ownership and first exposure from the event history.
        let predicates = || {
            let mut builder = Predicates::builder();
            let agreement =
                builder.new_predicate(ClosurePredicate::new(|value: &bool| *value, "agrees"));
            (
                builder.build(),
                Property::always(Property::atomic(agreement)),
            )
        };
        let (set, property) = predicates();
        let machine = property.to_monitoring_automaton(&set);
        let actions = [
            Action::Park,
            Action::Coach,
            Action::Resume,
            Action::Redisplay,
        ];
        let parked = |prefix: &[Action]| {
            prefix
                .iter()
                .rev()
                .find(|&&a| matches!(a, Action::Park | Action::Resume))
                == Some(&Action::Park)
        };
        for mut word in 0..actions.len().pow(4) {
            let mut turn = CourseTurn::opened();
            let mut monitor = Monitor::new(predicates().0, machine.clone());
            let mut history = Vec::new();
            for _ in 0..4 {
                let action = actions[word % actions.len()];
                word /= actions.len();
                let parked_before = parked(&history);
                let accepted = match action {
                    Action::Park => turn.park().is_ok(),
                    Action::Coach => {
                        turn.coached();
                        true
                    }
                    Action::Resume => turn.resume().is_ok(),
                    Action::Redisplay => {
                        turn.redisplayed();
                        true
                    }
                };
                let expected_acceptance = match action {
                    Action::Park => !parked_before,
                    Action::Coach | Action::Redisplay => true,
                    Action::Resume => parked_before,
                };
                history.push(action);
                let parked_after = parked(&history);
                let exposed = history.iter().enumerate().any(|(i, &a)| {
                    a == Action::Redisplay || (a == Action::Coach && parked(&history[..i]))
                });
                let agrees = accepted == expected_acceptance
                    && turn.is_parked() == parked_after
                    && turn.practice_only() == exposed;
                assert!(monitor.next_state(&agrees), "detour history {history:?}");
            }
        }
    }
}
