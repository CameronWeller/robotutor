//! A bounded authoring job over one release. The coordinator freezes one brief digest per
//! objective, including jurisdiction, audience, intended performance, source policy and edition.
//! The host stamps the actual author and supplies the reviewer's authenticated, scoped grant.
//! Rows start unpublished. Only independent approval changes that state to approved, and
//! only unpublished rows can be revised. A held row stays held. Availability also requires
//! every commissioned span to remain authorized at the time of the read.
use super::{Document, OutcomeId, Span};
use crate::material::document_id;
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

pub const MAX_ROWS: usize = 16;
pub(super) const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Work {
    pub release: u64,
    pub objective: OutcomeId,
    pub brief: Document,
    pub source_manifest: Document,
}

/// Digest of the complete required span set, supplied by the curriculum commissioner.
/// A source gained or dropped during drafting requires a newly commissioned work order.
pub fn source_manifest(sources: &BTreeSet<Span>) -> Document {
    let spans: Vec<_> = sources
        .iter()
        .map(|s| (s.document, s.start, s.end))
        .collect();
    let body = serde_json::json!(["curriculum-spans/1", spans]);
    document_id(body.to_string().as_bytes())
}

/// Private publication record. The learner view receives only the material for the current
/// teaching move. In particular the rubric is evaluator material, and a future check is sealed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pack {
    pub lesson: String,
    pub worked: String,
    pub practice: String,
    pub check: String,
    pub criterion: String,
    pub rubric: String,
    pub sources: BTreeSet<Span>,
}

impl Pack {
    pub fn digest(&self) -> Document {
        let body = serde_json::json!([
            "curriculum-pack/1",
            self.lesson,
            self.worked,
            self.practice,
            self.check,
            self.criterion,
            self.rubric,
            source_manifest(&self.sources)
        ]);
        document_id(body.to_string().as_bytes())
    }

    pub(super) fn validate(&self, work: Work) -> Result<(), Error> {
        for field in [
            &self.lesson,
            &self.worked,
            &self.practice,
            &self.check,
            &self.criterion,
            &self.rubric,
        ] {
            if field.len() > MAX_TEXT_BYTES || field.trim().is_empty() {
                return Err(Error::InvalidText);
            }
        }
        if self.sources.is_empty()
            || self.sources.len() > 16
            || self.sources.iter().any(|s| s.start >= s.end)
            || source_manifest(&self.sources) != work.source_manifest
        {
            return Err(Error::InvalidSources);
        }
        // Catch a verbatim check already displayed in instruction or practice. The reviewer
        // must also inspect semantic aliases, answer leakage and equivalent representations.
        let check = words(&self.check);
        if check.is_empty() {
            return Err(Error::InvalidText);
        }
        for earlier in [&self.lesson, &self.worked, &self.practice] {
            if words(earlier)
                .windows(check.len())
                .any(|phrase| phrase == check)
            {
                return Err(Error::ExposedCheck);
            }
        }
        Ok(())
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub work: Work,
    pub author: u64,
    pub pack: Pack,
}

/// Trusted authority for this particular work order. The issuer checks the person's subject,
/// jurisdiction and review scope. The provider supplies neither this grant nor its actor ID.
pub struct ReviewerGrant {
    pub actor: u64,
    pub work: Work,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    BatchSize,
    UnknownObjective,
    StaleWork,
    DuplicateRow,
    InvalidText,
    InvalidSources,
    ExposedCheck,
    Conflict,
    MissingDraft,
    WrongDigest,
    UnauthorizedReviewer,
    TooManyContributors,
    Unapproved,
    SourceUnavailable,
    Withdrawn,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Review {
    Draft,
    Approved(u64),
    Withdrawn(u64),
}

struct Row {
    contributors: BTreeSet<u64>,
    pack: Pack,
    digest: Document,
    review: Review,
}

impl Row {
    fn draft(candidate: Candidate, predecessor: Option<Document>) -> Self {
        let work = candidate.work;
        let receipt = serde_json::json!([
            "curriculum-proposal/1",
            work.release,
            work.objective.0,
            work.brief,
            work.source_manifest,
            candidate.author,
            candidate.pack.digest(),
            predecessor
        ]);
        Self {
            contributors: BTreeSet::from([candidate.author]),
            pack: candidate.pack,
            digest: document_id(receipt.to_string().as_bytes()),
            review: Review::Draft,
        }
    }
}

/// One sequential reference state machine. A durable adapter needs a unique publication key
/// per (release, objective) and atomic revision, source and approval comparisons at commit.
pub struct Batch {
    commissioned: BTreeMap<OutcomeId, Work>,
    rows: BTreeMap<OutcomeId, Row>,
}

impl Batch {
    pub fn new(release: u64, works: impl IntoIterator<Item = Work>) -> Result<Self, Error> {
        let mut commissioned = BTreeMap::new();
        for work in works {
            if commissioned.len() == MAX_ROWS {
                return Err(Error::BatchSize);
            }
            if work.release != release {
                return Err(Error::StaleWork);
            }
            if commissioned.insert(work.objective, work).is_some() {
                return Err(Error::DuplicateRow);
            }
        }
        if commissioned.is_empty() {
            return Err(Error::BatchSize);
        }
        Ok(Self {
            commissioned,
            rows: BTreeMap::new(),
        })
    }

    fn check_work(&self, work: Work) -> Result<(), Error> {
        let expected = self
            .commissioned
            .get(&work.objective)
            .ok_or(Error::UnknownObjective)?;
        if *expected != work {
            return Err(Error::StaleWork);
        }
        Ok(())
    }

    /// Results correspond to input order. A missing requested objective stays unfilled.
    /// Duplicate identities within a reply are ambiguous, even if the first row looks good.
    /// A repeated delivery of a previously staged row is an idempotent success.
    pub fn stage(
        &mut self,
        candidates: Vec<Candidate>,
    ) -> Result<Vec<Result<Document, Error>>, Error> {
        if candidates.len() > MAX_ROWS {
            return Err(Error::BatchSize);
        }
        let mut seen = BTreeSet::new();
        let mut duplicates = BTreeSet::new();
        for row in &candidates {
            if !seen.insert(row.work.objective) {
                duplicates.insert(row.work.objective);
            }
        }
        Ok(candidates
            .into_iter()
            .map(|candidate| {
                if duplicates.contains(&candidate.work.objective) {
                    return Err(Error::DuplicateRow);
                }
                self.check_work(candidate.work)?;
                candidate.pack.validate(candidate.work)?;
                let work = candidate.work;
                let proposal = Row::draft(candidate, None);
                match self.rows.entry(work.objective) {
                    Entry::Occupied(e) if e.get().digest == proposal.digest => Ok(proposal.digest),
                    Entry::Occupied(_) => Err(Error::Conflict),
                    Entry::Vacant(e) => Ok(e.insert(proposal).digest),
                }
            })
            .collect())
    }

    /// Replace only the named, unpublished predecessor. A late original delivery cannot
    /// overwrite this repair. A different approved body needs a new publication release.
    pub fn revise(&mut self, expected: Document, candidate: Candidate) -> Result<Document, Error> {
        self.check_work(candidate.work)?;
        let row = self
            .rows
            .get_mut(&candidate.work.objective)
            .ok_or(Error::MissingDraft)?;
        candidate.pack.validate(candidate.work)?;
        let mut replacement = Row::draft(candidate, Some(expected));
        if row.digest == replacement.digest {
            return Ok(row.digest);
        }
        if row.review != Review::Draft || row.digest != expected || row.pack == replacement.pack {
            return Err(Error::Conflict);
        }
        // The predecessor may have supplied most of a repaired lesson. Keep its authors out
        // of the independent review even when the final edit came from someone else.
        replacement.contributors.extend(&row.contributors);
        if replacement.contributors.len() > MAX_ROWS {
            return Err(Error::TooManyContributors);
        }
        *row = replacement;
        Ok(row.digest)
    }

    fn review_row(
        &mut self,
        work: Work,
        digest: Document,
        grant: &ReviewerGrant,
    ) -> Result<&mut Row, Error> {
        self.check_work(work)?;
        let row = self
            .rows
            .get_mut(&work.objective)
            .ok_or(Error::MissingDraft)?;
        if grant.work != work {
            return Err(Error::UnauthorizedReviewer);
        }
        if row.digest != digest {
            return Err(Error::WrongDigest);
        }
        Ok(row)
    }

    /// The subject reviewer signs this digest after checking every constituent's substantive
    /// claims, legal authority and date, criterion alignment, independence, and rubric. The source
    /// broker checks exact bytes and resolves grants for this work's instructional AND assessment uses.
    pub fn approve(
        &mut self,
        work: Work,
        digest: Document,
        grant: &ReviewerGrant,
        sources: &BTreeSet<Span>,
    ) -> Result<(), Error> {
        let row = self.review_row(work, digest, grant)?;
        if row.contributors.contains(&grant.actor) {
            return Err(Error::UnauthorizedReviewer);
        }
        if source_manifest(&row.pack.sources) != work.source_manifest {
            return Err(Error::InvalidSources);
        }
        if !row.pack.sources.is_subset(sources) {
            return Err(Error::SourceUnavailable);
        }
        match row.review {
            Review::Withdrawn(_) => return Err(Error::Withdrawn),
            Review::Approved(reviewer) if reviewer != grant.actor => return Err(Error::Conflict),
            _ => {}
        }
        row.review = Review::Approved(grant.actor);
        Ok(())
    }

    /// A scoped reviewer, including a contributor who discovers an error, can hold a
    /// defective rubric or task even if every source grant is still valid.
    /// Reintroduce corrected material in a new release.
    pub fn withdraw(
        &mut self,
        work: Work,
        digest: Document,
        grant: &ReviewerGrant,
    ) -> Result<(), Error> {
        let row = self.review_row(work, digest, grant)?;
        if !matches!(row.review, Review::Withdrawn(_)) {
            row.review = Review::Withdrawn(grant.actor);
        }
        Ok(())
    }

    /// Broker-side read against the currently assigned work order and live source permissions.
    /// Release acceptance alone establishes neither learner exposure nor achievement.
    pub fn available(&self, work: Work, sources: &BTreeSet<Span>) -> Result<&Pack, Error> {
        self.check_work(work)?;
        let row = self.rows.get(&work.objective).ok_or(Error::MissingDraft)?;
        match row.review {
            Review::Draft => return Err(Error::Unapproved),
            Review::Withdrawn(_) => return Err(Error::Withdrawn),
            Review::Approved(_) => {}
        }
        if !row.pack.sources.is_subset(sources) {
            return Err(Error::SourceUnavailable);
        }
        Ok(&row.pack)
    }
}
