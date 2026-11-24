//! Select among approved forms for one authorized learner and release. The catalog
//! supplies semantic identities across visual, translated, and editorial variants.
pub mod coverage;
use crate::curriculum::{OutcomeId, Span};
use crate::history::Scope;
use crate::material::{FractionCard, RuleSource};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Enrollment {
    pub scope: Scope,
    pub class: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Form {
    pub class: u64,
    pub release: u64,
    pub outcome: OutcomeId,
    /// Curated unit material is shared. Material drafted with a learner packet
    /// stays private even when the resulting card happens to be reusable.
    pub owner: Option<u64>,
    pub revision: u64,
    pub task: u64,
    pub family: u64,
    pub source: Span,
    pub card: FractionCard,
}

/// Received or possibly received. A projected class example is attributed using
/// the audience at delivery time. Both conditions preclude certifying freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contact {
    Delivered,
    Possible,
}

/// The loader resolves any class deliveries, approved translations and task
/// aliases. A coverage gap (for example an incomplete migration) stays explicit.
#[derive(Debug)]
pub struct ExposureBook {
    seat: Enrollment,
    complete: bool,
    delivered: BTreeSet<u64>,
    possible: BTreeSet<u64>,
    families: BTreeSet<u64>,
}

/// Holds exclusive access to one learner's exposure frontier until the opening succeeds or is dropped.
pub(crate) struct OpeningPermit<'a> {
    book: &'a mut ExposureBook,
    form: Form,
}

impl OpeningPermit<'_> {
    pub(crate) fn commit(self) {
        self.book.note(&self.form, Contact::Possible);
    }
}

impl ExposureBook {
    pub fn new(seat: Enrollment, complete: bool) -> Self {
        Self {
            seat,
            complete,
            delivered: BTreeSet::new(),
            possible: BTreeSet::new(),
            families: BTreeSet::new(),
        }
    }

    pub fn record(
        &mut self,
        seat: Enrollment,
        form: &Form,
        contact: Contact,
    ) -> Result<(), Unavailable> {
        if seat != self.seat
            || form.class != seat.class
            || form.release != seat.scope.revision
            || form.owner.is_some_and(|owner| owner != seat.scope.learner)
        {
            return Err(Unavailable::Scope);
        }
        self.note(form, contact);
        Ok(())
    }

    fn note(&mut self, form: &Form, contact: Contact) {
        match contact {
            Contact::Delivered => {
                self.possible.remove(&form.task);
                self.delivered.insert(form.task);
            }
            Contact::Possible if !self.delivered.contains(&form.task) => {
                self.possible.insert(form.task);
            }
            Contact::Possible => {}
        }
        self.families.insert(form.family);
    }

    pub(crate) fn prepare_opening(
        &mut self,
        seat: Enrollment,
        target: OutcomeId,
        requirement: Requirement,
        form: &Form,
        source: &RuleSource<'_>,
    ) -> Result<OpeningPermit<'_>, Unavailable> {
        select(
            seat,
            target,
            requirement,
            self,
            std::slice::from_ref(form),
            source,
        )?;
        Ok(OpeningPermit {
            book: self,
            form: *form,
        })
    }

    pub fn exposure(&self, task: u64) -> Option<Contact> {
        if self.delivered.contains(&task) {
            Some(Contact::Delivered)
        } else if self.possible.contains(&task) {
            Some(Contact::Possible)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    Practice,
    FreshTask,
    FreshFamily,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    Scope,
    NoSupportedForm,
    NoFreshForm,
}

/// Approved curriculum crosswalk for a continuing learner in the same class.
/// Each entry lists *all* plausible successors, including an empty set for an
/// identity a reviewer established was retired. An absent entry is unknown.
/// The caller supplies the active crosswalk only after checking retention rights.
pub struct ExposureCrosswalk {
    pub class: u64,
    pub earlier_release: u64,
    pub later_release: u64,
    pub tasks: BTreeMap<u64, BTreeSet<u64>>,
    pub families: BTreeMap<u64, BTreeSet<u64>>,
}

impl ExposureBook {
    /// Apply during release migration, off the interactive question-selection path.
    /// Carry possible contact with a semantic task, without claiming the learner
    /// saw a particular rendering of the new release or importing assessment credit.
    pub fn carry_forward(
        &self,
        seat: Enrollment,
        crosswalk: &ExposureCrosswalk,
    ) -> Result<Self, Unavailable> {
        if self.seat.scope.learner != seat.scope.learner
            || self.seat.class != seat.class
            || crosswalk.class != seat.class
            || crosswalk.earlier_release != self.seat.scope.revision
            || crosswalk.later_release != seat.scope.revision
            || crosswalk.earlier_release == crosswalk.later_release
        {
            return Err(Unavailable::Scope);
        }
        let mut next = Self::new(seat, self.complete);
        for old in self.delivered.union(&self.possible) {
            match crosswalk.tasks.get(old) {
                Some(successors) => next.possible.extend(successors),
                None => next.complete = false,
            }
        }
        for family in &self.families {
            match crosswalk.families.get(family) {
                Some(successors) => next.families.extend(successors),
                None => next.complete = false,
            }
        }
        Ok(next)
    }
}

/// Use reviewed curriculum order. A missing suitable form is a reason to draft
/// or offer practice with disclosed help. It grants no new assessment authority.
pub fn select<'a>(
    seat: Enrollment,
    target: OutcomeId,
    requirement: Requirement,
    book: &ExposureBook,
    catalog: &'a [Form],
    source: &RuleSource<'_>,
) -> Result<&'a Form, Unavailable> {
    if book.seat != seat {
        return Err(Unavailable::Scope);
    }
    let mut failure = Unavailable::NoSupportedForm;
    for form in catalog {
        if form.class != seat.class
            || form.release != seat.scope.revision
            || form.owner.is_some_and(|owner| owner != seat.scope.learner)
            || form.outcome != target
            || form.source != source.pin
            || form.card.question(source).is_none()
        {
            continue;
        }
        let fresh_task = coverage::TaskContact::on_channel(book.exposure(form.task), book.complete)
            == coverage::TaskContact::NoRecordedContact;
        let usable = match requirement {
            Requirement::Practice => true,
            Requirement::FreshTask => fresh_task,
            Requirement::FreshFamily => fresh_task && !book.families.contains(&form.family),
        };
        if usable {
            return Ok(form);
        }
        failure = Unavailable::NoFreshForm;
    }
    Err(failure)
}
