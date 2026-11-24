//! An appointment preserves an answer's place. Only the live operation gate can
//! release the requested feedback or independent task.
use super::Lane;
use std::collections::BTreeSet;
use tutor_lab::context::{self, Authority, Purpose};
use tutor_lab::curriculum::{Document, OutcomeId, Span};
use tutor_lab::history::Scope;
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::{Assistance, Effect, Event, Gate, Judgment, Next, Question, State};
use tutor_lab::routing::{
    route_local, Action, Intent, LocalGrant, Need, Options, Request, Reviewed,
};
use tutor_lab::supply::{select, Contact, Enrollment, ExposureBook, Form, Requirement};
use tutor_lab::temporal::check_trace;

const PEER: usize = 5;
const OBJECTIVE: OutcomeId = OutcomeId(1);

struct Source {
    bytes: Vec<u8>,
    pin: Span,
}

impl Source {
    fn new(edition: &str) -> Self {
        let bytes = format!("{edition}\n{UNIT_RULE}").into_bytes();
        let pin = Span {
            document: document_id(&bytes),
            start: (edition.len() + 1) as u32,
            end: bytes.len() as u32,
        };
        Self { bytes, pin }
    }
}

#[derive(Clone, Copy)]
struct Variant {
    form: Form,
    available_from: usize,
    readable: bool,
    preserves_criterion: bool,
}

fn variants(pin: Span) -> Vec<Variant> {
    // Format review is a scenario input. The arithmetic kernel checks the same
    // source on each read, but cannot certify the accessibility of a rendition.
    [
        (1, 0, true, true),
        (2, 0, false, true),
        (3, 0, true, false),
        (4, 6, true, true),
    ]
    .map(
        |(task, available_from, readable, preserves_criterion)| Variant {
            form: Form {
                class: 10,
                release: 2,
                outcome: OBJECTIVE,
                owner: None,
                revision: task,
                task,
                family: task,
                source: pin,
                card: FractionCard::propose(FractionTask::Sum, 1, task as u16, 8).unwrap(),
            },
            available_from,
            readable,
            preserves_criterion,
        },
    )
    .into()
}

struct Learner {
    lane: Lane,
    enrollment: Enrollment,
    book: ExposureBook,
    recheck: bool,
    deliveries: Vec<usize>,
}

impl Learner {
    fn new(seat: usize, original: &Form) -> Self {
        let enrollment = Enrollment {
            scope: Scope {
                learner: seat as u64 + 1,
                revision: 2,
            },
            class: 10,
        };
        let mut book = ExposureBook::new(enrollment, true);
        book.record(enrollment, original, Contact::Delivered)
            .unwrap();
        let recheck = seat == 0;
        let mut lane = Lane::default();
        if recheck {
            lane.step(Event::Teach(Gate::Supported), Effect::Taught);
            lane.step(
                Event::Ask(Question::Practice, Gate::Supported),
                Effect::Asked(Question::Practice),
            );
            lane.step(
                Event::Answer(Judgment::Correct, Assistance::Reported),
                Effect::Analyzed(Next::IndependentCheck),
            );
            lane.step(
                Event::Feedback(Gate::Supported),
                Effect::GaveFeedback(Next::IndependentCheck),
            );
        } else {
            lane.submit();
        }
        Self {
            lane,
            enrollment,
            book,
            recheck,
            deliveries: Vec::new(),
        }
    }

    fn form(
        &self,
        exact: bool,
        tick: usize,
        options: &[Variant],
        source: &RuleSource<'_>,
    ) -> Option<(Form, Document)> {
        let forms: Vec<Form> = options
            .iter()
            .filter(|v| v.available_from <= tick)
            .filter(|v| {
                !exact
                    || if self.recheck {
                        v.preserves_criterion && v.readable
                    } else {
                        v.form.task == 1
                    }
            })
            .map(|v| v.form)
            .collect();
        let requirement = if exact && self.recheck {
            Requirement::FreshTask
        } else {
            Requirement::Practice
        };
        let form = select(
            self.enrollment,
            OBJECTIVE,
            requirement,
            &self.book,
            &forms,
            source,
        )
        .ok()?;
        let (body, _) = if self.recheck {
            form.card.question(source)?
        } else {
            form.card.worked(source)?
        };
        Some((*form, document_id(body.as_bytes())))
    }

    fn granted(&self, form: Form, artifact: Document, tick: usize, practice_only: bool) -> bool {
        let purpose = if self.recheck {
            Purpose::Probe
        } else {
            Purpose::Teach
        };
        let (mut brief, _, records) = context::example(purpose);
        brief.snapshot.scope = self.enrollment.scope;
        brief.snapshot.outcome = OBJECTIVE;
        brief.snapshot.class = self.enrollment.class;
        brief.snapshot.turn = tick as u64;
        let authority = Authority::new(&brief, &records).unwrap();
        let binding = document_id(
            &serde_json::to_vec(&(
                self.enrollment.scope.learner,
                tick,
                form.revision,
                form.source.document,
                artifact,
                self.recheck,
            ))
            .unwrap(),
        );
        let need = Need {
            scope: self.enrollment.scope,
            outcome: OBJECTIVE,
            intent: if self.recheck {
                Intent::Ask(Question::IndependentCheck)
            } else {
                Intent::Explain
            },
            packet: binding,
        };
        let requested = Request {
            id: tick as u64,
            need,
            binding,
            deadline_ms: tick as u64 + 1,
            max_micros: 0,
        };
        let granted_need = if practice_only && self.recheck {
            Need {
                intent: Intent::Ask(Question::Practice),
                ..need
            }
        } else {
            need
        };
        let grant = LocalGrant {
            valid_until_ms: requested.deadline_ms,
            binding,
            snapshot: authority.snapshot,
            need: granted_need,
        };
        let reviewed = Reviewed {
            valid_until_ms: requested.deadline_ms,
            need,
            binding,
            artifact,
        };
        let action = route_local(
            requested,
            &authority,
            grant,
            &Options {
                reviewed: Some(reviewed),
                ..Options::default()
            },
            tick as u64,
        );
        action == Action::UseReviewed(artifact)
    }

    fn discharge(&mut self, form: &Form, tick: usize) {
        if self.recheck {
            self.lane.step(
                Event::Ask(Question::IndependentCheck, Gate::Supported),
                Effect::Asked(Question::IndependentCheck),
            );
            self.book
                .record(self.enrollment, form, Contact::Delivered)
                .unwrap();
        } else {
            self.lane.step(
                Event::Feedback(Gate::Supported),
                Effect::GaveFeedback(Next::Explain),
            );
        }
        self.deliveries.push(tick);
    }
}

#[derive(Clone, Copy, Debug)]
enum Appointment {
    InitialSnapshot,
    AnyMaterial,
    RequestedAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Revocation {
    SharedSource,
    PersonalCheck,
}

#[derive(Clone, Copy, Debug, Default)]
struct Retry {
    lookups: usize,
    skip_revoked_sources: bool,
}

fn compare(
    policy: Appointment,
    permission_gap: bool,
    race: Revocation,
    retry: Retry,
) -> Result<Vec<Option<usize>>, String> {
    let sources = [Source::new("class text"), Source::new("independent text")];
    let catalogs = sources.each_ref().map(|s| variants(s.pin));
    let mut learners: Vec<_> = (0..=PEER)
        .map(|i| Learner::new(i, &catalogs[usize::from(i == PEER)][0].form))
        .collect();
    let mut order: Vec<usize> = (0..=PEER).collect();
    let mut stopped = Vec::new();
    let mut outcomes = Vec::new();
    let mut second_reads = Vec::new();
    for tick in 0..10 {
        let withdrawn: BTreeSet<Document> = if tick < 3 {
            [sources[0].pin.document].into()
        } else {
            BTreeSet::new()
        };
        let deliverable = |seat: usize, exact, withdrawn: &BTreeSet<Document>, changed_grant| {
            let which = usize::from(seat == PEER);
            let src = &sources[which];
            let (form, artifact) = learners[seat].form(
                exact,
                tick,
                &catalogs[which],
                &RuleSource {
                    bytes: Some(&src.bytes),
                    pin: src.pin,
                    revoked: withdrawn,
                },
            )?;
            (!exact
                || learners[seat].granted(
                    form,
                    artifact,
                    tick,
                    (permission_gap && tick == 7) || changed_grant,
                ))
            .then_some(form)
        };
        let found = order.iter().copied().find(|&seat| match policy {
            Appointment::InitialSnapshot => true,
            Appointment::AnyMaterial => deliverable(seat, false, &withdrawn, false).is_some(),
            Appointment::RequestedAction => deliverable(seat, true, &withdrawn, false).is_some(),
        });
        let Some(seat) = found else {
            outcomes.push(None);
            continue;
        };
        // The rights epoch changes between reserving the tick and delivering its output.
        let final_withdrawn = if tick == 6 && race == Revocation::SharedSource {
            [sources[0].pin.document].into()
        } else {
            withdrawn
        };
        let changed_grant = tick == 6 && race == Revocation::PersonalCheck;
        let check =
            |id| deliverable(id, true, &final_withdrawn, changed_grant).map(|form| (id, form));
        let chosen = check(seat).or_else(|| {
            stopped.push((tick, seat + 1));
            order
                .iter()
                .copied()
                .filter(|&other| other != seat)
                .filter(|&other| {
                    let source = &sources[usize::from(other == PEER)];
                    !retry.skip_revoked_sources || !final_withdrawn.contains(&source.pin.document)
                })
                .take(retry.lookups)
                .find_map(|other| {
                    second_reads.push((tick, other + 1));
                    check(other)
                })
        });
        let Some((seat, form)) = chosen else {
            outcomes.push(None);
            continue;
        };
        learners[seat].discharge(&form, tick);
        order.retain(|&s| s != seat);
        if seat == PEER {
            learners[seat].lane.submit();
            order.push(seat);
        }
        outcomes.push(Some(seat + 1));
    }
    for learner in &learners {
        check_trace(State::default(), &learner.lane.events)?;
    }
    let remaining: Vec<_> = order.iter().map(|seat| seat + 1).collect();
    println!("appointments {policy:?} permission_gap={permission_gap} race={race:?} retry={retry:?} granted_seats={outcomes:?} held_at_delivery={stopped:?} remaining_seats={remaining:?} secondary_reads={second_reads:?} first_accessible_check={:?}",learners[0].deliveries.first());
    Ok(outcomes)
}

pub(super) fn investigate() -> Result<(), String> {
    for policy in [
        Appointment::InitialSnapshot,
        Appointment::AnyMaterial,
        Appointment::RequestedAction,
    ] {
        compare(policy, false, Revocation::SharedSource, Retry::default())?;
        compare(policy, true, Revocation::SharedSource, Retry::default())?;
    }
    for race in [Revocation::SharedSource, Revocation::PersonalCheck] {
        for retry in [
            Retry {
                lookups: 1,
                skip_revoked_sources: false,
            },
            Retry {
                lookups: 2,
                skip_revoked_sources: false,
            },
            Retry {
                lookups: 1,
                skip_revoked_sources: true,
            },
        ] {
            compare(Appointment::RequestedAction, true, race, retry)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_promise_survives_both_denials_while_authorized_peers_receive_feedback() {
        let normal = compare(
            Appointment::RequestedAction,
            false,
            Revocation::SharedSource,
            Retry::default(),
        )
        .unwrap();
        let restricted = compare(
            Appointment::RequestedAction,
            true,
            Revocation::SharedSource,
            Retry::default(),
        )
        .unwrap();
        assert_eq!(normal[6..9], [None, Some(1), Some(5)]);
        assert_eq!(restricted[6..9], [None, Some(5), Some(1)]);
        assert_eq!(restricted[..3], [Some(6); 3]);
        let broad = compare(
            Appointment::AnyMaterial,
            true,
            Revocation::SharedSource,
            Retry::default(),
        )
        .unwrap();
        assert_eq!(broad[3..8], [None; 5]);
    }

    #[test]
    fn sharing_a_revoked_source_and_sharing_an_active_source_require_different_replacements() {
        let retry = Retry {
            lookups: 1,
            skip_revoked_sources: true,
        };
        for (race, expected) in [
            (Revocation::SharedSource, 6),
            (Revocation::PersonalCheck, 5),
        ] {
            let outcomes = compare(Appointment::RequestedAction, true, race, retry).unwrap();
            assert_eq!(outcomes[6], Some(expected));
            assert_eq!(outcomes[8], Some(1));
        }
    }
}
