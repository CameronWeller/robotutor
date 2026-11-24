use super::{session, talk, Channel, TaskContact};
use std::{
    fs,
    io::{self, Cursor, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Barrier,
    },
    thread,
};
use telo::{
    monitor::Monitor,
    predicate::{ClosurePredicate, Predicates},
    Property,
};
use tutor_lab::supply::coverage::open_once;

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tutor-reservation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn simultaneous_saved_conversations_can_open_the_lamp_once_and_both_keep_teaching() {
    let root = Directory::new();
    let learner = root.0.join("authorized-learner");
    fs::create_dir(&learner).unwrap();
    let home = Channel::create(&root.0, "home", 1, "sample\npause\n");
    let school = Channel::create(&root.0, "school", 1, "sample\npause\n");
    let coverage = TaskContact::across([home.read_at(&home.head), school.read_at(&school.head)]);
    let start = Barrier::new(2);
    let open = |channel: &Channel| {
        start.wait();
        open_once(&learner, 1, coverage, || {
            talk(&channel.path, "check\npause\n")
        })
        .unwrap()
        .unwrap_or_else(|| talk(&channel.path, "practice\npause\n"))
    };
    let outputs = thread::scope(|s| {
        let first = s.spawn(|| open(&home));
        let second = open(&school);
        [first.join().unwrap(), second]
    });
    assert_eq!(
        outputs
            .iter()
            .filter(|s| s.contains("night worker"))
            .count(),
        1
    );
    assert_eq!(outputs.iter().filter(|s| s.contains("gardener")).count(), 1);
    assert!(
        open_once(&learner, 1, coverage, || panic!("same case on restart"))
            .unwrap()
            .is_none()
    );

    // Neither a different person's grant nor a different task is consumed.
    let other = root.0.join("another-authorized-learner");
    fs::create_dir(&other).unwrap();
    assert_eq!(open_once(&other, 1, coverage, || 1).unwrap(), Some(1));
    assert_eq!(open_once(&learner, 2, coverage, || 2).unwrap(), Some(2));
    // Unknown/possible/confirmed do not even look for a host directory.
    let absent = root.0.join("no-access");
    for status in [
        TaskContact::Unknown,
        TaskContact::Possible,
        TaskContact::Confirmed,
    ] {
        assert!(
            open_once(&absent, 1, status, || panic!("insufficient evidence"))
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        open_once(&absent, 1, coverage, || panic!("missing storage"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

struct BrokenDisplay {
    after_case: bool,
    case_written: bool,
    bytes: Vec<u8>,
}

impl Write for BrokenDisplay {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let contains_case = String::from_utf8_lossy(bytes).contains("night worker");
        if self.case_written || (contains_case && !self.after_case) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.bytes.extend_from_slice(bytes);
        self.case_written |= contains_case;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_broken_display_with_or_without_visible_case_bytes_leaves_guided_practice_available() {
    for after_case in [false, true] {
        let root = Directory::new();
        let gate = root.0.join("learner");
        fs::create_dir(&gate).unwrap();
        let school = Channel::create(&root.0, "school", 1, "sample\npause\n");
        let home = Channel::create(&root.0, "home", 1, "sample\npause\n");
        let clear = TaskContact::across([school.read_at(&school.head), home.read_at(&home.head)]);
        let mut display = BrokenDisplay {
            after_case,
            case_written: false,
            bytes: vec![],
        };
        let attempt = open_once(&gate, 1, clear, || {
            session(&school.path, &mut Cursor::new("check\n"), &mut display)
        })
        .unwrap();
        assert!(attempt.unwrap().is_err());
        assert_eq!(
            String::from_utf8(display.bytes)
                .unwrap()
                .contains("night worker"),
            after_case
        );
        // A later request in the private conversation still uses its earlier clear read.
        let result = open_once(&gate, 1, clear, || talk(&home.path, "check\npause\n"))
            .unwrap()
            .unwrap_or_else(|| talk(&home.path, "practice\npause\n"));
        assert!(result.contains("gardener") && !result.contains("night worker"));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FirstSend {
    DiesBeforeText,
    TextWithoutReceipt,
    Acknowledged,
}

#[test]
fn check_all_two_channel_timeout_and_retry_interleavings() {
    use FirstSend::*;
    let root = Directory::new();
    let modes = [DiesBeforeText, TextWithoutReceipt, Acknowledged];
    let monitor = || {
        let mut names = Predicates::builder();
        let once = names.new_predicate(ClosurePredicate::new(
            |count: &u8| *count <= 1,
            "at most one rendered copy",
        ));
        let names = names.build();
        let machine = Property::always(Property::atomic(once)).to_monitoring_automaton(&names);
        Monitor::new(names, machine)
    };
    let mut traces = 0;
    let mut durable_violations = 0;
    let mut timeout_violations = 0;
    let mut no_render = 0;
    let mut timeout_no_render = 0;

    // Each channel sends, handles the result (receipt or timeout), then tries again with a working display.
    // Both hold the same previously clear coverage read. Each six-event schedule preserves local ordering.
    for bits in (0u8..64).filter(|bits| bits.count_ones() == 3) {
        for home in modes {
            for school in modes {
                traces += 1;
                let modes = [home, school];
                let mut steps = [0; 2];
                let mut durable_monitor = monitor();
                let mut timeout_monitor = monitor();
                let (mut durable_render, mut timeout_render) = (0, 0);
                let (mut durable_failed, mut timeout_failed) = (false, false);
                let mut admissions = 0;
                let mut timeout_owner = None;
                let mut acknowledged = false;
                for event in 0..6 {
                    let caller = usize::from((bits >> event) & 1);
                    let step = steps[caller];
                    steps[caller] += 1;
                    let mode = modes[caller];
                    if step == 1 {
                        // Negative control: release an unacknowledged reservation on timeout.
                        if timeout_owner == Some(caller) {
                            timeout_owner = None;
                            acknowledged = mode == Acknowledged;
                        }
                    } else {
                        let renders = step == 2 || mode != DiesBeforeText;
                        open_once(&root.0, traces, TaskContact::NoRecordedContact, || {
                            admissions += 1;
                            durable_render += u8::from(renders);
                        })
                        .unwrap();
                        if timeout_owner.is_none() && !acknowledged {
                            timeout_render += u8::from(renders);
                            if step == 2 {
                                acknowledged = true;
                            } else {
                                timeout_owner = Some(caller);
                            }
                        }
                    }
                    durable_failed |= !durable_monitor.next_state(&durable_render);
                    timeout_failed |= !timeout_monitor.next_state(&timeout_render);
                }
                assert_eq!(admissions, 1);
                assert_eq!(
                    durable_render,
                    u8::from(modes[usize::from(bits & 1)] != DiesBeforeText)
                );
                no_render += usize::from(durable_render == 0);
                timeout_no_render += usize::from(timeout_render == 0);
                durable_violations += usize::from(durable_failed);
                timeout_violations += usize::from(timeout_failed);
            }
        }
    }
    println!("{traces} histories, {} prefixes per policy, durable violations={durable_violations}, timeout violations={timeout_violations}, no text with durable={no_render}, no text with timeout={timeout_no_render}", traces*6);
    assert_eq!(traces, 180);
    assert_eq!(durable_violations, 0);
    assert_eq!(timeout_violations, 66);
    assert_eq!(no_render, 60);
    assert_eq!(timeout_no_render, 0);
}
