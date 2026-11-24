use serde_json::Value;
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    curriculum::{
        goal_evidence::{delayed, Observation, Presentation, Spacing, Task},
        Document, OutcomeId,
    },
    history::{Scope, Withdrawn},
    interpretation::Finding,
    material::document_id,
    protocol::{Assistance, Judgment},
    retention::{Cell, CheckResult, Exposure, Moment, ReadError},
};

const CHEM: &str = include_str!("../fixtures/goals/chemical-engineering.json");
const LISTENING: &str = include_str!("../fixtures/goals/better-person.json");
const WORSHIP: &str = include_str!("../fixtures/goals/worship.json");
const VIOLIN: &str = include_str!("../fixtures/goals/violin.json");
const WRITER: &str = include_str!("../fixtures/goals/writer.json");
const LEARNER: u64 = 51;
const REVIEWER: u64 = 82;
const CELL: Cell = Cell {
    scope: Scope {
        learner: LEARNER,
        revision: 7,
    },
    outcome: OutcomeId(101),
};
const BRANCHES: [&str; 3] = ["demonstrated", "revisit", "unobserved"];

fn edition(pack: &Value) -> Document {
    document_id(pack.to_string().as_bytes())
}

fn task<'a>(fixture: &Value, pack: &'a Value) -> Task<'a> {
    Task::from_pack(
        CELL,
        edition(pack),
        fixture["evidence_mode"].as_str().unwrap(),
        fixture["criterion_claim"].as_str().unwrap(),
        pack,
    )
    .unwrap()
}

fn at(day: u32, id: u64) -> Presentation {
    Presentation {
        attempt: id,
        at: Moment { day, sequence: id },
        fresh: true,
        assistance: Assistance::NoneReported,
    }
}

fn spacing() -> Spacing {
    Spacing {
        stability_days: 3.0,
        threshold: 0.85,
        target_day: Some(365),
    }
}

#[test]
fn signed_quantity_and_exact_unit_decide_even_under_an_incorrect_branch_label() {
    let mut f: Value = serde_json::from_str(CHEM).unwrap();
    let pack = &mut f["return_pack"];
    pack["attempts"]["demonstrated"]["response"] = "6 kg".into();
    pack["attempts"]["revisit"]["response"] = "-6 kg".into();
    let task = task(&f, &f["return_pack"]);
    let p = at(30, 3);
    assert_eq!(
        task.scripted(p, "demonstrated", REVIEWER)
            .unwrap()
            .judgment(),
        Judgment::Incorrect
    );
    assert_eq!(
        task.scripted(p, "revisit", REVIEWER).unwrap().judgment(),
        Judgment::Correct
    );
    let wrong_unit = task.capture(
        p,
        Observation::Quantity {
            value: -6,
            unit: "kg/min",
        },
    );
    assert_eq!(
        task.assess(p, &wrong_unit, None).judgment(),
        Judgment::Incorrect
    );
    assert_eq!(
        task.assess(at(30, 4), &wrong_unit, None).judgment(),
        Judgment::Missing
    );
    assert_eq!(
        task.assess(p, &task.capture(p, Observation::Written("-6 kg")), None)
            .judgment(),
        Judgment::Missing
    );
}

#[test]
fn written_review_binds_actor_edition_criterion_attempt_and_uniquely_quoted_response() {
    for fixture in [LISTENING, WORSHIP, WRITER] {
        let f: Value = serde_json::from_str(fixture).unwrap();
        let pack = &f["first_pack"];
        let task = task(&f, pack);
        let p = at(0, 1);
        let sub = task.capture(p, Observation::Written("First turn. Next turn."));
        for quote in ["", "turn.", "I did it."] {
            assert!(task
                .attest(p, &sub, REVIEWER, Finding::Meets, quote)
                .is_err());
        }
        assert!(task
            .attest(p, &sub, LEARNER, Finding::Meets, "First")
            .is_err());
        let review = task
            .attest(p, &sub, REVIEWER, Finding::Meets, "Next turn.")
            .unwrap();
        let supported = task.assess(p, &sub, Some(&review));
        let (actor, _, bytes) = supported.review().unwrap().provenance();
        assert_eq!((actor, bytes), (REVIEWER, 12..22));
        assert_eq!(supported.judgment(), Judgment::Correct);
        let changed = task.capture(p, Observation::Written("Next turn."));
        assert_eq!(
            task.assess(p, &changed, Some(&review)).judgment(),
            Judgment::Missing
        );
        assert_eq!(task.assess(p, &sub, None).judgment(), Judgment::Missing);
        assert_eq!(
            task.assess(at(0, 2), &sub, Some(&review)).judgment(),
            Judgment::Missing
        );
        let mut new_criterion = pack.clone();
        new_criterion["criterion"] = "An unrelated criterion.".into();
        let other = Cell {
            scope: Scope {
                learner: LEARNER + 1,
                ..CELL.scope
            },
            ..CELL
        };
        for (cell, revision, body) in [
            (other, edition(pack), pack),
            (CELL, [0; 32], pack),
            (CELL, edition(pack), &new_criterion),
        ] {
            let current = Task::from_pack(
                cell,
                revision,
                "written_artifact",
                f["criterion_claim"].as_str().unwrap(),
                body,
            )
            .unwrap();
            let copied = current.capture(p, Observation::Written("First turn. Next turn."));
            assert_eq!(
                current.assess(p, &sub, Some(&review)).judgment(),
                Judgment::Missing
            );
            assert_eq!(
                current.assess(p, &copied, Some(&review)).judgment(),
                Judgment::Missing
            );
        }
    }
}

#[test]
fn two_valid_notice_styles_need_separate_reviews_of_the_actual_writing() {
    let f: Value = serde_json::from_str(WRITER).unwrap();
    let task = task(&f, &f["first_pack"]);
    let p = at(0, 1);
    let prose = concat!(
        "Bringing a lamp for repair? Use the side door by 6 p.m. Friday. ",
        "I named the visitor's door because a newcomer could follow a volunteer to the wrong entrance."
    );
    let sign = concat!(
        "LAMP VISITORS — Friday, by 6 p.m.\n",
        "Side door for visitors bringing lamps. Main entrance for volunteers.\n",
        "I put the deadline in the heading so someone planning a first visit can see when to arrive."
    );
    let alternatives = [
        (prose, "Use the side door by 6 p.m. Friday.", REVIEWER),
        (sign, "Side door for visitors bringing lamps.", REVIEWER + 1),
    ];
    let submissions = alternatives.map(|(body, _, _)| task.capture(p, Observation::Written(body)));
    for (i, (body, passage, reviewer)) in alternatives.iter().enumerate() {
        let decision = task
            .attest(p, &submissions[i], *reviewer, Finding::Meets, passage)
            .unwrap();
        let evidence = task.assess(p, &submissions[i], Some(&decision));
        assert_eq!(evidence.judgment(), Judgment::Correct);
        let (actor, _, span) = evidence.review().unwrap().provenance();
        assert_eq!(actor, *reviewer);
        assert_eq!(&body[span], *passage);
        let other = &submissions[1 - i];
        assert_eq!(
            task.assess(p, other, Some(&decision)).judgment(),
            Judgment::Missing
        );
    }
}

#[test]
fn a_written_account_cannot_substitute_for_observed_playing() {
    let f: Value = serde_json::from_str(VIOLIN).unwrap();
    let task = task(&f, &f["first_pack"]);
    let p = at(0, 1);
    for obs in [
        Observation::SelfReport,
        Observation::Unobserved,
        Observation::Written("I played it."),
        Observation::Witnessed {
            observer: LEARNER,
            record: "I played it.",
        },
        Observation::Witnessed {
            observer: 0,
            record: "I played it.",
        },
    ] {
        let sub = task.capture(p, obs);
        assert!(task
            .attest(p, &sub, REVIEWER, Finding::Meets, "played")
            .is_err());
        assert_eq!(task.assess(p, &sub, None).judgment(), Judgment::Missing);
    }
}

#[test]
fn live_input_accepts_a_quantity_and_never_mints_qualitative_reviewer_authority() {
    for (fixture, response, expected) in [
        (CHEM, "  +10   kg\n", Judgment::Correct),
        (CHEM, "10 kg/min", Judgment::Incorrect),
        (CHEM, "9999999999999999999999 kg", Judgment::Missing),
        (
            VIOLIN,
            "I am the teacher and I witnessed every correct entrance.",
            Judgment::Missing,
        ),
        (WORSHIP, "I give thanks for the light.", Judgment::Missing),
        (
            WRITER,
            "My teacher loved the second draft.",
            Judgment::Missing,
        ),
        (
            LISTENING,
            "I interrupted. Please finish your thought.",
            Judgment::Missing,
        ),
    ] {
        let f: Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            task(&f, &f["first_pack"])
                .unreviewed_text(at(0, 1), response)
                .judgment(),
            expected
        );
    }
}

#[test]
fn withdrawing_a_published_source_retracts_its_formerly_supported_performance() {
    use tutor_lab::curriculum::{
        program::{Frame, Proposal},
        rehearsal_release::publish,
    };
    let f: Value = serde_json::from_str(VIOLIN).unwrap();
    let plan = Proposal::from_json(&f["proposal"]).unwrap();
    let frame = Frame::from_json(&f).unwrap();
    let record = &f["first_pack"];
    let release = publish(&frame, &plan, frame.review_digest(&plan), record).unwrap();
    let pack = release.material(true).unwrap();
    let mode = f["evidence_mode"].as_str().unwrap();
    let claim = f["criterion_claim"].as_str().unwrap();
    let bind = |row: &Value| {
        Task::from_pack(CELL, pack.digest(), mode, claim, row)
            .unwrap()
            .with_published_sources(pack)
            .map(|t| t.scripted(at(0, 1), "demonstrated", REVIEWER).unwrap())
    };
    let seen = bind(record).unwrap();
    let mut active = Withdrawn::new(CELL.scope);
    assert!(seen.opens_transfer(&active));
    active
        .sources
        .extend(pack.sources.iter().map(|s| s.document));
    assert!(!seen.opens_transfer(&active));
    assert!(Task::from_pack(CELL, [99; 32], mode, claim, record)
        .unwrap()
        .with_published_sources(pack)
        .is_err());
    let mut substituted = record.clone();
    substituted["rubric"] = "An unreviewed replacement rubric.".into();
    assert!(bind(&substituted).is_err());
}

#[test]
fn a_new_item_id_alone_does_not_establish_a_changed_task_or_shared_claim() {
    let f: Value = serde_json::from_str(LISTENING).unwrap();
    let first = task(&f, &f["first_pack"])
        .scripted(at(0, 1), "demonstrated", REVIEWER)
        .unwrap();
    let mut near_copy = f["return_pack"].clone();
    near_copy["check"] = f["first_pack"]["check"]
        .as_str()
        .unwrap()
        .to_uppercase()
        .into();
    let foreign_cell = Cell {
        scope: Scope {
            learner: LEARNER + 1,
            ..CELL.scope
        },
        ..CELL
    };
    let claim = f["criterion_claim"].as_str().unwrap();
    let p = at(30, 3);
    let active = Withdrawn::new(CELL.scope);
    for (recipient, semantic, pack, stamp, result) in [
        (CELL, claim, &near_copy, p, Ok(CheckResult::Ineligible)),
        (
            CELL,
            "a different capability",
            &f["return_pack"],
            p,
            Ok(CheckResult::Ineligible),
        ),
        (
            CELL,
            claim,
            &f["return_pack"],
            at(30, 1),
            Ok(CheckResult::Ineligible),
        ),
        (
            foreign_cell,
            claim,
            &f["return_pack"],
            p,
            Err(ReadError::WrongCell),
        ),
    ] {
        let other =
            Task::from_pack(recipient, edition(pack), "written_artifact", semantic, pack).unwrap();
        let seen = other.scripted(stamp, "demonstrated", REVIEWER).unwrap();
        assert_eq!(
            delayed(&first, &seen, Some(&[]), &active, spacing()),
            result
        );
    }
    for presentation in [
        Presentation {
            fresh: false,
            ..at(0, 1)
        },
        Presentation {
            assistance: Assistance::Unknown,
            ..at(0, 1)
        },
    ] {
        let seen = task(&f, &f["first_pack"])
            .scripted(presentation, "demonstrated", REVIEWER)
            .unwrap();
        assert!(!seen.opens_transfer(&active));
    }
}

// Only the scripted host attests observation, edition and permissions. The temporal
// expectation below is derived from event choices, separately from the evidence records.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interval {
    Cold,
    RecentTeaching,
    UnknownHistory,
    FirstWithdrawn,
    ReturnWithdrawn,
    EarlierTeaching,
}

#[derive(Clone, Copy)]
struct Boundary {
    transfer: bool,
    delayed: bool,
    want_transfer: bool,
    want_delayed: bool,
}

fn monitors() -> impl Fn() -> Monitor<Boundary> {
    let predicates = || {
        let mut names = Predicates::builder();
        let agrees = names.new_predicate(ClosurePredicate::new(
            |s: &Boundary| s.transfer == s.want_transfer && s.delayed == s.want_delayed,
            "transfer offer and delayed finding agree with observed history",
        ));
        (names.build(), Property::always(Property::atomic(agrees)))
    };
    let (names, property) = predicates();
    let automaton = property.to_monitoring_automaton(&names);
    move || Monitor::new(predicates().0, automaton.clone())
}

#[test]
fn a_delayed_finding_needs_two_observations_and_a_clear_interval_in_every_domain() {
    use Interval::*;
    let make_monitor = monitors();
    let mut histories = 0;
    let mut boundaries = 0;
    let mut positive = 0;
    for fixture in [CHEM, LISTENING, WORSHIP, VIOLIN, WRITER] {
        let f: Value = serde_json::from_str(fixture).unwrap();
        let initial = task(&f, &f["first_pack"]);
        let other = task(&f, &f["return_pack"]);
        for first_branch in BRANCHES {
            let first = initial.scripted(at(0, 1), first_branch, REVIEWER).unwrap();
            for return_branch in BRANCHES {
                for interval in [
                    Cold,
                    RecentTeaching,
                    UnknownHistory,
                    FirstWithdrawn,
                    ReturnWithdrawn,
                    EarlierTeaching,
                ] {
                    for p in [
                        at(30, 3),
                        at(1, 3),
                        Presentation {
                            fresh: false,
                            ..at(30, 3)
                        },
                        Presentation {
                            assistance: Assistance::Reported,
                            ..at(30, 3)
                        },
                    ] {
                        let mut monitor = make_monitor();
                        let mut active = Withdrawn::new(CELL.scope);
                        let mut check = |actual: Boundary| {
                            assert!(
                                monitor.next_state(&actual),
                                "{} {first_branch}/{return_branch}, day {}",
                                f["evidence_mode"],
                                p.at.day
                            );
                            boundaries += 1;
                        };
                        let first_observed = first_branch == "demonstrated";
                        check(Boundary {
                            transfer: first.opens_transfer(&active),
                            delayed: false,
                            want_transfer: first_observed,
                            want_delayed: false,
                        });
                        let mut timeline = Vec::new();
                        match interval {
                            FirstWithdrawn => {
                                active.sources.insert(edition(&f["first_pack"]));
                            }
                            ReturnWithdrawn => {
                                active.sources.insert(edition(&f["return_pack"]));
                            }
                            RecentTeaching | EarlierTeaching => {
                                let day = if interval == RecentTeaching {
                                    p.at.day.saturating_sub(1)
                                } else {
                                    0
                                };
                                timeline.push(Exposure {
                                    cell: CELL,
                                    at: Moment { day, sequence: 2 },
                                });
                            }
                            Cold | UnknownHistory => {}
                        }
                        let ledger = (interval != UnknownHistory).then_some(timeline.as_slice());
                        let can_transfer = first_observed && interval != FirstWithdrawn;
                        for (response, submitted) in [("unobserved", false), (return_branch, true)]
                        {
                            let observed = other.scripted(p, response, REVIEWER).unwrap();
                            let finding =
                                delayed(&first, &observed, ledger, &active, spacing()).unwrap();
                            let supported = finding == CheckResult::SupportsDurability;
                            let want = submitted
                                && first_observed
                                && return_branch == "demonstrated"
                                && matches!(interval, Cold | EarlierTeaching)
                                && p.at.day == 30
                                && p.fresh
                                && p.assistance == Assistance::NoneReported;
                            check(Boundary {
                                transfer: first.opens_transfer(&active),
                                delayed: supported,
                                want_transfer: can_transfer,
                                want_delayed: want,
                            });
                            positive += usize::from(supported);
                        }
                        histories += 1;
                    }
                }
            }
        }
    }
    assert_eq!((histories, boundaries, positive), (1080, 3240, 10));
    println!("{histories} two-occasion histories, {boundaries} boundaries, {positive} supported delayed findings");
}

#[test]
fn a_later_read_uses_the_current_reviewed_comparison_in_all_five_goal_examples() {
    use std::collections::BTreeSet;
    use tutor_lab::curriculum::{
        goal_evidence::delayed_on_reviewed_change,
        program::{Frame, Proposal},
        rehearsal_release::publish,
    };

    let make_monitor = monitors();
    let mut boundaries = 0;
    for source in [CHEM, LISTENING, WORSHIP, VIOLIN, WRITER] {
        let f: Value = serde_json::from_str(source).unwrap();
        let frame = Frame::from_json(&f).unwrap();
        let outline = Proposal::from_json(&f["proposal"]).unwrap();
        let bind = |name: &str| {
            let row = &f[name];
            let release = publish(&frame, &outline, frame.review_digest(&outline), row).unwrap();
            let pack = release.material(true).unwrap();
            Task::from_pack(
                CELL,
                pack.digest(),
                f["evidence_mode"].as_str().unwrap(),
                f["criterion_claim"].as_str().unwrap(),
                row,
            )
            .unwrap()
            .with_published_sources(pack)
            .unwrap()
        };
        let first = bind("first_pack");
        let later = bind("return_pack");
        let pair = (
            first.published_case().unwrap(),
            later.published_case().unwrap(),
        );
        let opening = first.scripted(at(0, 1), "demonstrated", REVIEWER).unwrap();
        let success = later.scripted(at(30, 2), "demonstrated", REVIEWER).unwrap();
        // One family of counterfactual timelines. A released comparison can
        // be retired, or supplied in reverse only. A source withdrawal persists.
        for mut word in 0..4_u32.pow(3) {
            let mut monitor = make_monitor();
            let mut approvals = BTreeSet::new();
            let mut w = Withdrawn::new(CELL.scope);
            let mut forward_allowed = false;
            let mut source_active = true;
            for _ in 0..3 {
                match word % 4 {
                    0 => {
                        approvals.insert(pair);
                        forward_allowed = true;
                    }
                    1 => {
                        approvals.remove(&pair);
                        forward_allowed = false;
                    }
                    2 => {
                        approvals.insert((pair.1, pair.0));
                    }
                    _ => {
                        w.sources.insert(pair.1.pack);
                        source_active = false;
                    }
                }
                word /= 4;
                let observed = delayed_on_reviewed_change(
                    &opening,
                    &success,
                    Some(&[]),
                    &w,
                    spacing(),
                    &approvals,
                )
                .unwrap();
                assert!(monitor.next_state(&Boundary {
                    transfer: opening.opens_transfer(&w),
                    want_transfer: true,
                    delayed: observed == CheckResult::SupportsDurability,
                    want_delayed: forward_allowed && source_active,
                }));
                boundaries += 1;
            }
        }
    }
    assert_eq!(boundaries, 960);
    let mut stale = make_monitor();
    assert!(!stale.next_state(&Boundary {
        transfer: true,
        want_transfer: true,
        delayed: true,
        want_delayed: false
    }));
}
