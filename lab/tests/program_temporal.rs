//! A booked unit, commissioned work and a currently readable pack have different lifetimes.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    curriculum::{
        creation::{Batch, Candidate, Error, Pack, ReviewerGrant, Work},
        program::{Frame, Proposal},
        Document, Span,
    },
    material::document_id,
};

const REVIEWER: u64 = 20;

fn span(text: &str) -> Span {
    Span {
        document: document_id(text.as_bytes()),
        start: 0,
        end: text.len().try_into().unwrap(),
    }
}

struct Jobs {
    rows: [Candidate; 2],
    rights: BTreeSet<Span>,
    new_edition: Work,
    revised_program: [Work; 2],
    reservations: [[bool; 2]; 2],
}

impl Jobs {
    fn law_survey() -> Self {
        let law: Value = serde_json::from_str(include_str!("../fixtures/law-survey.json")).unwrap();
        let frame = Frame::from_json(&law).unwrap();
        let proposal = Proposal::from_json(&law["proposal"]).unwrap();
        // Frozen synthetic editorial decisions. These tests do not assess legal accuracy.
        let reviewed = frame.review_digest(&proposal);
        let first = &law["first_pack"];
        let field = |name: &str| first[name].as_str().unwrap().to_owned();
        let article = span(&field("excerpt"));
        let village =
            span("In the fictional village a fee applies immediately after a notice appears.");
        let scopes = [BTreeSet::from([article]), BTreeSet::from([village])];
        let ids = [101, 102];
        let work = |i: usize| {
            frame
                .commission(&proposal, reviewed, ids[i], &scopes[i])
                .unwrap()
        };
        let reading = Pack {
            lesson: field("lesson"),
            worked: field("worked"),
            practice: field("practice"),
            check: field("check"),
            criterion: field("criterion"),
            rubric: field("rubric"),
            sources: scopes[0].clone(),
        };
        let philosophy = Pack {
            lesson: "Distinguish the village rule from a reason for or against it.".into(),
            worked: "A posted fee can meet this village rule even if someone objects that it is unfair.".into(),
            practice: "State an objection to immediate fees in the fictional village.".into(),
            check: "A neighbor calls an unannounced fee fair. Which question about its applicability remains?".into(),
            criterion: "Separate notice from fairness under this fictional rule.".into(),
            rubric: "Identify the missing notice and explain the difference between the two claims.".into(),
            sources: scopes[1].clone(),
        };
        let rows = [(work(0), reading), (work(1), philosophy)].map(|(work, pack)| Candidate {
            work,
            author: 11,
            pack,
        });
        let full_minutes =
            law["calendar_minutes"].as_u64().unwrap() - law["reserve_minutes"].as_u64().unwrap();
        let reservations = [u32::try_from(full_minutes).unwrap(), 40].map(|budget| {
            let calendar = frame.sequence(&proposal, budget).unwrap();
            ids.map(|id| calendar.scheduled.contains(&id))
        });

        // Same numerical unit, two distinct reasons an old work order can lose its binding.
        let revised_source = span(&format!("Classroom edition. {}", field("excerpt")));
        let new_edition = frame
            .commission(
                &proposal,
                reviewed,
                ids[0],
                &BTreeSet::from([revised_source]),
            )
            .unwrap();
        let mut updated = proposal.wire();
        updated["units"][0]["performance"] =
            "Identify the operative condition and paraphrase it independently.".into();
        let updated = Proposal::from_json(&updated).unwrap();
        assert!(frame
            .commission(&updated, reviewed, ids[0], &scopes[0])
            .is_err());
        let replacement = frame.review_digest(&updated);
        let revised_program = [0, 1].map(|i| {
            frame
                .commission(&updated, replacement, ids[i], &scopes[i])
                .unwrap()
        });
        assert_ne!(revised_program[0], work(0));
        assert_eq!(revised_program[1], work(1));
        Self {
            rows,
            rights: BTreeSet::from([article, village, revised_source]),
            new_edition,
            revised_program,
            reservations,
        }
    }

    fn start(&self) -> Run {
        let works = self.rows.each_ref().map(|row| row.work);
        let mut batch = Batch::new(works[0].release, works).unwrap();
        let mut wrong_source = self.rows[1].clone();
        wrong_source.pack.sources = self.rows[0].pack.sources.clone();
        // The substituted span is readable for this course, but was never commissioned for unit 102.
        let receipts = batch
            .stage(vec![self.rows[0].clone(), wrong_source])
            .unwrap();
        let reading = receipts[0].unwrap();
        assert_eq!(receipts[1], Err(Error::InvalidSources));
        let signed_by_author = ReviewerGrant {
            actor: self.rows[0].author,
            work: works[0],
        };
        assert_eq!(
            batch.approve(works[0], reading, &signed_by_author, &self.rights),
            Err(Error::UnauthorizedReviewer)
        );
        Run {
            batch,
            current: works,
            rights: self.rights.clone(),
            booked: self.reservations[0],
            receipts: [Some(reading), None],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    ReviewReading,
    RepairPhilosophy,
    ReviewPhilosophy,
    Withdraw,
    Restore,
    ReissueReading,
    ShortCalendar,
    ReviseProgram,
}
use Event::*;
const EVENTS: [Event; 8] = [
    ReviewReading,
    RepairPhilosophy,
    ReviewPhilosophy,
    Withdraw,
    Restore,
    ReissueReading,
    ShortCalendar,
    ReviseProgram,
];

struct Run {
    batch: Batch,
    current: [Work; 2],
    rights: BTreeSet<Span>,
    booked: [bool; 2],
    receipts: [Option<Document>; 2],
}

impl Run {
    fn apply(&mut self, event: Event, jobs: &Jobs) {
        let reading_source = jobs.rows[0].pack.sources.first().unwrap();
        match event {
            Withdraw => {
                self.rights.remove(reading_source);
            }
            Restore => {
                self.rights.insert(*reading_source);
            }
            ReissueReading if self.current[0] == jobs.rows[0].work => {
                self.current[0] = jobs.new_edition
            }
            ReissueReading => {}
            ReviseProgram => self.current = jobs.revised_program,
            ShortCalendar => self.booked = jobs.reservations[1],
            RepairPhilosophy => {
                self.receipts[1] =
                    Some(self.batch.stage(vec![jobs.rows[1].clone()]).unwrap()[0].unwrap());
            }
            ReviewReading | ReviewPhilosophy => {
                // A delayed reviewer can sign an older job. The broker reads the current one.
                let index = usize::from(event == ReviewPhilosophy);
                let Some(receipt) = self.receipts[index] else {
                    return;
                };
                let row = &jobs.rows[index];
                let grant = ReviewerGrant {
                    actor: REVIEWER,
                    work: row.work,
                };
                let _result = self.batch.approve(row.work, receipt, &grant, &self.rights);
            }
        }
    }

    fn observed(&self, jobs: &Jobs) -> View {
        let released = [0, 1].map(
            |i| match self.batch.available(self.current[i], &self.rights) {
                Ok(actual) => {
                    assert_eq!(actual, &jobs.rows[i].pack);
                    true
                }
                Err(_) => false,
            },
        );
        View {
            booked: self.booked,
            released,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    booked: [bool; 2],
    released: [bool; 2],
}

// Expectation uses events alone. It never consults a scheduler, work digest or batch decision.
fn expected(prefix: &[Event]) -> View {
    fn rights_at(events: &[Event]) -> bool {
        events
            .iter()
            .rev()
            .find(|e| matches!(e, Withdraw | Restore))
            != Some(&Withdraw)
    }
    let reading_signed = prefix
        .iter()
        .enumerate()
        .any(|(i, &e)| e == ReviewReading && rights_at(&prefix[..i]));
    let philosophy_signed = prefix
        .iter()
        .position(|&e| e == RepairPhilosophy)
        .zip(prefix.iter().rposition(|&e| e == ReviewPhilosophy))
        .is_some_and(|(repair, review)| repair < review);
    View {
        booked: [true, !prefix.contains(&ShortCalendar)],
        released: [
            !prefix.contains(&ReviseProgram)
                && !prefix.contains(&ReissueReading)
                && reading_signed
                && rights_at(prefix),
            philosophy_signed,
        ],
    }
}

type Boundary = (View, View);

fn monitor_factory() -> impl Fn() -> Monitor<Boundary> {
    let symbols = || {
        let mut builder = Predicates::builder();
        let agree = builder.new_predicate(ClosurePredicate::new(
            |s: &Boundary| s.0 == s.1,
            "calendar and release agree with event history",
        ));
        (builder.build(), Property::always(Property::atomic(agree)))
    };
    let (predicates, property) = symbols();
    let automaton = property.to_monitoring_automaton(&predicates);
    move || Monitor::new(symbols().0, automaton.clone())
}

#[test]
fn mixed_bulk_reply_and_later_changes_preserve_separate_calendar_and_release_facts() {
    let jobs = Jobs::law_survey();
    let make_monitor = monitor_factory();
    let traces = EVENTS.len().pow(4);
    let mut observations = 0;
    let mut counts = BTreeMap::new();
    for mut encoding in 0..traces {
        let history = [(); 4].map(|_| {
            let event = EVENTS[encoding % EVENTS.len()];
            encoding /= EVENTS.len();
            event
        });
        let mut run = jobs.start();
        let mut monitor = make_monitor();
        for n in 0..=history.len() {
            if n > 0 {
                run.apply(history[n - 1], &jobs);
            }
            let actual = run.observed(&jobs);
            assert!(
                monitor.next_state(&(actual, expected(&history[..n]))),
                "{:?}: {actual:?}",
                &history[..n]
            );
            *counts
                .entry((actual.booked[1], actual.released))
                .or_insert(0usize) += 1;
            observations += 1;
        }
    }
    assert_eq!(counts.len(), 8); // Both calendar states and every pair of current release results.
    assert_eq!(observations, traces * 5);
    println!("{traces} traces, {observations} boundaries, state counts {counts:?}");
}

#[test]
fn audit_flags_two_wrong_broker_projections_on_concrete_traces() {
    let jobs = Jobs::law_survey();
    let mut run = jobs.start();
    let make_monitor = monitor_factory();
    let mut guessed = run.observed(&jobs);
    guessed.released = guessed.booked;
    assert!(!make_monitor().next_state(&(guessed, expected(&[]))));
    let mut monitor = make_monitor();
    run.apply(ReviewReading, &jobs);
    let cached = run.observed(&jobs);
    assert!(monitor.next_state(&(cached, expected(&[ReviewReading]))));
    run.apply(ReissueReading, &jobs);
    assert_ne!(cached, run.observed(&jobs));
    assert!(!monitor.next_state(&(cached, expected(&[ReviewReading, ReissueReading]))));
}
