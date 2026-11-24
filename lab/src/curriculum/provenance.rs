//! Decide whether a released claim still has enough independently sourced support for one use.
//! Inputs are editorially verified spans, lineage assignments and current grants.
use super::{Document, OutcomeId, Span};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim {
    pub course: u64,
    pub outcome: OutcomeId,
    pub criterion: u64,
    /// Immutable edition of the course reference under which this assertion applies.
    pub edition: Document,
    pub convention: u64,
}

pub struct Release {
    pub revision: u64,
    pub claim: Claim,
    pub candidates: BTreeSet<Span>,
    /// Exact spans whose wording or key this form uses. Each must be an approved candidate.
    pub required: BTreeSet<Span>,
    pub minimum_lineages: u8,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Purpose {
    Instruction,
    Assessment,
}

#[derive(Clone, Copy, Debug)]
pub struct Use {
    pub revision: u64,
    pub claim: Claim,
    pub audience: u64,
    pub purpose: Purpose,
    pub day: u32,
}

pub enum Lineage {
    /// Identity of the reviewed independent origin. Several documents may have the same origin.
    Original(u64),
    /// Every parent contributed to this claim. Ordinary bibliographic links are excluded.
    Derived(BTreeSet<Span>),
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub enum Standing {
    Current,
    HeldFrom(u32),
    WithdrawnFrom(u32),
}

pub struct Support {
    pub claim: Claim,
    pub lineage: Lineage,
    /// Authorized [first day, exclusive end day) by audience and instructional use.
    pub grants: BTreeMap<(u64, Purpose), (u32, u32)>,
    /// A trusted editorial hold or verified withdrawal for this exact span and edition.
    pub standing: Standing,
}

pub struct Catalog {
    /// Changes on rights, lineage or editorial correction. The use adapter fences this at commit.
    pub generation: u64,
    pub spans: BTreeMap<Span, Support>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Issue {
    WrongScope,
    InvalidPolicy,
    Inapplicable,
    NoGrant,
    Withdrawn,
    UnderReview,
    Missing,
    UnknownOrigin,
    Cycle,
    Budget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Witness {
    pub citation: Span,
    pub dependencies: BTreeSet<Span>,
    pub origins: BTreeSet<u64>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Decision {
    Approved(Vec<Span>),
    /// Known eligible witnesses do not meet the independent-origin requirement.
    Insufficient,
    /// The current facts or bounded search do not settle the requirement.
    Withheld(Issue),
}

pub struct Receipt {
    pub request: Use,
    pub generation: u64,
    pub decision: Decision,
    pub traced: Vec<Witness>,
    pub excluded: BTreeMap<Span, Issue>,
    pub steps: usize,
}

struct Work {
    left: usize,
    steps: usize,
}

impl Work {
    fn charge(&mut self) -> Result<(), Issue> {
        if self.left == 0 {
            return Err(Issue::Budget);
        }
        self.left -= 1;
        self.steps += 1;
        Ok(())
    }
}

impl Catalog {
    fn trace(
        &self,
        span: Span,
        request: Use,
        path: &mut BTreeSet<Span>,
        witness: &mut Witness,
        work: &mut Work,
    ) -> Result<(), Issue> {
        work.charge()?;
        if path.contains(&span) {
            return Err(Issue::Cycle);
        }
        if path.len() >= 32 {
            return Err(Issue::Budget);
        }
        if witness.dependencies.contains(&span) {
            return Ok(());
        }
        if span.start >= span.end {
            return Err(Issue::Missing);
        }
        let node = self.spans.get(&span).ok_or(Issue::Missing)?;
        if node.claim != request.claim {
            return Err(Issue::Inapplicable);
        }
        match node.standing {
            Standing::WithdrawnFrom(day) if request.day >= day => return Err(Issue::Withdrawn),
            Standing::HeldFrom(day) if request.day >= day => return Err(Issue::UnderReview),
            _ => {}
        }
        let permitted = node
            .grants
            .get(&(request.audience, request.purpose))
            .is_some_and(|(start, end)| *start <= request.day && request.day < *end);
        if !permitted {
            return Err(Issue::NoGrant);
        }
        path.insert(span);
        match &node.lineage {
            Lineage::Original(id) => {
                witness.origins.insert(*id);
            }
            Lineage::Derived(parents) if !parents.is_empty() => {
                for &parent in parents {
                    self.trace(parent, request, path, witness, work)?;
                }
            }
            _ => return Err(Issue::UnknownOrigin),
        }
        path.remove(&span);
        witness.dependencies.insert(span);
        Ok(())
    }

    pub fn inspect(&self, release: &Release, request: Use, max_steps: usize) -> Receipt {
        let mut work = Work {
            left: max_steps.min(4096),
            steps: 0,
        };
        let mut receipt = Receipt {
            request,
            generation: self.generation,
            decision: Decision::Insufficient,
            traced: Vec::new(),
            excluded: BTreeMap::new(),
            steps: 0,
        };
        receipt.decision = self
            .decide(release, &mut receipt, &mut work)
            .unwrap_or_else(Decision::Withheld);
        receipt.steps = work.steps;
        receipt
    }

    fn decide(
        &self,
        release: &Release,
        receipt: &mut Receipt,
        work: &mut Work,
    ) -> Result<Decision, Issue> {
        let request = receipt.request;
        if request.revision != release.revision || request.claim != release.claim {
            return Err(Issue::WrongScope);
        }
        if release.minimum_lineages == 0
            || release.required.len() > 16
            || !release.required.is_subset(&release.candidates)
        {
            return Err(Issue::InvalidPolicy);
        }
        if release.candidates.len() > 16 {
            return Err(Issue::Budget);
        }
        let mut unknown = None;
        for &citation in &release.candidates {
            let mut w = Witness {
                citation,
                dependencies: BTreeSet::new(),
                origins: BTreeSet::new(),
            };
            match self.trace(citation, request, &mut BTreeSet::new(), &mut w, work) {
                Ok(()) => receipt.traced.push(w),
                Err(reason) => {
                    receipt.excluded.insert(citation, reason);
                    if reason == Issue::Budget || release.required.contains(&citation) {
                        return Err(reason);
                    }
                    if matches!(
                        reason,
                        Issue::Missing | Issue::UnknownOrigin | Issue::Cycle | Issue::UnderReview
                    ) {
                        unknown.get_or_insert(reason);
                    }
                }
            }
        }
        let mut chosen = vec![];
        if independent(
            &receipt.traced,
            usize::from(release.minimum_lineages),
            &BTreeSet::new(),
            &mut chosen,
            work,
        )? {
            return Ok(Decision::Approved(chosen));
        }
        Ok(unknown.map_or(Decision::Insufficient, Decision::Withheld))
    }
}

/// Search combinations because greedily choosing a mixed-origin witness can mask two valid separate witnesses.
fn independent(
    candidates: &[Witness],
    needed: usize,
    used: &BTreeSet<u64>,
    chosen: &mut Vec<Span>,
    work: &mut Work,
) -> Result<bool, Issue> {
    work.charge()?;
    if needed == 0 {
        return Ok(true);
    }
    if candidates.len() < needed {
        return Ok(false);
    }
    let (candidate, tail) = candidates.split_first().expect("needed is positive");
    if candidate.origins.is_disjoint(used) {
        let next = used.union(&candidate.origins).copied().collect();
        chosen.push(candidate.citation);
        if independent(tail, needed - 1, &next, chosen, work)? {
            return Ok(true);
        }
        chosen.pop();
    }
    independent(tail, needed, used, chosen, work)
}
