//! Bind performance evidence to the learner, current edition, criterion and exact response.
//! The host authenticates the capture channel, reviewer and witness. A typed observation
//! comes from that channel. A learner's claim to have used the channel is a self-report.
pub mod contact_history;
use super::{creation::Pack, Document, OutcomeId};
use crate::{
    history::{eligible, Attempt, Withdrawn},
    interpretation::Finding,
    material::document_id,
    protocol::{Assistance, Judgment, Question},
    retention::{self, Cell, CheckResult, Exposure, Moment, Prior, ReadError},
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode<'a> {
    Quantity { value: i64, unit: &'a str },
    WrittenArtifact,
    WitnessedPerformance,
}

#[derive(Clone, Copy)]
pub struct Presentation {
    pub attempt: u64,
    pub at: Moment,
    pub fresh: bool,
    pub assistance: Assistance,
}

#[derive(Clone, Copy)]
pub enum Observation<'a> {
    Quantity { value: i64, unit: &'a str },
    Written(&'a str),
    Witnessed { observer: u64, record: &'a str },
    SelfReport,
    Unobserved,
}

// Quantity exercises can use the same checked response format in teaching,
// diagnostics, and assessed submissions. Malformed or absent input is missing.
fn quantity_observation(response: &str) -> Observation<'_> {
    let mut tokens = response.split_whitespace();
    let value = tokens.next().and_then(|s| s.parse::<i64>().ok());
    match (value, tokens.next(), tokens.next()) {
        (Some(value), Some(unit), None) => Observation::Quantity { value, unit },
        _ => Observation::Unobserved,
    }
}

fn check_quantity(expected: i64, unit: &str, given: i64, given_unit: &str) -> Judgment {
    if given == expected && given_unit == unit {
        Judgment::Correct
    } else {
        Judgment::Incorrect
    }
}

/// Check an instructional response without creating a synthetic evidence event.
pub fn check_quantity_text(expected: i64, unit: &str, response: &str) -> Judgment {
    match quantity_observation(response) {
        Observation::Quantity {
            value,
            unit: given_unit,
        } => check_quantity(expected, unit, value, given_unit),
        _ => Judgment::Missing,
    }
}

pub struct Task<'a> {
    cell: Cell,
    edition: Document,
    dependencies: BTreeSet<Document>,
    item: u64,
    mode: Mode<'a>,
    claim: Document,
    prompt: Document,
    checked_material: Document,
    published_case: Option<PublishedCase>,
    task_digest: Document,
    samples: &'a Value,
}

/// An exact released exercise under one criterion. A publisher may reuse a
/// reviewed ordered comparison across learners in the same course release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublishedCase {
    pub unit_release: u64,
    pub outcome: OutcomeId,
    pub criterion: Document,
    pub pack: Document,
}

pub struct Submission<'a> {
    opening: Document,
    digest: Document,
    observation: Observation<'a>,
}

/// Trusted reviewer decision about the uniquely located passage in one submitted artifact.
#[derive(Clone, Debug)]
pub struct Review {
    response: Document,
    finding: Finding,
    reviewer: u64,
    quote: std::ops::Range<usize>,
}

impl Review {
    pub fn provenance(&self) -> (u64, Document, std::ops::Range<usize>) {
        (self.reviewer, self.response, self.quote.clone())
    }
}

/// Private artifact paired with its currently authorized adjudication.
#[derive(Clone, Copy)]
pub struct ReviewedWriting<'a> {
    pub at: Presentation,
    pub submission: &'a Submission<'a>,
    pub review: &'a Review,
}

/// A description of two versions under one exact criterion. Permission to
/// publish this private result belongs to the caller's audience boundary.
#[derive(Debug, PartialEq, Eq)]
pub struct WritingComparison {
    pub wording_changed: bool,
    pub earlier_quote: std::ops::Range<usize>,
    pub later_quote: std::ops::Range<usize>,
    pub earlier: Finding,
    pub later: Finding,
    pub earlier_help: Assistance,
    pub later_help: Assistance,
}

pub struct Evidence {
    attempt: Attempt,
    claim: Document,
    prompt: Document,
    published_case: Option<PublishedCase>,
    review: Option<Review>,
}

fn digest(value: Value) -> Document {
    document_id(value.to_string().as_bytes())
}

fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str, String> {
    row[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

impl<'a> Task<'a> {
    /// The host supplies the current published pack, its digest and the reviewed shared claim.
    /// A fixture can additionally carry scripted observations used by the offline replay.
    pub fn from_pack(
        cell: Cell,
        edition: Document,
        evidence_mode: &str,
        criterion_claim: &str,
        pack: &'a Value,
    ) -> Result<Self, String> {
        let item = pack["unit"]
            .as_u64()
            .filter(|&id| id > 0)
            .ok_or("missing unit")?;
        if criterion_claim.trim().is_empty() {
            return Err("missing reviewed common criterion".into());
        }
        let mode = match evidence_mode {
            "quantity" => Mode::Quantity {
                value: pack["quantity"]["value"]
                    .as_i64()
                    .ok_or("missing signed quantity")?,
                unit: text(&pack["quantity"], "unit")?,
            },
            "written_artifact" => Mode::WrittenArtifact,
            "witnessed_performance" => Mode::WitnessedPerformance,
            _ => return Err("unsupported evidence mode".into()),
        };
        let question = text(pack, "check")?;
        let criterion = text(pack, "criterion")?;
        let rubric = text(pack, "rubric")?;
        let task_digest = digest(json!([
            "goal-assessment/1",
            cell.scope.learner,
            cell.scope.revision,
            cell.outcome.0,
            edition,
            item,
            question,
            criterion,
            rubric,
            criterion_claim,
            evidence_mode,
            pack["quantity"]
        ]));
        let normalized = question
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        Ok(Self {
            cell,
            edition,
            dependencies: BTreeSet::from([edition]),
            item,
            mode,
            claim: digest(json!(["goal-criterion/1", criterion_claim, evidence_mode])),
            prompt: document_id(normalized.as_bytes()),
            checked_material: digest(json!([question, criterion, rubric])),
            published_case: None,
            task_digest,
            samples: &pack["attempts"],
        })
    }

    /// Import the release broker's corpus dependencies for correction and withdrawal replay.
    pub fn with_published_sources(mut self, pack: &Pack) -> Result<Self, String> {
        if pack.digest() != self.edition
            || digest(json!([pack.check, pack.criterion, pack.rubric])) != self.checked_material
        {
            return Err("assessment material must match the current published pack".into());
        }
        self.dependencies
            .extend(pack.sources.iter().map(|s| s.document));
        self.published_case = Some(PublishedCase {
            unit_release: self.cell.scope.revision,
            outcome: self.cell.outcome,
            criterion: self.claim,
            pack: self.edition,
        });
        Ok(self)
    }

    pub fn published_case(&self) -> Option<PublishedCase> {
        self.published_case
    }

    fn opening(&self, p: Presentation) -> Document {
        let help = match p.assistance {
            Assistance::NoneReported => "none_reported",
            Assistance::Reported => "reported",
            Assistance::Unknown => "unknown",
        };
        digest(json!([
            "goal-opening/1",
            self.task_digest,
            p.attempt,
            p.at.day,
            p.at.sequence,
            p.fresh,
            help
        ]))
    }

    pub fn capture<'b>(&self, p: Presentation, observation: Observation<'b>) -> Submission<'b> {
        let opening = self.opening(p);
        let body = match observation {
            Observation::Quantity { value, unit } => json!(["quantity", value, unit]),
            Observation::Written(body) => json!(["written", body]),
            Observation::Witnessed { observer, record } => json!(["witnessed", observer, record]),
            Observation::SelfReport => json!("self_report"),
            Observation::Unobserved => json!("unobserved"),
        };
        Submission {
            opening,
            digest: digest(json!(["goal-response/1", opening, body])),
            observation,
        }
    }

    fn artifact<'b>(&self, observation: Observation<'b>) -> Option<&'b str> {
        match (self.mode, observation) {
            (Mode::WrittenArtifact, Observation::Written(body)) => Some(body),
            (Mode::WitnessedPerformance, Observation::Witnessed { observer, record })
                if observer != 0 && observer != self.cell.scope.learner =>
            {
                Some(record)
            }
            _ => None,
        }
    }

    /// The authenticated review adapter supplies its adjudication and a verbatim evidence
    /// location. A matching quote establishes provenance. The reviewer decides its meaning.
    pub fn attest(
        &self,
        p: Presentation,
        sub: &Submission<'_>,
        reviewer: u64,
        finding: Finding,
        quote: &str,
    ) -> Result<Review, &'static str> {
        if reviewer == 0 || reviewer == self.cell.scope.learner || self.opening(p) != sub.opening {
            return Err("reviewer or presentation mismatch");
        }
        let artifact = self
            .artifact(sub.observation)
            .ok_or("no observable artifact in this modality")?;
        let start = artifact
            .find(quote)
            .ok_or("quote absent from the observed artifact")?;
        if quote.trim().is_empty() || Some(start) != artifact.rfind(quote) {
            return Err("quote must identify a unique passage in the observed artifact");
        }
        Ok(Review {
            response: sub.digest,
            finding,
            reviewer,
            quote: start..start + quote.len(),
        })
    }

    pub fn assess(
        &self,
        p: Presentation,
        sub: &Submission<'_>,
        review: Option<&Review>,
    ) -> Evidence {
        let current = self.opening(p) == sub.opening;
        let accepted = review.filter(|r| {
            current && r.response == sub.digest && self.artifact(sub.observation).is_some()
        });
        let judgment = if !current {
            Judgment::Missing
        } else {
            match (self.mode, sub.observation) {
                (
                    Mode::Quantity { value, unit },
                    Observation::Quantity {
                        value: given,
                        unit: given_unit,
                    },
                ) => check_quantity(value, unit, given, given_unit),
                _ => match accepted.map(|r| r.finding) {
                    Some(Finding::Meets) => Judgment::Correct,
                    Some(Finding::NotYet) => Judgment::Incorrect,
                    Some(Finding::Unresolved) => Judgment::Ambiguous,
                    None => Judgment::Missing,
                },
            }
        };
        Evidence {
            attempt: Attempt {
                scope: self.cell.scope,
                id: p.attempt,
                sequence: p.at.sequence,
                outcome: self.cell.outcome,
                item: self.item,
                day: p.at.day,
                question: Question::IndependentCheck,
                judgment,
                assistance: p.assistance,
                fresh: p.fresh,
                sources: self.dependencies.clone(),
            },
            claim: self.claim,
            prompt: self.prompt,
            published_case: self.published_case,
            review: accepted.cloned(),
        }
    }

    /// Compare assessed versions of the same written exercise after it has
    /// already been seen. Regrading an unchanged submission can change a
    /// judgment without establishing a change in the learner's writing.
    pub fn compare_writing(
        &self,
        earlier: ReviewedWriting<'_>,
        revised: ReviewedWriting<'_>,
        withdrawn: &Withdrawn,
    ) -> Option<WritingComparison> {
        if !matches!(self.mode, Mode::WrittenArtifact)
            || self.cell.scope != withdrawn.scope
            || earlier.at.at >= revised.at.at
            || earlier.at.attempt == revised.at.attempt
            || revised.at.fresh
            || withdrawn.attempts.contains(&earlier.at.attempt)
            || withdrawn.attempts.contains(&revised.at.attempt)
            || withdrawn.items.contains(&self.item)
            || !self.dependencies.is_disjoint(&withdrawn.sources)
        {
            return None;
        }
        for version in [earlier, revised] {
            if self.opening(version.at) != version.submission.opening
                || version.review.response != version.submission.digest
            {
                return None;
            }
        }
        let old_text = self.artifact(earlier.submission.observation)?;
        let new_text = self.artifact(revised.submission.observation)?;
        Some(WritingComparison {
            wording_changed: old_text != new_text,
            earlier_quote: earlier.review.quote.clone(),
            later_quote: revised.review.quote.clone(),
            earlier: earlier.review.finding,
            later: revised.review.finding,
            earlier_help: earlier.at.assistance,
            later_help: revised.at.assistance,
        })
    }

    fn text_observation<'b>(&self, response: &'b str) -> Observation<'b> {
        match self.mode {
            Mode::Quantity { .. } => quantity_observation(response),
            Mode::WrittenArtifact => Observation::Written(response),
            Mode::WitnessedPerformance => Observation::SelfReport,
        }
    }

    /// Live learner text has no attached qualitative review or authenticated witness.
    pub fn unreviewed_text(&self, p: Presentation, text: &str) -> Evidence {
        let sub = self.capture(p, self.text_observation(text));
        self.assess(p, &sub, None)
    }

    /// Offline adapter for reviewed scenarios. A qualitative label represents the recorded
    /// human decision only in this simulation. The numeric kernel always reads the response.
    pub fn scripted(
        &self,
        p: Presentation,
        branch: &str,
        reviewer: u64,
    ) -> Result<Evidence, String> {
        let response = text(&self.samples[branch], "response")?;
        let finding = match branch {
            "demonstrated" => Finding::Meets,
            "revisit" => Finding::NotYet,
            "unobserved" => Finding::Unresolved,
            _ => return Err("unknown scripted observation".into()),
        };
        let obs = match (self.mode, branch) {
            (_, "unobserved") => Observation::Unobserved,
            (Mode::WitnessedPerformance, _) => Observation::Witnessed {
                observer: reviewer,
                record: response,
            },
            _ => self.text_observation(response),
        };
        let sub = self.capture(p, obs);
        let review = self
            .artifact(obs)
            .map(|_| self.attest(p, &sub, reviewer, finding, response))
            .transpose()?;
        Ok(self.assess(p, &sub, review.as_ref()))
    }
}

impl Evidence {
    pub fn review(&self) -> Option<&Review> {
        self.review.as_ref()
    }

    pub fn judgment(&self) -> Judgment {
        self.attempt.judgment
    }

    pub fn opens_transfer(&self, withdrawn: &Withdrawn) -> bool {
        self.attempt.scope == withdrawn.scope
            && self.attempt.judgment == Judgment::Correct
            && !withdrawn.postponed.contains(&self.attempt.outcome)
            && eligible(&self.attempt, self.attempt.day, withdrawn)
    }
}

pub struct Spacing {
    pub stability_days: f64,
    pub threshold: f64,
    pub target_day: Option<u32>,
}

/// Read the pair against today's withdrawals. The caller's complete exposure ledger
/// includes guided encounters, with `None` explicitly indicating incomplete coverage.
pub fn delayed(
    first: &Evidence,
    transfer: &Evidence,
    exposures: Option<&[Exposure]>,
    withdrawn: &Withdrawn,
    policy: Spacing,
) -> Result<CheckResult, ReadError> {
    let different = first.attempt.item != transfer.attempt.item;
    delayed_if_changed(first, transfer, exposures, withdrawn, policy, different)
}

/// Applies current editorial decisions about a *particular sequence* of
/// exercises. The host reads these approved pairs from the applicable release.
/// A task proposal, a differing hash or a missing historical edition supplies
/// no evidence that the later exercise requires a new application.
pub fn delayed_on_reviewed_change(
    first: &Evidence,
    transfer: &Evidence,
    exposures: Option<&[Exposure]>,
    withdrawn: &Withdrawn,
    policy: Spacing,
    approved: &BTreeSet<(PublishedCase, PublishedCase)>,
) -> Result<CheckResult, ReadError> {
    let different = first
        .published_case
        .zip(transfer.published_case)
        .is_some_and(|(a, b)| a != b && approved.contains(&(a, b)));
    delayed_if_changed(first, transfer, exposures, withdrawn, policy, different)
}

fn delayed_if_changed(
    first: &Evidence,
    transfer: &Evidence,
    exposures: Option<&[Exposure]>,
    withdrawn: &Withdrawn,
    policy: Spacing,
    different: bool,
) -> Result<CheckResult, ReadError> {
    let a = &first.attempt;
    let b = &transfer.attempt;
    if a.scope != b.scope || a.scope != withdrawn.scope || a.outcome != b.outcome {
        return Err(ReadError::WrongCell);
    }
    if !first.opens_transfer(withdrawn)
        || !different
        || a.id == b.id
        || first.prompt == transfer.prompt
        || first.claim != transfer.claim
    {
        return Ok(CheckResult::Ineligible);
    }
    let cell = Cell {
        scope: a.scope,
        outcome: a.outcome,
    };
    let prior = Prior {
        cell,
        verified: Moment {
            day: a.day,
            sequence: a.sequence,
        },
        stability_days: policy.stability_days,
        threshold: policy.threshold,
        had_durable_success: false,
    };
    let as_of = Moment {
        day: b.day,
        sequence: b.sequence,
    };
    let (_, result) = retention::read(
        cell,
        as_of,
        policy.target_day,
        Some(prior),
        exposures,
        Some(b),
        withdrawn,
    )?;
    Ok(result)
}
