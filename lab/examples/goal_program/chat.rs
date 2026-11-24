//! A live, in-memory conversation for the goal examples. Only an explicit submission
//! goes to the course checker. A workshop draft stays in its own activity.
use serde_json::Value;
use std::io::{BufRead, Read, Write};
use std::time::Instant;
use tutor_lab::curriculum::goal_rehearsal::{
    interactive_opening, opening_check, opening_feedback, Options,
};
use tutor_lab::curriculum::live_turn;
use tutor_lab::openrouter::{Config, Role};
use tutor_lab::workshop::live;
use tutor_lab::workshop::{
    course::CourseTurn,
    draft_model::{self, ExportChoice, MAX_CAPTURE_BYTES},
    Focus, Workshop, MAX_DRAFT_BYTES,
};

pub struct Course<'a> {
    pub fixture: &'a str,
    pub pin: &'a str,
    pub options: &'a Options,
}

impl Course<'_> {
    pub(super) fn opening(&self) -> Result<(String, bool), String> {
        interactive_opening(self.fixture, self.pin, self.options)
    }
}

#[derive(PartialEq, Eq)]
pub enum Start {
    Course,
    Workshop,
    Switch,
}

/// Recognize a few explicit ways to change activity. A literal answer can always be
/// prefixed with /answer. Text inside a submitted draft never passes through here.
pub fn requests_work(line: &str) -> bool {
    let text = line.trim().to_ascii_lowercase();
    if text == "/work" {
        return true;
    }
    let words: Vec<_> = text
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|s| !s.is_empty())
        .collect();
    let request = [
        "can we ",
        "could we ",
        "can you ",
        "could you ",
        "would you ",
        "can i ",
        "could i ",
        "please ",
        "i want ",
        "i'd like ",
        "let's ",
    ]
    .iter()
    .any(|start| text.starts_with(start));
    let artifacts = [
        "writing",
        "draft",
        "paragraph",
        "passage",
        "essay",
        "story",
        "poem",
        "chapter",
        "letter",
        "email",
        "project",
        "proof",
        "notes",
        "work",
        "solution",
    ];
    let editing = [
        "work", "switch", "share", "bring", "paste", "use", "critique", "review", "read", "edit",
        "feedback", "look", "help",
    ];
    let own_artifact = words
        .windows(2)
        .any(|pair| pair[0] == "my" && artifacts.contains(&pair[1]))
        || words.windows(3).any(|triple| {
            triple[0] == "my"
                && ["own", "existing", "new", "unfinished"].contains(&triple[1])
                && artifacts.contains(&triple[2])
        });
    request && own_artifact && words.iter().any(|w| editing.contains(w))
}

pub(super) fn say(output: &mut impl Write, text: &str) -> Result<(), String> {
    writeln!(output, "{text}")
        .and_then(|()| output.flush())
        .map_err(|e| e.to_string())
}

// Cap every read, including a pipe with no line break.
pub(super) fn line(input: &mut impl BufRead, limit: usize) -> Result<Option<String>, String> {
    let mut value = String::new();
    let bytes = input
        .take((limit + 3) as u64)
        .read_line(&mut value)
        .map_err(|e| e.to_string())?;
    if bytes == 0 {
        return Ok(None);
    }
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    if value.len() > limit {
        return Err(format!("Input line exceeds {limit} bytes."));
    }
    Ok(Some(value))
}

enum Block {
    Text(String),
    Cancel,
    EndSession,
}

fn block(input: &mut impl BufRead, limit: usize) -> Result<Block, String> {
    let mut text = String::new();
    let mut first = true;
    while let Some(line) = line(input, limit)? {
        match line.as_str() {
            ".end" => return Ok(Block::Text(text)),
            ".cancel" => return Ok(Block::Cancel),
            _ => {}
        }
        // Dot-stuffing lets a learner include the delimiter or a literal leading dot.
        let content = if line.starts_with("..") {
            &line[1..]
        } else {
            &line
        };
        let sep = usize::from(!first);
        if text.len() + sep + content.len() > limit {
            return Err(format!("Pasted text exceeds {limit} bytes."));
        }
        if !first {
            text.push('\n');
        }
        text.push_str(content);
        first = false;
    }
    Ok(Block::EndSession)
}

const FOCUS: &str = "What would you like to explore in your draft? For writing choose voice, rhythm or clarity. For other work in progress choose next-step. Press Return for clarity, /resume to leave the workshop, or /quit.";
const PASTE: &str = "Paste your own passage across as many lines as you like. Type .end on a line by itself to finish, or .cancel to discard this paste. Double a leading dot to keep it literally.";
const ACTIONS: &str = "Try /revise to paste a new version, /focus voice (or rhythm, clarity or next-step), /resume or /quit. /request MODEL prints a private request containing this draft. /replay reads a captured reply.";

fn local_feedback(output: &mut impl Write, workshop: &Workshop) -> Result<(), String> {
    let feedback = workshop.feedback();
    let quote = &workshop.draft()[feedback.span];
    let message = format!(
        "Local {} exercise · draft {}\n“{}”\n{}\n{}\n{}",
        workshop.focus(),
        workshop.revision(),
        quote,
        feedback.observation,
        feedback.experiment,
        ACTIONS
    );
    // Preserve the submitted bytes for revision and quotation binding. Escape terminal
    // control sequences only in the human-facing copy of learner-supplied prose.
    let mut display = String::new();
    for c in message.chars() {
        if (c.is_control() && !matches!(c, '\n' | '\t'))
            || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            display.extend(c.escape_unicode());
        } else {
            display.push(c);
        }
    }
    say(output, &display)
}

// Display billable usage when OpenRouter returns it, even if the subsequent
// educational validator decides to withhold the result. Response bodies stay private.
fn complete(
    output: &mut impl Write,
    config: &Config,
    role: Role,
    body: &Value,
) -> Result<Value, String> {
    let began = Instant::now();
    let reply = config.complete(role, body)?;
    // Emit a label from the local decision menu only. A draft or an unknown
    // label echoed by a provider does not belong in a transport receipt.
    let decision = body["questions"]
        .as_object()
        .and_then(|questions| {
            let (key, question) = questions.iter().next()?;
            let answer = &reply["answers"][key];
            let choice = answer["choice"].as_str()?;
            (answer["type"] == "choice" && question["criteria"].get(choice).is_some())
                .then(|| format!(" · chose {choice}"))
        })
        .unwrap_or_default();
    let cost = reply["usage"]["cost"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| format!(" · reported cost ${n:.8}"))
        .unwrap_or_default();
    let reported_model = reply["model"]
        .as_str()
        .ok_or("missing OpenRouter model receipt")?;
    say(
        output,
        &format!(
            "OpenRouter · {reported_model}{decision} · {} ms{cost}",
            began.elapsed().as_millis()
        ),
    )?;
    Ok(reply)
}

fn draft_feedback(
    output: &mut impl Write,
    work: &Workshop,
    config: Option<&Config>,
    reply: Option<(&str, &str)>,
    compare: bool,
) -> Result<Option<String>, String> {
    let Some(config) = config else {
        local_feedback(output, work)?;
        return Ok(None);
    };
    let send = |role, body: &Value| complete(output, config, role, body);
    let jev = config.model(Role::Jev);
    let luna = config.model(Role::Luna);
    let result = if compare {
        live::coach_revision(work, jev, luna, ExportChoice::ShareThisDraft, send)
    } else {
        live::coach_with_reply(work, jev, luna, ExportChoice::ShareThisDraft, reply, send)
    };
    let report = match result {
        Ok(report) => report,
        Err(failure) => {
            say(
                output,
                &format!(
                    "Live coaching unavailable at {:?} ({} attempted calls). {}",
                    failure.stage, failure.calls, failure.message
                ),
            )?;
            local_feedback(output, work)?;
            return Ok(None);
        }
    };
    match report.outcome {
        live::Outcome::Abstain => {
            say(
                output,
                "The live pass offered no grounded suggestion for this version.",
            )?;
            local_feedback(output, work)?;
        }
        live::Outcome::Suggestion(s) => {
            let passage = if let Some(c) = report.comparison {
                format!(
                    "Earlier · revision {} · “{}”\nNow · revision {} · “{}”",
                    c.from_revision, c.before, c.to_revision, c.after
                )
            } else {
                format!("OpenRouter coaching about “{}”", s.quote)
            };
            say(
                output,
                &format!("{passage}\n{}\n{}\n{ACTIONS}", s.suggestion, s.question),
            )?;
        }
        live::Outcome::Clarify(s) => {
            say(output, &format!("OpenRouter asks about “{}”\n{}\nReply on one line, or use a workshop command.\n{ACTIONS}", s.quote, s.question))?;
            return Ok(Some(s.question));
        }
    }
    Ok(None)
}

enum WorkshopExit {
    Resume,
    Quit,
}

/// Send no document text to the course lane. Only the fact of receiving coaching
/// can change how a resumed check is used.
fn workshop(
    input: &mut impl BufRead,
    output: &mut impl Write,
    mut parked: Option<&mut CourseTurn>,
    config: Option<&Config>,
) -> Result<WorkshopExit, String> {
    if config.is_some() {
        say(output, "Live workshop: /revise shares the current draft only. To invite a before-and-after comparison of an edit, use /revise compare.")?;
    }
    say(output, FOCUS)?;
    let mut focus = Focus::Clarity;
    loop {
        let Some(choice) = line(input, 256)? else {
            return Ok(WorkshopExit::Quit);
        };
        match choice.trim() {
            "/resume" => return Ok(WorkshopExit::Resume),
            "/quit" => return Ok(WorkshopExit::Quit),
            "" => break,
            s => match s.to_lowercase().parse() {
                Ok(f) => {
                    focus = f;
                    break;
                }
                Err(_) => say(output, FOCUS)?,
            },
        }
    }

    let mut draft = loop {
        say(output, PASTE)?;
        match block(input, MAX_DRAFT_BYTES)? {
            Block::EndSession => return Ok(WorkshopExit::Quit),
            Block::Cancel => return Ok(WorkshopExit::Resume),
            Block::Text(text) => match Workshop::new(&text, focus) {
                Ok(w) => break w,
                Err(reason) => say(output, reason)?,
            },
        }
    };
    if let Some(lane) = parked.as_mut() {
        lane.coached();
    }
    let mut clarification = draft_feedback(output, &draft, config, None, false)?;
    while let Some(action) = line(input, 1024)? {
        let (verb, arg) = action.trim().split_once(' ').unwrap_or((action.trim(), ""));
        let arg = arg.trim();
        match (verb, arg) {
            ("/resume", "") => return Ok(WorkshopExit::Resume),
            ("/quit", "") => return Ok(WorkshopExit::Quit),
            ("/revise", "" | "compare") => {
                let compare = arg == "compare";
                if compare && config.is_none() {
                    say(output,"Revision comparison is available in the live workshop. Use /revise for local feedback.")?;
                    continue;
                }
                if compare {
                    say(output,"For this edit, Jev may receive one earlier excerpt of up to 384 bytes alongside your current draft. Luna receives that earlier excerpt only if Jev selects comparison. Use .cancel to keep it private.")?;
                }
                say(output, PASTE)?;
                match block(input, MAX_DRAFT_BYTES)? {
                    Block::EndSession => return Ok(WorkshopExit::Quit),
                    Block::Cancel => say(
                        output,
                        "Kept the previous draft. Choose /revise, /focus, /resume or /quit.",
                    )?,
                    Block::Text(text) if config.is_some() && text == draft.draft() => say(
                        output,
                        "This is the current draft. Your feedback still applies.",
                    )?,
                    Block::Text(text) => match if compare {
                        draft.revise_for_comparison(&text)
                    } else {
                        draft.revise(&text)
                    } {
                        Ok(()) => {
                            if compare && draft.comparison().is_none() {
                                say(output,"I couldn’t match one distinct edited passage under this focus. Continuing with the current draft.")?;
                            }
                            clarification = draft_feedback(output, &draft, config, None, compare)?;
                        }
                        Err(reason) => say(output, reason)?,
                    },
                }
            }
            ("/focus", s) => match s.to_lowercase().parse() {
                Ok(f) if config.is_some() && draft.focus() == f => {
                    say(output, "You already have feedback for this focus.")?
                }
                Ok(f) => {
                    draft.set_focus(f);
                    clarification = draft_feedback(output, &draft, config, None, false)?;
                }
                Err(_) => say(
                    output,
                    "Use /focus voice, /focus rhythm, /focus clarity or /focus next-step.",
                )?,
            },
            ("/request", model) if !model.is_empty() => {
                match draft_model::request(
                    draft.draft(),
                    draft.revision(),
                    draft.focus(),
                    model,
                    ExportChoice::ShareThisDraft,
                ) {
                    Ok(request) => say(output, &request.to_string())?,
                    Err(reason) => say(output, &reason)?,
                }
            }
            ("/replay", "") => {
                say(
                    output,
                    "Paste the JSON reply for this draft and focus. End with .end on its own line.",
                )?;
                match block(input, MAX_CAPTURE_BYTES)? {
                    Block::EndSession => return Ok(WorkshopExit::Quit),
                    Block::Cancel => say(output, "Reply discarded. Your draft is still open.")?,
                    Block::Text(reply) => match draft_model::replay(
                        draft.draft(),
                        draft.revision(),
                        draft.focus(),
                        reply.as_bytes(),
                    ) {
                        Ok(Some(s)) => say(
                            output,
                            &format!(
                                "Captured suggestion about “{}”\n{}\n{}",
                                s.quote, s.suggestion, s.question
                            ),
                        )?,
                        Ok(None) => {
                            say(output, "The model offered no suggestion for this passage.")?
                        }
                        Err(reason) => say(output, &reason)?,
                    },
                }
            }
            _ if !action.trim().is_empty()
                && !action.trim_start().starts_with('/')
                && clarification.is_some() =>
            {
                let question = clarification.take().expect("a clarification is pending");
                clarification = draft_feedback(
                    output,
                    &draft,
                    config,
                    Some((&question, action.trim())),
                    false,
                )?;
            }
            _ => say(output, ACTIONS)?,
        }
    }
    Ok(WorkshopExit::Quit)
}

/// Offer one reply to the optional question. The submitted exercise already has
/// its local disposition; only an explicitly entered reflection reaches inference.
pub(super) fn close_course_question<W: Write>(
    note: live_turn::Coaching,
    input: &mut impl BufRead,
    output: &mut W,
    workshop_config: Option<&Config>,
    mut send: impl FnMut(&mut W, Role, &Value) -> Result<Value, String>,
) -> Result<(), String> {
    say(output, &format!("Optional coaching about “{}”\n{}\nReply on one line for a closing suggestion, enter /work to bring your own work, or press Enter or type /quit to finish.", note.quote(), note.question()))?;
    loop {
        let Some(message) = line(input, 2048)? else {
            return Ok(());
        };
        let trimmed = message.trim();
        if trimmed.is_empty() || trimmed == "/quit" {
            return Ok(());
        }
        if requests_work(trimmed) {
            if matches!(
                workshop(input, output, None, workshop_config)?,
                WorkshopExit::Resume
            ) {
                say(
                    output,
                    "Your course exercise was already submitted. This session is complete.",
                )?;
            }
            return Ok(());
        }
        if trimmed.starts_with('/') && !trimmed.starts_with("/reply ") {
            say(
                output,
                "Enter /reply TEXT for a literal reply, or /quit to finish.",
            )?;
            continue;
        }
        let reply = message
            .trim_start()
            .strip_prefix("/reply ")
            .unwrap_or(&message);
        match note.close(reply, |role,body| send(output,role,body)) {
            Ok(report) => match report.coaching {
                Some(note) => say(output, &format!("Closing suggestion about “{}”\n{}",note.quote,note.text))?,
                None => say(output, "You can keep that reflection with your exercise. This coaching turn is complete.")?,
            },
            Err(failure) => say(output, &format!("Closing suggestion unavailable at {:?} ({} attempted calls). {}\nYour earlier course feedback still applies.",failure.stage,failure.calls,failure.message))?,
        }
        return Ok(());
    }
}

/// A literal answer has priority. Only an explicit command or a narrow request
/// for course material opens the pre-submission help lane.
pub(super) fn course_help_request(line: &str) -> Option<&str> {
    let line = line.trim();
    if line == "/help" {
        return Some("");
    }
    if let Some(request) = line.strip_prefix("/help ") {
        return Some(request.trim());
    }
    if line.starts_with('/') {
        return None;
    }
    let normalized = line.trim_end_matches(['?', '.', '!']).to_ascii_lowercase();
    let ask = [
        "can you ",
        "could you ",
        "would you ",
        "can i ",
        "could i ",
        "can we ",
        "could we ",
        "please ",
    ]
    .iter()
    .find_map(|prefix| normalized.strip_prefix(prefix))
    .unwrap_or(&normalized);
    let request = [
        "show me ",
        "give me ",
        "give an example",
        "see ",
        "look at ",
        "walk me through ",
        "help me ",
    ]
    .iter()
    .any(|prefix| ask.starts_with(prefix))
        && [
            "example",
            "source",
            "passage",
            "how to start",
            "how to begin",
            "how to do",
        ]
        .iter()
        .any(|target| ask.contains(target));
    (request
        || ask.starts_with("where in the passage ")
        || ask.starts_with("where in the brief ")
        || (line.ends_with('?')
            && matches!(ask, "example" | "worked example" | "source" | "passage"))
        || matches!(
            ask,
            "i'm stuck" | "i don't know where to start" | "i don't know how to begin"
        ))
    .then_some(line)
}

pub(super) fn show_course_help<W: Write>(
    course: &Course<'_>,
    lane: &mut CourseTurn,
    input: &str,
    output: &mut W,
    jev: Option<&str>,
    mut send: impl FnMut(&mut W, Role, &Value) -> Result<Value, String>,
) -> Result<(), String> {
    let guided = lane.practice_only();
    let route = match live_turn::explicit_help(
        course.fixture,
        course.pin,
        course.options,
        input,
        guided,
    ) {
        Ok(Some(card)) => Ok(live_turn::Help::Material(card)),
        Ok(None) => {
            let request = course_help_request(input).unwrap_or(input);
            let Some(jev) = jev.filter(|_| !request.is_empty()) else {
                return say(output, "Use /help example or /help source to read a reviewed card locally. Your exercise is still open.");
            };
            live_turn::help(
                course.fixture,
                course.pin,
                course.options,
                request,
                guided,
                jev,
                |role, body| send(output, role, body),
            )
        }
        Err(reason) => Err(reason),
    };
    match route {
        Ok(live_turn::Help::Material(card)) => {
            let question = opening_check(course.fixture, course.pin, course.options)?;
            say(output, &card.display())?;
            say(output, &format!("Try the same exercise as guided practice.\n{question}"))?;
            lane.redisplayed();
            Ok(())
        }
        Ok(live_turn::Help::Clarify) => say(
            output,
            "Choose /help example for the worked example or /help source for the source passage. Both open locally.",
        ),
        Ok(live_turn::Help::KeepOpen) => say(output, "Your exercise is still open. Take your time."),
        Err(failure) => say(
            output,
            &format!(
                "Course help unavailable at {:?} ({} attempted calls). {}\nYour exercise is still open.",
                failure.stage, failure.calls, failure.message
            ),
        ),
    }
}

/// Close after one local display. The ordinary help action invites guided practice.
pub(super) fn final_example(
    course: &Course<'_>,
    guided: bool,
    output: &mut impl Write,
) -> Result<(), String> {
    match live_turn::explicit_help(
        course.fixture,
        course.pin,
        course.options,
        "/help example",
        guided,
    ) {
        Ok(Some(live_turn::ReleasedMaterial::WorkedExample(text))) => say(
            output,
            &format!("Worked example\n{text}\nWe can stop here."),
        ),
        _ => say(
            output,
            "The worked example is unavailable for this opening. We can stop here.",
        ),
    }
}

pub fn session(
    input: &mut impl BufRead,
    output: &mut impl Write,
    course: Option<Course<'_>>,
    start: Start,
    config: Option<&Config>,
) -> Result<(), String> {
    if let Some(config) = config {
        say(output, &format!(
            "OpenRouter enabled. Submitted course answers and workshop drafts go to {} for a decision and, when useful, {} for coaching with low reasoning effort. Calls use your OpenRouter credits. Enter /quit to stop.",
            config.model(Role::Jev), config.model(Role::Luna)
        ))?;
    }
    let Some(course) = course else {
        let exit = workshop(input, output, None, config)?;
        if matches!(exit, WorkshopExit::Resume) {
            say(
                output,
                "This workshop began without a course. Your writing session is closed.",
            )?;
        }
        return Ok(());
    };
    let (opening, question_shown) = course.opening()?;
    if start != Start::Switch {
        say(output, &opening)?;
    }
    let mut lane = question_shown.then(CourseTurn::opened);
    let mut switch = start == Start::Switch;
    loop {
        if switch {
            if let Some(lane) = &mut lane {
                lane.park()?;
            }
            say(
                output,
                "The course is parked. Workshop feedback is ungraded.",
            )?;
            if matches!(
                workshop(input, output, lane.as_mut(), config)?,
                WorkshopExit::Quit
            ) {
                return Ok(());
            }
            if let Some(lane) = &mut lane {
                let check = opening_check(course.fixture, course.pin, course.options)?;
                lane.resume()?;
                if lane.practice_only() {
                    lane.redisplayed();
                    say(output, &format!("Back to your course. You can use the original question as guided practice. An independent check needs a fresh task.\n{check}"))?;
                } else {
                    say(
                        output,
                        "Back to your course. The question above is still open.",
                    )?;
                }
            } else {
                say(output, &course.opening()?.0)?;
            }
            switch = false;
        }
        if !question_shown {
            say(
                output,
                "Enter /work to use your own work, or /quit to finish this unit preview.",
            )?;
        } else if config.is_some() {
            say(output, "Your turn. Answer the question, use /help TEXT to ask for help, /help example or /help source to open a card locally, /example-and-stop to finish with one example, /work to bring your own work, or /quit. Use /answer TEXT for a literal answer.")?;
        } else {
            say(output, "Your turn. Answer the question, enter /help example or /help source for a local card, /example-and-stop to finish with one example, /work to bring your own work, or /quit. Use /answer TEXT for a literal answer.")?;
        }
        let Some(message) = line(input, 4096)? else {
            return Ok(());
        };
        if message.trim() == "/quit" {
            return Ok(());
        }
        if message.trim() == "/example-and-stop" {
            if !question_shown {
                return say(
                    output,
                    "There is no open exercise with an available worked example. We can stop here.",
                );
            }
            return final_example(
                &course,
                lane.as_ref().is_some_and(CourseTurn::practice_only),
                output,
            );
        }
        if requests_work(&message) {
            switch = true;
            continue;
        }
        if message.trim().is_empty() || !question_shown {
            continue;
        }
        let raw = message.trim_start();
        if course_help_request(raw).is_some() && (config.is_some() || raw.starts_with("/help")) {
            let lane = lane.as_mut().expect("a question was displayed");
            show_course_help(
                &course,
                lane,
                raw,
                output,
                config.map(|cfg| cfg.model(Role::Jev)),
                |out, role, body| {
                    complete(
                        out,
                        config.ok_or("live help requires a provider")?,
                        role,
                        body,
                    )
                },
            )?;
            continue;
        }
        let request_text = raw.to_ascii_lowercase();
        if !raw.starts_with("/answer ")
            && raw.trim_end().ends_with('?')
            && [
                "can i ",
                "could i ",
                "can you ",
                "could you ",
                "could we ",
                "can we ",
                "would you ",
            ]
            .iter()
            .any(|start| request_text.starts_with(start))
        {
            say(output, "Would you like coaching on your own passage? Enter /work, or /answer TEXT if those words are your course response.")?;
            continue;
        }
        let body = raw.strip_prefix("/answer ").unwrap_or(&message);
        let guided = lane.as_ref().is_some_and(CourseTurn::practice_only);
        let feedback = opening_feedback(course.fixture, course.pin, course.options, body, guided)?;
        say(output, &feedback)?;
        if let Some(config) = config {
            let result = live_turn::coach(
                course.fixture,
                course.pin,
                course.options,
                body,
                guided,
                (config.model(Role::Jev), config.model(Role::Luna)),
                |role, request| complete(output, config, role, request),
            );
            match result {
                Ok(report) => match (report.material, report.coaching) {
                    (Some(material), _) => say(output, &material.display())?,
                    (_, Some(note)) => close_course_question(
                        note,
                        input,
                        output,
                        Some(config),
                        |out, role, request| complete(out, config, role, request),
                    )?,
                    (None, None) => say(output, "The live pass kept the course feedback above.")?,
                },
                Err(failure) => say(
                    output,
                    &format!(
                        "Live coaching unavailable at {:?} ({} attempted calls). {}",
                        failure.stage, failure.calls, failure.message
                    ),
                )?,
            }
        }
        return Ok(());
    }
}
