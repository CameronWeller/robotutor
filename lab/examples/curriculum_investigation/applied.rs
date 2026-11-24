//! Compare the observable work in water-inventory and instrumental-performance exercises.
use super::Course;
use tutor_lab::{
    curriculum::{
        goal_evidence::{check_quantity_text, delayed, Observation, Presentation, Spacing, Task},
        rehearsal_release::publish,
        OutcomeId,
    },
    history::{Scope, Withdrawn},
    interpretation::Finding,
    protocol::Assistance,
    retention::{Cell, Moment},
};

fn presented(day: u32, assistance: Assistance) -> Presentation {
    Presentation {
        attempt: u64::from(day) + 1,
        at: Moment { day, sequence: 1 },
        fresh: true,
        assistance,
    }
}

fn task<'a>(course: &'a Course, key: &str, cell: Cell) -> Result<Task<'a>, String> {
    let record = &course.data[key];
    let release = publish(
        &course.frame,
        &course.plan,
        course.frame.review_digest(&course.plan),
        record,
    )?;
    let pack = release.material(true)?;
    Task::from_pack(
        cell,
        pack.digest(),
        course.data["evidence_mode"]
            .as_str()
            .ok_or("missing modality")?,
        course.data["criterion_claim"]
            .as_str()
            .ok_or("missing criterion")?,
        record,
    )?
    .with_published_sources(pack)
}

fn water(course: &Course, cell: Cell) -> Result<(), String> {
    let reviewed = task(course, "return_pack", cell)?;
    let p = presented(30, Assistance::NoneReported);
    let e = reviewed.unreviewed_text(p, "-6 kg");
    println!(
        "Released combined-basin return, '-6 kg': {:?}",
        e.judgment()
    );

    // All movements in this counterexample have identified endpoints, unlike
    // an aggregate meter reading. No candidate item is published as new coursework.
    for (name, outside_to_west, west_to_east, east_to_outside) in [
        ("coincident net changes", 2, 2, 8),
        ("only the internal transfer changes", 2, 11, 8),
        ("only the external inflow increases", 5, 11, 8),
    ] {
        let combined = outside_to_west - east_to_outside;
        let east_only = west_to_east - east_to_outside;
        let constant_from_first = "-6 kg";
        println!("{name}: combined={combined}, east_only={east_only}, combined answer={:?}, east-only answer={:?}, repeating -6 kg={:?}",
            check_quantity_text(combined,"kg",&format!("{combined} kg")),
            check_quantity_text(combined,"kg",&format!("{east_only} kg")),
            check_quantity_text(combined,"kg",constant_from_first));
    }
    Ok(())
}

fn violin(course: &Course, cell: Cell) -> Result<(), String> {
    let opening = task(course, "first_pack", cell)?;
    let returning = task(course, "return_pack", cell)?;
    let p = presented(0, Assistance::NoneReported);
    let first = opening.scripted(p, "demonstrated", 70)?;
    let body = course.data["return_pack"]["attempts"]["demonstrated"]["response"]
        .as_str()
        .ok_or("missing witness example")?;
    for (name, obs, review, help, exposure) in [
        (
            "learner pastes teacher's wording",
            Observation::Written(body),
            true,
            Assistance::NoneReported,
            true,
        ),
        (
            "learner speaks as their own witness",
            Observation::Witnessed {
                observer: cell.scope.learner,
                record: body,
            },
            true,
            Assistance::NoneReported,
            true,
        ),
        (
            "separate witness, review pending",
            Observation::Witnessed {
                observer: 71,
                record: body,
            },
            false,
            Assistance::NoneReported,
            true,
        ),
        (
            "consented witness and separate review",
            Observation::Witnessed {
                observer: 71,
                record: body,
            },
            true,
            Assistance::NoneReported,
            true,
        ),
        (
            "same report with a cue disclosed",
            Observation::Witnessed {
                observer: 71,
                record: body,
            },
            true,
            Assistance::Reported,
            true,
        ),
        (
            "same report, unknown practice interval",
            Observation::Witnessed {
                observer: 71,
                record: body,
            },
            true,
            Assistance::NoneReported,
            false,
        ),
    ] {
        let q = presented(30, help);
        let sub = returning.capture(q, obs);
        let decision = review
            .then(|| returning.attest(q, &sub, 70, Finding::Meets, "first click of bar three"))
            .transpose();
        let receipt = decision.as_ref().ok().and_then(|r| r.as_ref());
        let current = returning.assess(q, &sub, receipt);
        let later = delayed(
            &first,
            &current,
            exposure.then_some(&[][..]),
            &Withdrawn::new(cell.scope),
            Spacing {
                stability_days: 3.0,
                threshold: 0.85,
                target_day: Some(365),
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        println!(
            "{name}: receipt={}, current={:?}, delayed={later:?}",
            receipt.is_some(),
            current.judgment()
        );
    }
    Ok(())
}

pub(super) fn examine(course: &Course, subject: &str) -> Result<(), String> {
    let cell = Cell {
        scope: Scope {
            learner: 42,
            revision: 1,
        },
        outcome: OutcomeId(course.first_unit()?.id),
    };
    match subject {
        "chemical-engineering" => water(course, cell),
        "violin" => violin(course, cell),
        _ => Err("applied evidence compares chemical-engineering and violin".into()),
    }
}
