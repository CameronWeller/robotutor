//! Commission a bounded water-only exercise, then verify the ledger independently of its author.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, env, fs};
use tutor_lab::{
    curriculum::{
        creation::{source_manifest, Batch, Candidate, Error, Pack, ReviewerGrant, Work},
        goal_evidence::{
            check_quantity_text, delayed_on_reviewed_change, Evidence, Presentation, PublishedCase,
            Spacing, Task,
        },
        OutcomeId, Span,
    },
    history::{Scope, Withdrawn},
    material::document_id,
    protocol::{Assistance, Judgment},
    retention::{Cell, CheckResult, Exposure, Moment},
};
#[path = "water_author/classroom_appeal.rs"]
mod classroom_appeal;
#[path = "water_author/teaching.rs"]
mod teaching;

fn reviewed() -> Value {
    let raw = include_str!("../fixtures/goals/chemical-engineering.json").replace("\r\n", "\n");
    let reviews: Value =
        serde_json::from_str(include_str!("../fixtures/goals/reviews.json")).unwrap();
    assert_eq!(
        reviews["chemical-engineering"],
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    serde_json::from_str(&raw).unwrap()
}

// Flow facts are independently commissioned. Positive amounts move from `from` to `to`.
#[derive(Clone, Copy)]
struct Flow {
    id: &'static str,
    from: &'static str,
    to: &'static str,
    amount: i64,
    unit: &'static str,
}
struct Ledger {
    id: &'static str,
    boundary: &'static [&'static str],
    minutes: Option<i64>,
    flows: Vec<Flow>,
}
impl Ledger {
    fn wire(&self) -> Value {
        json!({"id":self.id,"boundary":self.boundary,"interval_min":self.minutes,"flows":
        self.flows.iter().map(|f|json!({"id":f.id,"from":f.from,"to":f.to,"amount":f.amount,"unit":f.unit})).collect::<Vec<_>>()})
    }
    fn calculation(&self) -> Option<(i64, Vec<&str>)> {
        let mut kg = 0;
        let mut crossings = Vec::new();
        for f in &self.flows {
            let sign = i64::from(self.boundary.contains(&f.to))
                - i64::from(self.boundary.contains(&f.from));
            if sign != 0 {
                crossings.push(f.id);
                let mass = match f.unit {
                    "kg" => f.amount,
                    "kg/min" => f.amount.checked_mul(self.minutes?)?,
                    _ => return None,
                };
                kg = i64::checked_add(kg, sign.checked_mul(mass)?)?;
            }
        }
        Some((kg, crossings))
    }
    fn question(&self) -> String {
        let records = self
            .flows
            .iter()
            .map(|f| format!("{} to {}: {} {}", f.from, f.to, f.amount, f.unit))
            .collect::<Vec<_>>()
            .join("; ");
        let interval = self
            .minutes
            .map(|m| format!("Over {m} minutes"))
            .unwrap_or_else(|| "During this interval".into());
        format!("Count the water inside {}. {interval}, the complete ledger records {records}. Each rate is constant. What is the signed change in stored water in kg?",
            self.boundary.join(" and "))
    }
}
fn ledgers() -> Vec<Ledger> {
    let flow = |id, from, to, amount, unit| Flow {
        id,
        from,
        to,
        amount,
        unit,
    };
    let tank = |id, minutes, enter, leave| Ledger {
        id,
        minutes,
        boundary: &["west"],
        flows: vec![
            flow("feed", "outside", "west", enter, "kg/min"),
            flow("drain", "west", "outside", leave, "kg/min"),
        ],
    };
    vec![
        tank("T1", Some(4), 4, 2),
        tank("T2", None, 4, 2),
        tank("T3", Some(2), 7, 3),
        Ledger {
            id: "P1",
            minutes: Some(6),
            boundary: &["west", "east"],
            flows: vec![
                flow("feed", "outside", "west", 1, "kg/min"),
                flow("transfer", "west", "east", 9, "kg"),
                flow("drain", "east", "outside", 10, "kg"),
            ],
        },
    ]
}
fn packet(group: &str) -> Result<Value, String> {
    let data = reviewed();
    let ledgers = ledgers();
    let items = match group {
        "first" => &ledgers[..2],
        "return" => &ledgers[2..],
        _ => return Err("group must be first or return".into()),
    };
    Ok(
        json!({"author_instruction":"Write one fluent exercise per supplied ledger asking only for the signed total water inventory change in kg for its given boundary. Preserve the original boundary, interval and movement records in the question. Calculate a proposed key independently; if the evidence cannot determine a quantity, set key_kg to null. List the crossing flow IDs in ledger order. Do not assume missing measurements. Return items in the supplied order. Use only these synthetic water readings and the two reviewed source passages. Do not use tools.",
       "criterion":data["criterion_claim"],"sources":[data["first_pack"]["excerpt"].clone(),data["return_pack"]["excerpt"].clone()],
       "ledgers":items.iter().map(Ledger::wire).collect::<Vec<_>>()}),
    )
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--packet") => println!("{}", packet(&args.next().ok_or("choose a group")?)?),
        Some("--replay") => study(
            &args
                .next()
                .ok_or("supply a receipt JSON file or study Markdown")?,
        )?,
        Some("--teaching-questions") => println!("{}", teaching::questions(&args.next().ok_or("supply author receipts")?)?),
        Some("--teaching-score") => println!("{}", teaching::score(
            &args.next().ok_or("supply author receipts")?,
            &args.next().ok_or("supply teaching receipts")?,
        )?),
        Some("--classroom-appeal") => println!("{}", serde_json::to_string_pretty(&classroom_appeal::run(
            &args.next().ok_or("supply author receipts")?, &args.next().ok_or("supply teaching receipts")?,
        )?).map_err(|e|e.to_string())?),
        _ => return Err("water_author --packet first|return OR --replay RECEIPTS OR --teaching-questions RECEIPTS OR --teaching-score AUTHOR TEACHING OR --classroom-appeal AUTHOR TEACHING".into()),
    }
    Ok(())
}

fn validate(l: &Ledger, v: &Value) -> Result<i64, &'static str> {
    let Some((key, crossings)) = l.calculation() else {
        return Err("interval or units unavailable");
    };
    if v["id"] != l.id || v["boundary"] != json!(l.boundary) {
        return Err("wrong physical boundary or ledger");
    }
    if v["interval_min"] != json!(l.minutes) || v["answer_unit"] != "kg" {
        return Err("wrong duration or answer unit");
    }
    if v["crossing_flows"] != json!(crossings) {
        return Err("wrong boundary crossings");
    }
    if v["key_kg"].as_i64() != Some(key) {
        return Err("incorrect interval change");
    }
    Ok(key)
}

fn span(data: &[u8]) -> Span {
    Span {
        document: document_id(data),
        start: 0,
        end: u32::try_from(data.len()).unwrap(),
    }
}
fn source_span(source: &Value) -> Span {
    span(
        json!([source["source_url"], source["locator"], source["excerpt"]])
            .to_string()
            .as_bytes(),
    )
}

struct Released {
    receipt: [u8; 32],
    work: Work,
    batch: Batch,
    pins: BTreeSet<Span>,
    record: Value,
    key: i64,
}
impl Released {
    fn build(l: &Ledger, v: &Value, ordinal: u64, course: &Value) -> Result<Self, String> {
        let key = validate(l, v)?;
        let primary = if l.boundary.len() == 1 {
            &course["first_pack"]
        } else {
            &course["return_pack"]
        };
        let ledger = l.wire().to_string();
        let pins: BTreeSet<_> = [
            span(ledger.as_bytes()),
            source_span(primary),
            source_span(&course["first_pack"]),
        ]
        .into();
        let criterion = course["criterion_claim"].as_str().unwrap();
        let check = l.question();
        let rubric = format!("Accept {key:+} kg for this ledger and boundary.");
        let pack = Pack {
            lesson: primary["lesson"].as_str().unwrap().into(),
            worked: primary["worked"].as_str().unwrap().into(),
            practice: primary["practice"].as_str().unwrap().into(),
            check: check.clone(),
            criterion: criterion.into(),
            rubric: rubric.clone(),
            sources: pins.clone(),
        };
        let work = Work {
            release: ordinal,
            objective: cell().outcome,
            brief: document_id(ledger.as_bytes()),
            source_manifest: source_manifest(&pins),
        };
        let mut batch = Batch::new(work.release, [work]).map_err(|e| format!("{e:?}"))?;
        let receipt = batch
            .stage(vec![Candidate {
                work,
                pack,
                author: 1,
            }])
            .map_err(|e| format!("{e:?}"))?
            .remove(0)
            .map_err(|e| format!("{e:?}"))?;
        assert_eq!(batch.available(work, &pins), Err(Error::Unapproved));
        // Distinct simulated reviewer in this laboratory. Only the deterministic rendering is admitted.
        batch
            .approve(work, receipt, &ReviewerGrant { actor: 2, work }, &pins)
            .map_err(|e| format!("{e:?}"))?;
        let record = json!({"unit":ordinal,"check":check,"rubric":rubric,"criterion":criterion,"quantity":{"unit":"kg","value":key}});
        Ok(Self {
            receipt,
            work,
            batch,
            pins,
            record,
            key,
        })
    }
    fn assess(
        &self,
        p: Presentation,
        answer: &str,
        current: &BTreeSet<Span>,
    ) -> Result<(Evidence, PublishedCase), String> {
        self.assess_for(cell(), p, answer, current)
    }
    fn assess_for(
        &self,
        learner: Cell,
        p: Presentation,
        answer: &str,
        current: &BTreeSet<Span>,
    ) -> Result<(Evidence, PublishedCase), String> {
        let pack = self
            .batch
            .available(self.work, current)
            .map_err(|e| format!("{e:?}"))?;
        let task = Task::from_pack(
            learner,
            pack.digest(),
            "quantity",
            &pack.criterion,
            &self.record,
        )?
        .with_published_sources(pack)?;
        Ok((
            task.unreviewed_text(p, answer),
            task.published_case().unwrap(),
        ))
    }
}

fn cell() -> Cell {
    Cell {
        scope: Scope {
            learner: 42,
            revision: 1,
        },
        outcome: OutcomeId(201),
    }
}
fn visit(day: u32, help: Assistance) -> Presentation {
    Presentation {
        attempt: u64::from(day) + 1,
        at: Moment { day, sequence: 1 },
        fresh: true,
        assistance: help,
    }
}
fn receipt(path: &str) -> Result<Value, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = if raw.trim_start().starts_with('{') {
        raw.as_str()
    } else {
        raw.split_once("```json")
            .and_then(|(_, r)| r.split_once("```"))
            .map(|(j, _)| j)
            .ok_or("no receipt fence")?
    };
    serde_json::from_str(data).map_err(|e| e.to_string())
}
fn commissioned(receipts: &Value, report: bool) -> Result<[Released; 3], String> {
    let course = reviewed();
    let ledgers = ledgers();
    let calls = receipts["calls"]
        .as_array()
        .filter(|c| c.len() == 2)
        .ok_or("expected two author receipts")?;
    let mut admitted = Vec::new();
    for (i, call) in calls.iter().enumerate() {
        let group = ["first", "return"][i];
        let digest = format!(
            "{:x}",
            Sha256::digest(packet(group)?.to_string().as_bytes())
        );
        if call["group"] != group || call["request_sha256"] != digest {
            return Err("request hash differs from commissioned packet".into());
        }
        let items = call["items"]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or("two candidates per packet required")?;
        for (j, v) in items.iter().enumerate() {
            let index = 2 * i + j;
            let l = &ledgers[index];
            if v["id"] != l.id
                || v["draft_question"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .is_none()
            {
                return Err("missing or misbound draft".into());
            }
            let raw_key = v["key_kg"].as_i64();
            let closed = Released::build(l, v, 1 + index as u64, &course);
            if report {
                println!(
                    "{}: fluent prompt available; proposed key {:?}; checked result {:?}",
                    l.id,
                    raw_key,
                    closed.as_ref().map(|r| r.key)
                );
            }
            if let Ok(r) = closed {
                admitted.push(r);
            }
        }
    }
    admitted
        .try_into()
        .map_err(|_| "expected T1, T3 and P1 releases for the saved experiment".into())
}
fn study(path: &str) -> Result<(), String> {
    let receipts = receipt(path)?;
    let calls = receipts["calls"].as_array().ok_or("missing author calls")?;
    let ledgers = ledgers();
    let [first, alias, mut fresh] = commissioned(&receipts, true)?;
    if [first.key, alias.key, fresh.key] != [8, 8, -4] {
        return Err("unexpected study ledger keys".into());
    }
    let actual = &calls[1]["items"][1];
    assert_eq!(
        check_quantity_text(actual["key_kg"].as_i64().unwrap(), "kg", "-4 kg"),
        Judgment::Correct
    );
    let mut fault = actual.clone();
    fault["key_kg"] = json!(5);
    assert_eq!(
        check_quantity_text(fault["key_kg"].as_i64().unwrap(), "kg", "-4 kg"),
        Judgment::Incorrect
    );
    assert_eq!(
        check_quantity_text(fault["key_kg"].as_i64().unwrap(), "kg", "+5 kg"),
        Judgment::Correct
    );
    assert_eq!(
        validate(&ledgers[3], &fault),
        Err("incorrect interval change")
    );
    println!("Model-only keys: saved P1 accepts -4 kg. Injecting +5 kg into the same fluent item accepts +5 kg and marks -4 kg wrong. Checked admission withholds it.");
    let w = Withdrawn::new(cell().scope);
    let practice = first.batch.available(first.work, &first.pins).unwrap();
    println!(
        "Guided visit: {}\nWorked example: {}\nLearner question: {}",
        practice.practice, practice.worked, practice.check
    );
    println!(
        "Return question: {}",
        fresh
            .batch
            .available(fresh.work, &fresh.pins)
            .unwrap()
            .check
    );
    let (guided, _) = first.assess(visit(1, Assistance::Reported), "+8 kg", &first.pins)?;
    let (a, first_id) = first.assess(visit(1, Assistance::NoneReported), "+8 kg", &first.pins)?;
    let (similar, _) = alias.assess(visit(30, Assistance::NoneReported), "+8 kg", &alias.pins)?;
    let (b, fresh_id) = fresh.assess(visit(30, Assistance::NoneReported), "-4 kg", &fresh.pins)?;
    let (regurgitated, _) =
        fresh.assess(visit(30, Assistance::NoneReported), "+8 kg", &fresh.pins)?;
    // This exact ordered pair was reviewed for a combined boundary, a mixture of interval masses
    // and rates, and a different key. An identical numeric answer is a deliberate negative control.
    let approved = [(first_id, fresh_id)].into();
    let observations = |prior: &Evidence, next: &Evidence, exposures: &[Exposure]| {
        delayed_on_reviewed_change(
            prior,
            next,
            Some(exposures),
            &w,
            Spacing {
                stability_days: 5.,
                threshold: 0.85,
                target_day: None,
            },
            &approved,
        )
        .unwrap()
    };
    assert!(!guided.opens_transfer(&w));
    assert_eq!(observations(&a, &similar, &[]), CheckResult::Ineligible);
    assert_eq!(observations(&guided, &b, &[]), CheckResult::Ineligible);
    assert_eq!(observations(&a, &b, &[]), CheckResult::SupportsDurability);
    assert_eq!(regurgitated.judgment(), Judgment::Incorrect);
    let recent = Exposure {
        cell: cell(),
        at: Moment {
            day: 29,
            sequence: 1,
        },
    };
    assert_eq!(
        observations(&a, &b, &[recent]),
        CheckResult::InsufficientSpacing
    );
    println!("Guided correct does not open transfer; unchanged answer on T3 is ineligible. For P1, +8 kg is incorrect and -4 kg is correct. A day-29 refresher blocks the day-30 retention inference.");
    let mut current_sources = fresh.pins.clone();
    current_sources.remove(&span(ledgers[3].wire().to_string().as_bytes()));
    assert_eq!(
        fresh.batch.available(fresh.work, &current_sources),
        Err(Error::SourceUnavailable)
    );
    assert!(fresh
        .assess(visit(32, Assistance::Unknown), "-4 kg", &current_sources)
        .is_err());
    fresh
        .batch
        .withdraw(
            fresh.work,
            fresh.receipt,
            &ReviewerGrant {
                actor: 2,
                work: fresh.work,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
    assert!(fresh
        .assess(visit(33, Assistance::Unknown), "-4 kg", &fresh.pins)
        .is_err());
    println!("Removing the ledger source blocks availability; withdrawing the released row blocks a later correct submission.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use telo::{
        monitor::Monitor,
        predicate::{ClosurePredicate, Predicates},
        Property,
    };
    const DOC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/foundation/water-authoring.md"
    );

    #[test]
    fn replay_live_receipts_and_reject_boundary_interval_unit_crossing_or_key_substitution() {
        study(DOC).unwrap();
        let records = receipt(DOC).unwrap();
        let candidate = &records["calls"][1]["items"][1];
        let cases = ledgers();
        for (field, wrong) in [
            ("boundary", json!(["east"])),
            ("interval_min", json!(1)),
            ("answer_unit", json!("kg/min")),
            ("crossing_flows", json!(["feed", "transfer", "drain"])),
            ("key_kg", json!(5)),
        ] {
            let mut changed = candidate.clone();
            changed[field] = wrong;
            assert!(validate(&cases[3], &changed).is_err(), "{field}");
        }
        let mut wording = candidate.clone();
        wording["draft_question"] = json!("What was the mass in the east basin at the end?");
        let release = Released::build(&cases[3], &wording, 4, &reviewed()).unwrap();
        assert_eq!(release.record["check"], cases[3].question());
        assert_ne!(release.record["check"], wording["draft_question"]);
    }

    #[test]
    fn two_basin_balance_equals_sum_of_separate_inventories() {
        let mut basin = ledgers().pop().unwrap();
        for inlet_rate in 0..=4 {
            for exchanged in 0..=4 {
                for drain_mass in 0..=4 {
                    basin.flows[0].amount = inlet_rate;
                    basin.flows[1].amount = exchanged;
                    basin.flows[2].amount = drain_mass;
                    basin.boundary = &["west"];
                    let west = basin.calculation().unwrap().0;
                    basin.boundary = &["east"];
                    let east = basin.calculation().unwrap().0;
                    basin.boundary = &["west", "east"];
                    let combined = basin.calculation().unwrap().0;
                    assert_eq!(west, 6 * inlet_rate - exchanged);
                    assert_eq!(east, exchanged - drain_mass);
                    assert_eq!(combined, west + east);
                }
            }
        }
    }

    #[test]
    fn every_read_rechecks_source_after_withdrawal_or_regrant() {
        #[derive(Clone, Copy)]
        struct Frame {
            source: bool,
            withdrawn: bool,
            scored: bool,
        }
        let mut predicates = Predicates::builder();
        let valid = predicates.new_predicate(ClosurePredicate::new(
            |f: &Frame| f.scored == (f.source && !f.withdrawn),
            "grade iff support is current",
        ));
        let predicates = predicates.build();
        let machine =
            Property::always(Property::atomic(valid)).to_monitoring_automaton(&predicates);
        let mut safety = Monitor::new(predicates, machine);
        let cases = ledgers();
        let records = receipt(DOC).unwrap();
        let candidate = &records["calls"][1]["items"][1];
        // Each step chooses two independent bits: current source grant and explicit item withdrawal.
        let mut reachable = [0; 2];
        for trace in 0u8..64 {
            let mut r = Released::build(&cases[3], candidate, 4, &reviewed()).unwrap();
            let mut held = false;
            for day in 0..3 {
                let symbol = (trace >> (2 * day)) & 3;
                if symbol & 2 != 0 {
                    r.batch
                        .withdraw(
                            r.work,
                            r.receipt,
                            &ReviewerGrant {
                                actor: 2,
                                work: r.work,
                            },
                        )
                        .unwrap();
                    held = true;
                }
                let source = symbol & 1 != 0;
                let denied = BTreeSet::new();
                let allowed = if source { &r.pins } else { &denied };
                let scored = r
                    .assess(visit(day, Assistance::Unknown), "-4 kg", allowed)
                    .is_ok();
                reachable[usize::from(scored)] += 1;
                assert!(safety.next_state(&Frame {
                    source,
                    withdrawn: held,
                    scored
                }));
            }
        }
        assert!(reachable.iter().all(|n| *n > 0));
        assert!(!safety.next_state(&Frame {
            source: false,
            withdrawn: false,
            scored: true
        }));
    }
}
