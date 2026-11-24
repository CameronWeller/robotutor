//! Read-only day-resolution model of OLMS-V2 power-retrievability/1 and
//! durable-spaced-retrieval/1. All times and evidence belong to a frozen read.
use crate::curriculum::OutcomeId;
use crate::history::{eligible, Attempt, Scope, Withdrawn};
use crate::protocol::Judgment;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell {
    pub scope: Scope,
    pub outcome: OutcomeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Moment {
    pub day: u32,
    pub sequence: u64,
}

/// Adapter-reconciled snapshot of the last eligible performance at this read,
/// including any corrections or source withdrawals that affect its supports.
#[derive(Clone, Copy, Debug)]
pub struct Prior {
    pub cell: Cell,
    pub verified: Moment,
    pub stability_days: f64,
    pub threshold: f64,
    pub had_durable_success: bool,
}

/// A delivered encounter, including guided or otherwise unscored work.
#[derive(Clone, Copy, Debug)]
pub struct Exposure {
    pub cell: Cell,
    pub at: Moment,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Forecast {
    pub recall: f64,
    pub refresh_due: bool,
    pub days_since_exposure: Option<u32>,
    pub required_gap_days: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckResult {
    NoCheck,
    Ineligible,
    InsufficientSpacing,
    UnknownExposure,
    ReviewFailure,
    SupportsDurability,
    OpposesDurability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    WrongCell,
    InvalidSnapshot,
    InvalidChronology,
}

/// `Some(exposures)` attests complete coverage since the prior performance, including an empty set.
/// `None` means unknown coverage. This slice requires a prior eligible performance for positive
/// durability, where the full reference fold can start its clock from a guided encounter.
/// `check`, when supplied, is the submitted performance exactly at `as_of`.
/// A target is a calendar day in the same frozen plan. Only its remaining horizon enters the gap.
pub fn read(
    cell: Cell,
    as_of: Moment,
    target_day: Option<u32>,
    prior: Option<Prior>,
    exposures: Option<&[Exposure]>,
    check: Option<&Attempt>,
    withdrawn: &Withdrawn,
) -> Result<(Option<Forecast>, CheckResult), ReadError> {
    if withdrawn.scope != cell.scope
        || check.is_some_and(|a| a.scope != cell.scope || a.outcome != cell.outcome)
        || prior.is_some_and(|p| p.cell != cell)
    {
        return Err(ReadError::WrongCell);
    }
    if check.is_some_and(|a| (a.day, a.sequence) != (as_of.day, as_of.sequence)) {
        return Err(ReadError::InvalidChronology);
    }
    let mut last = None;
    for exposure in exposures.unwrap_or_default() {
        if exposure.cell != cell {
            return Err(ReadError::WrongCell);
        }
        if exposure.at > as_of {
            return Err(ReadError::InvalidChronology);
        }
        if check.is_none() || exposure.at < as_of {
            last = Some(last.map_or(exposure.at, |stamp: Moment| stamp.max(exposure.at)));
        }
    }
    let forecast = if let Some(p) = prior {
        if p.verified > as_of || (check.is_some() && p.verified == as_of) {
            return Err(ReadError::InvalidChronology);
        }
        if !(1.0..=3650.0).contains(&p.stability_days) || ![0.8, 0.85, 0.9].contains(&p.threshold) {
            return Err(ReadError::InvalidSnapshot);
        }
        let elapsed = f64::from(as_of.day - p.verified.day);
        let memory_gap = 9.0 * p.stability_days * (1.0 / p.threshold - 1.0);
        let horizon = f64::from(target_day.unwrap_or(as_of.day).saturating_sub(as_of.day));
        Some(Forecast {
            recall: 1.0 / (1.0 + elapsed / (9.0 * p.stability_days)),
            refresh_due: elapsed >= memory_gap,
            days_since_exposure: exposures
                .map(|_| as_of.day - last.unwrap_or(p.verified).max(p.verified).day),
            required_gap_days: memory_gap.max(1.11 * horizon.powf(0.52)),
        })
    } else {
        None
    };
    let clears_gap = forecast.is_some_and(|f| {
        f.days_since_exposure
            .is_some_and(|gap| f64::from(gap) >= f.required_gap_days)
    });
    let result = match check {
        None => CheckResult::NoCheck,
        Some(a) if !eligible(a, as_of.day, withdrawn) => CheckResult::Ineligible,
        Some(a) => match a.judgment {
            Judgment::Correct if clears_gap => CheckResult::SupportsDurability,
            Judgment::Correct if exposures.is_none() && prior.is_some() => {
                CheckResult::UnknownExposure
            }
            Judgment::Correct => CheckResult::InsufficientSpacing,
            Judgment::Incorrect if clears_gap || prior.is_some_and(|p| p.had_durable_success) => {
                CheckResult::OpposesDurability
            }
            Judgment::Incorrect => CheckResult::ReviewFailure,
            Judgment::Ambiguous | Judgment::Missing => CheckResult::Ineligible,
        },
    };
    Ok((forecast, result))
}
