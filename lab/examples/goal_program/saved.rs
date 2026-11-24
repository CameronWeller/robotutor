//! Learner entry to the same locked journal and released Psalm 100 lesson as the rehearsal.
use super::chat::{line, say};
use serde_json::Value;
use std::{
    io::{BufRead, Write},
    path::Path,
};
use tutor_lab::rehearsal::{journal, Action, Session};

fn goal(words: &[&str]) -> Action {
    Action::Goal(words.iter().map(|word| (*word).to_owned()).collect())
}

fn head(state: &Value) -> Result<&str, String> {
    state["head"]
        .as_str()
        .ok_or_else(|| "missing journal head".into())
}

pub(super) fn session(
    path: &Path,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), String> {
    let mut state = if path.try_exists().map_err(|e| e.to_string())? {
        journal::read(path, None, Session::status)?
    } else {
        journal::create(path)?
    };
    match state["goal_choice"]["requested_unit"].as_u64() {
        None => state=journal::apply(path,head(&state)?,||Ok(goal(&["choose","101"])))?,
        Some(101) => (),
        Some(_) => return Err("This journal has another active goal. Select Psalm 100 in that journal to continue here.".into()),
    }
    say(out,"Psalm 100 offers a way to explore thanksgiving using fictional situations. You can keep your own words private.")?;
    loop {
        let choice = &state["goal_choice"];
        if let Some(level) = choice["base_contact"].as_str() {
            say(
                out,
                if level == "confirmed" {
                    "You confirmed the first case reached you."
                } else {
                    "The first case may have reached you before."
                },
            )?;
            if let Some(extra) = choice["extra_contact"].as_str() {
                say(
                    out,
                    if extra == "confirmed" {
                        "You also confirmed the second case reached you. Both cases have been used. You can continue with guided practice."
                    } else {
                        "The second case may also have reached you. Both available cases have been used. You can continue with guided practice."
                    },
                )?;
            } else if choice["extra_available"] == true {
                say(
                    out,
                    "A different reviewed case is available if you choose check.",
                )?;
            } else {
                say(
                    out,
                    "You can choose sample or practice, or type request for a different case.",
                )?;
            }
        }
        if choice["awaiting"] == "RequestedProbe" {
            say(out,"Type seen if that case appeared on your screen, or answer followed by your optional response.")?;
        } else if choice["awaiting"] == "Practice" {
            say(out, "Type answer followed by an optional guided response.")?;
        }
        if choice["worked"] == true {
            if choice["base_contact"].is_null() {
                say(out,"First case in this lesson. If you think you have seen it elsewhere, type familiar first to set it aside.")?;
            } else if choice["extra_available"] == true {
                say(out,"A second case is available. If you think you have seen that case elsewhere, type familiar second to set it aside.")?;
            }
        }
        say(out,"Choose sample for a worked example, practice for a guided question, check to reveal an available case, or pause.")?;
        let Some(raw) = line(input, 4096)? else {
            return Ok(());
        };
        let command = raw.trim();
        let new_action = match command {
            "pause" => goal(&["wait"]),
            "familiar first" if choice["worked"] == true && choice["base_contact"].is_null() => {
                goal(&["familiar", "101", "first"])
            }
            "familiar second" if choice["worked"] == true && choice["extra_available"] == true => {
                goal(&["familiar", "101", "second"])
            }
            "sample" => goal(&["study", "101", "repeat"]),
            "practice" | "check" if choice["worked"] != true => {
                say(
                    out,
                    "Begin with the short sample for this lesson. You can also pause.",
                )?;
                continue;
            }
            "request" if choice["extra_contact"].is_string() => {
                say(out,"This lesson has no further reviewed case. The worked example and guided exercise are available.")?;
                continue;
            }
            "practice" | "check" => goal(&["study", "101", command]),
            "request" if choice["base_contact"].is_string() => goal(&["request", "101"]),
            "seen" if choice["awaiting"] == "RequestedProbe" => goal(&["received", "101"]),
            _ if command.starts_with("answer ") && choice["awaiting"].is_string() => {
                Action::GoalWriting(101, command[7..].to_owned())
            }
            _ => {
                say(
                    out,
                    "Choose one of the activities above. Your words have stayed private.",
                )?;
                continue;
            }
        };
        // Committing possible display precedes writing any bytes of a question.
        state = journal::apply(path, head(&state)?, || Ok(new_action))?;
        if command == "pause" {
            return say(out, "We can stop here.");
        }
        let display = &state["display"];
        if let Some(text) = display.as_str() {
            say(out, text)?;
        }
        for field in ["worked", "practice", "case", "availability", "message"] {
            if let Some(text) = display[field].as_str() {
                say(out, text)?;
            }
        }
        if let Some(source) = display["source"].as_str() {
            say(out, source)?;
        }
    }
}

#[cfg(test)]
#[path = "saved_coverage.rs"]
mod coverage;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{self, Cursor},
    };

    #[test]
    fn a_disconnected_display_stays_possible_across_return_and_editor_review() {
        struct DisconnectPrompt(Vec<u8>);
        impl Write for DisconnectPrompt {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if String::from_utf8_lossy(bytes).contains("night worker") {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let dir = std::env::temp_dir().join(format!("saved-lesson-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("lesson");
        let talk = |commands: &str| {
            let mut out = vec![];
            session(&path, &mut Cursor::new(commands), &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        let status = || journal::read(&path, None, Session::status).unwrap();
        let first = talk("sample\npause\n");
        assert!(first.contains("chair") && !first.contains("night worker"));
        assert!(status()["goal_choice"]["base_contact"].is_null());
        let mut broken = DisconnectPrompt(vec![]);
        assert!(session(&path, &mut Cursor::new("check\n"), &mut broken).is_err());
        assert!(!String::from_utf8(broken.0)
            .unwrap()
            .contains("night worker"));
        assert_eq!(status()["goal_choice"]["base_contact"], "possible");
        let resumed = talk("check\npractice\nrequest\npause\n");
        assert!(!resumed.contains("night worker") && resumed.contains("gardener"));
        let before = status();
        let editor = journal::apply(&path, head(&before).unwrap(), || {
            Ok(Action::CaseReview("hall".into()))
        })
        .unwrap();
        assert!(editor["display"].is_null());
        let late = talk("check\nseen\nanswer private invented line\npause\n");
        assert!(late.contains("dry hall") && !late.contains("night worker"));
        let returned = status();
        assert_eq!(returned["goal_choice"]["base_contact"], "possible");
        assert_eq!(returned["goal_choice"]["extra_contact"], "confirmed");
        assert_eq!(returned["checkpoint"], before["checkpoint"]);
        assert!(!fs::read_to_string(&path)
            .unwrap()
            .contains("private invented line"));
        let last = talk("check\npractice\npause\n");
        assert!(last.contains("no further reviewed case") && last.contains("gardener"));
        assert!(!last.contains("dry hall") && !last.contains("night worker"));
        fs::remove_dir_all(dir).unwrap();
    }
}
