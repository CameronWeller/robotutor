//! Publish a candidate from a known arithmetic template. The trusted release
//! broker issues the template binding before dispatch. The model supplies three
//! positive counts, never a new source, rubric, family ID, prompt, or answer key.
use crate::context::{Authority, Kind, Packet, Purpose, RecordRef};
use crate::curriculum::{Document, Span};
use crate::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use crate::routing::{Draft, Intent, Request, Router, Ticket};
use crate::supply::Form;
use std::collections::{btree_map::Entry, BTreeMap};

/// Approved template parameters bound to the operation before dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// Approved briefing and material records used to compile this exact packet.
    pub brief: RecordRef,
    pub material: RecordRef,
    pub span: Span,
    pub task: FractionTask,
    /// Immutable edition allocated by the catalog for the accepted result.
    pub edition: u64,
}

impl Template {
    pub fn binding(&self) -> Document {
        let task = match self.task {
            FractionTask::Sum => "sum",
            FractionTask::MissingPart => "missing-part",
        };
        let manifest = serde_json::json!([
            "fraction-template/1",
            [self.brief.id, self.brief.revision],
            [self.material.id, self.material.revision],
            self.span.document,
            self.span.start,
            self.span.end,
            task,
            self.edition
        ]);
        document_id(manifest.to_string().as_bytes())
    }
}

pub struct AuthoringJob {
    pub request: Request,
    pub template: Template,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationError {
    Stale,
    WrongJob,
    NoReceipt,
    WrongBytes,
    InvalidParameters,
    SourceUnavailable,
    EditionConflict,
}

/// Item editions are unique within an immutable unit release. Production
/// adapters need an equivalent unique constraint in the publication transaction.
#[derive(Default)]
pub struct Catalog {
    editions: BTreeMap<(u64, u64), (Form, Draft)>,
}

pub struct Publisher<'a> {
    pub router: &'a Router,
    pub catalog: &'a mut Catalog,
}

pub struct CandidateBytes<'a> {
    pub ticket: Ticket,
    pub bytes: &'a [u8],
}

impl AuthoringJob {
    /// The transaction adapter must compare the same authority, deadline and
    /// source revision when it commits this form. A completion alone emits no card.
    pub fn publish(
        &self,
        publisher: Publisher<'_>,
        candidate: CandidateBytes<'_>,
        packet: &Packet,
        current: &Authority,
        source: &RuleSource<'_>,
        now_ms: u64,
    ) -> Result<Form, PublicationError> {
        let CandidateBytes { ticket, bytes } = candidate;
        let Publisher { router, catalog } = publisher;
        if now_ms >= self.request.deadline_ms || packet.dispatch(current).is_err() {
            return Err(PublicationError::Stale);
        }
        if router.request_for(ticket) != Some(self.request)
            || self.request.binding != self.template.binding()
        {
            return Err(PublicationError::WrongJob);
        }
        let need = self.request.need;
        let owner = match (need.intent, packet.purpose()) {
            (Intent::AuthorForm, Purpose::Author) => None,
            (Intent::PersonalizeForm(_), Purpose::PrivateForm) => Some(need.scope.learner),
            _ => return Err(PublicationError::WrongJob),
        };
        if self.request.id != ticket.request_id
            || ticket.need != need
            || need.packet != packet.digest()
            || source.pin != self.template.span
            || packet
                .text_digest(self.template.brief, Kind::Brief)
                .is_none()
            || packet.text_digest(self.template.material, Kind::Material)
                != Some(document_id(UNIT_RULE.as_bytes()))
        {
            return Err(PublicationError::WrongJob);
        }
        let draft = router.draft(ticket).ok_or(PublicationError::NoReceipt)?;
        // Bound parser work even for a faulty provider or adapter.
        if bytes.len() > 64 {
            return Err(PublicationError::InvalidParameters);
        }
        if document_id(bytes) != draft.artifact {
            return Err(PublicationError::WrongBytes);
        }
        let [a, b, parts]: [u16; 3] =
            serde_json::from_slice(bytes).map_err(|_| PublicationError::InvalidParameters)?;
        let card = FractionCard::propose(self.template.task, a, b, parts)
            .ok_or(PublicationError::InvalidParameters)?;
        card.question(source)
            .ok_or(PublicationError::SourceUnavailable)?;
        // Task identity is semantic within these two versioned pack families.
        // Commuting the operands of a sum or translating its prose does not make it fresh.
        let (family, left, right): (u64, u16, u16) = match self.template.task {
            FractionTask::Sum => (1, a.min(b), a.max(b)),
            FractionTask::MissingPart => (2, a, b),
        };
        let task =
            (family << 48) | (u64::from(parts) << 32) | (u64::from(left) << 16) | u64::from(right);
        let form = Form {
            class: current.snapshot.class,
            release: need.scope.revision,
            outcome: need.outcome,
            owner,
            revision: self.template.edition,
            family,
            task,
            card,
            source: self.template.span,
        };
        match catalog.editions.entry((form.release, form.revision)) {
            Entry::Vacant(entry) => {
                entry.insert((form, draft));
                Ok(form)
            }
            Entry::Occupied(entry) if *entry.get() == (form, draft) => Ok(form),
            Entry::Occupied(_) => Err(PublicationError::EditionConflict),
        }
    }
}
