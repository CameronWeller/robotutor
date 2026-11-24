//! Replay a pre-reviewed scenario's author response and editorial decision through the
//! actual creation state machine. The host verifies its separately pinned whole-fixture
//! digest before selecting the record. Production reviewer authentication and provider
//! response capture belong to their respective adapters.
use super::author_input::{AuthorInput, FIELDS};
use super::creation::{Batch, Candidate, Error, Pack, ReviewerGrant, Work};
use super::program::{Frame, Proposal};
use super::{Document, Span};
use crate::material::document_id;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub struct PublishedLesson {
    work: Work,
    batch: Batch,
    pins: BTreeSet<Span>,
    request: AuthorInput,
}

impl PublishedLesson {
    /// Read-only publication catalog and commissioned work for broker-side
    /// checks that require the exact approved edition and current source rights.
    pub fn publication(&self) -> (&Batch, Work) {
        (&self.batch, self.work)
    }

    pub fn unit(&self) -> u32 {
        self.work.objective.0
    }

    /// The provider-independent authoring request, including the exact evidence shown.
    pub fn request(&self) -> &str {
        self.request.wire()
    }

    pub(super) fn into_author_input(self) -> AuthorInput {
        self.request
    }

    /// Broker-side material includes the sealed check and private rubric. This fixture
    /// grants or withdraws its single source at read time. The caller projects the actual
    /// learner move separately.
    pub fn material(&self, permitted: bool) -> Result<&Pack, String> {
        let withdrawn = BTreeSet::new();
        self.batch
            .available(self.work, if permitted { &self.pins } else { &withdrawn })
            .map_err(release_error)
    }
}

fn release_error(error: Error) -> String {
    format!("scenario release failed: {error:?}")
}

fn field<'a>(record: &'a Value, name: &str) -> Result<&'a str, String> {
    record[name]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .ok_or_else(|| format!("missing or oversized scenario field {name}"))
}

/// Compile the same evidence edition and author contract for a new or existing package.
pub(super) fn prepare(
    frame: &Frame,
    plan: &Proposal,
    reviewed: Document,
    unit: u32,
    record: &Value,
) -> Result<(Work, BTreeSet<Span>, AuthorInput), String> {
    let source = field(record, "source_url")?;
    let locator = field(record, "locator")?;
    let excerpt = field(record, "excerpt")?;

    // An immutable local excerpt edition. Identical prose attributed to a different
    // source or locator has a different digest. The model sees just the passage bytes.
    let mut edition = json!(["scenario-excerpt/1", source, locator]).to_string();
    edition.push('\n');
    let start = edition.len() as u32;
    edition.push_str(excerpt);
    let document = document_id(edition.as_bytes());
    let pins = BTreeSet::from([Span {
        document,
        start,
        end: edition.len() as u32,
    }]);
    let corpus = BTreeMap::from([(document, edition.as_bytes())]);
    let work = frame.commission(plan, reviewed, unit, &pins)?;
    let input = frame.author_input(plan, reviewed, work, &pins, &corpus)?;
    Ok((work, pins, input))
}

/// Replay approval for a record selected from the host's independently pinned scenario.
/// `reviewed` is the outline digest from the program review. The external fixture pin
/// attests the exact material, attribution and simulated editorial decision. A live draft
/// must go through a separately obtained ReviewerGrant at the Batch boundary.
pub fn publish(
    frame: &Frame,
    plan: &Proposal,
    reviewed: Document,
    record: &Value,
) -> Result<PublishedLesson, String> {
    let unit = record["unit"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("missing or invalid scenario unit")?;
    let (work, pins, input) = prepare(frame, plan, reviewed, unit, record)?;
    let mut body = Map::new();
    for name in FIELDS {
        body.insert(name.into(), Value::from(field(record, name)?));
    }
    let pack = input.decode(&json!({"job":input.job(),"pack":body}))?;
    let mut batch = Batch::new(work.release, [work]).map_err(release_error)?;
    let candidate = Candidate {
        work,
        author: 1,
        pack,
    };
    let receipt = batch
        .stage(vec![candidate])
        .map_err(release_error)?
        .remove(0)
        .map_err(release_error)?;
    if batch.available(work, &pins) != Err(Error::Unapproved) {
        return Err("staged material must await the recorded editorial decision".into());
    }
    batch
        .approve(work, receipt, &ReviewerGrant { actor: 2, work }, &pins)
        .map_err(release_error)?;
    Ok(PublishedLesson {
        work,
        batch,
        pins,
        request: input,
    })
}
