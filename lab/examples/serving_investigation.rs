//! Compare learned serving forecasts with forecasts grounded in a reviewed two-form catalog.
//! Usage: serving_investigation score|cases JOURNAL SEED... | race SEATS SLOTS BUDGET | handoff DELAY_MS | target
#[path = "serving_investigation/contention.rs"]
mod contention;
#[path = "serving_investigation/target.rs"]
mod target;

use serde_json::{json, Value};
use std::{collections::BTreeSet, env, fs, process::ExitCode};
use tutor_lab::{
    campaign::forward::Experiment,
    curriculum::{OutcomeId, Span},
    history::Scope,
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    supply::{self, Contact, Enrollment, ExposureBook, Form, Requirement, Unavailable},
};

const ARMS: [&str; 4] = [
    "learned",
    "source",
    "inventory",
    "inventory_and_draft_preflight",
];
const DEFER: &str = "ask_broker";
const USAGE: &str = "serving_investigation score|cases JOURNAL SEED... | race SEATS SLOTS BUDGET | handoff DELAY_MS | target";

struct Frame<'a> {
    facts: BTreeSet<&'a str>,
    learned: &'a str,
}

impl<'a> Frame<'a> {
    fn from_record(record: &'a Value) -> Result<Self, String> {
        let names = record["true_facts"]
            .as_array()
            .ok_or("missing input facts")?;
        let facts = names
            .iter()
            .map(|name| name.as_str().ok_or("invalid fact"))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            facts,
            learned: record["predicted"].as_str().ok_or("missing forecast")?,
        })
    }

    fn has(&self, fact: &str) -> bool {
        self.facts.contains(fact)
    }

    fn requirement(&self) -> Requirement {
        if self.has("needs_new_family") {
            Requirement::FreshFamily
        } else if self.has("needs_fresh_task") {
            Requirement::FreshTask
        } else {
            Requirement::Practice
        }
    }

    // A snapshot of four necessary conditions. Executing the call still requires admission.
    fn draft_forecast(&self) -> Option<&'static str> {
        if self.has("short_model_deadline") {
            return None;
        }
        let (permitted, occupied, balance, action) = match self.requirement() {
            Requirement::Practice => (
                "language_role_permitted",
                "language_slot_occupied",
                "balance_covers_language",
                "dispatch_language_draft",
            ),
            _ => (
                "reasoning_role_permitted",
                "reasoning_slot_occupied",
                "balance_covers_reasoner",
                "dispatch_reasoning_draft",
            ),
        };
        (self.has(permitted) && !self.has(occupied) && self.has(balance)).then_some(action)
    }
}

// The serving fixture has exactly two forms; the trace provides their relative identities.
// The concrete seat and catalog IDs below preserve those relations. The production selector
// checks scope, provenance, completeness, task exposure and exposure to the same family.
struct Inventory {
    seat: Enrollment,
    shared: Form,
}

impl Inventory {
    fn new() -> Self {
        let seat = Enrollment {
            scope: Scope {
                learner: 1,
                revision: 1,
            },
            class: 1,
        };
        let shared = Form {
            class: seat.class,
            release: seat.scope.revision,
            outcome: OutcomeId(1),
            owner: None,
            revision: 1,
            task: 1,
            family: 1,
            source: Span {
                document: document_id(UNIT_RULE.as_bytes()),
                start: 0,
                end: UNIT_RULE.len() as u32,
            },
            card: FractionCard::propose(FractionTask::Sum, 1, 2, 5)
                .expect("valid reviewed template"),
        };
        Self { seat, shared }
    }

    fn local(&self, f: &Frame<'_>) -> Result<Option<&'static str>, String> {
        let private = Form {
            owner: Some(if f.has("private_card_owned") {
                self.seat.scope.learner
            } else {
                2
            }),
            revision: 2,
            task: 2,
            family: if f.has("cards_share_a_family") {
                self.shared.family
            } else {
                2
            },
            ..self.shared
        };
        let mut history = ExposureBook::new(self.seat, f.has("exposure_history_complete"));
        for (name, form) in [
            ("shared_task_encountered", &self.shared),
            ("private_task_encountered", &private),
        ] {
            if f.has(name) {
                history
                    .record(self.seat, form, Contact::Possible)
                    .map_err(|e| format!("inconsistent supplied history: {e:?}"))?;
            }
        }
        let mut withdrawn = BTreeSet::new();
        if !f.has("source_rights_active") {
            withdrawn.insert(self.shared.source.document);
        }
        let source = RuleSource {
            bytes: Some(UNIT_RULE.as_bytes()),
            pin: self.shared.source,
            revoked: &withdrawn,
        };
        let catalog = [self.shared, private];
        let offered = if f.has("shared_card_released") {
            &catalog[..]
        } else {
            &catalog[1..]
        };
        match supply::select(
            self.seat,
            self.shared.outcome,
            f.requirement(),
            &history,
            offered,
            &source,
        ) {
            Ok(form) if form.owner.is_some() => Ok(Some("serve_private_form")),
            Ok(_) => Ok(Some("serve_shared_form")),
            Err(Unavailable::NoSupportedForm | Unavailable::NoFreshForm) => Ok(None),
            Err(e) => Err(format!(
                "catalog and exposure history have different scope: {e:?}"
            )),
        }
    }
}

fn forecasts<'a>(inventory: &Inventory, f: &Frame<'a>) -> Result<[&'a str; 4], String> {
    if !f.has("source_rights_active") {
        return Ok([
            f.learned,
            "hold_for_authority",
            "hold_for_authority",
            "hold_for_authority",
        ]);
    }
    let local = inventory.local(f)?;
    let without_local = if f.learned.starts_with("serve_") {
        DEFER
    } else {
        f.learned
    };
    let inventory_based = local.unwrap_or(without_local);
    let without_dispatch = if without_local.starts_with("dispatch_") {
        DEFER
    } else {
        without_local
    };
    let with_preflight = local
        .or_else(|| f.draft_forecast())
        .unwrap_or(without_dispatch);
    Ok([f.learned, f.learned, inventory_based, with_preflight])
}

#[derive(Default)]
struct Score {
    correct: usize,
    wrong: usize,
    deferred: usize,
    wrong_delivery: usize,
    missed_delivery: usize,
    wrong_draft: usize,
    wrong_referral: usize,
    false_clear: usize,
}

impl Score {
    fn observe(&mut self, expected: &str, predicted: &str) {
        if predicted == expected {
            self.correct += 1;
            return;
        }
        if predicted == DEFER {
            self.deferred += 1;
        } else {
            self.wrong += 1;
        }
        self.wrong_delivery += usize::from(predicted.starts_with("serve_"));
        self.missed_delivery += usize::from(expected.starts_with("serve_"));
        self.wrong_draft += usize::from(predicted.starts_with("dispatch_"));
        self.wrong_referral += usize::from(predicted == "refer_to_teacher");
        self.false_clear += usize::from(
            predicted.starts_with("serve_")
                && (expected.starts_with("hold_") || expected == "refer_to_teacher"),
        );
    }

    fn json(&self) -> Value {
        json!({"correct":self.correct,"wrong":self.wrong,"deferred":self.deferred,"wrong_delivery":self.wrong_delivery,
            "missed_delivery":self.missed_delivery,"wrong_draft":self.wrong_draft,"wrong_referral":self.wrong_referral,"false_clear":self.false_clear})
    }
}

fn probe(mode: &str, journal: &str, seeds: &[String]) -> Result<(), String> {
    let experiment = Experiment::new()?;
    let bytes = fs::read_to_string(journal).map_err(|e| e.to_string())?;
    let mut records = bytes
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter();
    let session =
        experiment.restore(&records.next().ok_or("empty journal")?, records.as_slice())?;
    let inventory = Inventory::new();
    let mut scores: [Score; ARMS.len()] = std::array::from_fn(|_| Score::default());
    let mut seen = BTreeSet::new();
    for arg in seeds {
        let seed = arg.parse::<u64>().map_err(|e| e.to_string())?;
        if !seen.insert(seed) {
            return Err(format!("repeated seed {seed}"));
        }
        let replay = experiment.simulate(&session, "serving", seed)?;
        for row in replay["observations"]
            .as_array()
            .ok_or("missing observations")?
        {
            let f = Frame::from_record(row)?;
            let predicted = forecasts(&inventory, &f)?;
            let actual = row["observed"].as_str().ok_or("missing observed action")?;
            for (score, prediction) in scores.iter_mut().zip(predicted) {
                score.observe(actual, prediction);
            }
            if mode == "cases" {
                let labels: serde_json::Map<_, _> = ARMS
                    .iter()
                    .zip(predicted)
                    .map(|(n, p)| (n.to_string(), json!(p)))
                    .collect();
                println!(
                    "{}",
                    json!({"seed":seed,"case":row["step"],"facts":row["true_facts"],"observed":actual,"predictions":labels})
                );
            }
        }
    }
    if mode == "score" {
        let numbers: serde_json::Map<_, _> = ARMS
            .iter()
            .zip(scores)
            .map(|(n, s)| (n.to_string(), s.json()))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"seeds":seen,"scores":numbers}))
                .map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args.as_slice() {
        [mode] if mode == "target" => target::run(),
        [mode, delay] if mode == "handoff" => contention::handoff(delay),
        [mode, seats, slots, budget] if mode == "race" => contention::run(seats, slots, budget),
        [mode, journal, seeds @ ..]
            if matches!(mode.as_str(), "score" | "cases") && !seeds.is_empty() =>
        {
            probe(mode, journal, seeds)
        }
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
