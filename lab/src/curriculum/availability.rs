//! Resolve one requested objective against the pinned outline and publication records.
//! Availability is a read. It neither queues work nor records practice as evidence.
use super::{
    author_input::AuthorInput,
    goal_rehearsal::{source, Course, Options},
    program::{Frame, Proposal, Unit},
    rehearsal_release::{prepare, publish},
};
use crate::history::Withdrawn;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// A private activity that works even before the learner and educator agree on a syllabus.
pub fn orientation() -> &'static str {
    "Choose one situation where you would like to use this skill. Jot down what you want to do there, what you already know, and one question you would bring to a tutor. You can keep these notes private or use them to plan the first lesson."
}

/// A pinned fixture has one publication per objective. Opening and transfer designate
/// scripted roles; other packs only need a complete reviewed teaching record.
pub(super) fn catalog<'a>(
    data: &'a Value,
    plan: &Proposal,
) -> Result<BTreeMap<u32, &'a Value>, String> {
    let others = match data.get("published_packs") {
        None => &[][..],
        Some(value) => value
            .as_array()
            .ok_or("published_packs must be an array")?
            .as_slice(),
    };
    if others.len() > plan.units().len() {
        return Err("more published packs than planned units".into());
    }
    let roles = [&data["first_pack"], &data["return_pack"]]
        .into_iter()
        .filter(|record| !record.is_null());
    let mut catalog = BTreeMap::new();
    for record in roles.chain(others) {
        let id = record["unit"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("invalid unit ID in the publication catalog")?;
        if !plan.units().iter().any(|u| u.id == id) {
            return Err(format!("publication for unknown unit {id}"));
        }
        if catalog.insert(id, record).is_some() {
            return Err(format!("conflicting publications for unit {id}"));
        }
    }
    Ok(catalog)
}

/// Display only the worked example and attribution from the independently pinned edition.
/// A new learner can try this without accepting the edition's proposed scope.
pub fn introductory_sample(data: &Value, frame: &Frame, plan: &Proposal) -> Result<String, String> {
    let record = &data["first_pack"];
    let release = publish(frame, plan, frame.review_digest(plan), record)?;
    let unit = plan
        .units()
        .iter()
        .find(|unit| unit.id == release.unit() && unit.requires.is_empty())
        .ok_or("no introductory sample available")?;
    Ok(format!(
        "Sample · {}\n{}\n{}\n{}",
        unit.title,
        release.material(true)?.worked,
        record["locator"].as_str().ok_or("missing attribution")?,
        record["source_url"].as_str().ok_or("missing source")?
    ))
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    NeedsEvidence,
    Brief,
    NeedsSource,
}

impl Availability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "available from the start",
            Self::NeedsEvidence => "released; earlier evidence needed",
            Self::Brief => "authoring brief available",
            Self::NeedsSource => "source selection needed",
        }
    }
}

/// Read-only view of a host-verified fixture for a learner with no recorded prerequisite evidence.
pub struct MaterialForecast<'a> {
    frame: &'a Frame,
    plan: &'a Proposal,
    data: &'a Value,
    review: super::Document,
    units: BTreeMap<u32, &'a Unit>,
    released: BTreeSet<u32>,
}

impl<'a> MaterialForecast<'a> {
    pub fn from_pinned(
        data: &'a Value,
        frame: &'a Frame,
        plan: &'a Proposal,
    ) -> Result<Self, String> {
        let review = frame.review_digest(plan);
        let mut released = BTreeSet::new();
        for (id, record) in catalog(data, plan)? {
            publish(frame, plan, review, record)?.material(true)?;
            released.insert(id);
        }
        Ok(Self {
            data,
            frame,
            plan,
            review,
            units: plan.units().iter().map(|unit| (unit.id, unit)).collect(),
            released,
        })
    }

    pub fn status(&self, unit_id: u32) -> Result<Availability, String> {
        let unit = self.units.get(&unit_id).ok_or("unknown forecast unit")?;
        self.unit_status(unit)
    }

    fn unit_status(&self, unit: &Unit) -> Result<Availability, String> {
        let unit_id = unit.id;
        if self.released.contains(&unit_id) {
            return Ok(if unit.requires.is_empty() {
                Availability::Available
            } else {
                Availability::NeedsEvidence
            });
        }
        if let Some(record) = self.data["authoring_sources"].get(unit_id.to_string()) {
            prepare(self.frame, self.plan, self.review, unit_id, record)?;
            return Ok(Availability::Brief);
        }
        Ok(Availability::NeedsSource)
    }

    pub fn forecast(&self, total: u32) -> Result<String, String> {
        let calendar = self.frame.calendar.ok_or("missing reviewed calendar")?;
        let forward = calendar.forward_within(total).ok_or_else(|| {
            format!(
                "this reviewed outline covers at most {} minutes",
                calendar.total_minutes()
            )
        })?;
        let schedule = self
            .frame
            .sequence(self.plan, forward)
            .map_err(|e| format!("{e:?}"))?;
        let mut lines = vec![format!(
            "{} of {} outline units fit {} forward minutes; {} minutes reserved.",
            schedule.scheduled.len(),
            self.plan.units().len(),
            forward,
            total - forward
        )];
        let mut counts = [0usize; 4];
        for id in schedule.scheduled {
            let unit = self.units.get(&id).ok_or("schedule lost a unit")?;
            let status = self.unit_status(unit)?;
            counts[status as usize] += 1;
            lines.push(format!(
                "{id}  {}  {} min  {}",
                unit.title,
                unit.minutes,
                status.label()
            ));
        }
        lines.push(format!(
            "Available now: {}. Released with prerequisites: {}. Brief only: {}. Need source: {}.",
            counts[0], counts[1], counts[2], counts[3]
        ));
        if schedule.used_minutes == 0 {
            lines.push(
                "No complete unit fits this forecast. You can sample one worked example.".into(),
            );
        }
        Ok(lines.join("\n"))
    }
}

pub(super) fn author_input(
    course: &Course<'_>,
    id: u32,
    options: &Options,
) -> Result<AuthorInput, String> {
    if !course.plan.units().iter().any(|u| u.id == id) {
        return Err(format!(
            "unit {id} needs an approved objective before authoring"
        ));
    }
    if course.data["return_pack"]["unit"].as_u64() == Some(u64::from(id)) && options.withdraw_return
    {
        return Err(
            "the required source permission was withdrawn; select a replacement before authoring"
                .into(),
        );
    }
    if let Some(record) = course.published.get(&id) {
        return Ok(publish(&course.frame, &course.plan, course.review, record)?.into_author_input());
    }
    let record = course.data["authoring_sources"]
        .get(id.to_string())
        .ok_or("a subject reviewer must authorize a passage before authoring this unit")?;
    let (_, _, input) = prepare(&course.frame, &course.plan, course.review, id, record)?;
    Ok(input)
}

pub(super) fn lookup(course: &Course<'_>, id: u32, options: &Options) -> Result<String, String> {
    let Some(unit) = course.plan.units().iter().find(|u| u.id == id) else {
        return Ok(format!("Unit {id} is outside this outline. Ask to add the objective or choose a listed unit.\n{}", orientation()));
    };

    let withdrawn = course.data["return_pack"]["unit"].as_u64() == Some(u64::from(id))
        && options.withdraw_return;
    let mut lines = vec![format!(
        "Unit {} · {}\nIntended performance · {}",
        unit.id, unit.title, unit.performance
    )];
    if withdrawn {
        lines.push("The approved passage for this exercise was withdrawn. A replacement needs source and subject review.".into());
    } else if let Some(record) = course.published.get(&id) {
        let release = publish(&course.frame, &course.plan, course.review, record)?;
        let pack = release.material(true)?;
        let pending = pending_prerequisites(course, &unit.requires, options)?;
        if pending.is_empty() {
            lines.push(format!(
                "Lesson\n{}\n{}\nWorked example\n{}\nGuided practice\n{}",
                pack.lesson,
                source(record)?,
                pack.worked,
                pack.practice
            ));
            return Ok(lines.join("\n"));
        }
        lines.push(format!("A reviewed lesson is available. First establish the prerequisite skills for units {pending:?}."));
    } else {
        let source = course.data["authoring_sources"].get(id.to_string());
        if let Some(record) = source {
            prepare(&course.frame, &course.plan, course.review, id, record)?;
            lines.push(format!("An authoring brief is ready for this {}-minute objective. A draft and independent subject review are still needed.", unit.minutes));
        } else {
            let need = "A subject reviewer needs to select and authorize a passage for this objective before an author can draft its lesson.";
            lines.push(need.into());
        }
        let pending = pending_prerequisites(course, &unit.requires, options)?;
        if !pending.is_empty() {
            lines.push(format!(
                "This learner also needs evidence for prerequisite units {pending:?}."
            ));
        }
    }
    lines.push("While you wait, write down one example where you would like to use the intended skill and one question about it. You can keep this planning exercise private. It carries no assessment.".into());
    Ok(lines.join("\n"))
}

fn pending_prerequisites(
    course: &Course<'_>,
    parents: &[u32],
    options: &Options,
) -> Result<Vec<u32>, String> {
    let mut pending = parents.to_vec();
    if let Some(answer) = options.first_response.as_deref() {
        let first = course.data["first_pack"]["unit"].as_u64();
        if let Some(index) = pending.iter().position(|id| Some(u64::from(*id)) == first) {
            let (evidence, _) = course.first_grade(answer, false)?;
            if evidence.opens_transfer(&Withdrawn::new(course.cell(pending[index]).scope)) {
                pending.remove(index);
            }
        }
    }
    Ok(pending)
}
