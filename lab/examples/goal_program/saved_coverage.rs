use super::*;
use std::{fs, io::Cursor, path::PathBuf};
use tutor_lab::supply::{coverage::TaskContact, Contact};

#[path = "saved_reservations.rs"]
mod reservations;

fn talk(path: &Path, commands: &str) -> String {
    let mut bytes = Vec::new();
    session(path, &mut Cursor::new(commands), &mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

struct Channel {
    path: PathBuf,
    owner: u64,
    head: String,
}

impl Channel {
    fn create(dir: &Path, name: &str, owner: u64, commands: &str) -> Self {
        let path = dir.join(name);
        talk(&path, commands);
        let state = journal::read(&path, None, Session::status).unwrap();
        // Journal contents alone cannot identify a real person.
        assert_eq!(state["learner"], 42);
        Self {
            path,
            owner,
            head: head(&state).unwrap().into(),
        }
    }

    fn read_at(&self, head: &str) -> TaskContact {
        // Return just one contact field. The requester never receives the
        // journal's private activity choices, other objectives or raw events.
        let snapshot = journal::read(&self.path, Some(head), |s| {
            let state = s.status()?;
            Ok(serde_json::json!({"contact":state["goal_choice"]["base_contact"]}))
        });
        let Ok(snapshot) = snapshot else {
            return TaskContact::Unknown;
        };
        let marker = match &snapshot["contact"] {
            Value::Null => None,
            v if v == "possible" => Some(Contact::Possible),
            v if v == "confirmed" => Some(Contact::Delivered),
            _ => return TaskContact::Unknown,
        };
        TaskContact::on_channel(marker, true)
    }
}

#[test]
fn two_saved_channels_need_scoped_complete_history_before_the_lamp_is_called_unseen() {
    use TaskContact::*;
    let dir = std::env::temp_dir().join(format!("two-saved-channels-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let home = Channel::create(&dir, "home", 1, "sample\npause\n");
    let other = Channel::create(&dir, "school", 1, "sample\npause\n");
    let possible = Channel::create(&dir, "school-possible", 1, "sample\ncheck\npause\n");
    let confirmed = Channel::create(&dir, "school-confirmed", 1, "sample\ncheck\nseen\npause\n");
    let bystander = Channel::create(&dir, "housemate", 2, "sample\ncheck\nseen\npause\n");
    assert_eq!(confirmed.head, bystander.head);
    assert_eq!(home.head, other.head);
    let mut scores = [0; 3];
    for (name, local, remote, consent, remote_head, expected, expected_remote_reads) in [
        (
            "complete",
            &home,
            Some(&other),
            true,
            other.head.as_str(),
            NoRecordedContact,
            1,
        ),
        (
            "confirmed",
            &home,
            Some(&confirmed),
            true,
            &confirmed.head,
            Confirmed,
            1,
        ),
        (
            "possible",
            &home,
            Some(&possible),
            true,
            &possible.head,
            Possible,
            1,
        ),
        (
            "declines to link",
            &home,
            Some(&confirmed),
            false,
            &confirmed.head,
            Unknown,
            0,
        ),
        (
            "wrong owner",
            &home,
            Some(&bystander),
            true,
            &bystander.head,
            Unknown,
            0,
        ),
        ("channel missing", &home, None, true, "", Unknown, 0),
        (
            "confirmed home, other missing",
            &confirmed,
            None,
            false,
            "",
            Confirmed,
            0,
        ),
        (
            "head changed",
            &home,
            Some(&confirmed),
            true,
            &other.head,
            Unknown,
            1,
        ),
    ] {
        let local_contact = local.read_at(&local.head);
        let mut reads = 0;
        let external = remote
            .filter(|c| consent && c.owner == local.owner)
            .map_or(Unknown, |c| {
                reads += 1;
                c.read_at(remote_head)
            });
        assert_eq!(reads, expected_remote_reads);
        let scoped = TaskContact::across([local_contact, external]);
        // Counterfactual over these synthetic records: a machine-wide search
        // includes the other person's confirmed contact on every read.
        let pooled = TaskContact::across([
            local_contact,
            bystander.read_at(&bystander.head),
            remote.map_or(Unknown, |c| c.read_at(&c.head)),
        ]);
        assert_eq!(scoped, expected, "{name}");
        for (score, actual) in scores.iter_mut().zip([local_contact, pooled, scoped]) {
            *score += usize::from(actual == expected);
        }
        println!("{name}: local={local_contact:?} machine={pooled:?} scoped={scoped:?} authorized external reads={reads}");
    }
    assert_eq!(scores, [2, 2, 8]);
    assert_eq!(TaskContact::across([]), Unknown);

    // Both accurate preflight reads can be invalid for commitment: the two
    // independently locked journals do not serialize delivery for the person.
    assert_eq!(
        TaskContact::across([home.read_at(&home.head), other.read_at(&other.head)]),
        NoRecordedContact
    );
    assert!(talk(&home.path, "check\npause\n").contains("night worker"));
    assert!(talk(&other.path, "check\npause\n").contains("night worker"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_returning_learner_can_set_aside_an_exact_case_without_linking_any_history() {
    let dir = std::env::temp_dir().join(format!("reported-familiar-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let earlier = Channel::create(&dir, "school", 1, "sample\ncheck\nseen\npause\n");
    let home = Channel::create(&dir, "home", 1, "sample\npause\n");
    let initial = TaskContact::across([home.read_at(&home.head), earlier.read_at(&earlier.head)]);
    fs::rename(&earlier.path, dir.join("outside-this-session")).unwrap();
    let without_link = TaskContact::across([home.read_at(&home.head), TaskContact::Unknown]);
    // A vague concern leaves the decision with the learner.
    talk(&home.path, "I think I've seen this\nfamiliar second\n");
    assert_eq!(home.read_at(&home.head), TaskContact::NoRecordedContact);
    let first = talk(
        &home.path,
        "familiar first\ncheck\npractice\nrequest\npause\n",
    );
    assert!(first.contains("set that case aside") && first.contains("gardener"));
    assert!(!first.contains("night worker"));
    let state = journal::read(&home.path, None, Session::status).unwrap();
    let volunteered =
        TaskContact::across([home.read_at(head(&state).unwrap()), TaskContact::Unknown]);
    let violates = |history: [TaskContact; 3]| {
        use telo::{
            monitor::Monitor,
            predicate::{ClosurePredicate, Predicates},
            Property,
        };
        let mut names = Predicates::builder();
        let permitted = names.new_predicate(ClosurePredicate::new(
            |v: &(TaskContact, TaskContact)| v.0 == v.1,
            "read only currently permitted contact evidence at every boundary",
        ));
        let names = names.build();
        let machine = Property::always(Property::atomic(permitted)).to_monitoring_automaton(&names);
        let mut monitor = Monitor::new(names, machine);
        let expected = [
            TaskContact::Confirmed,
            TaskContact::Unknown,
            TaskContact::Possible,
        ];
        history
            .into_iter()
            .zip(expected)
            .position(|pair| !monitor.next_state(&pair))
    };
    assert_eq!(violates([initial, without_link, volunteered]), None);
    assert_eq!(violates([initial, initial, volunteered]), Some(1));
    assert_eq!(violates([initial, without_link, without_link]), Some(2));
    assert_eq!(state["goal_choice"]["base_contact"], "possible");
    assert!(state["goal_choice"]["extra_contact"].is_null());
    journal::apply(&home.path, head(&state).unwrap(), || {
        Ok(Action::CaseReview("hall".into()))
    })
    .unwrap();
    let second = talk(&home.path, "familiar second\ncheck\npractice\npause\n");
    assert!(!second.contains("dry hall") && second.contains("gardener"));
    let final_state = journal::read(&home.path, None, Session::status).unwrap();
    assert_eq!(final_state["goal_choice"]["extra_contact"], "possible");
    assert_eq!(state["checkpoint"], final_state["checkpoint"]);
    assert!(!fs::read_to_string(&home.path)
        .unwrap()
        .contains("I think I've seen this"));
    fs::remove_dir_all(dir).unwrap();
}
