//! Study the teaching opportunities at a return with no evidence of form novelty.
use std::collections::BTreeSet;
use tutor_lab::{
    curriculum::{Checkpoint, Decision, Outcome, OutcomeId, Progress, ReviewedUnit, Slot, Span},
    history::{Attempt, Scope, Withdrawn},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{transition, Assistance, Effect, Event, Gate, Phase, Question, State},
    retention::{self, Cell, Exposure, Moment, Prior},
    supply::{self, Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable},
};

const P: OutcomeId = OutcomeId(8);
const A: OutcomeId = OutcomeId(9);
const B: OutcomeId = OutcomeId(10);
const BOTH: OutcomeId = OutcomeId(11);

fn form(id: u64, target: OutcomeId, task: FractionTask, second: u16, pin: Span) -> Form {
    Form {
        class: 7,
        release: 1,
        owner: None,
        revision: id,
        task: id,
        family: id,
        outcome: target,
        source: pin,
        card: FractionCard::propose(task, 3, second, 16).unwrap(),
    }
}

fn value<T, E: std::fmt::Debug>(r: Result<T, E>) -> Result<T, String> {
    r.map_err(|e| format!("{e:?}"))
}

pub(super) fn run() -> Result<(), String> {
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &BTreeSet::new(),
    };
    let seat = Enrollment {
        scope: Scope {
            learner: 42,
            revision: 1,
        },
        class: 7,
    };
    let outcome = |id, requires| Outcome {
        id,
        requires,
        support: [pin].into(),
    };
    let unit = value(ReviewedUnit::build(
        1,
        vec![
            outcome(P, [].into()),
            outcome(A, [P].into()),
            outcome(B, [].into()),
            outcome(BOTH, [A, B].into()),
        ],
    ))?;
    let first_b = form(1, B, FractionTask::MissingPart, 2, pin);
    let new_b = form(2, B, FractionTask::MissingPart, 3, pin);
    let sibling = form(3, A, FractionTask::Sum, 3, pin);
    let distinct_b = form(4, B, FractionTask::MissingPart, 4, pin);
    let forms = [first_b, new_b, sibling, distinct_b];

    for (name, goal, minutes, refresh, sibling_eligible) in [
        ("combined goal, one teaching slot", BOTH, 8, false, true),
        ("only B, decline refresher", B, 8, false, true),
        ("only B, request refresher", B, 8, true, true),
        ("combined goal, too little time", BOTH, 3, false, true),
        (
            "combined goal, sibling prerequisite missing and postponed",
            BOTH,
            8,
            false,
            false,
        ),
    ] {
        let mut progress = Progress {
            revision: 1,
            checkpoints: [(
                B,
                Checkpoint {
                    observed: 0,
                    due: 21,
                },
            )]
            .into(),
            postponed: [].into(),
        };
        if sibling_eligible {
            progress.checkpoints.insert(
                P,
                Checkpoint {
                    observed: 20,
                    due: 45,
                },
            );
        } else {
            progress.postponed.insert(P);
        }
        let mut book = ExposureBook::new(seat, false);
        let cell = Cell {
            scope: seat.scope,
            outcome: B,
        };
        let due = State {
            phase: Phase::ReviewDue,
            paused: false,
        };
        let review = value(unit.next(goal, &progress, 30, &BTreeSet::new(), Slot::Review))?;
        if review != Decision::Recheck(B)
            || supply::select(seat, B, Requirement::FreshTask, &book, &forms, &source)
                != Err(Unavailable::NoFreshForm)
        {
            return Err("prior return case failed to reproduce".into());
        }
        let next = value(unit.next(goal, &progress, 30, &BTreeSet::new(), Slot::Forward))?;
        let mut phase = due.phase;
        let opening = if minutes < 8 {
            None
        } else if refresh {
            let (s, e) = transition(due, Event::RequestRefresher(Gate::Supported));
            if e != Effect::RetaughtBeforeReview {
                return Err("requested refresher unavailable".into());
            }
            phase = s.phase;
            Some(value(supply::select(
                seat,
                B,
                Requirement::Practice,
                &book,
                &forms,
                &source,
            ))?)
        } else if let Decision::Teach(id) = next {
            let (_, e) = transition(State::default(), Event::Teach(Gate::Supported));
            if e != Effect::Taught {
                return Err("sibling did not receive instruction".into());
            }
            Some(value(supply::select(
                seat,
                id,
                Requirement::Practice,
                &book,
                &forms,
                &source,
            ))?)
        } else {
            None
        };
        let observed_at = Moment {
            day: 30,
            sequence: 1,
        };
        if let Some(f) = opening {
            let (worked, _) = f
                .card
                .worked(&source)
                .ok_or("source no longer supports worked example")?;
            value(book.record(seat, f, Contact::Delivered))?;
            println!("{name} -> shown {}: {worked}", f.outcome.0);
        } else {
            println!("{name} -> no worked task shown; offer a later return");
        }
        let latest = Moment {
            day: 31,
            sequence: 2,
        };
        let prior = Prior {
            cell,
            verified: Moment {
                day: 0,
                sequence: 2,
            },
            stability_days: 14.0,
            threshold: 0.85,
            had_durable_success: false,
        };
        // A separately restored complete contact ledger and reviewed new B family
        // are conditional follow-up cases. Today's missing history cannot certify them.
        let mut restored = ExposureBook::new(seat, true);
        value(restored.record(seat, &first_b, Contact::Delivered))?;
        if let Some(f) = opening {
            value(restored.record(seat, f, Contact::Delivered))?;
        }
        let later = value(supply::select(
            seat,
            B,
            Requirement::FreshFamily,
            &restored,
            &forms,
            &source,
        ))?;
        let answer = Attempt {
            scope: seat.scope,
            id: 12,
            sequence: latest.sequence,
            outcome: B,
            item: later.revision,
            day: latest.day,
            question: Question::DelayedReview,
            judgment: later
                .card
                .assess("3/16", &source)
                .ok_or("unable to assess")?,
            assistance: Assistance::NoneReported,
            fresh: true,
            sources: [pin.document].into(),
        };
        let b_contacts: Vec<_> = opening
            .filter(|f| f.outcome == B)
            .map(|_| Exposure {
                cell,
                at: observed_at,
            })
            .into_iter()
            .collect();
        let (_, finding) = value(retention::read(
            cell,
            latest,
            None,
            Some(prior),
            Some(&b_contacts),
            Some(&answer),
            &Withdrawn::new(seat.scope),
        ))?;
        let (_, today) = value(retention::read(
            cell,
            observed_at,
            None,
            Some(prior),
            None,
            None,
            &Withdrawn::new(seat.scope),
        ))?;
        println!("  day30 B phase={phase:?}, independent finding={today:?}; day31 correct B with restored ledger={finding:?}");
        if opening.is_some_and(|f| f.outcome == A) {
            // An editor recognizes a worked sum and the inverse ribbon question
            // use the same numbers. Compare two catalog policies on the SAME shown text.
            for (policy, all_one_family) in [("inverse pair", false), ("entire topic", true)] {
                let mut indexed = forms;
                if all_one_family {
                    for f in &mut indexed {
                        f.family = 80;
                    }
                } else {
                    indexed[2].family = new_b.family;
                }
                let mut contacts = ExposureBook::new(seat, true);
                value(contacts.record(seat, &indexed[0], Contact::Delivered))?;
                value(contacts.record(seat, &indexed[2], Contact::Delivered))?;
                match supply::select(
                    seat,
                    B,
                    Requirement::FreshFamily,
                    &contacts,
                    &indexed,
                    &source,
                ) {
                    Ok(next_form) => {
                        let repeated = next_form
                            .card
                            .assess("3/16", &source)
                            .ok_or("unavailable new card")?;
                        let prompt = next_form
                            .card
                            .question(&source)
                            .ok_or("unavailable new prompt")?
                            .0;
                        println!(
                            "  {policy} -> task {}, {prompt}; repeating 3/16 -> {repeated:?}",
                            next_form.task
                        );
                    }
                    Err(why) => println!("  {policy} -> {why:?}"),
                }
            }
        }
    }
    Ok(())
}
