//! Queue a public key adjudication separately from each private appeal.
use super::*;
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};

type Edition = (u64, [u8; 32]);
type ResultOf<T> = Result<T, String>;

fn edition(row: &Released) -> Edition {
    (row.work.release, row.receipt)
}

// Deliberately emulate a historical release whose human sign-off missed a bad key.
// The independent arithmetic checker would reject this author input on today's path.
fn faulty_history(tank: &Released) -> ResultOf<Released> {
    let mut pack = tank
        .batch
        .available(tank.work, &tank.pins)
        .map_err(|e| format!("{e:?}"))?
        .clone();
    let bad_key = tank.key * 2;
    pack.rubric = format!("Accept {bad_key:+} kg for this ledger and boundary.");
    let mut work = tank.work;
    work.release = 81;
    let mut batch = Batch::new(work.release, [work]).map_err(|e| format!("{e:?}"))?;
    let digest = batch
        .stage(vec![Candidate {
            work,
            pack: pack.clone(),
            author: 1,
        }])
        .map_err(|e| format!("{e:?}"))?
        .remove(0)
        .map_err(|e| format!("{e:?}"))?;
    batch
        .approve(work, digest, &ReviewerGrant { actor: 2, work }, &tank.pins)
        .map_err(|e| format!("{e:?}"))?;
    let mut record = tank.record.clone();
    record["rubric"] = json!(pack.rubric);
    record["quantity"]["value"] = json!(bad_key);
    Ok(Released {
        receipt: digest,
        work,
        batch,
        pins: tank.pins.clone(),
        record,
        key: bad_key,
    })
}

struct Seat<'a> {
    id: u32,
    original: &'a Released,
    checked: &'a Released,
    answer: &'a str,
    helped: bool,
    refresher: bool,
    grant_day: u32,
}
impl Seat<'_> {
    fn cell(&self) -> Cell {
        Cell {
            scope: Scope {
                learner: self.id as _,
                revision: 1,
            },
            ..cell()
        }
    }
    fn presentation(&self) -> Presentation {
        visit(
            1,
            if self.helped {
                Assistance::Reported
            } else {
                Assistance::NoneReported
            },
        )
    }
}

// No learner, response, exposure or grade is stored in this public finding.
#[derive(Clone, Copy)]
struct Finding {
    original: Edition,
    replacement: Edition,
}
fn inspect(original: &Released, replacement: &Released) -> ResultOf<Finding> {
    let old = original
        .batch
        .available(original.work, &original.pins)
        .map_err(|e| format!("{e:?}"))?;
    let new = replacement
        .batch
        .available(replacement.work, &replacement.pins)
        .map_err(|e| format!("{e:?}"))?;
    if old.check != new.check
        || old.criterion != new.criterion
        || old.sources != new.sources
        || original.work.brief != replacement.work.brief
    {
        return Err("different original question, brief, criterion or ledger".into());
    }
    // Recompute from the independent movement ledger of this exact rendered question.
    let ledgers = ledgers();
    let ledger = ledgers
        .iter()
        .find(|l| l.question() == new.check)
        .ok_or("no matching movement ledger")?;
    if ledger.calculation().map(|(kg, _)| kg) != Some(replacement.key) {
        return Err("unverified replacement key".into());
    }
    Ok(Finding {
        original: edition(original),
        replacement: edition(replacement),
    })
}

struct Delivery {
    seat: u32,
    finding: Finding,
    day: u32,
    corrected: Judgment,
    text: String,
}
fn individual(seat: &Seat, finding: Finding, day: u32) -> ResultOf<Delivery> {
    if finding.original != edition(seat.original) || finding.replacement != edition(seat.checked) {
        return Err("wrong edition for this learner".into());
    }
    let pins = if day >= seat.grant_day {
        seat.original.pins.clone()
    } else {
        BTreeSet::new()
    };
    let old = seat
        .original
        .assess_for(seat.cell(), seat.presentation(), seat.answer, &pins)?
        .0
        .judgment();
    let corrected = seat
        .checked
        .assess_for(seat.cell(), seat.presentation(), seat.answer, &pins)?
        .0
        .judgment();
    let operation = if seat.original.key != seat.checked.key {
        "corrected"
    } else {
        "confirmed"
    };
    let text=format!("The review {operation} the key at {:+} kg. Your submitted {} was {old:?} under the old key and is {corrected:?} on review.",seat.checked.key,seat.answer);
    Ok(Delivery {
        seat: seat.id,
        finding,
        day,
        corrected,
        text,
    })
}

struct Boundary {
    paid: bool,
    grant: bool,
    exact: bool,
    personal: bool,
    closed: bool,
}
fn debt_monitor() -> Monitor<Boundary> {
    let mut p = Predicates::builder();
    let safe = p.new_predicate(ClosurePredicate::new(
        |x: &Boundary| !x.closed || (x.paid && x.grant && x.exact && x.personal),
        "closing an appeal needs paid individual review, grant, matching edition and learner",
    ));
    let p = p.build();
    let m = Property::always(Property::atomic(safe)).to_monitoring_automaton(&p);
    Monitor::new(p, m)
}

struct Course<'a> {
    tank: &'a Released,
    alias: &'a Released,
    fresh: &'a Released,
    readers: &'a Value,
}
fn next(seat: &Seat, d: &Delivery, day: u32, course: &Course) -> ResultOf<Value> {
    let cell = seat.cell();
    let (prior, old_case) =
        seat.checked
            .assess_for(cell, seat.presentation(), seat.answer, &seat.checked.pins)?;
    let guided = d.corrected != Judgment::Correct || seat.helped;
    let (id, row) = if guided {
        ("T3", course.alias)
    } else {
        ("P1", course.fresh)
    };
    let packet = row
        .batch
        .available(row.work, &row.pins)
        .map_err(|e| format!("{e:?}"))?;
    if course.readers[id]["packet"]["question"] != packet.check {
        return Err("archived reader saw a different question".into());
    }
    let response = course.readers[id]["response"]["display"]
        .as_str()
        .ok_or("missing archived answer")?;
    let help = if guided {
        Assistance::Reported
    } else {
        Assistance::NoneReported
    };
    let (later, new_case) = row.assess_for(cell, visit(day, help), response, &row.pins)?;
    let mut contacts = vec![Exposure {
        cell,
        at: Moment {
            day: d.day,
            sequence: 2,
        },
    }];
    if seat.refresher {
        contacts.push(Exposure {
            cell,
            at: Moment {
                day: 29,
                sequence: 1,
            },
        });
    }
    let approved = if old_case
        == course
            .tank
            .assess_for(cell, seat.presentation(), seat.answer, &course.tank.pins)?
            .1
        && id == "P1"
    {
        [(old_case, new_case)].into()
    } else {
        BTreeSet::new()
    };
    let evidence = delayed_on_reviewed_change(
        &prior,
        &later,
        Some(&contacts),
        &Withdrawn::new(cell.scope),
        Spacing {
            stability_days: 5.,
            threshold: 0.85,
            target_day: None,
        },
        &approved,
    )
    .map_err(|e| format!("{e:?}"))?;
    let follow= match evidence {
        CheckResult::SupportsDurability=>"Offer another inventory problem or a choice of topic.",
        CheckResult::InsufficientSpacing=>"Leave the changed-boundary check open after the recent contact.",
        _ if guided=>"Rework the single-tank balance together. Keep a fresh independent check open.",
        _=>"Record the response. The approved transfer comparison still needs a matching first task.",
    };
    Ok(
        json!({"day":day,"task":id,"question":packet.check,"answer":response,"judgment":format!("{:?}",later.judgment()),
        "assistance":format!("{help:?}"),"spacing":format!("{evidence:?}"),"follow_up":follow}),
    )
}

fn schedule(seats: &[Seat], shared: bool, course: &Course) -> ResultOf<Value> {
    let mut cache: Vec<Finding> = vec![];
    let mut deliveries: Vec<Option<Delivery>> = seats.iter().map(|_| None).collect();
    let mut individual_minutes = vec![0; seats.len()];
    let mut next_visits = vec![Value::Null; seats.len()];
    let mut windows = vec![];
    let mut safety = debt_monitor();
    for (day, learning_day) in [(2, 30), (31, 60), (61, 90)] {
        let mut minutes = 0;
        let mut source_checks = 0;
        for (i, s) in seats.iter().enumerate() {
            if deliveries[i].is_some() || day < s.grant_day {
                continue;
            }
            let known = shared
                .then(|| {
                    cache
                        .iter()
                        .find(|r| r.original == edition(s.original))
                        .copied()
                })
                .flatten();
            let cost = if known.is_some() { 2 } else { 6 };
            if minutes + cost > 12 {
                break;
            }
            let finding = if let Some(f) = known {
                f
            } else {
                let f = inspect(s.original, s.checked)?;
                source_checks += 1;
                if shared {
                    cache.push(f);
                }
                f
            };
            minutes += cost;
            individual_minutes[i] += 2;
            deliveries[i] = Some(individual(s, finding, day)?);
        }
        let mut served = vec![];
        let mut owed = vec![];
        for (i, s) in seats.iter().enumerate() {
            let d = deliveries[i].as_ref();
            let row = Boundary {
                closed: d.is_some(),
                paid: individual_minutes[i] == 2,
                grant: day >= s.grant_day,
                exact: d.is_some_and(|d| {
                    d.finding.original == edition(s.original)
                        && d.finding.replacement == edition(s.checked)
                }),
                personal: d.is_some_and(|d| d.seat == s.id),
            };
            if !safety.next_state(&row) {
                return Err("closed appeal without an authorized personal resolution".into());
            }
            if let Some(d) = d {
                if d.day == day {
                    served.push(json!({"learner":s.id,"feedback":d.text}));
                    next_visits[i] = next(s, d, learning_day, course)?;
                }
            } else {
                owed.push(s.id);
            }
        }
        windows.push(json!({"day":day,"minutes":minutes,"source_checks":source_checks,"feedback":served,"owed":owed}));
    }
    if deliveries.iter().any(Option::is_none) {
        return Err("unserved learner after grants restored".into());
    }
    Ok(json!({"windows":windows,"next_visits":next_visits}))
}

pub(super) fn run(author: &str, teaching: &str) -> ResultOf<Value> {
    let [tank, alias, fresh] = commissioned(&receipt(author)?, false)?;
    let faulty = faulty_history(&tank)?;
    let readers = receipt(teaching)?;
    let answer = readers["T1"]["response"]["display"]
        .as_str()
        .ok_or("missing T1 reader")?;
    if readers["T1"]["packet"]["question"] != tank.record["check"] {
        return Err("T1 receipt mismatch".into());
    }
    let c = Course {
        tank: &tank,
        alias: &alias,
        fresh: &fresh,
        readers: &readers,
    };
    let seats: Vec<_> = (1..=6)
        .map(|id| Seat {
            id,
            original: if id == 5 { &alias } else { &faulty },
            checked: if id == 5 { &alias } else { &tank },
            answer: if id == 3 { "+16 kg" } else { answer },
            helped: id == 2,
            refresher: id == 4,
            grant_day: if id == 6 { 61 } else { 1 },
        })
        .collect();
    Ok(
        json!({"fifo":schedule(&seats,false,&c)?,"shared":schedule(&seats,true,&c)?,
        "solo_fifo":schedule(&seats[..1],false,&c)?,"solo_shared":schedule(&seats[..1],true,&c)?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_review_never_resolves_a_private_appeal_alone() {
        for mask in 0..16 {
            let frame = Boundary {
                closed: true,
                paid: mask & 1 != 0,
                grant: mask & 2 != 0,
                exact: mask & 4 != 0,
                personal: mask & 8 != 0,
            };
            assert_eq!(debt_monitor().next_state(&frame), mask == 15);
        }
    }
    #[test]
    fn a_similar_edition_or_an_unavailable_grant_cannot_close_this_appeal() {
        let source = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/foundation/water-authoring.md"
        );
        let [tank, alias, _] = commissioned(&receipt(source).unwrap(), false).unwrap();
        let bad = faulty_history(&tank).unwrap();
        let held = Seat {
            id: 6,
            original: &bad,
            checked: &tank,
            answer: "+8 kg",
            helped: false,
            refresher: false,
            grant_day: 61,
        };
        let right = inspect(&bad, &tank).unwrap();
        assert!(inspect(&bad, &alias).is_err());
        assert!(individual(&held, inspect(&alias, &alias).unwrap(), 61).is_err());
        assert!(individual(&held, right, 31).is_err());
        assert_eq!(
            individual(&held, right, 61).unwrap().corrected,
            Judgment::Correct
        );
    }

    #[test]
    fn grouped_reviews_change_individual_routes_and_preserve_other_editions() {
        let v = run(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../docs/foundation/water-authoring.md"
            ),
            concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/water-teaching.json"),
        )
        .unwrap();
        assert_eq!(v["shared"]["windows"][0]["owed"], json!([5, 6]));
        assert_eq!(v["fifo"]["windows"][0]["owed"], json!([3, 4, 5, 6]));
        assert_eq!(v["solo_fifo"], v["solo_shared"]);
        let n = &v["shared"]["next_visits"];
        assert_eq!(n[0]["spacing"], "SupportsDurability");
        assert_eq!(n[1]["spacing"], "Ineligible");
        assert_eq!(n[2]["task"], "T3");
        assert_eq!(n[3]["spacing"], "InsufficientSpacing");
        assert_eq!(v["fifo"]["next_visits"][3]["spacing"], "SupportsDurability");
        assert_eq!(n[4]["spacing"], "Ineligible");
        assert_eq!(n[5]["day"], 90);
    }
}
