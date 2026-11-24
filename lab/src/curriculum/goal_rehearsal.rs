//! Rehearse an independently pinned goal, two published forms and one learner trajectory.
//! The fixture carries simulated choices and human observations. An optional supplied
//! answer goes through the actual observation channel without inheriting those reviews.
use super::{
    author_input::AuthorInput,
    creation::Pack,
    discovery,
    goal_evidence::{delayed, Evidence, Presentation, Spacing, Task},
    program::{Frame, Intake, Proposal},
    rehearsal_release::{publish, PublishedLesson},
    OutcomeId,
};
use crate::{
    history::{Scope as LearnerScope, Withdrawn},
    material::document_id,
    protocol::{transition, Assistance, Effect, Event, Gate, Judgment, Question, State},
    retention::{Cell, CheckResult, Exposure, Moment},
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum View {
    #[default]
    Audit,
    Learner,
    Opening,
    Intake,
    AuthorFirst,
    AuthorReturn,
    AuthorUnit,
}

#[derive(Default, Clone, Copy)]
pub enum Branch {
    #[default]
    Demonstrated,
    Revisit,
    Unobserved,
}

impl Branch {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "demonstrated" => Ok(Self::Demonstrated),
            "revisit" => Ok(Self::Revisit),
            "unobserved" => Ok(Self::Unobserved),
            _ => Err(format!("unknown trajectory {value}")),
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Demonstrated => "demonstrated",
            Self::Revisit => "revisit",
            Self::Unobserved => "unobserved",
        }
    }
}

#[derive(Clone)]
pub struct Options {
    pub view: View,
    pub unit: Option<u32>,
    pub first: Branch,
    pub returning: Branch,
    pub return_day: u32,
    pub practice_day: Option<u32>,
    pub withdraw_return: bool,
    pub unknown_history: bool,
    pub changed_setting: Option<String>,
    pub first_response: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            view: View::Audit,
            unit: None,
            first: Branch::Demonstrated,
            returning: Branch::Demonstrated,
            return_day: 28,
            practice_day: None,
            withdraw_return: false,
            unknown_history: false,
            changed_setting: None,
            first_response: None,
        }
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|x| !x.trim().is_empty())
        .ok_or_else(|| format!("missing scenario text for {key}"))
}

pub(super) fn source(record: &Value) -> Result<String, String> {
    Ok(format!(
        "“{}”\n{}\n{}",
        text(record, "excerpt")?,
        text(record, "locator")?,
        text(record, "source_url")?
    ))
}

fn apply(state: &mut State, event: Event) -> Result<(), String> {
    let (next, effect) = transition(*state, event);
    if matches!(effect, Effect::Withheld | Effect::Rejected) {
        return Err(format!(
            "invalid rehearsal transition from {:?} on {event:?}",
            state.phase
        ));
    }
    *state = next;
    Ok(())
}

fn teach(
    state: &mut State,
    pack: &Pack,
    record: &Value,
    lines: &mut Vec<String>,
) -> Result<(), String> {
    apply(state, Event::Teach(Gate::Supported))?;
    lines.push(format!(
        "Day 0 · Lesson\n{}\n{}\nWorked example\n{}",
        pack.lesson,
        source(record)?,
        pack.worked
    ));
    apply(state, Event::Ask(Question::Practice, Gate::Supported))?;
    lines.push(format!(
        "Guided practice\n{}\nAssisted response (scripted)\n{}",
        pack.practice,
        text(record, "practice_response")?
    ));
    apply(
        state,
        Event::Answer(Judgment::Correct, Assistance::Reported),
    )?;
    apply(state, Event::Feedback(Gate::Supported))?;
    lines.push(text(record, "practice_feedback")?.into());
    Ok(())
}

struct Visit<'a> {
    record: &'a Value,
    material: &'a Pack,
    presentation: Presentation,
    question: Question,
    branch: Branch,
}

struct Trajectory<'a> {
    cell: Cell,
    mode: &'a str,
    claim: &'a str,
    state: State,
    lines: Vec<String>,
}

impl Trajectory<'_> {
    fn open(&mut self, question: Question, day: u32, material: &Pack) -> Result<(), String> {
        apply(&mut self.state, Event::Ask(question, Gate::Supported))?;
        self.lines.push(format!(
            "Independent exercise · day {day}\n{}",
            material.check
        ));
        Ok(())
    }

    fn check(&mut self, visit: Visit<'_>, input: Option<&str>) -> Result<Evidence, String> {
        let p = visit.presentation;
        self.open(visit.question, p.at.day, visit.material)?;
        let task = Task::from_pack(
            self.cell,
            visit.material.digest(),
            self.mode,
            self.claim,
            visit.record,
        )?
        .with_published_sources(visit.material)?;
        let (evidence, feedback) = if let Some(supplied) = input {
            unreviewed(&task, self.mode, p, supplied)
        } else {
            let row = &visit.record["attempts"][visit.branch.key()];
            let label = if self.mode == "witnessed_performance"
                && !matches!(visit.branch, Branch::Unobserved)
            {
                "Simulated teacher observation (with permission)"
            } else {
                "Scripted response"
            };
            self.lines
                .push(format!("{label}\n{}", text(row, "response")?));
            (
                task.scripted(p, visit.branch.key(), 7)?,
                text(row, "feedback")?,
            )
        };
        apply(
            &mut self.state,
            Event::Answer(evidence.judgment(), p.assistance),
        )?;
        apply(&mut self.state, Event::Feedback(Gate::Supported))?;
        self.lines.push(feedback.into());
        Ok(evidence)
    }
}

fn unreviewed(
    task: &Task<'_>,
    mode: &str,
    p: Presentation,
    supplied: &str,
) -> (Evidence, &'static str) {
    let e = task.unreviewed_text(p, supplied);
    let reply = match (mode, e.judgment()) {
                ("quantity", Judgment::Correct) => "Your number and unit match the requested quantity.",
                ("quantity", Judgment::Incorrect) => {
                    "Recheck the given information and requested unit. You can try the worked example again."
                }
                ("quantity", _) => {
                    "A signed whole number and its unit are needed to check this result. You can ask for help."
                }
                ("witnessed_performance", _) => {
                    "A text description can guide practice. This check needs a performance you choose to have witnessed."
                }
                _ => {
                    "You can keep this exercise private. A written exercise you choose to share would need a reviewer before it could support this criterion."
                }
            };
    (e, reply)
}

fn describe_return(result: CheckResult) -> &'static str {
    match result {
        CheckResult::SupportsDurability => {
            "The two exercises support a delayed observation of this specific skill under the recorded conditions."
        }
        CheckResult::InsufficientSpacing => {
            "The fresh exercise supplies evidence about today. Too little time has passed since the last recorded exposure for a delayed finding."
        }
        CheckResult::UnknownExposure => {
            "This exercise supplies evidence about today. The time since the last relevant practice is unknown."
        }
        CheckResult::OpposesDurability | CheckResult::ReviewFailure => {
            "Reopen this particular skill for practice and offer a different exercise on a later visit."
        }
        CheckResult::Ineligible | CheckResult::NoCheck => {
            "The delayed question remains open. This visit adds no independent delayed finding."
        }
    }
}

pub(super) struct Course<'a> {
    pub(super) data: &'a Value,
    pub(super) published: BTreeMap<u32, &'a Value>,
    pub(super) frame: Frame,
    pub(super) plan: Proposal,
    pub(super) review: super::Document,
}

impl Course<'_> {
    pub(super) fn release(&self, key: &str) -> Result<PublishedLesson, String> {
        publish(&self.frame, &self.plan, self.review, &self.data[key])
    }

    pub(super) fn cell(&self, unit: u32) -> Cell {
        // Outcome IDs are local to a release. Distinguish these synthetic enrollments even
        // where two courses both call their first unit 101 and their fixture revision 1.
        let scope = LearnerScope {
            learner: 10,
            revision: u64::from_be_bytes(self.review[..8].try_into().expect("SHA-256 prefix")),
        };
        Cell {
            scope,
            outcome: OutcomeId(unit),
        }
    }

    pub(super) fn first_grade(
        &self,
        answer: &str,
        guided: bool,
    ) -> Result<(Evidence, &'static str), String> {
        let release = self.release("first_pack")?;
        let pack = release.material(true)?;
        let mode = text(self.data, "evidence_mode")?;
        let task = Task::from_pack(
            self.cell(release.unit()),
            pack.digest(),
            mode,
            text(self.data, "criterion_claim")?,
            &self.data["first_pack"],
        )?
        .with_published_sources(pack)?;
        let p = Presentation {
            attempt: 1,
            at: Moment {
                day: 0,
                sequence: 1,
            },
            fresh: !guided,
            assistance: if guided {
                Assistance::Reported
            } else {
                Assistance::NoneReported
            },
        };
        Ok(unreviewed(&task, mode, p, answer))
    }

    fn follow(&self, options: &Options, mut lines: Vec<String>) -> Result<String, String> {
        let opening = self.release("first_pack")?;
        let pack = opening.material(true)?;
        let initial = &self.data["first_pack"];
        let cell = self.cell(opening.unit());
        let scope = cell.scope;
        let mode = text(self.data, "evidence_mode")?;
        let claim = text(self.data, "criterion_claim")?;
        if options.view == View::Audit {
            lines.push(format!("Prepared unit {} · {} author-request bytes. Evidence channel {mode}.\nShared criterion · {claim}",opening.unit(),opening.request().len()));
        }
        let mut path = Trajectory {
            cell,
            mode,
            claim,
            state: State::default(),
            lines,
        };
        teach(&mut path.state, pack, initial, &mut path.lines)?;
        let p = |attempt, day, sequence| Presentation {
            attempt,
            at: Moment { day, sequence },
            fresh: true,
            assistance: Assistance::NoneReported,
        };
        if options.view == View::Opening {
            path.open(Question::IndependentCheck, 0, pack)?;
            return Ok(path.lines.join("\n"));
        }
        let first = path.check(
            Visit {
                record: initial,
                material: pack,
                presentation: p(1, 0, 1),
                question: Question::IndependentCheck,
                branch: options.first,
            },
            options.first_response.as_deref(),
        )?;
        let active = Withdrawn::new(scope);
        if !first.opens_transfer(&active) {
            if first.judgment() == Judgment::Incorrect {
                apply(&mut path.state, Event::Teach(Gate::Supported))?;
                path.lines
                    .push(format!("Revisit the worked example\n{}", pack.worked));
            }
            path.lines.push(
                "The later exercise stays open. You can pause or choose what to practise next."
                    .into(),
            );
            return Ok(path.lines.join("\n"));
        }

        if self.data["return_pack"].is_null() {
            path.lines.push("The opening exercise is complete. A fresh return exercise for this skill still needs material and subject review. You can save a question for that visit or choose another planned unit.".into());
            return Ok(path.lines.join("\n"));
        }

        let mut exposures = Vec::new();
        if let Some(day) = options.practice_day {
            // Extra instruction is a separate attended visit. Resume from the dated first
            // observation for the subsequent changed-example check; record the exposure.
            let mut practice_visit = State::default();
            apply(&mut practice_visit, Event::Teach(Gate::Supported))?;
            path.lines.push(format!(
                "Day {day} · Additional guided review\n{}",
                pack.worked
            ));
            exposures.push(Exposure {
                cell,
                at: Moment { day, sequence: 2 },
            });
        }
        let later = self.release("return_pack")?;
        if options.withdraw_return {
            if later.material(false).is_ok() {
                return Err("withdrawn source was available".into());
            }
            path.lines.push(format!("Day {} · Permission to use the passage for this return exercise has been withdrawn. Hold the exercise for a replacement. Your first observation stays on record.",options.return_day));
            return Ok(path.lines.join("\n"));
        }
        let transfer = later.material(true)?;
        let record = &self.data["return_pack"];
        path.lines.push(format!(
            "Simulated return visit · day {}. A different task checks the same criterion.",
            options.return_day
        ));
        if record["show_source_during_check"]
            .as_bool()
            .ok_or("missing return source-display policy")?
        {
            path.lines.push(source(record)?);
        }
        let second = path.check(
            Visit {
                record,
                material: transfer,
                presentation: p(2, options.return_day, 3),
                question: Question::ChangedExample,
                branch: options.returning,
            },
            None,
        )?;
        let result = delayed(
            &first,
            &second,
            (!options.unknown_history).then_some(exposures.as_slice()),
            &active,
            Spacing {
                stability_days: 14.0,
                threshold: 0.85,
                target_day: None,
            },
        )
        .map_err(|e| format!("invalid exposure history: {e:?}"))?;
        if options.view == View::Audit {
            let ledger = if options.unknown_history {
                "earlier exposure history is incomplete"
            } else if exposures.is_empty() {
                "the scripted history contains no intervening exposure to this criterion"
            } else {
                "the scripted history includes the additional guided review"
            };
            path.lines
                .push(format!("Return evidence · {result:?} ({ledger})."));
        }
        path.lines.push(describe_return(result).into());
        if second.judgment() == Judgment::Incorrect {
            apply(&mut path.state, Event::Teach(Gate::Supported))?;
            path.lines
                .push(format!("Worked return example\n{}", transfer.worked));
        }
        path.lines.push(text(self.data, "limit")?.into());
        Ok(path.lines.join("\n"))
    }
}

enum Read<'a> {
    Author(&'a mut dyn FnMut(&AuthorInput) -> Result<String, String>),
    Rehearsal,
    OpeningCheck,
    OpeningFeedback {
        answer: &'a str,
        guided: bool,
    },
    CoachingContext {
        input: CoachingInput<'a>,
        guided: bool,
        output: &'a mut Option<OpeningCoachingContext>,
    },
}

/// Open an interactive encounter once. The flag is true exactly when this call displayed
/// an independent question. A learner can use the workshop in either state.
pub fn interactive_opening(
    raw: &str,
    expected_hex: &str,
    options: &Options,
) -> Result<(String, bool), String> {
    let options = Options {
        view: View::Opening,
        ..options.clone()
    };
    run_read(raw, expected_hex, &options, Read::Rehearsal)
}

/// Read only the first independent question for an already opened session. The
/// session host decides whether a learner may resume or has previously seen the form.
pub fn opening_check(raw: &str, expected_hex: &str, options: &Options) -> Result<String, String> {
    run_read(raw, expected_hex, options, Read::OpeningCheck).map(|(text, _)| text)
}

/// Respond to one actual answer. A private draft or self-report carries no simulated
/// reviewer authority. The read does not attempt a later exercise or record progress.
pub fn opening_feedback(
    raw: &str,
    expected_hex: &str,
    options: &Options,
    answer: &str,
    guided: bool,
) -> Result<String, String> {
    if answer.len() > 4096 {
        return Err("opening response exceeds 4096 bytes".into());
    }
    run_read(
        raw,
        expected_hex,
        options,
        Read::OpeningFeedback { answer, guided },
    )
    .map(|(text, _)| text)
}

pub(super) enum CoachingInput<'a> {
    Answer(&'a str),
    Help(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OpeningCoachingContext {
    /// Decision inputs. Replaying a released card needs no card text on the wire.
    pub state: Value,
    pub worked: String,
    pub source: String,
    pub excerpt: String,
}

/// Project the learner's request or answer and the already released cards for
/// this opening. The private rubric, reference answers and future forms stay out.
pub(super) fn opening_coaching_context(
    raw: &str,
    expected_hex: &str,
    options: &Options,
    input: CoachingInput<'_>,
    guided: bool,
) -> Result<OpeningCoachingContext, String> {
    let text = match input {
        CoachingInput::Answer(text) | CoachingInput::Help(text) => text,
    };
    if text.trim().is_empty() || text.len() > 4096 {
        return Err("coaching needs a nonempty input of at most 4096 bytes".into());
    }
    let mut output = None;
    run_read(
        raw,
        expected_hex,
        options,
        Read::CoachingContext {
            input,
            guided,
            output: &mut output,
        },
    )?;
    output.ok_or_else(|| "missing released opening".into())
}

/// Process the same verified job as the offline author-request view. The callback
/// is reached only after the current scope, outline, objective and source have passed.
pub fn with_author_input(
    raw: &str,
    expected_hex: &str,
    options: &Options,
    mut process: impl FnMut(&AuthorInput) -> Result<String, String>,
) -> Result<String, String> {
    if !matches!(
        options.view,
        View::AuthorFirst | View::AuthorReturn | View::AuthorUnit
    ) {
        return Err("select an authoring request".into());
    }
    run_read(raw, expected_hex, options, Read::Author(&mut process)).map(|(text, _)| text)
}

/// Exercise one local edition. Its SHA-256 is recorded separately from the material.
pub fn run(raw: &str, expected_hex: &str, options: &Options) -> Result<String, String> {
    run_read(raw, expected_hex, options, Read::Rehearsal).map(|(text, _)| text)
}

fn run_read(
    raw: &str,
    expected_hex: &str,
    options: &Options,
    read: Read<'_>,
) -> Result<(String, bool), String> {
    let digest = document_id(raw.as_bytes());
    let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected_hex {
        return Err("scenario differs from its pinned simulated review".into());
    }
    if options.return_day == 0
        || options
            .practice_day
            .is_some_and(|day| day == 0 || day > options.return_day)
    {
        return Err(
            "use a return after day zero and an optional practice day between the two visits"
                .into(),
        );
    }
    if options
        .first_response
        .as_ref()
        .is_some_and(|v| v.len() > 4096)
    {
        return Err("opening response exceeds 4096 bytes".into());
    }
    let data: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let rehearsal = matches!(read, Read::Rehearsal);
    let author = matches!(
        options.view,
        View::AuthorFirst | View::AuthorReturn | View::AuthorUnit
    );
    if options.view == View::AuthorUnit && options.unit.is_none() {
        return Err("authoring a selected unit requires its planned unit ID".into());
    }
    if data["scope"].is_null()
        || data["facets"].is_null()
        || data["proposal"].is_null()
        || data["proposal"]["units"]
            .as_array()
            .is_some_and(Vec::is_empty)
    {
        if !rehearsal || author {
            return Err("a reviewed outline is required".into());
        }
        return Ok((
            format!(
                "There is no reviewed outline for this goal yet.\n{}",
                super::availability::orientation()
            ),
            false,
        ));
    }
    let frame = Frame::from_json(&data)?;
    let goal = &frame.scope.goal;
    if frame.intake(goal, None) != Intake::AskForScope {
        return Err("a broad goal needs scope confirmation".into());
    }
    let mut lines = vec![
        format!("Learner · {goal}"),
        format!("Tutor · {}", text(&data, "intake_question")?),
    ];
    if rehearsal && options.view == View::Intake {
        return Ok((lines.join("\n"), false));
    }
    let mut confirmed = frame.scope.clone();
    if let Some(setting) = &options.changed_setting {
        confirmed.jurisdiction = setting.clone();
    }
    if frame.intake(goal, Some(&confirmed)) != Intake::Confirmed {
        if author || !rehearsal {
            return Err(
                "the changed setting needs a reviewed outline before opening material".into(),
            );
        }
        lines.push(format!("Proposed setting · {}\nChoose a new scope and rebuild the map together before assigning a lesson.",confirmed.jurisdiction));
        lines.push(super::availability::orientation().into());
        return Ok((lines.join("\n"), false));
    }
    lines.push(format!(
        "Learner (simulated choice) · {}\nAgreed horizon · {}",
        text(&data, "scope_reply")?,
        confirmed.horizon
    ));

    let mut brief = data["discovery"].clone();
    brief["goal"] = data["scope"]["goal"].clone();
    brief["scope"] = data["scope"].clone();
    let report = discovery::rehearse(&brief, &frame)?;
    if !report.ready_for_editorial_review() {
        return Err("unresolved gaps in the scope proposal".into());
    }
    let plan = Proposal::from_json(&data["proposal"])?;
    let calendar = frame.calendar.ok_or("missing course calendar")?;
    let schedule = frame
        .sequence(&plan, calendar.forward_minutes())
        .map_err(|e| format!("outline held: {e:?}"))?;
    if !schedule.deferred.is_empty() {
        return Err("the whole reviewed survey does not fit its stated horizon".into());
    }
    let review = frame.review_digest(&plan);
    let published = super::availability::catalog(&data, &plan)?;
    let course = Course {
        data: &data,
        published,
        frame,
        plan,
        review,
    };
    let frame = &course.frame;
    let plan = &course.plan;
    let first = plan
        .units()
        .iter()
        .find(|u| Some(&u.id) == schedule.scheduled.first())
        .ok_or("the outline has no scheduled opening")?;
    let has_opening = !data["first_pack"].is_null();
    let has_return = !data["return_pack"].is_null();
    if !first.requires.is_empty()
        || (has_opening && data["first_pack"]["unit"].as_u64() != Some(u64::from(first.id)))
    {
        return Err("the published opening must match the first scheduled unit".into());
    }
    if has_return {
        let later = plan
            .units()
            .iter()
            .find(|u| Some(u64::from(u.id)) == data["return_pack"]["unit"].as_u64())
            .ok_or("the return pack has no planned unit")?;
        if later.requires != [first.id] || first.facet != later.facet || first.id == later.id {
            return Err(
                "the rehearsed transfer needs the reviewed opening skill and its own planned form"
                    .into(),
            );
        }
    }
    if author && matches!(read, Read::Rehearsal | Read::Author(_)) {
        let id = match (options.view, options.unit) {
            (View::AuthorFirst, None) => first.id,
            (View::AuthorReturn, None) if has_return && !options.withdraw_return => {
                data["return_pack"]["unit"]
                    .as_u64()
                    .expect("validated return") as u32
            }
            (View::AuthorReturn, None) => return Err(
                "author export requires an identified return task with current source permission"
                    .into(),
            ),
            (View::AuthorUnit, Some(id)) => id,
            _ => return Err("choose the selected-unit author view for an explicit unit ID".into()),
        };
        let input = super::availability::author_input(&course, id, options)?;
        let out = if let Read::Author(process) = read {
            process(&input)?
        } else {
            input.wire().to_owned()
        };
        return Ok((out, false));
    }
    if !rehearsal {
        if !has_opening || options.unit.is_some_and(|id| id != first.id) {
            return Err("the released opening does not match the requested first unit".into());
        }
        let text = match read {
            Read::OpeningCheck => course.release("first_pack")?.material(true)?.check.clone(),
            Read::OpeningFeedback { answer, guided } => {
                let (_, feedback) = course.first_grade(answer, guided)?;
                if guided {
                    format!("Guided practice\n{feedback}")
                } else {
                    feedback.into()
                }
            }
            Read::CoachingContext {
                input,
                guided,
                output,
            } => {
                let release = course.release("first_pack")?;
                let pack = release.material(true)?;
                let mut state = serde_json::json!({
                    "goal": frame.scope.goal,
                    "performance": first.performance,
                    "exercise": pack.check,
                    "mode": text(&data, "evidence_mode")?,
                    "guided": guided
                });
                match input {
                    CoachingInput::Answer(answer) => {
                        let (_, feedback) = course.first_grade(answer, guided)?;
                        state["submitted_answer"] = Value::from(answer);
                        state["local_feedback"] = Value::from(feedback);
                    }
                    CoachingInput::Help(request) => state["help_request"] = Value::from(request),
                }
                *output = Some(OpeningCoachingContext {
                    worked: pack.worked.clone(),
                    source: source(&data["first_pack"])?,
                    excerpt: text(&data["first_pack"], "excerpt")?.to_owned(),
                    state,
                });
                String::new()
            }
            Read::Rehearsal | Read::Author(_) => unreachable!(),
        };
        return Ok((text, false));
    }
    if let Some(id) = options.unit {
        return Ok((super::availability::lookup(&course, id, options)?, false));
    }
    let names = frame
        .facets
        .iter()
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!(
        "Your plan · {} units across {} strands\n{names}",
        plan.units().len(),
        frame.facets.len()
    ));
    if options.view == View::Audit {
        lines.push(format!("Discovery · {} source cards. Strands missing from the first draft: {}. After repair: {}.",report.source_count,report.initial_missing.len(),report.repaired_missing.len()));
        for (id, question) in &report.initial_challenges {
            lines.push(format!(
                "Independent coverage challenge · {} · {question}",
                report.candidate[id]
            ));
        }
        lines.push(format!(
            "Budget · {} forward minutes allocated, {} minutes protected out of {}.",
            schedule.used_minutes,
            calendar.total_minutes() - calendar.forward_minutes(),
            calendar.total_minutes()
        ));
        for id in &schedule.scheduled {
            let u = plan
                .units()
                .iter()
                .find(|u| u.id == *id)
                .expect("audited unit");
            lines.push(format!(
                "  {} · {} ({} min) · {}",
                u.id, u.title, u.minutes, u.performance
            ));
        }
        let count = course.published.len();
        lines.push(format!(
            "Reviewed packs for {count} units. {} further units have outlines.",
            plan.units().len() - count
        ));
    }
    if !has_opening {
        lines.push(super::availability::lookup(&course, first.id, options)?);
        return Ok((lines.join("\n"), false));
    }
    Ok((
        course.follow(options, lines)?,
        options.view == View::Opening,
    ))
}
