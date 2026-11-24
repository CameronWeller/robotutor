//! Preflight model selection before compiling a packet with its exact tokenizer.
//! The caller supplies authenticated, tenant-scoped qualifications and live grants.
use super::{Intent, Model, Role};
use crate::curriculum::OutcomeId;
use std::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract<'a> {
    pub release: u64,
    pub outcome: OutcomeId,
    pub intent: Intent,
    /// Canonical tags from the release. No language or accessibility fallback is implicit.
    pub locale: &'a str,
    pub modality: &'a str,
    /// Versioned output contract, e.g. "fraction-parameters/v1".
    pub schema: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct Qualification<'a> {
    pub contract: Contract<'a>,
    pub role: Role,
    /// Immutable model, tokenizer and price revision.
    pub profile: u64,
    pub benchmark_epoch: u64,
    pub grant_epoch: u64,
    /// Effective interval for using this reviewed qualification, in milliseconds.
    pub from_ms: u64,
    pub until_ms: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub now_ms: u64,
    pub deadline_ms: u64,
    /// Lesser of the turn ceiling and remaining shared funds.
    pub available_micros: u64,
    pub benchmark_epoch: u64,
    pub grant_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    LocalOnly,
    Unqualified,
    Unavailable,
    Budget,
    Deadline,
    /// Every available option failed at least one bound, and both bounds were observed.
    BudgetAndDeadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub role: Role,
    pub model: Model,
    /// Pin this exclusive deadline in the admitted request, through send and publication.
    pub deadline_ms: u64,
}

/// `deployable` checks current processing permission and reads live lane capacity.
/// This read reserves nothing. Recheck authority, token counts and funds at admission.
pub fn select(
    contract: Contract<'_>,
    window: Window,
    qualifications: &[Qualification<'_>],
    deployable: impl Fn(Role, u64) -> Option<Model>,
) -> Result<Selection, Miss> {
    let role = contract.intent.role().ok_or(Miss::LocalOnly)?;
    if [contract.locale, contract.modality, contract.schema]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(Miss::Unqualified);
    }
    if window.now_ms >= window.deadline_ms {
        return Err(Miss::Deadline);
    }
    let mut matched = false;
    let mut over_budget = false;
    let mut too_slow = false;
    let mut best = None;
    let rank = |s: Selection| {
        (
            s.model.maximum_micros,
            s.model.admission_ms,
            s.model.revision,
            Reverse(s.deadline_ms),
        )
    };
    for q in qualifications {
        if q.contract != contract
            || q.role != role
            || q.benchmark_epoch != window.benchmark_epoch
            || q.grant_epoch != window.grant_epoch
            || !(q.from_ms..q.until_ms).contains(&window.now_ms)
        {
            continue;
        }
        matched = true;
        let Some(model) = deployable(role, q.profile) else {
            continue;
        };
        if model.revision != q.profile
            || model.slots == 0
            || model.max_input_tokens == 0
            || model.max_output_tokens == 0
        {
            continue;
        }
        let costly = model.maximum_micros > window.available_micros;
        let slow = model.admission_ms > window.deadline_ms - window.now_ms
            || model.admission_ms >= q.until_ms - window.now_ms;
        over_budget |= costly;
        too_slow |= slow;
        let choice = Selection {
            role,
            model,
            deadline_ms: window.deadline_ms.min(q.until_ms),
        };
        if !costly && !slow && best.is_none_or(|prior| rank(choice) < rank(prior)) {
            best = Some(choice);
        }
    }
    best.ok_or(match (over_budget, too_slow) {
        (true, true) => Miss::BudgetAndDeadline,
        (true, false) => Miss::Budget,
        (false, true) => Miss::Deadline,
        (false, false) if matched => Miss::Unavailable,
        (false, false) => Miss::Unqualified,
    })
}
