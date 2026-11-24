//! Carry a concrete planner selection to the learner's actual opening transaction.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::{
    context::{self, Authority, Purpose},
    curriculum::{Checkpoint, Decision, Outcome, OutcomeId, Progress, ReviewedUnit, Slot, Span},
    history::Scope,
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    presentation::{BoundLane, CatalogQuestion},
    protocol::{Assistance, Command, Event, Gate, Question},
    routing::{route_local, Action, Intent, LocalGrant, Need, Options, Request, Reviewed},
    supply::{Enrollment, ExposureBook, Form, Requirement},
};

fn reviewed_target(pin: Span) -> Result<(u64, OutcomeId), String> {
    let edition = 1;
    let (a, b, combined) = (OutcomeId(1), OutcomeId(2), OutcomeId(3));
    let outcomes = [a, b, combined].map(|id| Outcome {
        id,
        requires: if id == combined {
            [a, b].into()
        } else {
            BTreeSet::new()
        },
        support: [pin].into(),
    });
    let unit = ReviewedUnit::build(edition, outcomes.into())
        .map_err(|e| format!("invalid unit: {e:?}"))?;
    let progress = Progress {
        revision: edition,
        postponed: BTreeSet::new(),
        checkpoints: BTreeMap::from([
            (
                a,
                Checkpoint {
                    observed: 1,
                    due: 20,
                },
            ),
            (
                b,
                Checkpoint {
                    observed: 1,
                    due: 10,
                },
            ),
        ]),
    };
    match unit.next(combined, &progress, 30, &BTreeSet::new(), Slot::Review) {
        Ok(Decision::Recheck(target)) => Ok((edition, target)),
        other => Err(format!("expected a due component, got {other:?}")),
    }
}

fn authority(seat: Enrollment, id: OutcomeId, epoch: u64) -> Result<Authority, String> {
    let (mut brief, _, mut records) = context::example(Purpose::Probe);
    brief.snapshot.scope = seat.scope;
    brief.snapshot.class = seat.class;
    brief.snapshot.outcome = id;
    brief.snapshot.epoch = epoch;
    for r in &mut records {
        r.outcome = id;
        r.class = seat.class;
        r.release = seat.scope.revision;
        if r.learner.is_some() {
            r.learner = Some(seat.scope.learner);
        }
    }
    Authority::new(&brief, &records).map_err(|e| format!("authority: {e:?}"))
}

fn form(seat: Enrollment, id: OutcomeId, pin: Span, version: u16) -> Result<Form, String> {
    let operation = match id.0 {
        1 => FractionTask::Sum,
        2 => FractionTask::MissingPart,
        _ => return Err("unknown task".into()),
    };
    Ok(Form {
        class: seat.class,
        release: seat.scope.revision,
        outcome: id,
        owner: None,
        revision: u64::from(version),
        task: u64::from(version),
        family: u64::from(version),
        source: pin,
        card: FractionCard::propose(operation, version, 2, 16).ok_or("bad form template")?,
    })
}

fn event(lane: &mut BoundLane, event: Event) -> Result<(), String> {
    let after = lane.revision();
    lane.control(Command {
        after,
        record: after + 1,
        event,
    })
    .map_err(|e| format!("prior event: {e:?}"))?;
    Ok(())
}

fn review_ready(
    seat: Enrollment,
    target: OutcomeId,
    source: &RuleSource<'_>,
) -> Result<(BoundLane, ExposureBook), String> {
    let mut lane = BoundLane::for_enrollment(seat, target);
    let mut book = ExposureBook::new(seat, true);
    event(&mut lane, Event::Teach(Gate::Supported))?;
    for (version, question) in [(1, Question::Practice), (2, Question::ChangedExample)] {
        let item = form(seat, target, source.pin, version)?;
        let rev = lane.revision();
        let prompt = lane
            .present_from_catalog(
                &mut book,
                CatalogQuestion {
                    form: &item,
                    question,
                    requirement: Requirement::FreshTask,
                    source,
                },
                rev,
                rev + 1,
            )
            .map_err(|e| format!("historical opening: {e:?}"))?;
        let expected = if target.0 == 1 { version + 2 } else { 2 };
        lane.submit(
            &prompt.handle,
            lane.revision() + 1,
            &format!("{expected}/16"),
            Assistance::NoneReported,
            source,
        )
        .map_err(|e| format!("historical assessment: {e:?}"))?;
        event(&mut lane, Event::Feedback(Gate::Supported))?;
    }
    event(&mut lane, Event::ReviewDue)?;
    Ok((lane, book))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    None,
    ReissuedGrant,
    NewEdition,
    WithdrawSource,
    Deadline,
}

fn episode(epoch_change: Change, use_current: bool) -> Result<Value, String> {
    let empty = BTreeSet::new();
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let revoked = BTreeSet::from([pin.document]);
    let active = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &empty,
    };
    let (edition, target) = reviewed_target(pin)?;
    let seat = Enrollment {
        scope: Scope {
            revision: edition,
            learner: 2,
        },
        class: 7,
    };
    let (mut lane, mut book) = review_ready(seat, target, &active)?;
    let initial = authority(seat, target, 1)?;
    let mut current_seat = seat;
    if epoch_change == Change::NewEdition {
        current_seat.scope.revision += 1;
    }
    let current = authority(
        current_seat,
        target,
        if epoch_change == Change::ReissuedGrant {
            2
        } else {
            1
        },
    )?;
    let source = RuleSource {
        revoked: if use_current && epoch_change == Change::WithdrawSource {
            &revoked
        } else {
            &empty
        },
        ..active
    };
    let item = form(seat, target, pin, 3)?;
    let artifact = document_id(
        item.card
            .question(&active)
            .ok_or("no reviewed prompt")?
            .0
            .as_bytes(),
    );
    let need = Need {
        scope: seat.scope,
        outcome: target,
        intent: Intent::Ask(Question::DelayedReview),
        packet: artifact,
    };
    let request = Request {
        id: 30,
        need,
        binding: artifact,
        deadline_ms: 10,
        max_micros: 0,
    };
    let grant = LocalGrant {
        need,
        valid_until_ms: 20,
        binding: artifact,
        snapshot: initial.snapshot,
    };
    let options = Options {
        reviewed: Some(Reviewed {
            valid_until_ms: 20,
            need,
            artifact,
            binding: artifact,
        }),
        ..Options::default()
    };
    let before = lane.revision();
    let now = if use_current && epoch_change == Change::Deadline {
        10
    } else {
        0
    };
    let action = route_local(
        request,
        if use_current { &current } else { &initial },
        grant,
        &options,
        now,
    );
    let mut displayed = None;
    let mut assessment = None;
    let mut outcome = format!("{action:?}");
    if action == Action::UseReviewed(artifact) {
        match lane.present_from_catalog(
            &mut book,
            CatalogQuestion {
                form: &item,
                question: Question::DelayedReview,
                requirement: Requirement::FreshTask,
                source: &source,
            },
            before,
            before + 1,
        ) {
            Err(e) => outcome = format!("{e:?}"),
            Ok(opening) => {
                outcome = "opened".to_string();
                displayed = Some(opening.prompt);
                // Learner profile reproduced from the paired planning experiment: misses B.
                let response = if target == OutcomeId(2) {
                    "0/16"
                } else {
                    "5/16"
                };
                let checked = lane
                    .submit(
                        &opening.handle,
                        lane.revision() + 1,
                        response,
                        Assistance::NoneReported,
                        &source,
                    )
                    .map_err(|e| format!("submitted opening: {e:?}"))?;
                assessment = Some(format!("{:?}", checked.judgment));
            }
        }
    }
    Ok(
        json!({"change":format!("{epoch_change:?}"),"use_current":use_current,"planned_outcome":target.0,"planned_edition":edition,
        "outcome":outcome,"prompt":displayed,"answer":assessment,"revision_before":before,"revision_after":lane.revision(),
        "new_form_encountered":book.exposure(item.task).is_some()}),
    )
}

pub(super) fn run() -> Result<(), String> {
    for change in [
        Change::None,
        Change::ReissuedGrant,
        Change::NewEdition,
        Change::WithdrawSource,
        Change::Deadline,
    ] {
        for use_current in [false, true] {
            println!("{}", episode(change, use_current)?);
        }
    }
    for after in [false, true] {
        for reviewed_aliases in [false, true] {
            println!("{}", after_display(after, reviewed_aliases)?);
        }
    }
    Ok(())
}

/// Compare erasing the whole interaction with revising credit while retaining actual contact.
fn after_display(withdraw_before_response: bool, reviewed_aliases: bool) -> Result<Value, String> {
    use tutor_lab::history::{reconcile, Attempt, CheckpointProposal, Withdrawn};
    use tutor_lab::protocol::Judgment;
    use tutor_lab::supply::{select, Contact};

    let empty = BTreeSet::new();
    let original = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let replacement_bytes = format!("B alternate\n{UNIT_RULE}");
    let replacement = Span {
        document: document_id(replacement_bytes.as_bytes()),
        start: 12,
        end: replacement_bytes.len() as u32,
    };
    let original_source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin: original,
        revoked: &empty,
    };
    let replacement_source = RuleSource {
        bytes: Some(replacement_bytes.as_bytes()),
        pin: replacement,
        revoked: &empty,
    };
    let (revision, b) = reviewed_target(original)?;
    let seat = Enrollment {
        scope: Scope {
            learner: 2,
            revision,
        },
        class: 7,
    };
    let (mut lane, mut book) = review_ready(seat, b, &replacement_source)?;
    let item = form(seat, b, original, 3)?;
    let opening = lane
        .present_from_catalog(
            &mut book,
            CatalogQuestion {
                form: &item,
                question: Question::DelayedReview,
                requirement: Requirement::FreshTask,
                source: &original_source,
            },
            lane.revision(),
            lane.revision() + 1,
        )
        .map_err(|e| format!("initial B opening: {e:?}"))?;
    book.record(seat, &item, Contact::Delivered)
        .map_err(|e| format!("{e:?}"))?;
    let mut withdrawn = Withdrawn::new(seat.scope);
    withdrawn.sources.insert(original.document);
    let inactive_source = RuleSource {
        revoked: &withdrawn.sources,
        ..original_source
    };
    let accepted = lane.submit(
        &opening.handle,
        lane.revision() + 1,
        "2/16",
        Assistance::NoneReported,
        if withdraw_before_response {
            &inactive_source
        } else {
            &original_source
        },
    );
    let result = format!("{:?}", accepted.as_ref().map(|a| a.judgment));
    let start = Attempt {
        scope: seat.scope,
        outcome: b,
        id: 1,
        sequence: 1,
        item: 1,
        day: 0,
        question: Question::Practice,
        judgment: Judgment::Correct,
        assistance: Assistance::NoneReported,
        fresh: true,
        sources: [replacement.document].into(),
    };
    let mut attempts = vec![
        start.clone(),
        Attempt {
            id: 2,
            sequence: 2,
            item: 2,
            question: Question::ChangedExample,
            ..start.clone()
        },
    ];
    if let Ok(a) = &accepted {
        attempts.push(Attempt {
            id: 3,
            sequence: a.response_record,
            item: a.item.revision,
            day: 30,
            question: a.item.question,
            judgment: a.judgment,
            fresh: a.fresh_task(),
            sources: [a.span.document].into(),
            ..start
        });
    }
    let base = CheckpointProposal {
        scope: seat.scope,
        outcome: b,
        first: 1,
        confirmation: 2,
        due: 10,
    };
    let proposals = [
        base,
        CheckpointProposal {
            first: 2,
            confirmation: 3,
            due: 60,
            ..base
        },
    ];
    let due = |withdrawals: &Withdrawn| -> Result<Option<u32>, String> {
        let progress = reconcile(seat.scope, 31, &attempts, &proposals, withdrawals)
            .map_err(|e| format!("{e:?}"))?;
        Ok(progress.checkpoints.get(&b).map(|c| c.due))
    };
    let original_due = due(&Withdrawn::new(seat.scope))?;
    let actual_due = due(&withdrawn)?;
    let physically_seen = book.exposure(item.task);
    // Two reviewed releases of the same prompt can carry different catalog revision IDs.
    // The curated alternative preserves task/family equivalence across source replacements.
    let alias = Form {
        revision: 4,
        task: if reviewed_aliases { item.task } else { 4 },
        family: if reviewed_aliases { item.family } else { 4 },
        source: replacement,
        ..item
    };
    let different = Form {
        revision: 5,
        task: 5,
        family: 5,
        source: replacement,
        card: FractionCard::propose(FractionTask::MissingPart, 3, 5, 16)
            .ok_or("bad distinct form")?,
        ..item
    };
    let shelf = [alias, different];
    let selected = select(
        seat,
        b,
        Requirement::FreshFamily,
        &book,
        &shelf,
        &replacement_source,
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut recovery = BoundLane::for_enrollment(seat, b);
    event(&mut recovery, Event::Teach(Gate::Supported))?;
    let fresh = recovery
        .present_from_catalog(
            &mut book,
            CatalogQuestion {
                form: selected,
                question: Question::Practice,
                requirement: Requirement::FreshFamily,
                source: &replacement_source,
            },
            recovery.revision(),
            recovery.revision() + 1,
        )
        .map_err(|e| format!("{e:?}"))?;
    let repeated = recovery
        .submit(
            &fresh.handle,
            recovery.revision() + 1,
            "2/16",
            Assistance::NoneReported,
            &replacement_source,
        )
        .map_err(|e| format!("{e:?}"))?;
    Ok(
        json!({"stage":"after_display","withdraw_before_response":withdraw_before_response,"reviewed_aliases":reviewed_aliases,
        "seen":format!("{physically_seen:?}"),"original_prompt":opening.prompt,"historical_response":result,
        "recorded_attempts":attempts.len(),"due_if_all_sources_active":original_due,"reconciled_due":actual_due,
        "repair_prompt":fresh.prompt,"repair_version":selected.revision,"repeating_old_answer":format!("{:?}",repeated.judgment)}),
    )
}
