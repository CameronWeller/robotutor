//! Read scheduling consequences from an admitted performance and later corrections.
use super::moment;
use tutor_lab::{
    curriculum::{OutcomeId, Span},
    history::{reconcile, Attempt, Scope, Withdrawn},
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{Assistance, Judgment, Question},
    retention::{read, Cell, CheckResult, Exposure, Moment, Prior},
    supply::{select, Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable},
};

const CELL: Cell = Cell {
    scope: Scope {
        learner: 42,
        revision: 1,
    },
    outcome: OutcomeId(2),
};
const TARGET: u32 = 395;

fn prior() -> Prior {
    Prior {
        cell: CELL,
        verified: moment(0),
        stability_days: 14.0,
        threshold: 0.85,
        had_durable_success: false,
    }
}

fn response(day: u32) -> Attempt {
    Attempt {
        scope: CELL.scope,
        outcome: CELL.outcome,
        id: 30,
        sequence: moment(day).sequence,
        item: 2,
        day,
        question: Question::DelayedReview,
        judgment: Judgment::Correct,
        assistance: Assistance::NoneReported,
        fresh: true,
        sources: [document_id(UNIT_RULE.as_bytes())].into(),
    }
}

fn near_duplicate_is_fresh() -> Result<bool, String> {
    let pin = Span {
        document: document_id(UNIT_RULE.as_bytes()),
        start: 0,
        end: UNIT_RULE.len() as u32,
    };
    let revoked = Default::default();
    let source = RuleSource {
        bytes: Some(UNIT_RULE.as_bytes()),
        pin,
        revoked: &revoked,
    };
    let seat = Enrollment {
        scope: CELL.scope,
        class: 7,
    };
    let old = Form {
        class: 7,
        release: 1,
        outcome: CELL.outcome,
        owner: None,
        revision: 1,
        task: 1,
        family: 88,
        source: pin,
        card: FractionCard::propose(FractionTask::MissingPart, 3, 2, 16)
            .ok_or("invalid original")?,
    };
    let rewritten = Form {
        revision: 2,
        task: 2,
        ..old
    };
    let mut book = ExposureBook::new(seat, true);
    book.record(seat, &old, Contact::Delivered)
        .map_err(|e| format!("{e:?}"))?;
    match select(
        seat,
        CELL.outcome,
        Requirement::FreshFamily,
        &book,
        &[rewritten],
        &source,
    ) {
        Ok(_) => Ok(true),
        Err(Unavailable::NoFreshForm) => Ok(false),
        Err(e) => Err(format!("unexpected supply error {e:?}")),
    }
}

#[derive(Debug, PartialEq)]
struct Booking {
    finding: CheckResult,
    stability: f64,
    recall: f64,
    offer_refresh: u32,
    earliest_spaced_check: u32,
}

/// The supplied observations are immutable. Withdrawn IDs mark superseded revisions.
/// Reconcile first to reject conflicting active revisions of one performance.
fn project(
    attempts: &[Attempt],
    withdrawn: &Withdrawn,
    as_of: u32,
    exposures: &[Exposure],
) -> Result<Booking, String> {
    reconcile(CELL.scope, as_of, attempts, &[], withdrawn).map_err(|e| format!("{e:?}"))?;
    let mut rows: Vec<_> = attempts
        .iter()
        .filter(|a| {
            a.scope == CELL.scope
                && a.outcome == CELL.outcome
                && a.day <= as_of
                && !withdrawn.attempts.contains(&a.id)
        })
        .collect();
    rows.sort_by_key(|a| (a.day, a.sequence));
    rows.dedup_by_key(|a| a.id);
    let mut state = prior();
    let mut finding = CheckResult::NoCheck;
    for a in rows {
        let cutoff = Moment {
            day: a.day,
            sequence: a.sequence,
        };
        let known: Vec<_> = exposures
            .iter()
            .copied()
            .filter(|e| e.cell == CELL && e.at < cutoff)
            .collect();
        let (prediction, result) = read(
            CELL,
            cutoff,
            Some(TARGET),
            Some(state),
            Some(&known),
            Some(a),
            withdrawn,
        )
        .map_err(|e| format!("{e:?}"))?;
        finding = result;
        if finding == CheckResult::SupportsDurability {
            let recall = prediction.ok_or("missing forecast")?.recall;
            // Compare one full-weight successful clean check, using the V2 success update.
            // Other observations can inform teaching but cannot extend this positive booking.
            state.stability_days =
                (state.stability_days * (1.2 * (1.0 - recall)).exp()).clamp(1.0, 3650.0);
            state.verified = cutoff;
            state.had_durable_success = true;
        }
    }
    let known: Vec<_> = exposures
        .iter()
        .copied()
        .filter(|e| e.cell == CELL && e.at.day <= as_of)
        .collect();
    let forecast = |day| {
        read(
            CELL,
            Moment {
                day,
                sequence: u64::MAX,
            },
            Some(TARGET),
            Some(state),
            Some(&known),
            None,
            withdrawn,
        )
        .map_err(|e| format!("{e:?}"))?
        .0
        .ok_or("missing forecast".to_owned())
    };
    let current = forecast(as_of)?;
    // The memory threshold crossing (refresh) and a possible clean durability
    // reading answer different questions. Compute the latter against the fixed target.
    let due = state.verified.day
        + (9.0 * state.stability_days * (1.0 / state.threshold - 1.0)).ceil() as u32;
    let mut earliest = as_of;
    loop {
        let f = forecast(earliest)?;
        if f.days_since_exposure
            .is_some_and(|d| f64::from(d) >= f.required_gap_days)
        {
            break;
        }
        earliest = earliest.checked_add(1).ok_or("calendar exhausted")?;
    }
    Ok(Booking {
        finding,
        stability: state.stability_days,
        recall: current.recall,
        offer_refresh: as_of.max(due),
        earliest_spaced_check: earliest,
    })
}

pub(super) fn run() -> Result<(), String> {
    let active = Withdrawn::new(CELL.scope);
    let clean = response(30);
    let assisted = Attempt {
        assistance: Assistance::Reported,
        ..clean.clone()
    };
    let duplicate = Attempt {
        fresh: near_duplicate_is_fresh()?,
        ..clean.clone()
    };
    let early = Attempt {
        question: Question::Practice,
        ..response(5)
    };
    println!("case\tas of\tlast finding\tstability\trecall\trefresh eligible\tclean check eligible\tnext refresh slot\tnext clean slot");
    let show = |label: &str,
                now,
                attempts: &[Attempt],
                withdrawn: &Withdrawn,
                days: &[u32]|
     -> Result<Booking, String> {
        let exposed: Vec<_> = days
            .iter()
            .map(|&day| Exposure {
                cell: CELL,
                at: moment(day),
            })
            .collect();
        let b = project(attempts, withdrawn, now, &exposed)?;
        // The visit just ended (or was missed). Future opportunities are the booked
        // day-21 and day-30 windows, then a weekly calendar, assuming attendance.
        let next_slot = |eligible| {
            std::iter::once(21)
                .chain((30..).step_by(7))
                .find(|&d| d > now && d >= eligible)
                .expect("bounded demo dates")
        };
        println!(
            "{label}\t{now}\t{:?}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{}",
            b.finding,
            b.stability,
            b.recall,
            b.offer_refresh,
            b.earliest_spaced_check,
            next_slot(b.offer_refresh),
            next_slot(b.earliest_spaced_check)
        );
        Ok(b)
    };
    show(
        "clean day 30",
        30,
        std::slice::from_ref(&clean),
        &active,
        &[30],
    )?;
    show("assisted day 30", 30, &[assisted], &active, &[30])?;
    show("early independent practice", 5, &[early], &active, &[5])?;
    show("no-show", 30, &[], &active, &[])?;
    show(
        "catalog repeat used as practice",
        30,
        &[duplicate],
        &active,
        &[30],
    )?;
    // One recorded-time change, two contemporaneous reads. Occurrence time remains day 30.
    let changes = [
        (moment(30), None, clean.clone()),
        (
            Moment {
                day: 40,
                sequence: 2,
            },
            Some(clean.id),
            Attempt {
                id: 31,
                assistance: Assistance::Reported,
                ..clean.clone()
            },
        ),
    ];
    let as_known = |label: &str, cutoff: Moment| {
        let mut withdrawn = Withdrawn::new(CELL.scope);
        let mut attempts = Vec::new();
        for (recorded, supersedes, attempt) in &changes {
            if *recorded <= cutoff {
                if let Some(id) = supersedes {
                    withdrawn.attempts.insert(*id);
                }
                attempts.push(attempt.clone());
            }
        }
        show(label, cutoff.day, &attempts, &withdrawn, &[30])
    };
    let before = as_known(
        "day 40 before correction",
        Moment {
            day: 40,
            sequence: 1,
        },
    )?;
    let after = as_known(
        "day 40 learned of help",
        Moment {
            day: 40,
            sequence: 2,
        },
    )?;
    if changes[0].2 != clean || before.finding == after.finding {
        return Err("correction did not change the reading while retaining the observation".into());
    }
    let wrong_cell = Attempt {
        scope: CELL.scope,
        outcome: OutcomeId(3),
        id: 32,
        ..changes[1].2.clone()
    };
    let unchanged = show(
        "another skill recorded help",
        40,
        &[clean, wrong_cell],
        &Withdrawn::new(CELL.scope),
        &[30],
    )?;
    if unchanged != before {
        return Err("another skill changed this booking".into());
    }
    Ok(())
}
