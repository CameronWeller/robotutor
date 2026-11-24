//! Reopen a delayed finding against potentially visible dispatches, including
//! unanswered checks. Event time orders learning opportunities; append order
//! determines what the planner knew on a given read.
use super::{delayed_on_reviewed_change, Evidence, PublishedCase, Spacing};
use crate::curriculum::Document;
use crate::history::Withdrawn;
use crate::retention::{self, Cell, CheckResult, Exposure, Moment, Prior, ReadError};
use std::collections::{BTreeMap, BTreeSet};

/// Committed before any question bytes can leave the broker. Acknowledgement
/// is unnecessary. Each redisplay that could expose content has its own moment.
/// A replay of the *receipt* uses the original moment and payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dispatch {
    pub cell: Cell,
    pub at: Moment,
    pub case: PublishedCase,
    /// None for instruction, class projection, or another unassessed contact.
    pub attempt: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactError {
    WrongCell,
    ConflictingEvent,
}

/// Two editorial decisions at this read. Equivalence is symmetric. Approval
/// for a changed application is directional and scoped to the exact editions.
pub struct TaskCatalog<'a> {
    pub aliases: &'a BTreeMap<PublishedCase, Document>,
    pub approved: &'a BTreeSet<(PublishedCase, PublishedCase)>,
}

/// One authorized learner, criterion and unit release. The host projects class
/// and private dispatches into the same ordered stream. Pre-release history
/// needs a reviewed migration and complete coverage before this read is valid.
pub struct ContactHistory {
    cell: Cell,
    records: BTreeMap<Moment, Dispatch>,
    conflict: bool,
}

impl ContactHistory {
    pub fn new(cell: Cell) -> Self {
        Self {
            cell,
            records: BTreeMap::new(),
            conflict: false,
        }
    }

    pub fn record(&mut self, event: Dispatch) -> Result<(), ContactError> {
        if event.cell != self.cell
            || event.case.outcome != self.cell.outcome
            || event.case.unit_release != self.cell.scope.revision
        {
            return Err(ContactError::WrongCell);
        }
        if let Some(old) = self.records.get(&event.at) {
            if *old != event {
                // Rebuild from reconciled facts before issuing another finding.
                self.conflict = true;
                return Err(ContactError::ConflictingEvent);
            }
        } else {
            self.records.insert(event.at, event);
        }
        Ok(())
    }

    fn independent_opening(
        &self,
        evidence: &Evidence,
        aliases: &BTreeMap<PublishedCase, Document>,
    ) -> Result<Moment, CheckResult> {
        let case = evidence.published_case.ok_or(CheckResult::Ineligible)?;
        let identity = aliases.get(&case).ok_or(CheckResult::UnknownExposure)?;
        let answered = performance_time(evidence);
        let mut opening = None;
        for (&at, event) in self.records.range(..=answered) {
            let encountered = aliases
                .get(&event.case)
                .ok_or(CheckResult::UnknownExposure)?;
            // Any further relevant teaching during this opening contaminates
            // independence. Contacts in this stream pertain to this criterion.
            if opening.is_some() {
                return Err(CheckResult::Ineligible);
            }
            if event.attempt == Some(evidence.attempt.id) && event.case == case {
                opening = Some(at);
            } else if encountered == identity || event.attempt == Some(evidence.attempt.id) {
                return Err(CheckResult::Ineligible);
            }
        }
        opening.ok_or(CheckResult::UnknownExposure)
    }

    /// `complete_through` is a host-attested coverage watermark from the
    /// beginning of this learner's relevant history. A maximum event timestamp
    /// alone cannot attest completeness. The alias map binds reviewed editions
    /// to semantic tasks. Unknown equivalence leaves independence unresolved.
    pub fn delayed(
        &self,
        first: &Evidence,
        later: &Evidence,
        complete_through: Option<Moment>,
        catalog: TaskCatalog<'_>,
        withdrawn: &Withdrawn,
        spacing: Spacing,
    ) -> Result<CheckResult, ReadError> {
        if withdrawn.scope != self.cell.scope
            || [first, later].iter().any(|e| {
                e.attempt.scope != self.cell.scope || e.attempt.outcome != self.cell.outcome
            })
        {
            return Err(ReadError::WrongCell);
        }
        if self.conflict || !complete_through.is_some_and(|end| end >= performance_time(later)) {
            return Ok(CheckResult::UnknownExposure);
        }
        if let Err(reason) = self.independent_opening(first, catalog.aliases) {
            return Ok(reason);
        }
        let opened = match self.independent_opening(later, catalog.aliases) {
            Ok(at) => at,
            Err(reason) => return Ok(reason),
        };
        let verified = performance_time(first);
        if opened <= verified {
            return Ok(CheckResult::Ineligible);
        }
        // The retention kernel uses the most recent contact in this cell.
        // Its coverage guarantee comes from the watermark checked above.
        let last_contact = self
            .records
            .range(..opened)
            .next_back()
            .map(|(&at, _)| Exposure {
                cell: self.cell,
                at,
            });
        let exposures = last_contact.as_slice();
        // The learner can start solving the moment the question could arrive.
        // Parking a browser tab until a later day cannot supply extra spacing.
        let prior = Prior {
            cell: self.cell,
            verified,
            stability_days: spacing.stability_days,
            threshold: spacing.threshold,
            had_durable_success: false,
        };
        let (at_open, _) = retention::read(
            self.cell,
            opened,
            spacing.target_day,
            Some(prior),
            Some(exposures),
            None,
            withdrawn,
        )?;
        let cold_at_open = at_open.is_some_and(|f| {
            f.days_since_exposure
                .is_some_and(|days| f64::from(days) >= f.required_gap_days)
        });
        let finding = delayed_on_reviewed_change(
            first,
            later,
            Some(exposures),
            withdrawn,
            spacing,
            catalog.approved,
        )?;
        Ok(match finding {
            CheckResult::SupportsDurability if !cold_at_open => CheckResult::InsufficientSpacing,
            CheckResult::OpposesDurability if !cold_at_open => CheckResult::ReviewFailure,
            other => other,
        })
    }
}

fn performance_time(evidence: &Evidence) -> Moment {
    Moment {
        day: evidence.attempt.day,
        sequence: evidence.attempt.sequence,
    }
}
