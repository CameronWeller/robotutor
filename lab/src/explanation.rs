//! Release a private explanation assembled from a teacher-reviewed reading lesson.
//! The provider chooses an approved sequence. The displayed words, supporting
//! passages, follow-up question and exposure keys all come from the release.

use crate::authoring::CandidateBytes;
use crate::context::{Authority, Kind, Packet, Purpose, Record, RecordRef};
use crate::curriculum::{Document, OutcomeId};
use crate::material::document_id;
use crate::routing::{Intent, Request, Role, Router, Ticket};
use crate::supply::Enrollment;
use serde_json::json;
use std::collections::BTreeSet;

/// Stable within the cross-release catalog used by the exposure broker.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Exposure {
    pub task: u64,
    pub family: u64,
}

pub struct Passage<'a> {
    pub record: &'a Record,
    /// Half-open UTF-8 byte range in the exact reviewed source revision.
    pub start: usize,
    pub end: usize,
}

impl Passage<'_> {
    fn excerpt(&self) -> Option<&str> {
        self.record
            .text
            .get(self.start..self.end)
            .filter(|s| !s.trim().is_empty())
    }
}

pub struct Chunk<'a> {
    /// Human-reviewed instructional copy, interpretation and attribution.
    pub text: &'a str,
    pub support: &'a [Passage<'a>],
    pub exposure: Exposure,
}

/// Neutral prompts reviewed for this slice. Neither interpolates a resolution,
/// an answer key or student text.
#[derive(Clone, Copy, Debug)]
pub enum BridgePrompt {
    NextDetail,
    Discriminate,
}

impl BridgePrompt {
    pub fn text(self) -> &'static str {
        match self {
            Self::NextDetail => "Which detail would you check next, and why?",
            Self::Discriminate => {
                "What additional evidence would help you choose between the two explanations?"
            }
        }
    }
}

pub struct Bridge<'a> {
    pub record: &'a Record,
    pub prompt: BridgePrompt,
    pub exposure: Exposure,
}

/// Authenticated recipient and clock at the actual display boundary.
#[derive(Clone, Copy)]
pub struct Display {
    pub recipient: Enrollment,
    pub now_ms: u64,
}

pub struct Lesson<'a> {
    pub release: u64,
    pub outcome: OutcomeId,
    pub goal: &'a str,
    pub brief: RecordRef,
    pub chunks: &'a [Chunk<'a>],
    /// Teacher-approved orders, each containing one to three different chunks.
    /// An order can privilege a different example or start with a misconception.
    pub paths: &'a [&'a [u8]],
    pub bridge: Bridge<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidRelease,
    WrongTurn,
    Stale,
    SourceUnavailable,
    NoReceipt,
    WrongBytes,
    InvalidChoice,
    Abstained,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Citation<'a> {
    pub source: RecordRef,
    pub start: usize,
    pub end: usize,
    pub quote: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Paragraph<'a> {
    pub text: &'a str,
    pub sources: Vec<Citation<'a>>,
}

/// The host commits these contacts and the private delivery in one transaction,
/// keyed by ticket. Persist a possible contact if delivery is uncertain. A
/// retry with the same ticket cannot mint fresh evidence or a second delivery.
#[derive(Debug, PartialEq, Eq)]
pub struct PrivateExplanation<'a> {
    pub seat: Enrollment,
    pub outcome: OutcomeId,
    pub ticket: Ticket,
    pub paragraphs: Vec<Paragraph<'a>>,
    pub bridge: &'a str,
    pub bridge_source: RecordRef,
    pub exposed: BTreeSet<Exposure>,
}

impl Lesson<'_> {
    fn validate(&self) -> Result<(), Error> {
        if !(1..=256).contains(&self.goal.len())
            || self.goal.trim().is_empty()
            || !(1..=8).contains(&self.chunks.len())
            || !(1..=8).contains(&self.paths.len())
            || self.bridge.record.kind != Kind::Material
            || self.bridge.record.release != self.release
            || self.bridge.record.outcome != self.outcome
            || self.bridge.record.text != self.bridge.prompt.text()
        {
            return Err(Error::InvalidRelease);
        }
        for chunk in self.chunks {
            if !(1..=768).contains(&chunk.text.len())
                || chunk.text.trim().is_empty()
                || !(1..=3).contains(&chunk.support.len())
            {
                return Err(Error::InvalidRelease);
            }
            for pin in chunk.support {
                if pin.record.kind != Kind::Material
                    || pin.record.release != self.release
                    || pin.record.outcome != self.outcome
                    || pin.record.key == self.bridge.record.key
                    || pin.excerpt().is_none_or(|s| s.len() > 512)
                {
                    return Err(Error::InvalidRelease);
                }
            }
        }
        for path in self.paths {
            if !(1..=3).contains(&path.len())
                || path
                    .iter()
                    .enumerate()
                    .any(|(i, &n)| usize::from(n) >= self.chunks.len() || path[..i].contains(&n))
            {
                return Err(Error::InvalidRelease);
            }
        }
        Ok(())
    }

    /// The release broker stores these exact instructions as the authorized
    /// Brief record before compiling a Teach packet. Only local option indices
    /// and approved text appear in the prompt.
    pub fn brief_text(&self) -> Result<String, Error> {
        self.validate()?;
        let choices: Vec<_> = self.chunks.iter().map(|c| c.text).collect();
        Ok(json!({
            "goal":self.goal, "paragraphs":choices, "permitted_sequences":self.paths,
            "follow_up":self.bridge.prompt.text(),
            "response":"Choose the sequence that best teaches this learner the goal. Output its JSON array of zero-based paragraph indices, or null to abstain."
        }).to_string())
    }

    pub fn binding(&self) -> Result<Document, Error> {
        let brief = self.brief_text()?;
        let refs = |r: RecordRef| [r.id, r.revision];
        let exposure = |e: Exposure| [e.task, e.family];
        let chunks: Vec<_> = self
            .chunks
            .iter()
            .map(|c| {
                let pins: Vec<_> = c
                    .support
                    .iter()
                    .map(|p| {
                        json!([
                            refs(p.record.key),
                            p.start,
                            p.end,
                            document_id(p.excerpt().expect("validated source range").as_bytes())
                        ])
                    })
                    .collect();
                json!([exposure(c.exposure), pins])
            })
            .collect();
        let manifest = json!([
            "reviewed-explanation/1",
            self.release,
            self.outcome.0,
            refs(self.brief),
            document_id(brief.as_bytes()),
            chunks,
            refs(self.bridge.record.key),
            exposure(self.bridge.exposure)
        ]);
        Ok(document_id(manifest.to_string().as_bytes()))
    }

    /// Only an exact authorized work order and successful, byte-bound model
    /// completion can select this lesson. Every reviewed source is revalidated
    /// even when the job has been waiting or is being replayed.
    pub fn deliver<'a>(
        &'a self,
        request: Request,
        router: &Router,
        candidate: CandidateBytes<'_>,
        packet: &Packet,
        current: &Authority,
        display: Display,
    ) -> Result<PrivateExplanation<'a>, Error> {
        let CandidateBytes { ticket, bytes } = candidate;
        if display.now_ms >= request.deadline_ms || packet.dispatch(current).is_err() {
            return Err(Error::Stale);
        }
        let need = request.need;
        if current.snapshot.role != crate::context::Role::Learner
            || display.recipient.scope != current.snapshot.scope
            || display.recipient.class != current.snapshot.class
            || need.intent != Intent::Explain
            || ticket.role != Role::Language
            || packet.purpose() != Purpose::Teach
            || need.scope != packet.scope()
            || need.scope.revision != self.release
            || need.outcome != self.outcome
            || packet.outcome() != self.outcome
            || need.packet != packet.digest()
            || router.request_for(ticket) != Some(request)
            || request.binding != self.binding()?
        {
            return Err(Error::WrongTurn);
        }
        if packet.text_digest(self.brief, Kind::Brief)
            != Some(document_id(self.brief_text()?.as_bytes()))
            || !packet.includes_record(self.bridge.record)
            || self
                .chunks
                .iter()
                .any(|c| c.support.iter().any(|p| !packet.includes_record(p.record)))
        {
            return Err(Error::SourceUnavailable);
        }
        let draft = router.draft(ticket).ok_or(Error::NoReceipt)?;
        if bytes.len() > 32 {
            return Err(Error::InvalidChoice);
        }
        if document_id(bytes) != draft.artifact {
            return Err(Error::WrongBytes);
        }
        let sequence: Vec<u8> = serde_json::from_slice::<Option<Vec<u8>>>(bytes)
            .map_err(|_| Error::InvalidChoice)?
            .ok_or(Error::Abstained)?;
        if !self.paths.contains(&sequence.as_slice()) {
            return Err(Error::InvalidChoice);
        }
        let mut exposed = BTreeSet::from([self.bridge.exposure]);
        let mut paragraphs = Vec::new();
        for idx in sequence {
            let chunk = &self.chunks[usize::from(idx)];
            exposed.insert(chunk.exposure);
            let sources = chunk
                .support
                .iter()
                .map(|p| Citation {
                    source: p.record.key,
                    start: p.start,
                    end: p.end,
                    quote: p.excerpt().expect("validated immutable passage"),
                })
                .collect();
            paragraphs.push(Paragraph {
                text: chunk.text,
                sources,
            });
        }
        Ok(PrivateExplanation {
            seat: Enrollment {
                scope: need.scope,
                class: current.snapshot.class,
            },
            outcome: self.outcome,
            ticket,
            paragraphs,
            bridge: self.bridge.prompt.text(),
            bridge_source: self.bridge.record.key,
            exposed,
        })
    }
}
