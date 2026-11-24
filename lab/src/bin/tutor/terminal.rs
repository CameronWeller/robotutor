use crate::{
    display,
    judgment::{self, Finding, Kind},
    material,
    session::Visit,
    study::{Skill, Target},
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const EXAMPLES: &[&str] = include!(concat!(env!("OUT_DIR"), "/lab_examples.rs"));
pub fn catalog(input: &str) -> Result<(), String> {
    let mut words = input.split_whitespace();
    let Some(name) = words.next() else {
        println!(
            "Rust lab experiments · /lab NAME [arguments]\n{}",
            EXAMPLES.join("  ")
        );
        return Ok(());
    };
    if !EXAMPLES.contains(&name) {
        return Err("Unknown experiment. /lab lists the available names.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sibling = exe.parent().unwrap().join("examples").join(name);
    let mut cmd = if sibling.is_file() {
        Command::new(sibling)
    } else {
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.args(["run", "--locked", "--manifest-path"])
            .arg(tutor_lab::platform::project_root().join("lab/Cargo.toml"))
            .args(["--example", name, "--"]);
        cmd
    };
    let status = cmd
        .env("ROBOTUTOR_USE_LOGIN_KEYRING", "1")
        .args(words)
        .status()
        .map_err(|e| format!("Could not launch {name}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} exited with {status}"))
    }
}
pub fn default_workspace() -> Result<PathBuf, String> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(windows)]
            {
                std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            }
            #[cfg(target_os = "macos")]
            {
                std::env::var_os("HOME")
                    .map(|p| PathBuf::from(p).join("Library/Application Support"))
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state"))
            }
        })
        .ok_or("Give --workspace a private directory.")?;
    Ok(state.join("robotutor/learner-tests"))
}
pub fn help() {
    println!("\n/discover      Build or revisit your learner account\n/looks-right    Confirm a reflection\n/start-now     Request a provisional lesson\n/start         Accept the proposed first exercise\n/fit WORDS     Save private feedback about the starting point\n/goal WORDS    Change the study goal\n/plan          Current objective, next step and due reviews\n/skills        Evidence per skill · /skills N shows its source turns\n/assignment  /preview [TEXT]  /submit  /revision    Review and share assigned work\n/activities     Choose a structured activity · /activity NAME\n/work  /field NAME VALUE  /hint  /tap  /run A B  /draft TEXT  /send\n/practice irac  Apply legal reasoning to a fresh fictional case\n/review [N]    Try a fresh problem on a skill\n/continue      Resume the open work or take a due review\n/lessons       Reviewed lessons · /lesson NAME opens one\n/trace [N or VISIT#TURN]    Full decision and delivered reply\n/teacher        Review history and evidence\n/correct /redirect /assign WORDS   Add a teacher observation or direction\n/offline MINUTES   Extend thinking time\n/pilot  /predict  /outcome  Record a private review, forecast or subsequent result\n/sources       Inspect the sources used for the last reply\n/recall WORDS  Find past work · /show VISIT#TURN opens it\n/lab           Run a Rust lab experiment\n/next  /harder  /retry  /reload  /quit");
}
pub fn lessons() {
    for (id, title) in material::CARDS {
        println!("{id} · {title}");
    }
}
pub fn list(skills: &[Skill], arg: &str) -> Result<(), String> {
    if !arg.is_empty() {
        let n = number(arg, skills.len())?;
        let s = &skills[n];
        println!(
            "{} · {}\n{}",
            display(&s.target.objective),
            s.status(),
            display(&s.history.join("\n"))
        );
        println!("Latest source · {}", s.last_source);
    } else {
        if skills.is_empty() {
            println!("Your first delivered lesson will appear here.");
        }
        for (i, s) in skills.iter().enumerate() {
            println!(
                "{}  {} · {} · {}{} · {}",
                i + 1,
                display(&s.target.objective),
                s.status(),
                s.attempts,
                if s.attempts == 1 {
                    " attempt"
                } else {
                    " attempts"
                },
                if s.due {
                    "review due".into()
                } else {
                    s.due_in
                        .map_or("undated".into(), |d| format!("review in {d} days"))
                }
            );
        }
    }
    Ok(())
}
pub fn number(s: &str, len: usize) -> Result<usize, String> {
    s.parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|n| *n < len)
        .ok_or("Choose a skill number from /skills.".into())
}
pub fn plan(visit: &Visit, skills: &[Skill], external: &[Finding]) -> String {
    let lesson = &visit.lesson;
    let target = Target::from_lesson(lesson);
    let mut lines = Vec::new();
    for c in &visit.practice_corrections {
        lines.push(format!("Corrected reading · {} ({})", c.words, c.source));
    }
    for e in visit.practice_history.iter().rev().take(4) {
        lines.push(format!(
            "{} · {} (/show {})",
            e.activity, e.feedback, e.source
        ));
    }
    if let Some(d) = &lesson.discovery {
        lines.push(d.account());
        if d.active() {
            lines.push(format!(
                "Current question · {}\nWhy this question · {}",
                d.question, d.reason
            ));
            return display(&lines.join("\n"));
        }
    }
    if let Some(goal) = &lesson.declared_goal {
        lines.push(format!("Study goal · {goal}"));
    }

    for item in &lesson.recall_items {
        lines.push(format!(
            "Meaning of {} · {:?} (/show {})",
            item.term, item.state, item.source
        ));
    }
    if let Some(recall) = &lesson.name_recall {
        use crate::method::NameRecall::*;
        let status = match recall {
            Supplied => "Name supplied · check after intervening practice",
            Due => "A vocabulary check is ready on return or /continue",
            Asking => "Vocabulary check open",
            Recalled => "Name recalled on the last vocabulary check",
            Reminded => "Name supplied again on the last vocabulary check",
            Skipped => "Vocabulary check set aside",
        };
        lines.push(status.into());
    }
    if let Some(course) = &lesson.course {
        lines.push(format!("Course destination · {}", course.destination));
        for (index, milestone) in course.milestones.iter().enumerate() {
            let marker = if index == course.focus {
                " · current focus"
            } else {
                ""
            };
            lines.push(format!(
                "  {}. {}{marker} — {}",
                index + 1,
                milestone.skill,
                milestone.application
            ));
        }
    }
    // The workspace includes other visits. A terminal without a workspace uses its own journal.
    let mut assessments: std::collections::BTreeMap<_, _> = visit
        .assessments
        .iter()
        .map(|(key, obs)| (key.clone(), obs))
        .collect();
    for obs in skills.iter().filter_map(|s| s.latest_assessment.as_ref()) {
        assessments.insert(obs.target.key(), obs);
    }
    let demonstrated: Vec<_> = assessments
        .values()
        .copied()
        .filter(|o| {
            o.demonstrated() && o.target.label() == target.label() && o.target.card == target.card
        })
        .collect();
    for obs in &demonstrated {
        let label = if obs.assisted {
            "Demonstrated with support"
        } else {
            "Demonstrated"
        };
        lines.push(format!("{label} · {}", obs.target.objective));
    }
    if !lesson.objective.is_empty() {
        lines.push(format!("Current · {}", lesson.objective));
    }
    for finding in judgment::known(visit, external).into_iter().filter(|f| {
        f.proposal.task == lesson.objective
            || (lesson.studying_law()
                && lesson.action() == "method_practice"
                && f.proposal.kind == Kind::Strength
                && visit.teaching_history().iter().any(|(words, _)| {
                    words
                        .as_deref()
                        .is_some_and(|s| s.contains(&f.proposal.quote))
                })
                && ["legal", "legally", "law", "evidence", "argument"]
                    .iter()
                    .any(|term| judgment::mentions(&f.proposal.task, term)))
    }) {
        let label = match finding.proposal.kind {
            Kind::Strength => "Working from",
            Kind::Method => "Method you introduced",
            Kind::PromptedMethod => "Method practiced with help",
            _ => continue,
        };
        lines.push(format!(
            "{label} · {} (/show {})",
            finding.proposal.detail, finding.proposal.source
        ));
    }
    if let Some(r) = &lesson.rubric {
        lines.push(format!("Sufficient when · {}", r.criterion));
    }
    if !lesson.active_step().is_empty() {
        lines.push(format!("Open task · {}", lesson.active_step()));
    }

    if let Some(id) = &lesson.selected {
        let title_at = |stage| {
            material::card(id, stage)
                .ok()
                .and_then(|card| card["objective"].as_str().map(str::to_owned))
        };
        let current = title_at(lesson.stage);
        let next = title_at(lesson.stage.saturating_add(1));
        if let Some(title) = next.as_deref().or_else(|| {
            (!lesson.next_objective.is_empty()).then_some(lesson.next_objective.as_str())
        }) {
            lines.push(format!("Next objective · {title}"));
        }
        if let Ok(units) = material::program(id) {
            lines.push(format!("Course · {id}"));
            let covered: Vec<_> = demonstrated
                .iter()
                .filter(|o| o.target.claim.is_none())
                .filter_map(|o| title_at(o.target.stage))
                .collect();
            for (i, (title, status)) in units.iter().enumerate() {
                let state = if current.as_ref() == Some(title) {
                    "current"
                } else if covered.contains(title) {
                    "practiced successfully"
                } else if next.as_ref() == Some(title) {
                    "next"
                } else {
                    "planned"
                };
                lines.push(format!("  {}. {title} · {state} · {status}", i + 1));
            }
        }
    } else {
        let covered: Vec<_> = demonstrated
            .iter()
            .map(|o| o.target.objective.as_str())
            .collect();
        for (i, title) in lesson
            .remaining_objectives(lesson.stage.saturating_add(1), &covered)
            .iter()
            .take(if lesson.course.is_some() {
                1
            } else {
                usize::MAX
            })
            .enumerate()
        {
            let label = if i == 0 { "Next objective" } else { "Later" };
            lines.push(format!("{label} · {title}"));
        }
    }
    for (i, s) in skills.iter().enumerate().filter(|(_, s)| s.due).take(3) {
        lines.push(format!("Due · /review {} · {}", i + 1, s.target.objective));
    }
    if lesson.objective.is_empty() && skills.is_empty() {
        lines.push("Name a subject or open a reviewed lesson with /lessons.".into());
    }
    display(&lines.join("\n"))
}

/// On a recorded interactive visit, upgrade only while waiting for the next line.
/// The terminal's canonical input queue survives exec, including a partly typed line.
pub struct Installed {
    path: PathBuf,
    inode: u64,
    device: u64,
    resume: Command,
}
impl Installed {
    pub fn current(
        workspace: Option<&Path>,
        journal: Option<&Path>,
        shadow: bool,
    ) -> Result<Self, String> {
        let path = std::env::current_exe().map_err(|e| e.to_string())?;
        #[cfg(target_os = "linux")]
        let executable = std::fs::File::open("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let executable = std::fs::File::open(&path);
        let (device, inode) = executable
            .and_then(|f| tutor_lab::platform::file_identity(&f))
            .map_err(|e| e.to_string())?;
        let mut resume = Command::new(&path);
        if let Some(dir) = workspace {
            resume.arg("--workspace").arg(dir);
        }
        if let Some(p) = journal {
            let selected = if workspace.is_some() {
                Path::new(p.file_name().ok_or("Missing journal filename")?)
            } else {
                p
            };
            resume.arg("--session").arg(selected);
        }
        if shadow {
            resume.arg("--shadow");
        }
        resume.env("ROBOTUTOR_REOPENED", "1");
        Ok(Self {
            resume,
            path,
            inode,
            device,
        })
    }
    pub fn replaced(&self) -> bool {
        std::fs::File::open(&self.path)
            .and_then(|f| tutor_lab::platform::file_identity(&f))
            .is_ok_and(|id| id != (self.device, self.inode))
    }
    pub fn reload(&mut self) -> String {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            self.resume.exec().to_string()
        }
        #[cfg(windows)]
        {
            match self.resume.spawn() {
                Ok(_) => std::process::exit(0),
                Err(e) => e.to_string(),
            }
        }
    }
}

/// Open the learner's named, detachable terminal on their Linux desktop.
pub fn open_window() -> Result<(), String> {
    let socket = "luna-lab";
    let unit = "robotutor-luna-lab.service";
    let command = |program: &str| {
        let mut cmd = Command::new(program);
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
                cmd.env(
                    "DBUS_SESSION_BUS_ADDRESS",
                    format!("unix:path={}/bus", PathBuf::from(dir).display()),
                );
            }
        }
        cmd
    };
    let exists = || {
        command("screen")
            .args(["-S", socket, "-Q", "select", "."])
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !exists() {
        let dir = default_workspace()?;
        let prior = crate::optional_context(
            "previous visit",
            crate::memory::Workspace::open(&dir).and_then(|mut w| {
                w.sync()?;
                w.continuation("")
            }),
        )
        .flatten();
        crate::memory::local_journal(&dir, None).map_err(|e| e.to_string())?;
        let mut start = command("systemd-run");
        start
            .args([
                "--user",
                "--collect",
                "--unit",
                unit,
                "--setenv=LANG=C.UTF-8",
                "--",
                "screen",
                "-U",
                "-D",
                "-m",
                "-S",
                socket,
            ])
            .arg(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--workspace")
            .arg(&dir);
        if let Some((source, lesson)) = prior {
            if let Some((visit, _)) = source.source.rsplit_once('#') {
                let path = crate::memory::local_journal(
                    &dir,
                    Some(std::path::Path::new(&format!("{visit}.jsonl"))),
                )
                .map_err(|e| e.to_string())?;
                // A different active tutor may own this visit. Give the new terminal a private continuation.
                let locked = std::fs::File::open(&path).is_ok_and(|f| f.try_lock().is_err());
                if locked
                    && lesson
                        .discovery
                        .as_ref()
                        .is_some_and(crate::discovery::Discovery::active)
                {
                    return Err("This discovery conversation is open in another client. Close that visit to resume the same question here.".into());
                }
                if locked {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| e.to_string())?;
                    start.args(["--session", &format!("visit-{}.jsonl", now.as_micros())]);
                } else {
                    start.arg("--session").arg(path.file_name().unwrap());
                }
            }
        }
        let status = start
            .status()
            .map_err(|e| format!("Could not start the tutor service: {e}"))?;
        // Another launch may have started it concurrently.
        for _ in 0..30 {
            if exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !exists() {
            return Err(format!(
                "Tutor service did not open ({status}). Check systemctl --user status {unit}."
            ));
        }
    }
    let status = command("ptyxis")
        .args([
            "--new-window",
            "--title=Robotutor · Luna lab",
            "--",
            "screen",
            "-U",
            "-x",
            socket,
        ])
        .status()
        .map_err(|e| format!("Tutor is available with screen -x {socket}. Desktop window: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Attach to the tutor with screen -x {socket}."))
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    #[test]
    fn reload_pins_the_open_journal_even_when_the_workspace_day_changes() {
        let dir = Path::new("/study");
        let journal = dir.join("earlier-day.jsonl");
        let installed = Installed::current(Some(dir), Some(&journal), true).unwrap();
        assert_eq!(
            installed.resume.get_args().collect::<Vec<_>>(),
            [
                "--workspace",
                "/study",
                "--session",
                "earlier-day.jsonl",
                "--shadow"
            ]
        );
        let standalone = Installed::current(None, Some(&journal), false).unwrap();
        assert_eq!(
            standalone.resume.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("--session"), journal.as_os_str()]
        );
    }
}
