//! A reproducible, single-learner course rehearsal. Current authority, prior
//! contact and assessment history are re-evaluated on every return.
mod case_supply;
pub mod diagnostic;
mod forecast;
pub mod goal_choice;
mod instances;
pub mod journal;
mod private_practice;
mod reply;
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    context::{self, Authority, Purpose},
    curriculum::{Decision, Outcome, OutcomeId, ReviewedUnit, Slot, Span},
    history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{Assistance, Judgment, Question},
    routing::{route_local, Action as Route, Intent, LocalGrant, Need, Options, Request, Reviewed},
    supply::{select, Contact, Enrollment, ExposureBook, Form, Requirement},
    workshop::{course::CourseTurn, Focus, Workshop},
};
use serde_json::{json, Value};

const A: OutcomeId = OutcomeId(1);
const B: OutcomeId = OutcomeId(2);
const SEAT: Enrollment = Enrollment {
    scope: Scope {
        learner: 42,
        revision: 1,
    },
    class: 7,
};
const OBLIGATION: &str = "42/7/1/B/2";
pub const FORMAT: &str = "longitudinal-rehearsal/1";

/// The file adapter writes the returned event. A workshop event describes only
/// the delivered assistance. The private draft and feedback stay in the response.
pub enum Action {
    Open(u32),
    Resume(u32),
    Work(String),
    Answer(String, Assistance),
    Reply(String, Assistance),
    CourseAid(String),
    WithdrawSource,
    ReplaceSource,
    RevokeGrant,
    ReissueGrant,
    UnknownContact,
    RecoverContact,
    UseVariedInstances,
    Diagnostic,
    DiagnosticAnswer(String, Assistance),
    DiagnosticHelp,
    DiagnosticSkip,
    Goal(Vec<String>),
    CaseReview(String),
    GoalWriting(u32, String),
    PracticeCase(String),
    PracticeNext(String),
    PracticeVisit(u32),
}
fn assistance(a: Assistance) -> &'static str {
    match a {
        Assistance::NoneReported => "none",
        Assistance::Reported => "reported",
        Assistance::Unknown => "unknown",
    }
}
fn read_assistance(v: &Value) -> Result<Assistance, String> {
    match v.as_str() {
        Some("none") => Ok(Assistance::NoneReported),
        Some("reported") => Ok(Assistance::Reported),
        Some("unknown") => Ok(Assistance::Unknown),
        _ => Err("invalid assistance on recorded response".into()),
    }
}

struct Source {
    bytes: Vec<u8>,
    pin: Span,
}
impl Source {
    fn new(edition: u32) -> Self {
        let bytes = format!("Reviewed arithmetic edition {edition}\n{UNIT_RULE}").into_bytes();
        let pin = Span {
            document: document_id(&bytes),
            start: (bytes.len() - UNIT_RULE.len()) as u32,
            end: bytes.len() as u32,
        };
        Self { bytes, pin }
    }
    fn live<'a>(&'a self, withdrawn: &'a BTreeSet<[u8; 32]>) -> RuleSource<'a> {
        RuleSource {
            bytes: Some(&self.bytes),
            pin: self.pin,
            revoked: withdrawn,
        }
    }
}
#[derive(Clone)]
struct Opening {
    form: Form,
    epoch: u64,
    day: u32,
    course: CourseTurn,
    independent: bool,
}
#[derive(Clone)]
pub struct Session {
    revision: u64,
    day: u32,
    source_revision: u32,
    source_active: bool,
    grant: bool,
    epoch: u64,
    retired: BTreeSet<[u8; 32]>,
    complete: bool,
    contacts: BTreeMap<u64, Form>,
    return_contacts: forecast::ReturnContacts,
    varied_instances: bool,
    history: Vec<Attempt>,
    proposals: Vec<CheckpointProposal>,
    unit: std::sync::Arc<ReviewedUnit>,
    opening: Option<Opening>,
    parked: bool,
    coached_day: Option<u32>,
    last_independent: Option<(OutcomeId, Judgment, u64)>,
    diagnostic: diagnostic::Diagnostic,
    goal: goal_choice::Choice,
    private_practice: private_practice::SavedExercise,
}

fn form(target: OutcomeId, pin: Span, revision: u64, task: u64, first: u16, second: u16) -> Form {
    Form {
        class: SEAT.class,
        release: SEAT.scope.revision,
        outcome: target,
        owner: None,
        revision,
        task,
        family: task,
        source: pin,
        card: FractionCard::propose(
            if target == A {
                FractionTask::Sum
            } else {
                FractionTask::MissingPart
            },
            first,
            second,
            16,
        )
        .expect("valid arithmetic rehearsal form"),
    }
}

impl Session {
    pub fn new() -> Result<Self, String> {
        let base = Source::new(0);
        let unit = ReviewedUnit::build(
            SEAT.scope.revision,
            vec![
                Outcome {
                    id: A,
                    requires: BTreeSet::new(),
                    support: [base.pin].into(),
                },
                Outcome {
                    id: B,
                    requires: [A].into(),
                    support: [base.pin].into(),
                },
            ],
        )
        .map_err(|e| format!("{e:?}"))?;
        let mut history = Vec::new();
        let mut proposals = Vec::new();
        let empty = BTreeSet::new();
        for (outcome, offset, due) in [(A, 10, 10000), (B, 20, 21)] {
            for (i, question) in [(1, Question::Practice), (2, Question::ChangedExample)] {
                let original = form(outcome, base.pin, offset + i, offset + i, i as u16, 2);
                let answer = if outcome == A { i + 2 } else { 2 };
                let judgment = original
                    .card
                    .assess(&format!("{answer}/16"), &base.live(&empty))
                    .ok_or("unavailable fixture source")?;
                // The four admitted prehistory receipts have known independent,
                // fresh presentations. Their numeric contents go through the checker.
                history.push(Attempt {
                    scope: SEAT.scope,
                    id: offset + i,
                    sequence: offset + i,
                    outcome,
                    item: original.revision,
                    day: 0,
                    question,
                    judgment,
                    assistance: Assistance::NoneReported,
                    fresh: true,
                    sources: [base.pin.document].into(),
                });
            }
            proposals.push(CheckpointProposal {
                scope: SEAT.scope,
                outcome,
                first: offset + 1,
                confirmation: offset + 2,
                due,
            });
        }
        Ok(Self {
            revision: 0,
            day: 30,
            source_revision: 1,
            source_active: true,
            grant: true,
            epoch: 1,
            complete: true,
            retired: BTreeSet::new(),
            contacts: BTreeMap::new(),
            return_contacts: forecast::ReturnContacts::from_day(30),
            varied_instances: false,
            history,
            proposals,
            unit: std::sync::Arc::new(unit),
            opening: None,
            parked: false,
            coached_day: None,
            last_independent: None,
            diagnostic: Default::default(),
            goal: Default::default(),
            private_practice: private_practice::SavedExercise::default(),
        })
    }
    fn progress(&self) -> Result<crate::curriculum::Progress, String> {
        let mut withdrawn = Withdrawn::new(SEAT.scope);
        withdrawn.sources = self.retired.clone();
        reconcile(
            SEAT.scope,
            self.day,
            &self.history,
            &self.proposals,
            &withdrawn,
        )
        .map_err(|e| format!("{e:?}"))
    }
    fn target_for(&self, progress: &crate::curriculum::Progress) -> Result<Decision, String> {
        self.unit
            .next(B, progress, self.day, &BTreeSet::new(), Slot::Review)
            .map_err(|e| format!("{e:?}"))
    }
    fn target(&self) -> Result<Decision, String> {
        self.target_for(&self.progress()?)
    }
    fn selected_id(decision: Decision) -> Option<OutcomeId> {
        match decision {
            Decision::Teach(id) | Decision::Recheck(id) => Some(id),
            _ => None,
        }
    }
    pub fn status(&self) -> Result<Value, String> {
        let progress = self.progress()?;
        let target = self.target_for(&progress)?;
        let checkpoint = progress
            .checkpoints
            .get(&B)
            .map(|c| json!({"observed":c.observed,"due":c.due}));
        let opening=self.opening.as_ref().map(|o|json!({"task":o.form.task,"day":o.day,"epoch":o.epoch,"current":o.epoch==self.epoch && self.source_active && self.grant,"practice_only":!o.independent || o.course.practice_only()}));
        let mut status = json!({"revision":self.revision,"day":self.day,"obligation":OBLIGATION,"learner":SEAT.scope.learner,"class":SEAT.class,
            "target":Self::selected_id(target).map(|id|id.0),"planner":format!("{target:?}"),"pending":!matches!(target,Decision::CurrentUntil(_)),
            "source_revision":self.source_revision,"source_active":self.source_active,"varied_instances":self.varied_instances,"grant":self.grant,"authority_epoch":self.epoch,
            "checkpoint":checkpoint,"current_opening":opening,"parked":self.parked,"optional_diagnostic":self.diagnostic.view(),"exposure_complete":self.complete,"contacted_tasks":self.contacts.keys().collect::<Vec<_>>(),
            "display":Value::Null,"opened":false,"contact":Value::Null,"contribution":"none","credit":Value::Null});
        if let Some(goal) = self.goal.report() {
            status["goal_choice"] = goal;
        }
        Ok(status)
    }
    fn catalog(&self, target: OutcomeId, source: &Source) -> Vec<Form> {
        let revision = u64::from(self.source_revision) * 100;
        let key = if target == A { 1000 } else { 0 };
        let mut forms = vec![form(target, source.pin, revision + 1, key + 1, 3, 2)];
        if self.source_revision >= 2 {
            forms.push(form(target, source.pin, revision + 2, key + 2, 3, 5));
        }
        if self.varied_instances {
            instances::extend(&mut forms, target, source, self.source_revision);
        }
        forms
    }
    fn permits(&self, selected: Form, intent: Intent) -> Result<bool, String> {
        if !self.grant || !self.source_active {
            return Ok(false);
        }
        let purpose = match intent {
            Intent::GradeNumeric => Purpose::Evaluate,
            Intent::Explain => Purpose::Teach,
            _ => Purpose::Probe,
        };
        let (mut req, _, mut records) = context::example(purpose);
        req.snapshot.scope = SEAT.scope;
        req.snapshot.class = SEAT.class;
        req.snapshot.outcome = selected.outcome;
        req.snapshot.epoch = self.epoch;
        for r in &mut records {
            r.outcome = selected.outcome;
            r.release = SEAT.scope.revision;
        }
        let auth = Authority::new(&req, &records).map_err(|e| format!("{e:?}"))?;
        let binding = document_id(
            &serde_json::to_vec(&(
                SEAT.scope.learner,
                SEAT.class,
                self.epoch,
                selected.revision,
                selected.source.document,
            ))
            .unwrap(),
        );
        let need = Need {
            scope: SEAT.scope,
            outcome: selected.outcome,
            intent,
            packet: binding,
        };
        let grant = LocalGrant {
            valid_until_ms: 2,
            binding,
            snapshot: auth.snapshot,
            need,
        };
        let r = Request {
            id: self.revision,
            need,
            binding,
            deadline_ms: 2,
            max_micros: 0,
        };
        let action = route_local(
            r,
            &auth,
            grant,
            &Options {
                reviewed: Some(Reviewed {
                    valid_until_ms: 2,
                    need,
                    artifact: binding,
                    binding,
                }),
                exact_checker: true,
                ..Options::default()
            },
            1,
        );
        Ok(match intent {
            Intent::GradeNumeric => action == Route::CheckLocally,
            _ => action == Route::UseReviewed(binding),
        })
    }
    fn offer(&mut self, day: u32, resume: bool) -> Result<Value, String> {
        if day < self.day {
            return Err("course clock cannot move backwards".into());
        }
        if self.parked && !resume {
            return Err("resume the course before opening another exercise".into());
        }
        self.day = day;
        self.diagnostic.cancel();
        self.last_independent = None;
        if resume {
            if let Some(open) = self.opening.as_mut() {
                if open.course.is_parked() {
                    open.course.resume()?;
                }
            }
            self.parked = false;
        }
        let decision = self.target()?;
        let Some(target) = Self::selected_id(decision) else {
            return Ok(
                json!({"display":"The next return is scheduled.","authority":"no_due_action"}),
            );
        };
        let source = Source::new(self.source_revision);
        let revoked = if self.source_active {
            BTreeSet::new()
        } else {
            [source.pin.document].into()
        };
        let active = source.live(&revoked);
        let mut book = ExposureBook::new(SEAT, self.complete);
        for f in self.contacts.values() {
            book.record(SEAT, f, Contact::Delivered)
                .map_err(|e| format!("{e:?}"))?;
        }
        let same = self
            .opening
            .as_ref()
            .filter(|o| o.day == day && o.epoch == self.epoch && o.form.outcome == target)
            .map(|o| o.form);
        let due = matches!(decision, Decision::Recheck(_));
        let catalog = self.catalog(target, &source);
        if !self.grant {
            return Ok(
                json!({"display":"This assignment is unavailable.","authority":"grant_denied"}),
            );
        }
        let selected = if let Some(prior) = same {
            select(
                SEAT,
                target,
                Requirement::Practice,
                &book,
                std::slice::from_ref(&prior),
                &active,
            )
            .copied()
        } else {
            select(
                SEAT,
                target,
                if due {
                    Requirement::FreshTask
                } else {
                    Requirement::Practice
                },
                &book,
                &catalog,
                &active,
            )
            .copied()
        };
        let Ok(item) = selected else {
            self.opening = None;
            return Ok(
                json!({"display":"The promised exercise is unavailable. You can work on your own draft.","authority":format!("{:?}",selected.unwrap_err())}),
            );
        };
        let question = if due {
            Question::DelayedReview
        } else {
            Question::Practice
        };
        if !self.permits(item, Intent::Ask(question))? {
            return Ok(
                json!({"display":"The exercise is currently unavailable.","authority":"operation_denied"}),
            );
        }
        let (prompt, _) = item
            .card
            .question(&active)
            .ok_or("active material required")?;
        let independent =
            due && same.is_none() && self.coached_day.is_none_or(|d| day.saturating_sub(d) >= 21);
        let mut course = CourseTurn::opened();
        if same.is_some() {
            course.redisplayed();
        }
        self.record_return_contact(item.outcome);
        self.contacts.insert(item.task, item);
        self.opening = Some(Opening {
            form: item,
            epoch: self.epoch,
            day,
            course,
            independent,
        });
        Ok(
            json!({"display":prompt,"opened":true,"contact":"delivered","task":item.task,"item_revision":item.revision,"authority":"current",
            "contribution":"course_opening","independent":independent,"display_day":day}),
        )
    }
    fn answer(&mut self, text: &str, help: Assistance) -> Result<Value, String> {
        if self.parked {
            return Err("the course is parked".into());
        }
        let open = self.opening.as_ref().ok_or("no active course opening")?;
        let item = open.form;
        if open.epoch != self.epoch || !self.permits(item, Intent::GradeNumeric)? {
            return Ok(
                json!({"display":"This opening no longer authorizes an assessment.","authority":"stale_or_denied","contribution":"refused_submission"}),
            );
        }
        let source = Source::new(self.source_revision);
        let judgment = item
            .card
            .assess(text, &source.live(&BTreeSet::new()))
            .ok_or("unavailable checked material")?;
        let independent =
            open.independent && !open.course.practice_only() && help == Assistance::NoneReported;
        let is_return = matches!(self.target()?, Decision::Recheck(_));
        let id = self.revision + 100;
        let observed = Attempt {
            scope: SEAT.scope,
            id,
            sequence: id,
            outcome: item.outcome,
            item: item.revision,
            day: self.day,
            question: if is_return {
                Question::DelayedReview
            } else {
                Question::Practice
            },
            judgment,
            assistance: help,
            fresh: independent,
            sources: [source.pin.document].into(),
        };
        self.record_return_contact(item.outcome);
        self.history.push(observed);
        let qualified = independent && judgment == Judgment::Correct;
        if qualified {
            self.proposals.push(CheckpointProposal {
                scope: SEAT.scope,
                outcome: item.outcome,
                first: if item.outcome == B { 22 } else { 12 },
                confirmation: id,
                due: self.day.checked_add(21).ok_or("date overflow")?,
            });
        }
        let admitted = qualified
            && self
                .progress()?
                .checkpoints
                .get(&item.outcome)
                .is_some_and(|c| c.observed == self.day && c.due == self.day.saturating_add(21));
        self.last_independent = independent.then_some((item.outcome, judgment, id));
        self.opening = None;
        Ok(
            json!({"display":match judgment{Judgment::Correct=>"Your amount matches the check.",Judgment::Incorrect if independent=>"Your amount differs from the check. Would you like a different short problem, or would you prefer to continue?",Judgment::Incorrect=>"Your amount differs from the check.",_=>"Your amount could not be verified."},
            "contribution":if independent{"course_assessment"}else{"course_practice"},"credit":if admitted{Some("independent_return")}else{None},
            "attempt":id,"judgment":format!("{judgment:?}"),"authority":"current","independent":independent,"task":item.task}),
        )
    }
    pub fn apply(&mut self, action: Action) -> Result<(Value, Value), String> {
        let mut private = None;
        let event = match action {
            Action::CaseReview(candidate) => json!(["case-review", candidate]),
            Action::GoalWriting(id, text) => {
                private = self.goal.private_reply(id, &text)?;
                json!(["goal-writing-received", id])
            }
            Action::Goal(mut words) => {
                if words.first().map(String::as_str) == Some("requested-visit") {
                    return Err("unknown goal action".into());
                }
                // Earlier journals recorded a worked example on every visit.
                // New visits record the entry point which remembers prior contact.
                if words.as_slice() == ["visit"] {
                    words[0] = "requested-visit".into();
                }
                let mut event = vec![json!("goal-v2")];
                event.extend(words.into_iter().map(Value::String));
                Value::Array(event)
            }
            Action::Open(d) => json!(["open", d]),
            Action::PracticeCase(s) => json!(["practice-case", s]),
            Action::PracticeNext(s) => json!(["practice-next", s]),
            Action::PracticeVisit(d) => json!(["practice-visit", d]),
            Action::Resume(d) => json!(["resume", d]),
            Action::Work(draft) => {
                let w = Workshop::new(&draft, Focus::Clarity)?;
                let note = w.feedback();
                private = Some(
                    json!({"quote":&draft[note.span],"observation":note.observation,"experiment":note.experiment}),
                );
                json!(["work"])
            }
            Action::Answer(s, a) => json!(["answer", s, assistance(a)]),
            Action::Reply(s, a) => self.reply_event(&s, a)?,
            Action::CourseAid(kind) => json!(["course-aid", kind]),
            Action::DiagnosticAnswer(s, a) => json!(["diagnostic-answer", s, assistance(a)]),
            Action::WithdrawSource => json!(["withdraw"]),
            Action::ReplaceSource => json!(["replace"]),
            Action::RevokeGrant => json!(["revoke"]),
            Action::ReissueGrant => json!(["reissue"]),
            Action::UnknownContact => json!(["unknown"]),
            Action::RecoverContact => json!(["recover"]),
            Action::UseVariedInstances => json!(["varied-instances"]),
            Action::Diagnostic => json!(["diagnostic"]),
            Action::DiagnosticHelp => json!(["diagnostic-help"]),
            Action::DiagnosticSkip => json!(["diagnostic-skip"]),
        };
        let mut response = self.replay(&event)?;
        if let Some(note) = private {
            response["display"] = note;
        }
        Ok((event, response))
    }
    /// Recover a committed prefix. A rejected event discards the in-progress
    /// session, so recovery need not copy the previous history on every record.
    pub fn restore_next(mut self, event: &Value) -> Result<Self, String> {
        self.transition(event)?;
        // Every committed prefix must still produce valid reconciled history and
        // a planner decision. A discarded display receipt needs no rendering.
        self.target()?;
        Ok(self)
    }

    pub fn replay(&mut self, event: &Value) -> Result<Value, String> {
        let mut candidate = self.clone();
        let result = candidate.reduce(event)?;
        *self = candidate;
        Ok(result)
    }
    fn reduce(&mut self, event: &Value) -> Result<Value, String> {
        let delta = self.transition(event)?;
        let mut receipt = self.status()?;
        receipt
            .as_object_mut()
            .unwrap()
            .extend(delta.as_object().ok_or("invalid rehearsal result")?.clone());
        Ok(receipt)
    }

    fn transition(&mut self, event: &Value) -> Result<Value, String> {
        let fields = event.as_array().ok_or("event must be an array")?;
        let op = fields
            .first()
            .and_then(Value::as_str)
            .ok_or("event missing operation")?;
        let arity = match op {
            "goal" | "goal-v2" => fields.len(),
            "goal-writing-received" | "case-review" | "course-aid" => 2,
            "open" | "resume" | "practice-case" | "practice-next" | "practice-visit" => 2,
            "answer" | "diagnostic-answer" => 3,
            _ => 1,
        };
        if fields.len() != arity {
            return Err("unexpected event arguments".into());
        }
        let delta = match op {
            "reply-options" => self.reply_options()?,
            "course-aid" => self.course_aid(fields[1].as_str().ok_or("invalid course aid")?)?,
            "goal" | "goal-v2" => self.goal.apply(&fields[1..], op == "goal-v2")?,
            "case-review" => self
                .goal
                .review_case(fields[1].as_str().ok_or("invalid editorial fixture")?)?,
            "goal-writing-received" => self.goal.submitted(
                fields[1]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or("invalid written-work unit")?,
            )?,
            "varied-instances" => {
                self.varied_instances = true;
                json!({"contribution":"lab_catalog_setting"})
            }
            "practice-case" => {
                private_practice::example(fields[1].as_str().ok_or("expected example name")?)?
            }
            "practice-next" => self.private_practice.choose(
                self.day,
                fields[1].as_str().ok_or("expected practice choice")?,
            )?,
            "practice-visit" => {
                let day = u32::try_from(fields[1].as_u64().ok_or("invalid day")?)
                    .map_err(|_| "invalid day")?;
                let receipt = self.private_practice.visit(self.day, day)?;
                if day > self.day {
                    // End the prior opening, keeping its exposure receipt and course debt.
                    self.opening = None;
                    self.diagnostic.cancel();
                    self.last_independent = None;
                    self.day = day;
                }
                receipt
            }
            "open" | "resume" => {
                let d = u32::try_from(fields[1].as_u64().ok_or("invalid day")?)
                    .map_err(|_| "invalid day")?;
                self.offer(d, op == "resume")?
            }
            "work" => {
                // The durable workshop receipt cannot identify which skill the draft exercised.
                self.return_contacts.covered_from = self.event_moment();
                if let Some(o) = self.opening.as_mut() {
                    if !o.course.is_parked() {
                        o.course.park()?;
                    }
                    o.course.coached();
                }
                self.coached_day = Some(self.day);
                self.parked = true;
                self.last_independent = None;
                self.diagnostic.cancel();
                json!({"contribution":"private_workshop","contact":"coaching_delivered","authority":"private_local"})
            }
            "answer" | "diagnostic-answer" => {
                let text = fields[1].as_str().ok_or("invalid answer")?;
                if text.len() > 128 {
                    return Err("course response exceeds 128 bytes".into());
                }
                let help = read_assistance(&fields[2])?;
                if op == "answer" {
                    self.answer(text, help)?
                } else {
                    self.diagnostic.answer(text, help)?
                }
            }
            "diagnostic" => {
                let (target, j, attempt) = self
                    .last_independent
                    .ok_or("no independent course finding for this diagnostic")?;
                let result = self.diagnostic.offer(SEAT.scope, target, j, attempt)?;
                self.record_return_contact(target);
                self.coached_day = Some(self.day);
                self.last_independent = None;
                result
            }
            "diagnostic-help" => self.diagnostic.help()?,
            "diagnostic-skip" => self.diagnostic.skip()?,
            "withdraw" | "replace" | "revoke" | "reissue" | "unknown" | "recover" => {
                self.diagnostic.cancel();
                self.last_independent = None;
                if matches!(op, "withdraw" | "replace") {
                    self.retired
                        .insert(Source::new(self.source_revision).pin.document);
                }
                match op {
                    "withdraw" => self.source_active = false,
                    "replace" => {
                        self.source_revision = self
                            .source_revision
                            .checked_add(1)
                            .ok_or("source version overflow")?;
                        self.source_active = true;
                    }
                    "revoke" => self.grant = false,
                    "reissue" => self.grant = true,
                    "unknown" => {
                        self.complete = false;
                    }
                    "recover" => {
                        if !self.complete {
                            self.return_contacts.covered_from = self.event_moment();
                        }
                        self.complete = true;
                    }
                    _ => unreachable!(),
                }
                self.epoch = self.epoch.checked_add(1).ok_or("epoch overflow")?;
                json!({"contribution":"authority_update","authority":op})
            }
            _ => return Err(format!("unknown rehearsal operation: {op}")),
        };
        self.revision += 1;
        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requested_practice_can_change_without_recording_the_reason_or_an_assessment() {
        let mut session = Session::new().unwrap();
        let before = session.status().unwrap();
        let (_, first) = session.apply(Action::PracticeCase("warm".into())).unwrap();
        let (_, second) = session
            .apply(Action::PracticeCase("agreed".into()))
            .unwrap();
        assert_eq!(first["display"]["example"], second["display"]["example"]);
        assert_ne!(
            first["display"]["observable"],
            second["display"]["observable"]
        );
        let mut restored = Session::new().unwrap();
        for choice in ["limit", "invitation"] {
            let (event, _) = session.apply(Action::PracticeNext(choice.into())).unwrap();
            assert_eq!(event, json!(["practice-next", choice]));
            restored.replay(&event).unwrap();
        }
        assert!(restored.apply(Action::PracticeVisit(30)).is_err());
        let (_, later) = restored.apply(Action::PracticeVisit(44)).unwrap();
        assert!(later["display"]
            .as_str()
            .unwrap()
            .contains("inviting a choice"));
        assert_eq!(later["checkpoint"], before["checkpoint"]);
        assert_eq!(later["credit"], Value::Null);
        let (_, next) = restored.apply(Action::PracticeVisit(45)).unwrap();
        assert_eq!(next["display"], "There is no saved exercise to offer.");
        restored
            .apply(Action::PracticeNext("limit".into()))
            .unwrap();
        restored
            .apply(Action::PracticeNext("clear".into()))
            .unwrap();
        let (_, cleared) = restored.apply(Action::PracticeVisit(90)).unwrap();
        assert_eq!(cleared["display"], next["display"]);
    }

    #[test]
    fn an_invalid_action_preserves_the_whole_prior_projection() {
        let mut s = Session::new().unwrap();
        s.apply(Action::Open(30)).unwrap();
        let (event, note) = s
            .apply(Action::Work("Private personal draft.".into()))
            .unwrap();
        assert_eq!(event, json!(["work"]));
        assert!(note["display"]["quote"].as_str().is_some());
        let before = s.status().unwrap();
        assert!(s.apply(Action::Open(900)).is_err());
        assert!(s.replay(&json!(["answer", "5/16", "none"])).is_err());
        assert_eq!(s.status().unwrap(), before);
    }
    #[test]
    fn withdrawal_reconciles_the_return_credit_and_retains_the_original_due_date() {
        let mut s = Session::new().unwrap();
        s.apply(Action::Open(30)).unwrap();
        let (_, answer) = s
            .apply(Action::Answer("2/16".into(), Assistance::NoneReported))
            .unwrap();
        assert_eq!(answer["credit"], "independent_return");
        assert_eq!(answer["checkpoint"], json!({"observed":30,"due":51}));
        let (_, withdrawal) = s.apply(Action::WithdrawSource).unwrap();
        assert_eq!(withdrawal["checkpoint"], json!({"observed":0,"due":21}));
        assert_eq!(withdrawal["pending"], true);
        assert_eq!(withdrawal["contacted_tasks"], json!([1]));
    }
}
