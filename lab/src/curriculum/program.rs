//! Compile a proposed program against an independently supplied breadth frame.
//! Planned minutes reserve instruction. They create neither content nor learner evidence.
use super::{
    creation::{source_manifest, Work, MAX_ROWS},
    Document, OutcomeId, Span,
};
use crate::material::document_id;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_FACETS: usize = 64;
const MAX_UNITS: usize = 128;
const MAX_PARENTS: usize = 16;
const MAX_MINUTES: u32 = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub goal: String,
    pub purpose: String,
    /// The learner's chosen setting. The original law lab exposes this field as jurisdiction.
    /// Frame::scope_input preserves the setting's field name in the reviewed exchange.
    pub jurisdiction: String,
    pub horizon: String,
}

/// A frame can be scoped to a jurisdiction or to another explicit learning setting.
pub(super) fn setting_key(scope: &Value) -> Result<&'static str, String> {
    match (scope.get("jurisdiction"), scope.get("setting")) {
        (Some(_), None) => Ok("jurisdiction"),
        (None, Some(_)) => Ok("setting"),
        _ => Err("scope needs exactly one of jurisdiction or setting".into()),
    }
}

pub struct Facet {
    pub id: u32,
    pub name: String,
    pub performance: String,
    pub min_minutes: u32,
    /// Locator used to audit the breadth frame. Lesson claims need their own reviewed spans.
    pub source_url: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Calendar {
    total: u32,
    reserve: u32,
    session: u32,
}

impl Calendar {
    pub fn total_minutes(self) -> u32 {
        self.total
    }

    pub fn forward_minutes(self) -> u32 {
        self.total - self.reserve
    }

    /// Round protected time up when a learner shortens the current forecast window.
    pub fn forward_within(self, total: u32) -> Option<u32> {
        if total > self.total {
            return None;
        }
        let protected =
            (u64::from(total) * u64::from(self.reserve)).div_ceil(u64::from(self.total));
        Some(total - protected as u32)
    }
}

pub struct Frame {
    setting_key: &'static str,
    pub revision: u64,
    pub scope: Scope,
    pub facets: Vec<Facet>,
    pub calendar: Option<Calendar>,
}

#[derive(Clone, Debug)]
pub struct Unit {
    pub id: u32,
    pub facet: u32,
    pub title: String,
    pub performance: String,
    pub minutes: u32,
    pub requires: Vec<u32>,
}

pub struct Proposal {
    units: Vec<Unit>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Issue {
    DuplicateUnit(u32),
    UnknownFacet(u32),
    RepeatedPerformance(u32, u32),
    MissingFacet(u32),
    BelowFloor {
        facet: u32,
        proposed: u32,
        minimum: u32,
    },
    MissingPrerequisite {
        unit: u32,
        prerequisite: u32,
    },
    RepeatedPrerequisite(u32),
    Cycle,
    CalendarExceeded {
        requested: u32,
        available: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intake {
    AskForScope,
    FindOrBuildFrame,
    Confirmed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Schedule {
    pub scheduled: Vec<u32>,
    pub deferred: Vec<u32>,
    pub used_minutes: u32,
}

fn text(row: &Value, key: &str) -> Result<String, String> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 2048)
        .map(str::to_owned)
        .ok_or_else(|| format!("expected nonempty text of at most 2048 bytes for {key}"))
}

fn positive(val: &Value, field: &str) -> Result<u32, String> {
    val.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n != 0)
        .ok_or_else(|| format!("expected a positive 32-bit integer for {field}"))
}

fn minutes(row: &Value, field: &str) -> Result<u32, String> {
    let n = positive(&row[field], field)?;
    if n > MAX_MINUTES {
        return Err(format!("{field} exceeds {MAX_MINUTES}"));
    }
    Ok(n)
}

fn rows<'a>(obj: &'a Value, key: &str, cap: usize) -> Result<&'a [Value], String> {
    obj.get(key)
        .and_then(Value::as_array)
        .filter(|v| v.len() <= cap)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("expected an array of at most {cap} entries for {key}"))
}

impl Proposal {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        if value.as_object().is_none_or(|o| o.len() != 1) {
            return Err("expected exactly the units field".into());
        }
        let mut units = Vec::new();
        for row in rows(value, "units", MAX_UNITS)? {
            // Unknown fields cannot smuggle overrides of the release or scoring rules.
            if row.as_object().is_none_or(|o| o.len() != 6) {
                return Err("expected six fields per unit".into());
            }
            units.push(Unit {
                id: positive(&row["id"], "unit ID")?,
                facet: positive(&row["facet"], "facet ID")?,
                title: text(row, "title")?,
                performance: text(row, "performance")?,
                minutes: minutes(row, "minutes")?,
                requires: rows(row, "requires", MAX_PARENTS)?
                    .iter()
                    .map(|v| positive(v, "prerequisite ID"))
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(Self { units })
    }

    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    pub fn wire(&self) -> Value {
        let units: Vec<_> = self
            .units
            .iter()
            .map(|u| {
                json!({
                    "id":u.id,"facet":u.facet,"title":u.title,"performance":u.performance,
                    "minutes":u.minutes,"requires":u.requires
                })
            })
            .collect();
        json!({"units":units})
    }
}

impl Frame {
    /// Read only the independently supplied scope and facets from a fixture or trusted catalog.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let revision = value["revision"]
            .as_u64()
            .filter(|&n| n != 0)
            .ok_or("expected nonzero frame revision")?;
        let s = &value["scope"];
        let setting_key = setting_key(s)?;
        let scope = Scope {
            goal: text(s, "goal")?,
            purpose: text(s, "purpose")?,
            jurisdiction: text(s, setting_key)?,
            horizon: text(s, "horizon")?,
        };
        let mut facets = Vec::new();
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for f in rows(value, "facets", MAX_FACETS)? {
            let id = positive(&f["id"], "facet ID")?;
            let name = text(f, "name")?;
            if !ids.insert(id) || !names.insert(name.trim().to_lowercase()) {
                return Err("duplicate breadth obligation".into());
            }
            let source_url = f
                .get("source_url")
                .map(|_| text(f, "source_url"))
                .transpose()?;
            facets.push(Facet {
                id,
                name,
                performance: text(f, "performance")?,
                min_minutes: minutes(f, "min_minutes")?,
                source_url,
            });
        }
        if facets.is_empty() {
            return Err("the curriculum needs an independent breadth frame".into());
        }
        let calendar = if ["calendar_minutes", "reserve_minutes", "session_minutes"]
            .iter()
            .any(|k| value.get(k).is_some())
        {
            let total = positive(&value["calendar_minutes"], "total calendar minutes")?;
            let session = positive(&value["session_minutes"], "session minutes")?;
            let reserve = value["reserve_minutes"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or("invalid reserve minutes")?;
            if reserve > total || session > total {
                return Err("calendar reserve or session exceeds total".into());
            }
            Some(Calendar {
                total,
                reserve,
                session,
            })
        } else {
            None
        };
        Ok(Self {
            setting_key,
            revision,
            scope,
            facets,
            calendar,
        })
    }

    /// This fixture reads a confirmed intention; free text classification and elicitation
    /// are upstream. A different legal system or goal requires a different approved scope.
    pub fn intake(&self, stated_goal: &str, confirmation: Option<&Scope>) -> Intake {
        match confirmation {
            None => Intake::AskForScope,
            Some(scope) if scope == &self.scope && scope.goal == stated_goal => Intake::Confirmed,
            Some(_) => Intake::FindOrBuildFrame,
        }
    }

    pub fn scope_input(&self) -> Value {
        json!({"goal":self.scope.goal, "purpose":self.scope.purpose,
            (self.setting_key):self.scope.jurisdiction, "horizon":self.scope.horizon})
    }

    pub(super) fn setting_key(&self) -> &'static str {
        self.setting_key
    }

    pub fn model_input(&self) -> Value {
        let facets:Vec<_> = self.facets.iter().map(|f| json!({
            "id":f.id,"name":f.name,"performance":f.performance,"min_minutes":f.min_minutes,"breadth_source":f.source_url
        })).collect();
        json!({"schema":"breadth-plan/1","revision":self.revision,"scope":self.scope_input(),
            "facets":facets,"calendar":self.calendar.map(|c| json!({"total_minutes":c.total,"protected_minutes":c.reserve,"forward_minutes":c.forward_minutes(),"session_minutes":c.session})), "limits":{"units":MAX_UNITS,"prerequisites_per_unit":MAX_PARENTS,"minutes_per_unit":MAX_MINUTES}})
    }

    pub fn audit(&self, proposal: &Proposal) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut totals: BTreeMap<_, u32> = self.facets.iter().map(|f| (f.id, 0)).collect();
        let mut ids = BTreeSet::new();
        let mut descriptions = BTreeMap::new();
        let mut check_cycle = true;
        for u in proposal.units() {
            if !ids.insert(u.id) {
                issues.push(Issue::DuplicateUnit(u.id));
                check_cycle = false;
            }
            if let Some(sum) = totals.get_mut(&u.facet) {
                *sum += u.minutes;
            } else {
                issues.push(Issue::UnknownFacet(u.facet));
            }
            let words = u
                .performance
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            if let Some(prev) = descriptions.insert(words, u.id) {
                issues.push(Issue::RepeatedPerformance(prev, u.id));
            }
        }
        for f in &self.facets {
            match totals[&f.id] {
                0 => issues.push(Issue::MissingFacet(f.id)),
                n if n < f.min_minutes => issues.push(Issue::BelowFloor {
                    facet: f.id,
                    proposed: n,
                    minimum: f.min_minutes,
                }),
                _ => {}
            }
        }
        for u in proposal.units() {
            let mut seen = BTreeSet::new();
            for &p in &u.requires {
                if !ids.contains(&p) {
                    issues.push(Issue::MissingPrerequisite {
                        unit: u.id,
                        prerequisite: p,
                    });
                    check_cycle = false;
                } else if !seen.insert(p) {
                    issues.push(Issue::RepeatedPrerequisite(u.id));
                }
            }
        }
        if check_cycle && !allocate(proposal, u32::MAX).deferred.is_empty() {
            issues.push(Issue::Cycle);
        }
        issues
    }

    pub fn sequence(
        &self,
        proposal: &Proposal,
        available_minutes: u32,
    ) -> Result<Schedule, Vec<Issue>> {
        let mut issues = self.audit(proposal);
        if let Some(calendar) = self.calendar {
            let available = calendar.forward_minutes();
            if available_minutes > available {
                issues.push(Issue::CalendarExceeded {
                    requested: available_minutes,
                    available,
                });
            }
        }
        if !issues.is_empty() {
            return Err(issues);
        }
        Ok(allocate(proposal, available_minutes))
    }

    /// Select units whose scheduled instruction overlaps the upcoming half-open window.
    /// The forecast budget and elapsed cursor both use forward-instruction minutes.
    /// A partially taught unit stays eligible. Selection creates no evidence or approval.
    pub fn creation_window(
        &self,
        proposal: &Proposal,
        forecast_minutes: u32,
        elapsed_forward_minutes: u32,
        upcoming_forward_minutes: u32,
        max_jobs: usize,
    ) -> Result<Vec<u32>, Vec<Issue>> {
        let forecast = self.sequence(proposal, forecast_minutes)?;
        if upcoming_forward_minutes == 0 || elapsed_forward_minutes >= forecast.used_minutes {
            return Ok(Vec::new());
        }
        let end = elapsed_forward_minutes.saturating_add(upcoming_forward_minutes);
        let limit = max_jobs.min(MAX_ROWS);
        let durations: BTreeMap<_, _> =
            proposal.units().iter().map(|u| (u.id, u.minutes)).collect();
        let mut selected = Vec::new();
        let mut start = 0;
        for id in forecast.scheduled {
            if start >= end || selected.len() == limit {
                break;
            }
            let finish = start + durations[&id];
            if elapsed_forward_minutes < finish {
                selected.push(id);
            }
            start = finish;
        }
        Ok(selected)
    }

    /// The subject reviewer attests that the objectives mean what their facet tags claim,
    /// that required edges are defensible, and that the level fits the confirmed scope.
    /// A different order, rubric brief or scoped frame invalidates that decision.
    pub fn review_digest(&self, proposal: &Proposal) -> Document {
        document_id(
            json!(["program-review/1", self.model_input(), proposal.wire()])
                .to_string()
                .as_bytes(),
        )
    }

    /// Produce one immutable expansion brief only from a matching independent review.
    /// Input spans are supplied by the source broker; final rights/entailment are checked at release.
    pub fn commission(
        &self,
        proposal: &Proposal,
        reviewed: Document,
        id: u32,
        sources: &BTreeSet<Span>,
    ) -> Result<Work, String> {
        if self.review_digest(proposal) != reviewed || !self.audit(proposal).is_empty() {
            return Err("program has no matching approved review".into());
        }
        if sources.is_empty() || sources.len() > 16 || sources.iter().any(|s| s.start >= s.end) {
            return Err("expansion needs bounded, pinned source passages".into());
        }
        let unit = proposal
            .units()
            .iter()
            .find(|u| u.id == id)
            .ok_or("unknown unit")?;
        let source_manifest = source_manifest(sources);
        let facet = self
            .facets
            .iter()
            .find(|f| f.id == unit.facet)
            .expect("audited facet");
        let parents: Vec<_> = unit
            .requires
            .iter()
            .map(|id| {
                let parent = proposal
                    .units()
                    .iter()
                    .find(|u| u.id == *id)
                    .expect("audited edge");
                json!([parent.id, parent.performance])
            })
            .collect();
        // Re-review the whole outline above. Preserve work identity when an unrelated
        // sibling is edited within the same release. Only semantic inputs travel to authoring.
        let brief = document_id(
            json!([
                "unit-expansion/1",
                self.revision,
                self.scope_input(),
                [facet.id, &facet.performance],
                unit.id,
                unit.title,
                unit.performance,
                unit.minutes,
                parents,
                source_manifest
            ])
            .to_string()
            .as_bytes(),
        );
        Ok(Work {
            release: self.revision,
            objective: OutcomeId(unit.id),
            brief,
            source_manifest,
        })
    }
}

/// Prefer a first encounter with an untouched facet among units that fit and whose
/// prerequisites have already been *scheduled*. Proposal order breaks ties. This
/// is a bounded planning forecast. Delivery needs the learner's current readiness.
fn allocate(proposal: &Proposal, budget: u32) -> Schedule {
    let mut placed = BTreeSet::new();
    let mut facets = BTreeSet::new();
    let mut schedule = Schedule {
        scheduled: Vec::new(),
        deferred: Vec::new(),
        used_minutes: 0,
    };
    loop {
        let next = proposal
            .units()
            .iter()
            .filter(|u| {
                !placed.contains(&u.id)
                    && u.minutes <= budget - schedule.used_minutes
                    && u.requires.iter().all(|p| placed.contains(p))
            })
            .min_by_key(|u| facets.contains(&u.facet));
        let Some(unit) = next else {
            break;
        };
        schedule.used_minutes += unit.minutes;
        schedule.scheduled.push(unit.id);
        placed.insert(unit.id);
        facets.insert(unit.facet);
    }
    schedule.deferred = proposal
        .units()
        .iter()
        .filter(|u| !placed.contains(&u.id))
        .map(|u| u.id)
        .collect();
    schedule
}
