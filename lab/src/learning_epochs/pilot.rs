use super::{
    Learner, Noise, Participant, CHECK_ATTENDANCE, CHECK_HELP, PRACTICE_ATTENDANCE, PRACTICE_HELP,
};

/// Generalize the epochs model's recall and unit-application draws to a case's
/// frozen domain responses. No prompt version enters the random key.
pub struct Trajectory {
    participant: Participant,
    learner: Learner,
}

pub struct Observation {
    pub attended: bool,
    pub help: bool,
    pub response: &'static str,
    pub recall_probability: f64,
}

impl Trajectory {
    pub fn new(seed: u64, case: u32) -> Self {
        let participant = Participant { seed, id: case };
        Self {
            learner: Learner::new(&participant, true),
            participant,
        }
    }

    pub fn observe(&self, day: u32, slot: u8, delayed: bool) -> Observation {
        let draw = |channel| self.participant.draw(day, slot, channel);
        let recall_probability = self.learner.memory.recall(day);
        let attended = day == 0
            || draw(Noise::Attendance)
                < if delayed {
                    CHECK_ATTENDANCE
                } else {
                    PRACTICE_ATTENDANCE
                };
        let help = draw(Noise::Help) < if delayed { CHECK_HELP } else { PRACTICE_HELP };
        let response = if help {
            "success"
        } else if draw(Noise::Retrieve) >= recall_probability {
            "recall_error"
        } else if delayed && draw(Noise::Convert) >= self.learner.memory.conversion {
            "application_error"
        } else {
            "success"
        };
        Observation {
            attended,
            help,
            response,
            recall_probability,
        }
    }

    pub fn encounter(&mut self, day: u32, independent_success: bool) {
        self.learner.memory.encounter(day, independent_success);
    }
}
