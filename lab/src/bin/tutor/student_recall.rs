//! Per-workspace, per-objective forecast rebuilt from dated journal evidence.
use serde::Serialize;
use tutor_lab::retention::retrievability;

const STABILITY: [f64; 5] = [2., 7., 14., 28., 60.];
// Initial assumptions, before any delayed checks. The central value is the
// existing scheduler's 14-day stability. A miss leaves uncertain availability.
const WEIGHTS: [f64; 5] = [0.1, 0.2, 0.4, 0.2, 0.1];
const AFTER_MISS: f64 = 0.35;
const AFTER_SUCCESS: f64 = 0.95;

#[derive(Debug)]
pub struct StudentRecall {
    weights: [f64; 5],
    anchor: Option<(u32, bool, String)>,
    intervening_work: bool,
    delayed_checks: usize,
    squared_error: f64,
    fixed_squared_error: f64,
}

#[derive(Debug, Serialize)]
pub struct Estimate {
    pub probability: f64,
    pub elapsed_days: u32,
    pub stability_range_probability: [f64; 2],
    pub delayed_checks: usize,
    pub last_check_correct: bool,
    pub source: String,
}

impl Default for StudentRecall {
    fn default() -> Self {
        Self {
            weights: WEIGHTS,
            anchor: None,
            intervening_work: false,
            delayed_checks: 0,
            squared_error: 0.,
            fixed_squared_error: 0.,
        }
    }
}

impl StudentRecall {
    pub fn exposure(&mut self) {
        self.intervening_work = true;
    }

    pub fn check(&mut self, day: u32, correct: bool, source: &str) {
        if let Some((previous, succeeded, _)) = &self.anchor {
            if day < *previous {
                self.exposure();
                return;
            }
            if day > *previous && !self.intervening_work {
                let amplitude = if *succeeded {
                    AFTER_SUCCESS
                } else {
                    AFTER_MISS
                };
                // Score the forecast with the weights available before this answer.
                let gap = day - previous;
                let observed = f64::from(u8::from(correct));
                let fixed = amplitude * retrievability(gap, 14.);
                self.squared_error += (self.probability(gap, *succeeded) - observed).powi(2);
                self.fixed_squared_error += (fixed - observed).powi(2);
                for (weight, stability) in self.weights.iter_mut().zip(STABILITY) {
                    let p = amplitude * retrievability(day - previous, stability);
                    *weight *= if correct { p } else { 1. - p };
                }
                let total: f64 = self.weights.iter().sum();
                for weight in &mut self.weights {
                    *weight /= total;
                }
                self.delayed_checks += 1;
            }
        }
        self.anchor = Some((day, correct, source.to_owned()));
        self.intervening_work = false;
    }

    fn probability(&self, elapsed: u32, correct: bool) -> f64 {
        let amplitude = if correct { AFTER_SUCCESS } else { AFTER_MISS };
        amplitude
            * self
                .weights
                .iter()
                .zip(STABILITY)
                .map(|(weight, stability)| weight * retrievability(elapsed, stability))
                .sum::<f64>()
    }

    pub fn at(&self, day: u32) -> Option<Estimate> {
        let (previous, correct, source) = self.anchor.as_ref()?;
        if self.intervening_work {
            return None;
        }
        let elapsed = day.checked_sub(*previous)?;
        let amplitude = if *correct { AFTER_SUCCESS } else { AFTER_MISS };
        let quantile = |q: f64| {
            let mut cumulative = 0.;
            let mut stability = STABILITY[STABILITY.len() - 1];
            for (w, s) in self.weights.iter().zip(STABILITY) {
                cumulative += *w;
                if cumulative >= q {
                    stability = s;
                    break;
                }
            }
            amplitude * retrievability(elapsed, stability)
        };
        Some(Estimate {
            stability_range_probability: [quantile(0.1), quantile(0.9)],
            probability: self.probability(elapsed, *correct),
            elapsed_days: elapsed,
            delayed_checks: self.delayed_checks,
            last_check_correct: *correct,
            source: source.clone(),
        })
    }

    pub fn unavailable_reason(&self) -> &'static str {
        if self.anchor.is_none() {
            "No dated, verified, unaided check on a fresh task."
        } else if self.intervening_work {
            "Intervening work makes the last independent check insufficient for a current estimate."
        } else {
            "The check is later than the forecast date."
        }
    }

    pub fn brier(&self) -> Option<(usize, f64, f64)> {
        let n = self.delayed_checks;
        (n > 0).then(|| {
            (
                n,
                self.squared_error / n as f64,
                self.fixed_squared_error / n as f64,
            )
        })
    }

    // All grid curves cross 0.85 in the first 100 days after a success.
    pub fn review_interval(&self) -> Option<u32> {
        let (_, correct, _) = self.anchor.as_ref()?;
        if self.intervening_work {
            return None;
        }
        (1..=100).find(|d| self.probability(*d, *correct) <= 0.85)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_evidence_changes_the_curve_for_equal_elapsed_time() {
        let mut stronger = StudentRecall::default();
        let mut weaker = StudentRecall::default();
        for day in [0, 21, 42, 63] {
            stronger.check(day, true, "independent");
            weaker.check(day, day == 0, "independent");
            weaker.check(day, true, "fresh same-day success");
        }
        let a = stronger.at(84).unwrap();
        let b = weaker.at(84).unwrap();
        assert_eq!(b.delayed_checks, 3);
        assert!(a.probability > b.probability);
        assert!(stronger.review_interval() > weaker.review_interval());
        assert!(stronger.at(85).unwrap().probability < a.probability);
    }

    #[test]
    fn forecast_is_scored_before_updating_weights() {
        let mut s = StudentRecall::default();
        s.check(0, true, "start");
        assert!(s.brier().is_none());
        let forecast = s.at(28).unwrap();
        assert!(forecast.stability_range_probability[0] < forecast.probability);
        assert!(forecast.stability_range_probability[1] > forecast.probability);
        s.check(28, false, "delayed miss");
        let (count, learned, fixed) = s.brier().unwrap();
        assert_eq!(count, 1);
        assert!((learned - forecast.probability.powi(2)).abs() < 1e-12);
        assert!((fixed - (AFTER_SUCCESS * retrievability(28, 14.)).powi(2)).abs() < 1e-12);
    }

    #[test]
    fn interrupted_history_and_unknown_or_future_times() {
        let mut s = StudentRecall::default();
        assert!(s.at(1).is_none());
        s.check(8, true, "a");
        assert!(s.at(7).is_none());
        s.exposure();
        assert!(s.at(20).is_none());
        s.check(24, false, "b");
        let estimate = s.at(25).unwrap();
        assert_eq!(estimate.delayed_checks, 0);
        assert_eq!(estimate.source, "b");
        assert!(estimate.probability < AFTER_MISS);
        s.check(23, true, "out of order");
        assert!(s.at(25).is_none());
    }
}
