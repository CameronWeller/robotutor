//! Publication regressions against the real context, routing, source and supply gates.
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::authoring::{
    AuthoringJob, CandidateBytes, Catalog, PublicationError, Publisher, Template,
};
use tutor_lab::context::{compile, example, Authority, Count, Kind, Packet, Purpose, Record};
use tutor_lab::curriculum::Span;
use tutor_lab::material::{document_id, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::Question;
use tutor_lab::routing::profiles::{self, Contract, Qualification, Window};
use tutor_lab::routing::{
    Action, Completion, Intent, Model, Need, Options, Reason, Request, Returned, Role, Router,
    Ticket,
};
use tutor_lab::supply::{
    select, Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable,
};

struct Job {
    job: AuthoringJob,
    packet: Packet,
    authority: Authority,
    router: Router,
    ticket: Ticket,
}

fn issue(edition: u64, alter: impl FnOnce(&mut Template, &mut [Record])) -> Job {
    issue_for(20, edition, Purpose::Author, Intent::AuthorForm, alter)
}

fn issue_for(
    id: u64,
    edition: u64,
    purpose: Purpose,
    intent: Intent,
    alter: impl FnOnce(&mut Template, &mut [Record]),
) -> Job {
    let (brief, _, mut records) = example(purpose);
    let source = UNIT_RULE.as_bytes();
    let span = Span {
        document: document_id(source),
        start: 0,
        end: source.len() as u32,
    };
    let mut template = Template {
        brief: brief.required[0],
        material: brief.required[1],
        span,
        task: FractionTask::Sum,
        edition,
    };
    alter(&mut template, &mut records);
    let authority = Authority::new(&brief, &records).unwrap();
    let packet = compile(&brief, &authority, &records, &|_| Count::Measured(100)).unwrap();
    let need = Need {
        scope: packet.scope(),
        outcome: packet.outcome(),
        intent,
        packet: packet.digest(),
    };
    let model = Model {
        revision: brief.snapshot.profile,
        maximum_micros: 10,
        admission_ms: 2,
        slots: 1,
        max_input_tokens: 200,
        max_output_tokens: 400,
    };
    let mut router = Router::new(8, 12, 1, 2, BTreeMap::from([(Role::Language, model)]));
    let o = Options {
        permitted: BTreeSet::from([Role::Language]),
        ..Options::default()
    };
    let contract = Contract {
        release: need.scope.revision,
        outcome: need.outcome,
        intent,
        locale: "en",
        modality: "text",
        schema: if purpose == Purpose::Probe {
            "question-text/v1"
        } else {
            "fraction-parameters/v1"
        },
    };
    let row = Qualification {
        contract,
        role: Role::Language,
        profile: model.revision,
        benchmark_epoch: 1,
        grant_epoch: brief.snapshot.epoch,
        from_ms: 0,
        until_ms: 30,
    };
    let window = Window {
        now_ms: 1,
        deadline_ms: 100,
        available_micros: 10,
        benchmark_epoch: row.benchmark_epoch,
        grant_epoch: row.grant_epoch,
    };
    let selected = profiles::select(contract, window, &[row], |role, rev| {
        router.available_profile(role, rev)
    })
    .unwrap();
    let job = AuthoringJob {
        template,
        request: Request {
            id,
            binding: template.binding(),
            need,
            max_micros: 10,
            deadline_ms: selected.deadline_ms,
        },
    };
    let Action::Dispatch(ticket) = router.route(job.request, &packet, &authority, &o, 1) else {
        panic!("fixture should dispatch")
    };
    Job {
        job,
        packet,
        authority,
        router,
        ticket,
    }
}

impl Job {
    fn settle(&mut self, bytes: &[u8]) {
        self.router
            .settle(
                self.ticket,
                Completion {
                    result: Returned::Draft(document_id(bytes)),
                    charged_micros: 3,
                },
            )
            .unwrap();
    }
    fn publish(&self, bytes: &[u8], now: u64) -> Result<Form, PublicationError> {
        self.publish_in(&mut Catalog::default(), bytes, now)
    }

    fn publish_in(
        &self,
        catalog: &mut Catalog,
        bytes: &[u8],
        now: u64,
    ) -> Result<Form, PublicationError> {
        let active = BTreeSet::new();
        let source = RuleSource {
            bytes: Some(UNIT_RULE.as_bytes()),
            pin: self.job.template.span,
            revoked: &active,
        };
        self.job.publish(
            Publisher {
                router: &self.router,
                catalog,
            },
            CandidateBytes {
                ticket: self.ticket,
                bytes,
            },
            &self.packet,
            &self.authority,
            &source,
            now,
        )
    }
}

#[test]
fn qualification_expiry_stops_a_queued_send_and_a_late_publication() {
    let mut h = issue(27, |_, _| {});
    assert_eq!(h.job.request.deadline_ms, 30); // The turn itself allowed 100.
    let grants = Options {
        permitted: BTreeSet::from([Role::Language]),
        ..Options::default()
    };
    assert!(h
        .router
        .messages_for(h.ticket, &h.packet, &h.authority, &grants, 10)
        .is_ok());
    for now in [30, 40] {
        assert_eq!(
            h.router
                .messages_for(h.ticket, &h.packet, &h.authority, &grants, now),
            Err(Reason::Deadline)
        );
    }
    let bytes = b"[1,2,4]";
    h.settle(bytes);
    assert!(h.publish(bytes, 29).is_ok());
    for now in [30, 40] {
        assert_eq!(h.publish(bytes, now), Err(PublicationError::Stale));
    }
}

#[test]
fn a_paid_candidate_cannot_replace_its_original_job_or_deadline() {
    let bytes = b"[1,2,4]";
    let mut h = issue(27, |_, _| {});
    assert_eq!(h.publish(bytes, 10), Err(PublicationError::NoReceipt));
    h.settle(bytes);
    let valid = h.publish(bytes, 29).unwrap();
    assert_eq!(h.publish(bytes, 30), Err(PublicationError::Stale));
    h.job.request.deadline_ms = 40;
    assert_eq!(h.publish(bytes, 31), Err(PublicationError::WrongJob));
    h.job.request.deadline_ms = 30;
    h.job.template.task = FractionTask::MissingPart;
    assert_eq!(h.publish(bytes, 29), Err(PublicationError::WrongJob));
    h.job.request.binding = h.job.template.binding();
    assert_eq!(h.publish(bytes, 29), Err(PublicationError::WrongJob));
    h.job.template.task = FractionTask::Sum;
    h.job.request.binding = h.job.template.binding();
    assert_eq!(h.publish(bytes, 29).unwrap(), valid);
    h.authority.snapshot.epoch += 1;
    assert_eq!(h.publish(bytes, 29), Err(PublicationError::Stale));
}

#[test]
fn digest_tampering_and_malformed_candidates_never_become_forms() {
    let mut valid = issue(3, |_, _| {});
    valid.settle(b"[1,2,4]");
    assert_eq!(
        valid.publish(b"[2,1,4]", 2),
        Err(PublicationError::WrongBytes)
    );
    for invalid in [
        b"[1,2,4,5]".as_slice(),
        b"[0,2,4]",
        b"[4,4,4]",
        b"[1,2,65536]",
        b"{\"prompt\":\"unreviewed question\",\"key\":1}",
        &[b' '; 65],
    ] {
        let mut h = issue(4, |_, _| {});
        h.settle(invalid);
        assert_eq!(
            h.publish(invalid, 2),
            Err(PublicationError::InvalidParameters)
        );
    }
}

#[test]
fn brief_and_source_require_different_roles_and_the_actual_included_rule() {
    let bytes = b"[1,2,4]";
    let mut switched = issue(3, |template, _| {
        std::mem::swap(&mut template.material, &mut template.brief)
    });
    let mut repeated = issue(3, |template, _| template.material = template.brief);
    let mut different = issue(3, |_, records| {
        records
            .iter_mut()
            .find(|r| r.kind == Kind::Material)
            .unwrap()
            .text = "An unrelated but approved statement.".into();
    });
    for h in [&mut switched, &mut repeated, &mut different] {
        h.settle(bytes);
        assert!(h.publish(bytes, 2).is_err());
    }
}

#[test]
fn commuted_sums_published_by_distinct_jobs_share_one_exposure() {
    let mut left = issue(11, |_, _| {});
    left.settle(b"[1,2,4]");
    let form_a = left.publish(b"[1,2,4]", 3).unwrap();
    let mut right = issue(12, |_, _| {});
    right.settle(b"[2,1,4]");
    let form_b = right.publish(b"[2,1,4]", 3).unwrap();
    assert_ne!(form_a.revision, form_b.revision);
    assert_eq!(form_a.task, form_b.task);
    let seat = Enrollment {
        scope: left.packet.scope(),
        class: left.authority.snapshot.class,
    };
    let mut book = ExposureBook::new(seat, true);
    book.record(seat, &form_a, Contact::Possible).unwrap();
    let active = BTreeSet::new();
    let source = RuleSource {
        pin: form_b.source,
        bytes: Some(UNIT_RULE.as_bytes()),
        revoked: &active,
    };
    let forms = [form_b];
    assert_eq!(
        select(
            seat,
            form_b.outcome,
            Requirement::FreshTask,
            &book,
            &forms,
            &source
        ),
        Err(Unavailable::NoFreshForm)
    );
    assert_eq!(
        select(
            seat,
            form_b.outcome,
            Requirement::Practice,
            &book,
            &forms,
            &source
        )
        .unwrap()
        .revision,
        form_b.revision
    );
}

#[test]
fn competing_publications_cannot_rebind_one_edition() {
    let mut first = issue_for(20, 27, Purpose::Author, Intent::AuthorForm, |_, _| {});
    let mut rival = issue_for(21, 27, Purpose::Author, Intent::AuthorForm, |_, _| {});
    first.settle(b"[1,2,4]");
    rival.settle(b"[2,2,4]");
    let mut catalog = Catalog::default();
    let original = first.publish_in(&mut catalog, b"[1,2,4]", 2).unwrap();
    assert_eq!(
        rival.publish_in(&mut catalog, b"[2,2,4]", 3),
        Err(PublicationError::EditionConflict)
    );
    assert_eq!(
        first.publish_in(&mut catalog, b"[1,2,4]", 4).unwrap(),
        original
    );
}

#[test]
fn prose_and_private_parameter_drafts_have_different_publication_rights() {
    let bytes = b"[1,2,4]";
    let mut prose = issue_for(
        20,
        3,
        Purpose::Probe,
        Intent::Ask(Question::Practice),
        |_, _| {},
    );
    prose.settle(bytes);
    assert_eq!(prose.publish(bytes, 5), Err(PublicationError::WrongJob));
    let mut private = issue_for(
        21,
        4,
        Purpose::PrivateForm,
        Intent::PersonalizeForm(Question::Practice),
        |_, _| {},
    );
    private.settle(bytes);
    let form = private.publish(bytes, 5).unwrap();
    assert_eq!(form.owner, Some(private.packet.scope().learner));
    assert_eq!(form.class, private.authority.snapshot.class);
}
