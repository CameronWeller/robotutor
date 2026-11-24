//! One staffed review per meeting. Class-shared repair and individual teaching use the same slot.
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::{
    audience::{
        learner_view, Assignment, AssignmentKey, LearnerAccess, LearnerView, Participation, Roster,
    },
    curriculum::{OutcomeId, Span},
    history::Scope,
    material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE},
    protocol::{Assistance, Command, Event, Gate, Question},
    supply::{Enrollment, ExposureBook, Form, Requirement},
};
#[path = "classroom_goal/teacher_review.rs"]
mod teacher_review;

const RIBBON: u64 = 2;
const ADD: u64 = 1;
const CLASS: u64 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Policy {
    Roster,
    Oldest,
    AlternateAfterRepair,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Job {
    Repair,
    Individual(u64),
}

// A receipt stores the requested goal and queue age. Reopening never restores a publication grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Request {
    seat: u64,
    goal: u64,
    meeting: u64,
}
fn goal(seat: u64) -> u64 {
    if (2..=4).contains(&seat) {
        RIBBON
    } else {
        ADD
    }
}
fn needs_teacher(seat: u64) -> bool {
    matches!(seat, 1 | 3 | 5)
}
fn attending(seat: u64, meeting: u64) -> bool {
    !(seat == 4 && meeting == 2)
}
fn edition(meeting: u64) -> u64 {
    if meeting < 3 {
        1
    } else {
        2
    }
}

fn reopen(file: &Path, queue: &[Request], last_repair: bool) -> (Vec<Request>, bool) {
    let rows: Vec<_> = queue.iter().map(|r| [r.seat, r.goal, r.meeting]).collect();
    fs::write(file, serde_json::to_vec(&(rows, last_repair)).unwrap()).unwrap();
    let bytes = fs::read(file).unwrap();
    let (saved, marker): (Vec<[u64; 3]>, bool) = serde_json::from_slice(&bytes).unwrap();
    let got: Vec<_> = saved
        .into_iter()
        .map(|[seat, goal, meeting]| Request {
            seat,
            goal,
            meeting,
        })
        .collect();
    assert_eq!(queue, got);
    (got, marker)
}

struct Teaching {
    seat: u64,
    requested: u64,
    taught: u64,
    goal: u64,
    edition: u64,
    prompt: String,
    worked: String,
}

fn teach(
    r: Request,
    meeting: u64,
    roster: &Roster,
    review_current: bool,
    teacher_present: bool,
) -> Teaching {
    assert!(attending(r.seat, meeting));
    assert!(r.goal != RIBBON || review_current);
    assert!(!needs_teacher(r.seat) || teacher_present);
    let release = edition(meeting);
    let header = format!("Classroom workbook {release}\n");
    let bytes = format!("{header}{UNIT_RULE}").into_bytes();
    let pin = Span {
        document: document_id(&bytes),
        start: header.len() as u32,
        end: bytes.len() as u32,
    };
    let revoked = BTreeSet::new();
    let source = RuleSource {
        bytes: Some(&bytes),
        pin,
        revoked: &revoked,
    };
    let kind = if r.goal == RIBBON {
        FractionTask::MissingPart
    } else {
        FractionTask::Sum
    };
    let card = FractionCard::propose(kind, 1, 2, 4).unwrap();
    let assignment = Assignment {
        key: AssignmentKey {
            class: CLASS,
            activity: r.goal,
        },
        publication: release,
        form: Form {
            class: CLASS,
            release,
            outcome: OutcomeId(r.goal as u32),
            owner: None,
            revision: release,
            task: r.goal,
            family: r.goal,
            source: pin,
            card,
        },
        question: Question::Practice,
        assigned: [(r.seat, release)].into(),
        held: BTreeSet::new(),
        corrected: false,
        returns: Default::default(),
    };
    let scope = Scope {
        learner: r.seat,
        revision: release,
    };
    let mut book = ExposureBook::new(
        Enrollment {
            scope,
            class: CLASS,
        },
        false,
    );
    let access = LearnerAccess {
        actor: r.seat,
        roster,
        assignment: &assignment,
        source: &source,
    };
    let mut lane = Participation::start(&assignment, r.seat);
    lane.control(Command {
        after: 0,
        record: 1,
        event: Event::Teach(Gate::Supported),
    })
    .unwrap();
    let receipt = lane
        .present(access, &mut book, Requirement::Practice, 1, 2)
        .unwrap();
    let LearnerView::Question { prompt, .. } =
        learner_view(r.seat, roster, &assignment, &lane, &source).unwrap()
    else {
        unreachable!()
    };
    // A wrong numerical reply makes the returned worked explanation observable on both goals.
    lane.submit(access, &receipt.handle, 3, "0", Assistance::Unknown)
        .unwrap();
    lane.control(Command {
        after: lane.revision(),
        record: 4,
        event: Event::Feedback(Gate::Supported),
    })
    .unwrap();
    let LearnerView::Feedback {
        worked_answer: Some(worked),
        ..
    } = learner_view(r.seat, roster, &assignment, &lane, &source).unwrap()
    else {
        unreachable!()
    };
    Teaching {
        seat: r.seat,
        requested: r.meeting,
        taught: meeting,
        goal: r.goal,
        edition: release,
        prompt,
        worked,
    }
}

struct Boundary {
    meeting: u64,
    current: Option<u64>,
    jobs: Vec<Job>,
    before: Vec<Request>,
    after: Vec<Request>,
    delivered: Vec<Teaching>,
}

fn lawful(b: &Boundary) -> bool {
    let ids: BTreeSet<_> = b.delivered.iter().map(|t| t.seat).collect();
    let pending: BTreeSet<_> = b.after.iter().map(|r| r.seat).collect();
    b.jobs.len() <= 1
        && ids.len() == b.delivered.len()
        && pending.len() == b.after.len()
        && ids.is_disjoint(&pending)
        && b.before.len() == b.after.len() + b.delivered.len()
        && b.after.iter().all(|r| b.before.contains(r))
        && b.delivered.iter().all(|t| {
            b.before.contains(&Request {
                seat: t.seat,
                goal: t.goal,
                meeting: t.requested,
            }) && t.taught == b.meeting
                && t.edition == edition(b.meeting)
                && attending(t.seat, b.meeting)
                && (t.goal != RIBBON || b.current == Some(t.edition))
                && (!needs_teacher(t.seat) || b.jobs.contains(&Job::Individual(t.seat)))
        })
}

fn monitor() -> Monitor<Boundary> {
    let mut p = Predicates::builder();
    let ready = p.new_predicate(ClosurePredicate::new(
        lawful,
        "current support, staffed review and exact queue accounting",
    ));
    let p = p.build();
    let machine = Property::always(Property::atomic(ready)).to_monitoring_automaton(&p);
    Monitor::new(p, machine)
}

fn run(policy: Policy, seats: u64, file: &Path) -> Vec<Boundary> {
    let roster = Roster {
        class: CLASS,
        teachers: [100].into(),
        learners: (1..=seats).collect(),
    };
    let mut queue: Vec<_> = (1..=seats)
        .map(|seat| Request {
            seat,
            goal: goal(seat),
            meeting: 1,
        })
        .collect();
    let mut current = None;
    let mut last_repair = false;
    let mut safety = monitor();
    let mut frames = Vec::new();
    for meeting in 1..=8 {
        if meeting == 3 {
            (queue, last_repair) = reopen(file, &queue, last_repair);
        }
        if !queue.iter().any(|r| r.seat == 1) {
            queue.push(Request {
                seat: 1,
                goal: ADD,
                meeting,
            });
        }
        let before = queue.clone();
        if policy == Policy::Roster {
            queue.sort_by_key(|r| r.seat);
        } else {
            queue.sort_by_key(|r| (r.meeting, r.seat));
        }
        let eligible = queue
            .iter()
            .filter(|r| attending(r.seat, meeting))
            .filter_map(|r| {
                if r.goal == RIBBON && current != Some(edition(meeting)) {
                    (meeting >= 2).then_some(Job::Repair)
                } else {
                    needs_teacher(r.seat).then_some(Job::Individual(r.seat))
                }
            });
        let permitted = if policy == Policy::AlternateAfterRepair && last_repair {
            eligible
                .clone()
                .find(|job| matches!(job, Job::Individual(_)))
        } else {
            None
        };
        let job = permitted.or_else(|| eligible.into_iter().next());
        if let Some(action) = job {
            last_repair = action == Job::Repair;
        }
        if job == Some(Job::Repair) {
            current = Some(edition(meeting));
        }
        let mut delivered = Vec::new();
        queue.retain(|&r| {
            let ready = current == Some(edition(meeting));
            let staff = job == Some(Job::Individual(r.seat));
            if attending(r.seat, meeting)
                && (r.goal != RIBBON || ready)
                && (!needs_teacher(r.seat) || staff)
            {
                delivered.push(teach(r, meeting, &roster, ready, staff));
                false
            } else {
                true
            }
        });
        let frame = Boundary {
            meeting,
            current,
            jobs: job.into_iter().collect(),
            before,
            after: queue.clone(),
            delivered,
        };
        assert!(safety.next_state(&frame));
        frames.push(frame);
    }
    frames
}

fn report(policy: Policy, seats: u64, frames: &[Boundary]) -> Value {
    let mut individuals = Vec::new();
    for seat in 1..=seats {
        let deliveries: Vec<_> = frames
            .iter()
            .flat_map(|f| &f.delivered)
            .filter(|t| t.seat == seat)
            .collect();
        let first = deliveries.first();
        let waits: Vec<_> = deliveries.iter().map(|t| t.taught - t.requested).collect();
        individuals.push(json!({
            "seat": seat, "first": first.map(|t| t.taught), "visits": deliveries.len(),
            "waits": waits, "first_question": first.map(|t| &t.prompt),
            "first_feedback": first.map(|t| &t.worked)
        }));
    }
    json!({"policy":format!("{policy:?}"),"seats":seats,"learners":individuals,
        "jobs":frames.iter().map(|f|format!("{:?}",f.jobs)).collect::<Vec<_>>(),"pending":frames.last().unwrap().after.iter().map(|r|r.seat).collect::<Vec<_>>()})
}

fn scratch() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = env::temp_dir().join(format!(
        "robotutor-classroom-goal-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap();
    dir
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--teacher-review") {
        let input = env::args()
            .nth(2)
            .expect("supply the extracted paired evidence directory");
        println!(
            "{}",
            serde_json::to_string_pretty(
                &teacher_review::study(Path::new(&input)).expect("teacher review replay")
            )
            .unwrap()
        );
        return;
    }
    let dir = scratch();
    let file = dir.join("backlog.json");
    for seats in [1, 6] {
        for policy in [Policy::Roster, Policy::Oldest, Policy::AlternateAfterRepair] {
            let frames = run(policy, seats, &file);
            println!("{}", report(policy, seats, &frames));
        }
    }
    fs::remove_file(file).unwrap();
    fs::remove_dir(dir).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn firsts(frames: &[Boundary], seats: u64) -> Vec<Option<u64>> {
        (1..=seats)
            .map(|seat| {
                frames
                    .iter()
                    .flat_map(|b| &b.delivered)
                    .find(|t| t.seat == seat)
                    .map(|t| t.taught)
            })
            .collect()
    }
    fn coverage() -> Monitor<Boundary> {
        let mut p = Predicates::builder();
        let cutoff = p.new_predicate(ClosurePredicate::new(
            |b: &Boundary| b.meeting < 8 || b.after.iter().all(|r| r.meeting != 1),
            "all initial requests closed by meeting eight",
        ));
        let p = p.build();
        let machine = Property::always(Property::atomic(cutoff)).to_monitoring_automaton(&p);
        Monitor::new(p, machine)
    }
    #[test]
    fn finite_horizon_counterexample_and_repaired_receipts() {
        let dir = scratch();
        let file = dir.join("backlog.json");
        for policy in [Policy::Roster, Policy::Oldest, Policy::AlternateAfterRepair] {
            let mut frames = run(policy, 6, &file);
            let first = firsts(&frames, 6);
            let expected = match policy {
                Policy::Roster => vec![Some(1), None, None, None, None, Some(1)],
                Policy::Oldest => vec![Some(1), Some(2), Some(4), Some(3), Some(5), Some(1)],
                Policy::AlternateAfterRepair => {
                    vec![Some(1), Some(2), Some(5), Some(4), Some(3), Some(1)]
                }
            };
            assert_eq!(first, expected);
            assert_eq!(frames.iter().all(|b| lawful(b)), true);
            let mut deadline = coverage();
            assert_eq!(
                frames.iter().all(|b| deadline.next_state(b)),
                policy != Policy::Roster
            );
            if policy == Policy::Oldest {
                let reconnect = &mut frames[2];
                assert!(reconnect
                    .delivered
                    .iter()
                    .any(|t| t.goal == RIBBON && t.edition == 2));
                reconnect.current = Some(1);
                assert!(!monitor().next_state(reconnect));
            }
            let small = run(policy, 1, &file);
            assert_eq!(small.iter().flat_map(|b| &b.delivered).count(), 8);
        }
        fs::remove_file(file).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
