//! Common command line for the offline goal rehearsals.
mod chat;
mod intake;
mod saved;

use serde_json::Value;
use std::{
    io::{self, BufRead, IsTerminal, Read, Write},
    process::ExitCode,
};
use tutor_lab::curriculum::goal_rehearsal::{
    interactive_opening, run, with_author_input, Branch, Options, View,
};

use tutor_lab::{curriculum::live_author, openrouter::Config};

const HELP: &str = concat!(
    "The default shows the complete rehearsal. Views: --intake, --opening, --learner, --author-request first|return.\n",
    "Use --first or --return with demonstrated, revisit or unobserved. Use --answer-stdin for your own opening response.\n",
    "Vary the later visit with --return-day N, --practice-day N, --unknown-history or --withdraw-return. ",
    "Use --setting TEXT to change the proposed scope.\n",
    "--unit ID shows available material or what it needs. --author-request unit --unit ID shows a commissioned brief.\n",
    "--session opens a course conversation. Enter /work to bring a draft. --workshop starts directly with your own work.\n",
    "For the saved Psalm 100 conversation, use worship --session --journal FILE. Choose a sample or exercise before it opens.\n",
    "--intake-session starts without an assumed course. Add --openrouter to offer optional Jev recommendations and live workshops.\n",
    "Add --openrouter to either session to send submitted work to Jev and GPT-6 Luna (low reasoning), billed to OPENROUTER_API_KEY.\n",
    "Add --openrouter to --author-request first|return|unit for an unpublished editor draft and advisory report."
);
const REVIEWS: &str = include_str!("../../fixtures/goals/reviews.json");

fn execute(slug: &str, fixture: &str) -> Result<String, String> {
    let mut args = std::env::args().skip(1);
    let mut opts = Options::default();
    let mut from_stdin = false;
    let mut journal = None;
    let mut first_selected = false;
    let mut openrouter = false;
    let mut start = None;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(flag) = args.next() {
        if !seen.insert(flag.clone()) {
            return Err(format!("duplicate option {flag}"));
        }
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing argument for {flag}"))
        };
        match flag.as_str() {
            "--help" | "-h" => return Ok(HELP.into()),
            "--session" | "--workshop" => {
                if start.is_some() {
                    return Err("choose a course session or a standalone workshop".into());
                }
                start = Some(if flag == "--session" {
                    chat::Start::Course
                } else {
                    chat::Start::Workshop
                });
            }
            "--intake-session" => {}
            "--journal" => journal = Some(std::path::PathBuf::from(value()?)),
            "--openrouter" => openrouter = true,
            "--unit" => {
                let id = value()?
                    .parse::<u32>()
                    .map_err(|_| "unit must be a positive ID")?;
                if id == 0 {
                    return Err("unit must be a positive ID".into());
                }
                opts.unit = Some(id);
            }
            "--learner" | "--intake" | "--opening" | "--author-request" => {
                if opts.view != View::Audit {
                    return Err("choose one output view".into());
                }
                opts.view = match flag.as_str() {
                    "--learner" => View::Learner,
                    "--intake" => View::Intake,
                    "--opening" => View::Opening,
                    _ => match value()?.as_str() {
                        "first" => View::AuthorFirst,
                        "return" => View::AuthorReturn,
                        "unit" => View::AuthorUnit,
                        _ => return Err("author request must name first, return or unit".into()),
                    },
                };
            }
            "--first" => {
                opts.first = Branch::parse(&value()?)?;
                first_selected = true;
            }
            "--return" => opts.returning = Branch::parse(&value()?)?,
            "--return-day" => {
                opts.return_day = value()?.parse().map_err(|_| "invalid return day")?
            }
            "--practice-day" => {
                opts.practice_day = Some(value()?.parse().map_err(|_| "invalid practice day")?)
            }
            "--withdraw-return" => opts.withdraw_return = true,
            "--unknown-history" => opts.unknown_history = true,
            "--setting" => opts.changed_setting = Some(value()?),
            "--answer-stdin" => from_stdin = true,
            _ => return Err(format!("unknown option {flag}\n{HELP}")),
        }
    }
    if let Some(path) = journal {
        if slug != "worship" || start != Some(chat::Start::Course) || seen.len() != 2 {
            return Err(
                "the saved lesson is available with worship --session --journal FILE".into(),
            );
        }
        saved::session(&path, &mut io::stdin().lock(), &mut io::stdout().lock())?;
        return Ok(String::new());
    }
    if seen.contains("--intake-session") {
        if seen
            .iter()
            .any(|flag| !matches!(flag.as_str(), "--intake-session" | "--openrouter"))
        {
            return Err("intake-session accepts only the optional --openrouter flag".into());
        }
        let pins: Value = serde_json::from_str(REVIEWS).map_err(|e| e.to_string())?;
        let pin = pins[slug].as_str().ok_or("missing scenario review")?;
        intake::session(
            &mut io::stdin().lock(),
            &mut io::stdout().lock(),
            fixture,
            pin,
            openrouter,
            intake::live,
        )?;
        return Ok(String::new());
    }
    if let Some(mode) = &start {
        let session_flags = [
            "--session",
            "--workshop",
            "--openrouter",
            "--setting",
            "--unit",
            "--learner",
        ];
        if seen
            .iter()
            .any(|flag| !session_flags.contains(&flag.as_str()))
            || (*mode == chat::Start::Workshop
                && (opts.unit.is_some() || opts.changed_setting.is_some()))
        {
            return Err(
                "use --session for a course opening or --workshop to begin with your own work"
                    .into(),
            );
        }
    }
    let author = matches!(
        opts.view,
        View::AuthorFirst | View::AuthorReturn | View::AuthorUnit
    );
    if openrouter && start.is_none() {
        if !author {
            return Err("use --openrouter with --session, --workshop or --author-request".into());
        }
        if seen.iter().any(|flag| {
            ![
                "--openrouter",
                "--author-request",
                "--unit",
                "--setting",
                "--withdraw-return",
            ]
            .contains(&flag.as_str())
        }) {
            return Err("a live material job uses a selected unit, confirmed scope and current source access".into());
        }
    }
    let config = (openrouter && start.is_some())
        .then(Config::from_env)
        .transpose()?;
    if start == Some(chat::Start::Workshop) {
        chat::session(
            &mut io::stdin().lock(),
            &mut io::stdout().lock(),
            None,
            chat::Start::Workshop,
            config.as_ref(),
        )?;
        return Ok(String::new());
    }
    let reviews: Value = serde_json::from_str(REVIEWS).map_err(|e| e.to_string())?;
    let pin = reviews[slug]
        .as_str()
        .ok_or("missing frozen scenario review")?;
    if openrouter && author {
        return with_author_input(fixture, pin, &opts, live_author::dispatch);
    }
    if let Some(mode) = start {
        let course = chat::Course {
            fixture,
            pin,
            options: &opts,
        };
        chat::session(
            &mut io::stdin().lock(),
            &mut io::stdout().lock(),
            Some(course),
            mode,
            config.as_ref(),
        )?;
        return Ok(String::new());
    }
    let mut opening = None;
    if from_stdin {
        if first_selected
            || !matches!(opts.view, View::Learner | View::Audit)
            || opts.changed_setting.is_some()
        {
            return Err("supply an opening response in an agreed audit or learner session without a scripted first branch".into());
        }
        let mut value = String::new();
        let mut stdin = io::stdin().lock();
        let interactive = stdin.is_terminal();
        let prerequisite = opts.unit.is_some();
        if interactive {
            // Preview the first check that the response will actually be bound to.
            opts.view = View::Learner;
            let (preview, asked) = interactive_opening(
                fixture,
                pin,
                &Options {
                    view: View::Opening,
                    unit: None,
                    ..opts.clone()
                },
            )?;
            if !asked {
                return Ok(preview);
            }
            println!("{preview}");
            if prerequisite {
                println!("Answer this opening if you want it considered as a prerequisite for the selected unit.");
            } else {
                println!(
                    "Your response (press Return to submit, or enter /work for your own writing)"
                );
                opening = Some(preview);
            }
            io::stdout().flush().map_err(|e| e.to_string())?;
        }
        stdin
            .by_ref()
            .take(4097)
            .read_line(&mut value)
            .map_err(|e| format!("could not read response: {e}"))?;
        if value.len() > 4096 {
            return Err("opening response exceeds 4096 bytes".into());
        }
        if opts.unit.is_none() && chat::requests_work(&value) {
            let course = chat::Course {
                fixture,
                pin,
                options: &opts,
            };
            if !interactive {
                println!("{}", course.opening()?.0);
            }
            chat::session(
                &mut stdin,
                &mut io::stdout().lock(),
                Some(course),
                chat::Start::Switch,
                None,
            )?;
            return Ok(String::new());
        }
        if !interactive && value.len() < 4097 {
            stdin
                .take((4097 - value.len()) as u64)
                .read_to_string(&mut value)
                .map_err(|e| format!("could not read response: {e}"))?;
        }
        if value.len() > 4096 {
            return Err("opening response exceeds 4096 bytes".into());
        }
        if let Some(literal) = value.strip_prefix("/answer ") {
            value = literal.to_owned();
        }
        opts.first_response = Some(value);
    }
    let transcript = run(fixture, pin, &opts)?;
    match opening {
        Some(prompt) => transcript
            .strip_prefix(&prompt)
            .map(|s| s.trim_start_matches('\n').to_owned())
            .ok_or_else(|| "the opening changed during the rehearsal".into()),
        None => Ok(transcript),
    }
}

pub fn main_for(slug: &str, fixture: &str) -> ExitCode {
    match execute(slug, fixture) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
