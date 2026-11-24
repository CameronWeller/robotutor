//! Serve a narrower, learner-confirmed writing goal from the pre-reviewed lab publication.
use super::{line, practice_topic, publish_receipt, read_limited, save_receipt, Topic};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use tutor_lab::curriculum::{
    program::{Frame, Proposal},
    rehearsal_release::publish,
};

const FIXTURE: &str = include_str!("../../fixtures/goals/writer.json");
const PINS: &str = include_str!("../../fixtures/goals/reviews.json");
const UNIT: u32 = 101;
const SCOPE: &str = "Practice helping a first-time reader act on one written English notice. Translation, how it sounds aloud, and a real reader's reaction remain open.";

fn safe_goal(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 240 && !text.chars().any(char::is_control)
}

fn parse_plan(raw: &str) -> Result<Value, String> {
    let plan: Value = serde_json::from_str(raw).map_err(|_| "invalid writing plan")?;
    let fields = plan.as_object().ok_or("invalid writing plan")?;
    let original = plan["format"] == "writer-goal/1" && fields.len() == 3;
    let revised = plan["format"] == "writer-goal/2"
        && fields.len() == 4
        && plan["revision"].as_u64().is_some_and(|r| r > 0);
    if !(original || revised) || !fields.contains_key("lesson") {
        return Err("unrecognized writing plan".into());
    }
    plan["goal"]
        .as_str()
        .filter(|g| safe_goal(g))
        .ok_or("invalid saved goal")?;
    if !(revised && plan["lesson"].is_null()) {
        plan["lesson"]
            .as_str()
            .filter(|s| s.starts_with("writer-return/2 first-reader ") && s.len() <= 160)
            .ok_or("invalid booked lesson")?;
    }
    Ok(plan)
}

pub(super) fn reopen_plan(raw: &str, source: &Source<'_>, agenda: bool) -> Result<(), String> {
    let plan = parse_plan(raw)?;
    let goal = plan["goal"].as_str().expect("validated goal");
    let lesson = plan["lesson"].as_str();
    println!("Your longer-term goal: {goal}");
    let booking = if lesson.is_some() {
        "Your written exercise remains booked."
    } else {
        "You have no booked exercise."
    };
    if agenda {
        println!("{booking}");
        let choice = line("Choose today's activity. Use booked to try a saved exercise, boundary or authority for local writing practice, translation or speaking to revisit that aim, or :stop to skip today.")?;
        match choice.as_deref() {
            Some("booked") => {}
            Some("translation" | "speaking") => {
                println!(
                    "This writing library has no reviewed lesson for that performance. {booking}"
                );
                return Ok(());
            }
            None => {
                println!("Skipped this visit. {booking}");
                return Ok(());
            }
            Some(key) => {
                if let Some(topic) = Topic::parse(key) {
                    println!("{booking}");
                    return practice_topic(topic);
                }
                println!("No activity selected. {booking}");
                return Ok(());
            }
        }
    }
    if let Some(lesson) = lesson {
        return reopen(lesson, source);
    }
    println!("No written exercise is booked. Use the agenda to choose practice.");
    Ok(())
}

pub(super) fn revise(journal: &Path) -> Result<(), String> {
    let previous = read_limited(journal, 2048)?;
    let plan = parse_plan(&previous)?;
    println!(
        "Current longer-term goal: {}",
        plan["goal"].as_str().expect("validated goal")
    );
    let Some(goal) =
        line("Enter the new goal to save locally, or :stop to leave the plan as it is.")?
    else {
        return Ok(());
    };
    if !safe_goal(&goal) {
        return Err("goal must be one plain-text line of 1–240 bytes".into());
    }
    let lesson = if let Some(booking) = plan["lesson"].as_str() {
        let choice = line("Type keep to retain the written first-reader exercise with this new goal, release to drop that booking, or :stop to cancel.")?;
        match choice.as_deref() {
            Some("keep") => Some(booking),
            Some("release") => None,
            _ => return Ok(()),
        }
    } else {
        if line("Type save to adopt the new goal without a booking, or :stop.")?.as_deref()
            != Some("save")
        {
            return Ok(());
        }
        None
    };
    let revision = plan["revision"]
        .as_u64()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("too many revisions")?;
    let next = json!({"format":"writer-goal/2","revision":revision,"goal":goal,"lesson":lesson});
    // Serialize cooperative writers on a stable file. Readers open an entire old or new snapshot.
    let mut lock_name = journal
        .file_name()
        .ok_or("missing plan filename")?
        .to_os_string();
    lock_name.push(".lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(journal.with_file_name(lock_name))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    if read_limited(journal, 2048)? != previous {
        return Err("the plan changed while you were deciding; reopen it before revising".into());
    }
    publish_receipt(journal, &next.to_string(), true)?;
    println!("Saved goal revision {revision}.");
    Ok(())
}

pub(super) struct Source<'a> {
    path: Option<&'a Path>,
    permitted: bool,
}

impl<'a> Source<'a> {
    pub(super) fn from_options(args: &'a [String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self {
                path: None,
                permitted: true,
            }),
            [flag] if flag == "--withdraw-source" => Ok(Self {
                path: None,
                permitted: false,
            }),
            [flag, path] if flag == "--writer-pack" => Ok(Self {
                path: Some(Path::new(path)),
                permitted: true,
            }),
            _ => Err("choose --withdraw-source or --writer-pack FILE".into()),
        }
    }

    fn lesson(&self) -> Result<(String, String), String> {
        let raw = match self.path {
            Some(path) => read_limited(path, 256 * 1024)?,
            None => FIXTURE.to_owned(),
        };
        let pins: Value = serde_json::from_str(PINS).map_err(|e| e.to_string())?;
        let expected = pins["writer"].as_str().ok_or("missing writer review pin")?;
        if format!("{:x}", Sha256::digest(raw.as_bytes())) != expected {
            return Err("writer edition differs from its review".into());
        }
        let data: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let frame = Frame::from_json(&data)?;
        let plan = Proposal::from_json(&data["proposal"])?;
        let record = &data["first_pack"];
        let release = publish(&frame, &plan, frame.review_digest(&plan), record)?;
        if release.unit() != UNIT
            || !plan
                .units()
                .iter()
                .any(|u| u.id == UNIT && u.requires.is_empty())
        {
            return Err("writing sample needs a reviewed introductory unit".into());
        }
        let material = release.material(self.permitted)?;
        let receipt = format!("writer-return/2 first-reader {UNIT} {expected}\n");
        let text = format!(
            "{}\nWorked example\n{}\nSource: {}\nWritten practice\n{}",
            material.lesson,
            material.worked,
            record["locator"].as_str().ok_or("missing source locator")?,
            material.practice
        );
        Ok((receipt, text))
    }
}

pub(super) fn offer(request: &str, journal: &Path, source: &Source<'_>) -> Result<(), String> {
    // Matching a document type justifies showing a scope question, never booking the lesson.
    let asks_about_notice = request.split(|c: char| !c.is_alphabetic()).any(|word| {
        matches!(
            word.to_ascii_lowercase().as_str(),
            "notice" | "notices" | "announcement" | "newsletter"
        )
    });
    if !asks_about_notice {
        println!("The local library has no matching lesson. Your goal stays open.");
        return Ok(());
    }
    let Ok((receipt, _)) = source.lesson() else {
        println!("The first-reader writing source is unavailable. No lesson was booked.");
        return Ok(());
    };
    println!("{SCOPE}");
    let booking = line("Would that narrower lesson serve part of your goal? Type save for the exercise, save with goal to also remember a longer-term aim locally, or :stop.")?;
    if matches!(booking.as_deref(), Some("save" | "save with goal")) {
        let plan = if booking.as_deref() == Some("save with goal") {
            let Some(goal) = line("Enter only the goal you want saved locally (up to 240 UTF-8 bytes). Type :stop to cancel the booking.")? else { return Ok(()) };
            if !safe_goal(&goal) {
                return Err("goal must be one plain-text line of 1–240 bytes".into());
            }
            Some(json!({"format":"writer-goal/1","goal":goal,"lesson":receipt}).to_string())
        } else {
            None
        };
        if !source.lesson().is_ok_and(|(current, _)| current == receipt) {
            println!("The writing source changed while you were deciding. No lesson was booked.");
            return Ok(());
        }
        save_receipt(journal, plan.as_deref().unwrap_or(&receipt))?;
        if plan.is_some() {
            println!("Saved your chosen goal and the separate written exercise.");
        } else {
            println!("Saved the agreed written exercise and its reviewed edition. Your original request stays private.");
        }
    } else {
        println!("No topic was saved.");
    }
    Ok(())
}

pub(super) fn reopen(receipt: &str, source: &Source<'_>) -> Result<(), String> {
    let Ok((current, lesson)) = source.lesson() else {
        println!("The first-reader writing source is unavailable. Your lesson is on hold.");
        return Ok(());
    };
    if receipt != current {
        return Err("lesson receipt does not match the reviewed edition".into());
    }
    println!("{SCOPE}\n{lesson}");
    if line("Try the written practice privately, or :stop.")?.is_some() {
        println!("I received your text. A reader can compare a proposed sentence with the brief.");
    }
    Ok(())
}
