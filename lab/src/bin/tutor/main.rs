//! One learner’s terminal tutor, optionally grounded in their private workspace.
use tutor_lab::openrouter::credentials;
mod ablation;
mod assessment;
mod assignment;
mod composer;
mod course;
mod diagnostics;
mod discovery;
mod inspect;
mod judgment;
mod lesson;
mod material;
mod memory;
mod method;
mod metrics;
mod personas;
mod pilot;
mod practice;
mod preparation;
mod progression;
mod recall;
mod recovery;
mod replay;
mod session;
mod study;
mod supervisor;
mod terminal;
mod trace_view;

use serde_json::{json, Value};
use session::{Event, Journal, Visit};
use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    path::PathBuf,
};
use tutor_lab::openrouter::{assistant_text, Config, Role};

const TEACHER: &str = include_str!("teacher.txt");
const HELP:&str="tutor [--lesson CARD] [--session FILE] [--workspace DIR | --ephemeral] [--index] [--astra]
tutor --tui opens or reconnects the persistent desktop terminal.
--workspace opens a private learner archive and defaults to today's journal.
--pilot-notes JOURNAL records /pilot, /predict and /outcome lines from stdin without starting a lesson.\n--inspect-journal JOURNAL emits readable object fields for committed records.\n--replay JOURNAL verifies the decisions offline. --shadow silently records candidate observations.\n--diagnostics WORKSPACE... compares authorized learner records. --share-session JOURNAL NEW_FILE exports a whole visible session. --supervisor-worker WORKSPACE resumes a queued review.\nWithin a workspace, --session takes a journal filename. --index embeds all pending turns and exits.";

fn recent_messages(history: &[(Option<String>, String)]) -> Result<Vec<Vec<Value>>, String> {
    let mut turns = Vec::new();
    let mut bytes = 0;
    // Reserve space for the selected source, turn schema and the learner's new message.
    for (learner, tutor) in history.iter().rev().take(14) {
        let mut pair = Vec::new();
        if let Some(text) = learner {
            pair.push(json!({"role":"user","content":text}));
        }
        pair.push(json!({"role":"assistant","content":tutor}));
        bytes += serde_json::to_vec(&pair).map_err(|e| e.to_string())?.len();
        if bytes > 64000 {
            break;
        }
        turns.push(pair);
    }
    Ok(turns)
}

fn request(
    history: &[(Option<String>, String)],
    packet: &Value,
    schema: &Value,
) -> Result<Value, String> {
    let turns = recent_messages(history)?;
    let instruction = match packet["task"].as_str() {
        Some("discovery") => include_str!("discovery.txt"),
        Some("course_plan") => include_str!("course.txt"),
        Some("observe") => include_str!("observe.txt"),
        Some("review") => include_str!("review.txt"),
        Some("check_meaning") => include_str!("check_meaning.txt"),
        Some("edit_turn") => include_str!("edit_turn.txt"),
        Some("route") => include_str!("route.txt"),

        _ => TEACHER,
    };
    let mut messages = vec![json!({"role":"system","content":instruction})];
    if matches!(
        packet["task"].as_str(),
        Some("deliver" | "discovery" | "edit_turn")
    ) {
        messages.push(json!({"role":"system","content":include_str!("../../delivery-format.txt")}));
    }
    if packet["assigned_activity"].is_object() && packet["task"] != "discovery" {
        messages.push(json!({"role":"system","content":"Teach toward the assigned activity objective and submission criterion throughout this lesson. Adapt examples and intermediate questions. Teacher review determines assignment status."}));
    }
    let mut task = packet.clone();
    if matches!(packet["task"].as_str(), Some("deliver" | "discovery")) {
        task["optional_reviewed_activities"] = practice::menu();
        task["activity_selection"]=json!("When a listed activity fits the current learning objective, learner evidence, and assignment, you may use its exact prompt as the exercise. Otherwise author the best exercise for this learner.");
    }
    if task["task"] == "deliver"
        && task["working_plan"].is_object()
        && task["route"]["action"] == "complete"
    {
        // The lesson keeps a fallback target for rejected drafts. The tutor chooses a new
        // objective here, so presenting that fallback as an assignment would anchor its choice.
        task.as_object_mut().unwrap().remove("target");
        task["working_plan"]["just_demonstrated"] = task["previous_objective"].clone();
        task["instruction"] = json!("Look at the learner's answer and choose the next skill within their goal. Skip any operation in the provisional plan they have already carried out. Put the single new operation required by your exercise in objective, using a short, specific title. Teach it now with a concrete example and one fresh exercise. Rust supplies the acknowledgment.");
    }
    if task["task"] == "review" {
        // The tutor saves its provisional plan independently. Only the current
        // task and the feedback the learner will see go to the content reviewer.
        task.as_object_mut().unwrap().remove("working_plan");
        if let Some(draft) = task["draft"].as_object_mut() {
            draft.remove("next_objective");
        }
    }
    let earlier = &history[..history.len() - turns.len()];
    let mut remaining = 16000;
    let mut earlier_work = Vec::new();
    for (index, (learner, _)) in earlier.iter().enumerate().rev() {
        if let Some(words) = learner.as_ref().filter(|s| !s.starts_with('/')) {
            if words.len() > remaining {
                continue;
            }
            remaining -= words.len();
            earlier_work.push(json!({"turn":index+1,"learner":words}));
        }
    }
    earlier_work.reverse();
    if !earlier_work.is_empty() {
        task["earlier_learner_work"] = json!(earlier_work);
    }
    task["earlier_exchanges_outside_window"] = json!(history.len() - turns.len());
    messages.extend(turns.into_iter().rev().flatten());
    messages.push(json!({"role":"user","content":task.to_string()}));
    Ok(
        json!({"messages":messages,"max_tokens":2400,"provider":{"require_parameters":true},
       "response_format":{"type":"json_schema","json_schema":{"name":"lesson_turn","strict":true,"schema":schema}}}),
    )
}

fn append(journal: &mut Option<Journal>, visit: &mut Visit, event: Event) -> Result<(), String> {
    let at = session_time()?;
    visit.commit(event, at, |e| match journal {
        Some(journal) => journal.save_at(e, at),
        None => Ok(()),
    })
}
fn session_time() -> Result<u64, String> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs())
}
fn connection(config: &mut Option<Config>) -> Result<&Config, String> {
    if config.is_none() {
        *config = Some(credentials::connect()?);
    }
    Ok(config.as_ref().unwrap())
}
fn display(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}
fn visit_notice(message: &str) {
    if env::var_os("ROBOTUTOR_DESKTOP").is_some() {
        println!("\x1eNOTICE {}", json!(message));
    } else {
        println!("{}", display(message));
    }
}
fn optional_context<T, E: std::fmt::Display>(name: &str, result: Result<T, E>) -> Option<T> {
    result
        .map_err(|e| eprintln!("{name} unavailable: {}", display(&e.to_string())))
        .ok()
}

fn respond(
    config: &mut Option<Config>,
    journal: &mut Option<Journal>,
    visit: &mut Visit,
    workspace: &mut Option<memory::Workspace>,
    session: &str,
    shadow: bool,
) -> Result<(), String> {
    if visit.pending.as_deref() == Some("/discover") {
        let mut profile = discovery::profile_for(journal.as_ref())?;
        if let Some(d) = &visit.lesson.discovery {
            profile.merge(&d.profile);
        }
        let subject = visit.lesson.declared_goal.clone().unwrap_or_default();
        return discovery::begin(
            visit,
            journal,
            discovery::Discovery::new(subject, profile),
            true,
        );
    }
    if let Some(id) = visit
        .pending
        .as_deref()
        .and_then(|s| s.strip_prefix("/lesson "))
        .map(str::to_owned)
    {
        if discovery::active(visit) && journal.as_ref().is_none_or(|j| j.assignment.is_none()) {
            let profile = visit.lesson.discovery.as_ref().unwrap().profile.clone();
            return discovery::offer_card(visit, journal, &id, profile);
        }
    }
    if let Some(subject) = visit
        .pending
        .as_deref()
        .and_then(lesson::switch_destination)
        .filter(|_| journal.as_ref().is_none_or(|j| j.assignment.is_none()))
    {
        let key = subject.trim().to_lowercase();
        let mut saved = visit.subjects.get(&key).cloned();
        if saved.is_none() && visit.from.is_some() {
            if let Some(j) = journal.as_ref().filter(|j| j.workspace_mode) {
                // An explicit return may refer to work suspended before this visit began.
                let mut latest = 0;
                for path in replay::journals(j.path.parent().ok_or("Missing workspace")?)? {
                    if path == j.path || path.with_extension("assignment.json").exists() {
                        continue;
                    }
                    let Some(prior) = optional_context("earlier subject", session::replay(&path))
                    else {
                        continue;
                    };
                    if prior.last_delivery_at >= latest {
                        if let Some((lesson, draft, _)) = prior.subjects.get(&key) {
                            latest = prior.last_delivery_at;
                            saved = Some((lesson.clone(), draft.clone(), 0));
                        }
                    }
                }
            }
        }
        if let Some((saved, draft, turn)) = saved {
            let mut lesson = saved;
            lesson.resumed_from_turn = (turn > 0).then_some(turn);
            lesson.return_draft = Some(draft);
            if let (Some(d), Some(latest)) = (&mut lesson.discovery, &visit.lesson.discovery) {
                d.merge(&latest.profile);
            }
            let text = format!(
                "Let’s return to {}.\n\n{}",
                subject.trim(),
                lesson.current_question()
            );
            return discovery::deliver(visit, journal, lesson, text);
        }
        if visit.lesson.discovery.is_some() || !visit.lesson.objective.is_empty() {
            let mut state =
                visit.lesson.discovery.clone().unwrap_or_else(|| {
                    discovery::Discovery::new(String::new(), Default::default())
                });
            state.change_subject(subject);
            let mut lesson = lesson::Lesson::default();
            lesson.declared_goal = Some(subject.into());
            lesson.topic = subject.into();
            let text = format!("We’ll explore {subject}. {}", state.question);
            lesson.discovery = Some(state);
            return discovery::deliver(visit, journal, lesson, text);
        }
    }
    if !shadow {
        if let Some((target, words)) = visit
            .pending
            .as_deref()
            .and_then(|w| w.strip_prefix("/amend "))
            .and_then(|w| w.split_once(char::is_whitespace))
            .filter(|(id, _)| {
                id.rsplit_once('#')
                    .is_some_and(|(_, ordinal)| ordinal.contains('/'))
            })
        {
            let external = workspace
                .as_mut()
                .map(|w| {
                    w.sync()?;
                    w.external_findings(session)
                })
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            if judgment::known(visit, &external)
                .iter()
                .any(|f| f.id == target)
                && !words.trim().is_empty()
            {
                let source = format!("{session}#{}", visit.history.len() + 1);
                let proposals = json!({"findings":[{"kind":"correct","speaker":"learner","task":judgment::task(&visit.lesson),
                    "source":source,"quote":words.trim(),"detail":words.trim().chars().take(200).collect::<String>(),"target":target}]});
                let inputs = judgment::Inputs {
                    version: judgment::VERSION.into(),
                    source,
                    shadow: false,
                    proposals,
                    external_findings: external,
                    external_cues: vec![],
                    probe: None,
                };
                let (decision, lesson, text) = judgment::teach(
                    visit,
                    &inputs,
                    &mut |_, _| Err("Unexpected drafting request".into()),
                    &mut |_, _, action| assessment::Verdict::provisional(action),
                )?;
                let receipt = judgment::Receipt {
                    inputs,
                    decision,
                    calls: vec![],
                    assessment: None,
                };
                append(
                    journal,
                    visit,
                    Event::Delivered {
                        text: text.clone(),
                        lesson: Box::new(lesson),
                        sources: vec![],
                        judgment: Some(Box::new(receipt)),
                    },
                )?;
                println!("\nLuna › {}", display(&text));
            } else {
                discovery::deliver(visit,journal,visit.lesson.clone(),"That reading has already changed or is unavailable. Open the plan to select a current source.".into())?;
            }
            return Ok(());
        }
    }
    if discovery::active(visit)
        || (visit.lesson.discovery.is_some()
            && visit
                .pending
                .as_deref()
                .is_some_and(|s| s.starts_with("/amend ")))
    {
        return discovery::respond(config, journal, visit, session);
    }
    let interactive = io::stdin().is_terminal() || env::var_os("ROBOTUTOR_DESKTOP").is_some();
    if interactive {
        eprintln!("Luna · Reading your turn…");
    }
    let connection = match connection(config) {
        Ok(connection) => connection,
        Err(error) => {
            append(
                journal,
                visit,
                Event::Failed {
                    message: error.clone(),
                },
            )?;
            eprintln!(
                "{} The turn is pending. Use /retry when the connection is available.",
                display(&error)
            );
            return Ok(());
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(150);
    let has_workspace = workspace.is_some() || visit.from.is_some();
    let mut workspace = workspace.as_mut().and_then(|w| match w.sync() {
        Ok(_) => Some(w),
        Err(error) => {
            eprintln!(
                "Workspace index unavailable. Using this visit’s history. {}",
                display(&error.to_string())
            );
            None
        }
    });
    let skills = optional_context(
        "skill history",
        workspace.as_ref().map(|w| w.skills()).transpose(),
    )
    .flatten()
    .unwrap_or_default();
    let command = visit.pending.as_deref().unwrap_or("");
    let current_topic = study::Target::from_lesson(&visit.lesson);
    let relevant = |s: &&study::Skill| {
        s.target.label() == current_topic.label() || current_topic.label().is_empty()
    };
    let application = visit
        .lesson
        .course
        .as_ref()
        .and_then(|c| c.milestones.get(c.focus.saturating_add(1)))
        .map(|m| m.application.as_str())
        .or(Some(visit.lesson.next_objective.as_str()))
        .filter(|s| !s.trim().is_empty());
    let review = if matches!(command, "/next" | "/harder")
        && application.is_some()
        && visit.lesson.selected.is_none()
    {
        skills.iter().filter(relevant).find(|s| {
            s.due
                && s.target.card.is_none()
                && s.latest_assessment
                    .as_ref()
                    .is_some_and(study::Observation::demonstrated)
        })
    } else if command == "/continue"
        && !visit.lesson.meaning_due()
        && visit.lesson.asking_meaning().is_none()
        && !visit.lesson.label_check_due()
    {
        skills.iter().filter(relevant).find(|s| s.due)
    } else if command == "/review" {
        skills
            .iter()
            .filter(relevant)
            .next()
            .or_else(|| skills.first())
    } else if let Some(n) = command.strip_prefix("/review ") {
        Some(&skills[terminal::number(n, skills.len())?])
    } else {
        None
    };
    let query = match visit.pending.as_deref() {
        None | Some("/next" | "/harder") => visit
            .lesson
            .selected
            .as_deref()
            .filter(|_| visit.lesson.objective.is_empty())
            .unwrap_or(&visit.lesson.objective),
        Some(s) => s,
    };
    let recent_from = visit.history.len() - recent_messages(&visit.history)?.len() + 1;
    let recalled = workspace
        .as_mut()
        .map(|w| {
            metrics::stage("retrieval", || {
                w.recall(connection, query, session, recent_from)
            })
        })
        .transpose();
    let recalled = match recalled {
        Ok(hits) => hits.unwrap_or_default(),
        Err(error) => {
            eprintln!("Workspace recall unavailable: {error}");
            vec![]
        }
    };
    let mut recalled = recalled;
    if let (Some(w), Some(from)) = (workspace.as_ref(), visit.from.as_ref()) {
        for entry in optional_context("earlier visit", w.continuity(from)).unwrap_or_default() {
            if !recalled.iter().any(|h| h.source == entry.source) {
                recalled.push(entry);
            }
        }
    }
    let sources: Vec<_> = recalled
        .iter()
        .map(|h| session::SourceRef {
            source: h.source.clone(),
            digest: h.digest.clone(),
        })
        .collect();
    let lesson = &visit.lesson;
    let context:Vec<_>=skills.iter().filter(|s|s.target.label()==lesson.selected.as_deref().unwrap_or(&lesson.topic) || s.due).take(8)
        .map(|s|json!({"objective":s.target.objective,"state":s.status(),"completed_exercise":s.latest_assessment.as_ref().filter(|o| o.demonstrated()).map(|o|json!({"objective":o.target.objective,"with_support":o.assisted})),"review_due":s.due,"last_action":s.last_action,"source":s.last_source})).collect();
    let source = format!("{}#{}", session, visit.history.len() + 1);
    let external_findings = optional_context(
        "earlier findings",
        workspace
            .as_ref()
            .map(|w| w.external_findings(session))
            .transpose(),
    )
    .flatten()
    .unwrap_or_default();
    let known = judgment::known(visit, &external_findings);
    let guidance = optional_context(
        "supervisor guidance",
        workspace
            .as_ref()
            .map(|w| w.supervisor_guidance())
            .transpose(),
    )
    .flatten();
    let assigned = journal.as_ref().and_then(|j| j.assignment.clone());
    let work = recovery::TurnWork::new(std::mem::take(&mut visit.checkpoints), journal.as_mut());
    let infer = |packet: &Value, schema: &Value| -> Result<Value, String> {
        if interactive {
            match packet["task"].as_str() {
                Some("course_plan") => eprintln!("Luna · Planning the course across visits…"),
                Some("deliver") => eprintln!("Luna · Preparing your lesson…"),
                Some("review") => eprintln!("Luna · Checking the lesson…"),
                Some("edit_turn") => eprintln!("Luna · Revising the lesson from the review…"),
                Some("route") => eprintln!("Luna · Choosing a teaching step…"),
                _ => {}
            }
        }
        let mut packet = packet.clone();
        if let Some(assignment) = &assigned {
            packet["assigned_activity"] = assignment.clone();
        }
        if let Some(g) = guidance
            .as_ref()
            .filter(|_| packet["task"] != "check_meaning")
        {
            packet["teacher_guidance"] = g.clone();
        }
        if matches!(packet["task"].as_str(), Some("course_plan" | "deliver")) {
            let reasoning: Vec<_> = known
                .iter()
                .filter(|f| f.proposal.kind == judgment::Kind::Strength)
                .rev()
                .take(8)
                .collect();
            if !reasoning.is_empty() {
                packet["prior_strengths_to_consider_if_relevant"] = json!(reasoning);
            }
        }
        let history = visit.teaching_history();
        // The editor has the active case, learner's exact question and the two
        // reviews. Limit stale examples that led the original draft off topic.
        if preparation::consolidate()
            && matches!(
                packet["task"].as_str(),
                Some("observe" | "deliver" | "course_plan")
            )
        {
            packet["consolidated_evidence"] = preparation::consolidated(visit);
        }
        if assigned.is_none() && matches!(packet["task"].as_str(), Some("course_plan" | "deliver"))
        {
            if let Some(d) = &lesson.discovery {
                packet["discovery_plan"] = d.teaching_context();
            }
        }
        let history = if packet["task"] == "deliver"
            && packet["teaching_decision"]["policy"] == judgment::VERSION
        {
            &history[history.len().saturating_sub(2)..]
        } else if packet["task"] == "observe" && preparation::consolidate() {
            &history[history.len().saturating_sub(2)..]
        } else if packet["task"] == "check_meaning" {
            &history[..0]
        } else if packet["task"] == "edit_turn" {
            &history[history.len().saturating_sub(2)..]
        } else {
            history
        };
        let request = request(history, &packet, schema)?;
        let stage = packet["task"].as_str().unwrap_or("teaching");
        work.get_or_run(stage, connection.model(Role::Luna), &request, || {
            let mut attempt = 0;
            recovery::model_output(
                || {
                    attempt += 1;
                    work.model_attempt(stage, connection.model(Role::Luna), attempt)?;
                    let receipt = recovery::while_waiting(stage, interactive, || {
                        metrics::provider(
                            match stage {
                                "observe" | "check_meaning" => "verification",
                                "route" => "routing",
                                "deliver" => "drafting",
                                "edit_turn" => "repair",
                                other => other,
                            },
                            || connection.complete_by(Role::Luna, &request, Some(deadline)),
                        )
                    })?;
                    serde_json::from_str(assistant_text(&receipt)?)
                        .map_err(|_| "Could not read Luna's structured reply.".into())
                },
                || {
                    if interactive {
                        eprintln!("Luna · Retrying the {stage} step once…");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(400));
                },
            )
            .map_err(|e| format!("{stage}: {e}"))
        })
    };
    let mut calls: Vec<judgment::Call> = Vec::new();
    let mut assessment = None;
    let mut assessment_error = None;
    let result = (|| {
        let mut proposals = match judgment::observe(visit, &source, &known, &mut |p, s| infer(p, s))
        {
            Ok(value) => value,
            Err(error) if !error.starts_with("Cannot save the session") => {
                eprintln!("Observation unavailable. Continuing with existing evidence.");
                json!({"findings": [], "observer_unavailable":error})
            }
            Err(error) => return Err(error),
        };
        if has_workspace && workspace.is_none() {
            proposals = json!({"findings":[],"progression":proposals.get("progression"),"observer_unavailable":"Workspace history unavailable."});
        }
        let mut external_cues = Vec::new();
        if let Some(w) = workspace.as_ref() {
            if method::name_request(command) {
                if let Some(anchor) = &visit.from {
                    if let Some(cue) =
                        optional_context("method history", w.method_cue_before(anchor)).flatten()
                    {
                        external_cues.push(cue);
                    }
                }
            }
            let mut cue_check_failed = false;
            for method in proposals["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .take(4)
                .filter(|p| matches!(p["kind"].as_str(), Some("method" | "prompted_method")))
                .filter_map(|p| p["detail"].as_str())
                .filter(|s| !s.is_empty() && s.len() <= 800)
            {
                match w.earlier_cue(session, method) {
                    Ok(Some(cue)) => external_cues.push(cue),
                    Ok(None) => {}
                    Err(_) => cue_check_failed = true,
                }
            }
            if cue_check_failed {
                proposals = json!({"findings":[],"progression":proposals.get("progression"),"observer_unavailable":"Could not verify prior cues."});
            }
        }
        let inputs = judgment::Inputs {
            version: judgment::VERSION.into(),
            source,
            shadow,
            external_findings,
            external_cues,
            proposals,
            probe: review.map(|s| judgment::Probe {
                source: Some(s.last_source.clone()),
                application: application
                    .filter(|_| {
                        s.target.card.is_none() && s.target.label() == current_topic.label()
                    })
                    .map(str::to_owned),
                target: s.target.clone(),
                previous: s.last_task.clone(),
                tasks: s.prior_tasks(),
            }),
        };
        let (decision, lesson, text) = judgment::teach(
            visit,
            &inputs,
            &mut |packet, schema| {
                if let Some(issue) = packet["repair"].as_str() {
                    if let Some(last) = calls.last() {
                        work.record(&Event::Rejected {
                            draft: last.output.clone(),
                            reason: issue.into(),
                        })?;
                    }
                }
                let mut enriched = packet.clone();
                if workspace.is_some() && packet["task"] != "check_meaning" {
                    enriched["skill_record"] = json!(context);
                    enriched["workspace_recall"] = json!(recalled);
                }
                let result = match infer(&enriched, schema) {
                    Err(error)
                        if packet["task"] == "check_meaning"
                            && !error.starts_with("Cannot save the session") =>
                    {
                        json!({"result":"unverified","quote":""})
                    }
                    other => other?,
                };
                if packet["task"] == "review" {
                    work.record(&Event::Review {
                        draft: packet["draft"].clone(),
                        verdict: result.clone(),
                    })?;
                }
                calls.push(judgment::Call::new(packet, schema, result.clone()));
                Ok(result)
            },
            &mut |l, a, action| {
                let checked = work
                    .get_or_run(
                        "assessment",
                        connection.model(Role::Jev),
                        &json!(["independent-practice-check/2", l, a, action]),
                        || {
                            Ok(json!(recovery::while_waiting(
                                "assessment",
                                interactive,
                                || assessment::verify(l, a, action, connection, Some(deadline))
                            )))
                        },
                    )
                    .and_then(|value| {
                        serde_json::from_value(value)
                            .map_err(|e| format!("Cannot read the assessment checkpoint: {e}"))
                    });
                let verdict = checked.unwrap_or_else(|error| {
                    assessment_error = Some(error);
                    assessment::Verdict::provisional(action)
                });
                assessment = Some(verdict.clone());
                verdict
            },
        )?;
        Ok::<_, String>((inputs, decision, lesson, text))
    })();
    visit.checkpoints = work.finish();
    let result =
        assessment_error
            .map_or(result, Err)
            .and_then(|(inputs, decision, lesson, text)| {
                let receipt = judgment::Receipt {
                    inputs,
                    decision,
                    calls: std::mem::take(&mut calls),
                    assessment,
                };
                let course_change = lesson
                    .course
                    .as_ref()
                    .filter(|course| {
                        visit.lesson.course.as_ref().is_none_or(|before| {
                            before.goal != course.goal || before.focus != course.focus
                        })
                    })
                    .and_then(|course| {
                        course.current().map(|m| {
                            format!(
                                "Course destination · {}\nFocus {} of {} · {}   /plan",
                                course.destination,
                                course.focus + 1,
                                course.milestones.len(),
                                m.skill
                            )
                        })
                    });
                let discovery_offer = lesson.discovery.is_none() && lesson.closed.len() > visit.lesson.closed.len();
                append(
                    journal,
                    visit,
                    Event::Delivered {
                        text: text.clone(),
                        lesson: Box::new(lesson),
                        sources,
                        judgment: Some(Box::new(receipt)),
                    },
                )?;
                if let Some(focus) = course_change {
                    println!("\n{}", display(&focus));
                }
                println!("\nLuna › {text}");
                if discovery_offer {
                    visit_notice("At a break, use /discover to build a broader picture together from your earlier work.");
                }
                if !shadow {
                    println!(
                        "{}",
                        judgment::trace(visit, visit.history.len(), &[], false)
                    );
                }
                Ok(())
            });
    if let Err(message) = result {
        if let (Some(j), Some(last)) = (
            journal.as_mut(),
            calls.last().filter(|call| call.task == "deliver"),
        ) {
            j.save(&Event::Rejected {
                draft: last.output.clone(),
                reason: message.clone(),
            })?;
        }
        append(
            journal,
            visit,
            Event::Failed {
                message: message.clone(),
            },
        )?;
        if message.contains("OpenRouter returned HTTP 401;")
            || message.contains("OpenRouter returned HTTP 403;")
        {
            *config = None;
        }
        eprintln!(
            "{} The turn is pending. Use /retry to resume it.",
            display(&message)
        );
    }
    Ok(())
}

fn open_review(
    q: &supervisor::Queue,
    revision: i64,
    session: &str,
    journal: &mut Option<Journal>,
    visit: &mut Visit,
) -> Result<bool, String> {
    if discovery::active(visit)
        || visit.pending.is_some()
        || !visit.draft.empty()
        || practice::current(visit).is_some()
    {
        return Ok(false);
    }
    let from = visit.from.as_ref().map(|r| r.source.clone());
    let previous_revision = visit.supervisor_revision;
    let mut journal_error = None;
    let applied = q.apply_ready(
        revision,
        session,
        visit.history.len(),
        from.as_deref(),
        visit.supervisor_revision,
        |n, r| {
            let plan = r.plan();
            append(
                journal,
                visit,
                Event::SupervisorPlan {
                    replace_prompt: true,
                    revision: n,
                    objective: r.destination,
                    task: r.next_task,
                    criterion: r.sufficient,
                },
            )
            .inspect_err(|e| journal_error = Some(e.clone()))?;
            println!("{}", display(&plan));
            Ok(())
        },
    );
    if let Some(error) = journal_error {
        return Err(error);
    }
    Ok(optional_context("supervisor handoff", applied)
        .unwrap_or(visit.supervisor_revision != previous_revision))
}
fn read_input(
    lines: &mut impl Iterator<Item = io::Result<String>>,
    visit: &mut Visit,
    journal: &mut Option<Journal>,
) -> Result<Option<String>, String> {
    loop {
        let Some(line) = lines.next().transpose().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        if env::var_os("ROBOTUTOR_DESKTOP").is_none() {
            return Ok(Some(line));
        }
        let frame: Value = serde_json::from_str(&line).map_err(|_| "Invalid desktop message")?;
        if let Some(text) = frame.as_str() {
            return Ok(Some(text.into()));
        }
        if let Some(r) = frame.get("rendered") {
            if r["turn"].as_u64() == Some(visit.history.len() as u64) {
                let kind = match r["kind"].as_str() {
                    Some("return") => "return",
                    Some("correction") => "correction",
                    Some("practice") => "practice",
                    _ => "ordinary",
                };
                if let Some(us) = r["feedback_us"].as_u64() {
                    let sample = metrics::Timing {
                        entry: None,
                        kind: format!("gtk/{kind}"),
                        delivered: true,
                        feedback_us: Some(us),
                        actionable_us: r["exercise_us"].as_u64(),
                        stages: vec![],
                    };
                    append(journal, visit, Event::Timing { sample })?;
                }
            }
            continue;
        }
        if frame["turn"].as_u64() != Some(visit.history.len() as u64) || visit.pending.is_some() {
            visit_notice("The activity changed. Reopen the current draft.");
            if frame.get("submit").is_some() {
                return Ok(Some(String::new()));
            }
            continue;
        }
        if let Some(d) = frame.get("draft").or_else(|| frame.get("submit")) {
            let fields =
                serde_json::from_value(d["fields"].clone()).map_err(|_| "Invalid draft fields")?;
            match practice::set_fields(visit, d["text"].as_str().unwrap_or("").into(), fields) {
                Ok(draft) => {
                    practice::edit(visit, journal, draft)?;
                    if frame.get("submit").is_some() {
                        return Ok(Some("/send".into()));
                    }
                }
                Err(error) => {
                    visit_notice(&error);
                    if frame.get("submit").is_some() {
                        return Ok(Some(String::new()));
                    }
                }
            }
        } else if let Some(cmd) = frame["command"].as_str() {
            return Ok(Some(cmd.into()));
        } else {
            return Err("Unknown desktop message.".into());
        }
    }
}

fn run() -> Result<(), String> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("tutor {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let argv: Vec<_> = env::args_os().skip(1).collect();
    if let [flag, path] = argv.as_slice() {
        if flag == "--init-state-dir" {
            return tutor_lab::platform::create_private_dir(path.as_ref(), true)
                .map_err(|e| e.to_string());
        }
    }
    if argv
        .first()
        .is_some_and(|arg| arg == "--inspect-journal" || arg == "--lesson-events")
    {
        if !(2..=4).contains(&argv.len()) {
            return Err("Use --inspect-journal JOURNAL [BYTE_OFFSET [RECORD_LIMIT]].".into());
        }
        let number = |i: usize| {
            argv.get(i)
                .map(|arg| {
                    arg.to_str()
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or("Expected an unsigned integer for the inspection range.")
                })
                .transpose()
        };
        let offset = number(2)?.unwrap_or(0);
        let limit = number(3)?
            .map(usize::try_from)
            .transpose()
            .map_err(|_| "Invalid record limit")?
            .unwrap_or(usize::MAX);
        return inspect::read_range(
            argv[1].as_ref(),
            offset,
            limit,
            io::stdout().lock(),
            argv[0] == "--lesson-events",
        );
    }
    if let [cmd, name, dir] = argv.as_slice() {
        if cmd == "--seed-test-learner" {
            return personas::seed(
                name.to_str().ok_or("Invalid test learner name")?,
                dir.as_ref(),
            );
        }
    }
    if let [cmd, policy, path, answer, budget] = argv.as_slice() {
        if cmd == "--policy-replay" {
            let limit = budget
                .to_str()
                .and_then(|s| s.parse().ok())
                .ok_or("Specify a total token budget.")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&ablation::run(
                    policy.to_str().ok_or("Invalid policy")?,
                    path.as_ref(),
                    answer.as_ref(),
                    limit
                )?)
                .unwrap()
            );
            return Ok(());
        }
    }
    if let [cmd, path] = argv.as_slice() {
        if cmd == "--latency" {
            println!(
                "{}",
                serde_json::to_string_pretty(&metrics::report(std::path::Path::new(path))?)
                    .unwrap()
            );
            return Ok(());
        }
    }
    if let [cmd, dirs @ ..] = argv.as_slice() {
        if cmd == "--diagnostics" && !dirs.is_empty() {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &diagnostics::report(dirs).map_err(|e| e.to_string())?
                )
                .unwrap()
            );
            return Ok(());
        }
    }
    if let [cmd, source, dest] = argv.as_slice() {
        if cmd == "--share-session" {
            return diagnostics::share(source.as_ref(), dest.as_ref()).map_err(|e| e.to_string());
        }
    }
    if let [cmd, path] = argv.as_slice() {
        let path = std::path::Path::new(path);
        if cmd == "--pilot-notes" {
            return pilot::record_notes(path);
        }
        if cmd == "--supervisor-worker" {
            return supervisor::worker(path).map_err(|e| e.to_string());
        }
        if cmd == "--review-preview" {
            let visit = session::replay(path)?;
            let (_, r) = supervisor::assess(
                &visit,
                &json!({"objective":visit.lesson.objective,"assessments":visit.assessments}),
                &[],
            )
            .map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            return Ok(());
        }
    }
    if argv.first().is_some_and(|a| a == "--shadow-audit") {
        if argv.len() < 3 {
            return Err(
                "Use tutor --shadow-audit NEW_PRIVATE_OUTPUT JOURNAL... in chronological order."
                    .into(),
            );
        }
        return pilot::shadow_audit(std::path::Path::new(&argv[1]), &argv[2..]);
    }
    if argv.first().is_some_and(|a| a == "--pilot-report") {
        if argv.len() != 2 {
            return Err("Use tutor --pilot-report JOURNAL_OR_WORKSPACE.".into());
        }
        return pilot::report(std::path::Path::new(&argv[1]));
    }
    if argv.first().is_some_and(|a| a == "--replay") {
        if argv.len() != 2 {
            return Err("Use tutor --replay JOURNAL_OR_WORKSPACE.".into());
        }
        return replay::run(std::path::Path::new(&argv[1]));
    }
    if env::args().skip(1).eq(["--tui"]) {
        return terminal::open_window();
    }
    let mut args = env::args().skip(1);
    let mut workspace_dir = None;
    let mut index_only = false;
    let mut astra = false;
    let mut ephemeral = false;
    let mut shadow = false;
    let mut selected = None;
    let mut path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "--lesson" => {
                let id = args.next().ok_or("Give --lesson a card name.")?;
                if !material::CARDS.iter().any(|(k, _)| *k == id) {
                    return Err(format!(
                        "Unknown card. Choose from {}.",
                        material::CARDS
                            .iter()
                            .map(|(k, _)| *k)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                selected = Some(id);
            }
            "--workspace" => {
                workspace_dir = Some(PathBuf::from(
                    args.next().ok_or("Give --workspace a private directory.")?,
                ))
            }
            "--ephemeral" => ephemeral = true,
            "--index" => index_only = true,
            "--astra" => astra = true,
            "--shadow" => shadow = true,
            "--session" => {
                path = Some(PathBuf::from(
                    args.next().ok_or("Give --session a file path.")?,
                ))
            }
            _ => return Err(HELP.into()),
        }
    }
    if ephemeral && (path.is_some() || workspace_dir.is_some()) {
        return Err("--ephemeral takes no journal or workspace.".into());
    }
    if !ephemeral && path.is_none() && workspace_dir.is_none() {
        workspace_dir = Some(terminal::default_workspace()?);
    }
    let mut workspace = match workspace_dir
        .as_deref()
        .map(memory::Workspace::open)
        .transpose()
    {
        Ok(w) => w,
        Err(error) if !index_only => {
            let dir = workspace_dir.as_deref().unwrap();
            path = Some(memory::local_journal(dir, path.as_deref()).map_err(|e| e.to_string())?);
            eprintln!(
                "Recall index unavailable. Opening the private lesson journal directly. {}",
                display(&error.to_string())
            );
            None
        }
        Err(error) => return Err(error.to_string()),
    };
    if index_only {
        let w = workspace
            .as_mut()
            .ok_or("--index needs a learner workspace.")?;
        let n = w
            .build_vectors(&credentials::connect()?)
            .map_err(|e| e.to_string())?;
        println!("Indexed {n} delivered turns.");
        return Ok(());
    }
    let ordinary_return = path.is_none() && selected.is_none();
    if let Some(w) = &workspace {
        path = Some(w.journal_path(path.as_deref()).map_err(|e| e.to_string())?);
    }
    if ordinary_return && path.as_ref().is_some_and(|p| !p.exists()) {
        if let Some(dir) = workspace_dir.as_deref() {
            if let Some(previous) = replay::journals(dir)?
                .into_iter()
                .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
            {
                if session::replay(&previous).is_ok_and(|v| {
                    v.pending.is_some()
                        || v.interrupted_input().is_some()
                        || !v.draft.empty()
                        || practice::current(&v).is_some()
                }) {
                    path = Some(previous);
                }
            }
        }
    }
    let session = path
        .as_deref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("ephemeral")
        .to_owned();
    if astra {
        let dir = workspace_dir
            .as_deref()
            .ok_or("--astra requires a private learner workspace")?;
        supervisor::enable_comparison(dir, &session).map_err(|e| e.to_string())?;
    }
    let mut visit = Visit::default();
    let mut journal = path
        .as_deref()
        .map(|p| Journal::open(p, &mut visit))
        .transpose()?;
    if let Some(j) = &mut journal {
        j.workspace_mode = workspace_dir.is_some();
    }
    let fresh = match &journal {
        Some(j) => j.is_empty()?,
        None => true,
    };
    if !fresh && selected.is_some() {
        return Err("Resume this visit with --session alone.".into());
    }
    if fresh {
        append(
            &mut journal,
            &mut visit,
            Event::Open {
                version: 1,
                selected,
            },
        )?;
    }
    if fresh && visit.lesson.selected.is_none() {
        if let Some(w) = &mut workspace {
            if let Some((from, lesson)) = optional_context(
                "previous visit",
                w.sync().and_then(|_| w.continuation(&session)),
            )
            .flatten()
            {
                append(
                    &mut journal,
                    &mut visit,
                    Event::Continued {
                        from,
                        lesson: Box::new(lesson),
                    },
                )?;
            }
        }
    }
    if visit.lesson.declared_goal.is_none() {
        if let (Some(w), Some(from)) = (workspace.as_mut(), visit.from.as_ref()) {
            if let Some(goal) =
                optional_context("prior goal", w.sync().and_then(|_| w.goal_at(from))).flatten()
            {
                append(&mut journal, &mut visit, Event::RestoredGoal { goal })?;
            }
        }
    }
    println!("Robotutor · GPT-6 Luna via OpenRouter");
    let prior_profile = || discovery::related_profile(workspace_dir.as_deref(), path.as_deref());
    if (fresh || (visit.history.is_empty() && visit.pending.is_none()))
        && visit.from.is_none()
        && visit.lesson.discovery.is_none()
        && visit.lesson.exercise.is_empty()
    {
        let profile = prior_profile()?;
        if let Some(id) = visit
            .lesson
            .selected
            .clone()
            .filter(|_| journal.as_ref().is_none_or(|j| j.assignment.is_none()))
        {
            discovery::offer_card(&mut visit, &mut journal, &id, profile)?;
        } else {
            discovery::begin(
                &mut visit,
                &mut journal,
                discovery::Discovery::new(String::new(), profile),
                false,
            )?;
        }
    }
    // Resume untouched openings from older journals without changing the recorded exchange.
    if visit.pending.is_none()
        && visit.interrupted_input().is_none()
        && visit.lesson.discovery.as_ref().is_some_and(|d| {
            d.phase == discovery::Phase::Learner && d.question == discovery::OLD_OPENING
        })
    {
        let mut lesson = visit.lesson.clone();
        let d = lesson.discovery.as_mut().unwrap();
        d.question = discovery::OPENING.into();
        d.reason = discovery::OPENING_REASON.into();
        lesson.return_draft = Some(visit.draft.clone());
        discovery::deliver(&mut visit, &mut journal, lesson, discovery::OPENING.into())?;
    }
    if let Some(d) = visit.lesson.discovery.as_mut().filter(|d| d.active()) {
        d.merge(&prior_profile()?);
    }

    let mut queue = optional_context(
        "supervision",
        workspace_dir
            .as_deref()
            .map(supervisor::Queue::open)
            .transpose(),
    )
    .flatten();
    let mut revision = 0;
    let mut opened_plan = false;
    if let Some(q) = &queue {
        if let Some(n) = optional_context("supervisor activity", q.activity(&session)) {
            revision = n;
            if env::var_os("ROBOTUTOR_REOPENED").is_none() {
                optional_context("visit count", q.opened_visit(&session));
            }
            opened_plan = open_review(q, revision, &session, &mut journal, &mut visit)?;
        } else {
            queue = None;
        }
    }
    if let Some(p) = &path {
        println!("Recording this visit in {}.", p.display());
    } else {
        println!("This visit stays in memory locally.");
    }
    println!("Type /help for the lab, /plan for the lesson, or /quit to finish.");

    let mut config = None;
    let reopened = env::var_os("ROBOTUTOR_REOPENED").is_some();
    let active_plan = opened_plan || visit.last_plan_at.is_some();
    let open_next_topic = visit.draft.empty()
        && practice::current(&visit).is_none()
        && !discovery::active(&visit)
        && !active_plan
        && visit.pending.is_none()
        && (visit.lesson.awaiting_topic_opening()
            || (!shadow
                && (visit.lesson.meaning_due()
                    || recall::legacy_due(&visit)
                    || visit.lesson.label_check_due())
                && visit.lesson.asking_meaning().is_none()));
    if let Some((_, last)) = visit
        .history
        .last()
        .filter(|_| !discovery::active(&visit) && !active_plan && !reopened && !open_next_topic)
    {
        println!(
            "Resumed {} delivered exchanges. Last reply from Luna:\n\n{}",
            visit.history.len(),
            display(last)
        );
    }
    if !visit.history.is_empty() && !(fresh && discovery::active(&visit)) {
        let later = optional_context(
            "workspace trace",
            workspace
                .as_mut()
                .map(|w| w.sync().and_then(|_| w.external_findings(&session)))
                .transpose(),
        )
        .flatten()
        .unwrap_or_default();
        println!(
            "{}",
            judgment::trace(&visit, visit.history.len(), &later, false)
        );
    }
    if visit.pending.is_some() {
        println!("The last message is awaiting a reply. {} completed steps are available. Type /retry to resume.",visit.checkpoints.len());
    } else if discovery::active(&visit) {
        if !fresh || visit.from.is_some() {
            let d = visit.lesson.discovery.as_ref().unwrap();
            if env::var_os("ROBOTUTOR_DESKTOP").is_none() {
                visit_notice(&format!(
                    "Welcome back. {}\nYou can continue here or change direction.",
                    d.return_prompt()
                ));
            } else if let Some(context) = d.return_context() {
                visit_notice(&context);
            }
        }
    } else if open_next_topic {
        respond(
            &mut config,
            &mut journal,
            &mut visit,
            &mut workspace,
            &session,
            shadow,
        )?;
    } else if let Some(item) = visit.lesson.asking_meaning() {
        println!("{}", item.prompt());
    } else if visit.lesson.asking_name() {
        println!("{}", method::CHECK);
    } else if let Some((_, last)) = visit.history.last() {
        let step = visit.lesson.active_step().trim();
        if !step.is_empty() && (active_plan || reopened || !last.trim_end().ends_with(step)) {
            println!("\n{}", display(step));
        } else if reopened && step.is_empty() {
            println!("\n{}", display(last));
        }
    } else if !visit.lesson.objective.is_empty() {
        if !reopened {
            if let Some(w) = &workspace {
                println!(
                    "{}",
                    terminal::plan(
                        &visit,
                        &optional_context("skill history", w.skills()).unwrap_or_default(),
                        &optional_context("earlier findings", w.external_findings(&session))
                            .unwrap_or_default()
                    )
                );
            }
        }
        let step = visit.lesson.active_step().trim();
        if step.is_empty() {
            println!("Type /continue to pick up the current objective.");
        } else {
            println!("\n{}", display(step));
        }
    } else if visit.lesson.selected.is_some() {
        respond(
            &mut config,
            &mut journal,
            &mut visit,
            &mut workspace,
            &session,
            shadow,
        )?;
    } else {
        println!("What would you like to learn or work on?");
    }
    if !fresh
        && visit.lesson.discovery.is_none()
        && visit.pending.is_none()
        && visit.lesson.active_step().is_empty()
    {
        visit_notice("At this break you can use /discover to build a broader picture from the work you’ve already done.");
    }
    let stdin = io::stdin();
    let mut installation =
        terminal::Installed::current(workspace_dir.as_deref(), path.as_deref(), shadow)?;
    let watch_update = journal.is_some() && stdin.is_terminal();
    let mut lines = stdin.lock().lines();
    let mut first_submission = true;
    let mut offline_window = None;
    let mut prepared = practice::Prepared::default();
    if let Some(q) = &queue {
        if let Some(block) = optional_context("return activities", q.return_block(&session, &visit))
        {
            prepared.resume(&visit, block)?;
        }
    }
    let mut background = preparation::Background::default();
    if !visit.draft.empty() && env::var_os("ROBOTUTOR_DESKTOP").is_none() {
        println!(
            "Resumed your unfinished work. Use /work or /draft to inspect it, /send to submit."
        );
    }
    let mut displayed_turns = 0;
    loop {
        prepared.replenish(&visit)?;
        if !shadow {
            optional_context(
                "activity preparation",
                background.schedule(&visit, journal.as_ref(), queue.as_ref()),
            );
        }
        if let (Some(q), Some(dir)) = (&queue, &workspace_dir) {
            if preparation::consolidate() && !discovery::active(&visit) && !visit.history.is_empty()
            {
                let scheduled = q
                    .schedule(
                        revision,
                        session_time()?
                            + offline_window.unwrap_or_else(|| {
                                supervisor::idle_window(visit.lesson.active_step())
                            }),
                    )
                    .and_then(|_| supervisor::spawn(dir));
                optional_context("idle supervision", scheduled);
            }
        }
        if env::var_os("ROBOTUTOR_DESKTOP").is_some() {
            if displayed_turns > visit.history.len() {
                displayed_turns = 0;
            }
            let mut visible_start = displayed_turns;
            if displayed_turns == 0
                && matches!(visit.history.as_slice(),
                [(None, previous), (None, replacement), ..]
                    if previous == discovery::OLD_OPENING && replacement == discovery::OPENING)
            {
                visible_start = 1;
            }
            println!(
                "\x1eREADY {}",
                json!({
                    "working_plan":course::working_plan(&visit.lesson),
                    "course_seed": course::agreed_seed(&visit.lesson),
                    "exercise": visit.lesson.current_question(),
                    "history_start": displayed_turns,
                    "history_delta": &visit.history[visible_start..],
                    "pending_text": visit.pending,
                    "lesson_title": visit.lesson.objective,
                    "choices":visit.lesson.discovery.as_ref().map(|d|d.quick_choices()).unwrap_or_default(),
                    "discovery_reason":visit.lesson.discovery.as_ref().filter(|d|d.active()).map(|d|&d.reason),
                    "workspace":practice::snapshot(&visit),
                    "pending": visit.pending.is_some() || visit.interrupted_input().is_some()
                })
            );
            displayed_turns = visit.history.len();
        } else {
            print!("\nYou › ");
        }
        io::stdout().flush().map_err(|e| e.to_string())?;
        let input = if watch_update {
            composer::read(&mut visit, &mut journal, &mut installation)?
        } else {
            read_input(&mut lines, &mut visit, &mut journal)?
        };
        let Some(text) = input else { break };
        metrics::begin();
        if let Some(exact) = text.strip_prefix("/confirm-preview ") {
            if env::var_os("ROBOTUTOR_DESKTOP").is_some() {
                if let Err(e) =
                    assignment::handle("/confirm-preview", exact, &mut visit, &mut journal)
                {
                    visit_notice(&e);
                }
            } else {
                visit_notice("Use /preview followed by /submit to share your work.");
            }
            continue;
        }
        let line = text.trim();
        let command_line = line.trim_start_matches('\x1b');
        let text = if command_line.starts_with('/') {
            command_line
        } else {
            line
        };
        let (command, arg) = text
            .split_once(char::is_whitespace)
            .map(|(c, a)| (c, a.trim()))
            .unwrap_or((text, ""));
        match assignment::handle(command, arg, &mut visit, &mut journal) {
            Ok(true) => continue,
            Err(e) => {
                visit_notice(&e);
                continue;
            }
            Ok(false) => {}
        }
        if visit.pending.is_none() {
            match practice::command(command, arg, &mut visit, &mut journal, &mut prepared) {
                Ok(true) => continue,
                Err(e) => {
                    visit_notice(&e);
                    continue;
                }
                Ok(false) => {}
            }
        } else if matches!(
            command,
            "/activity" | "/draft" | "/field" | "/send" | "/hint" | "/run" | "/tap"
        ) {
            visit_notice("Resume the saved message with /retry first.");
            continue;
        }
        let submission = if command == "/send" {
            match practice::submission(&visit) {
                Ok(work) => work,
                Err(e) => {
                    visit_notice(&e);
                    continue;
                }
            }
        } else {
            None
        };
        let saved_text = visit.draft.text.clone();
        let text = if command == "/send" && submission.is_none() {
            if saved_text.trim_start().starts_with('/') {
                visit_notice("Enter the saved command directly at the prompt.");
                continue;
            }
            saved_text.trim()
        } else {
            text
        };
        if text.is_empty() {
            continue;
        }
        if let Some(q) = &queue {
            match optional_context("supervisor activity", q.activity(&session)) {
                Some(n) => revision = n,
                None => queue = None,
            }
        }
        if matches!(command, "/discover" | "/goal") && visit.pending.is_some() {
            println!("Resume the saved message with /retry first.");
            continue;
        }
        if matches!(command, "/goal" | "/lesson")
            && journal.as_ref().is_some_and(|j| j.assignment.is_some())
        {
            println!("This lesson has a pinned assignment. Open a personal lesson to change the subject.");
            continue;
        }
        if discovery::active(&visit) && matches!(command, "/continue" | "/discover") {
            let d = visit.lesson.discovery.as_ref().unwrap();
            println!("{}", display(&d.question));
            continue;
        }
        if command == "/fit" {
            if arg.is_empty() {
                println!("{}", discovery::FIT);
            } else {
                let at = session_time()?;
                append(
                    &mut journal,
                    &mut visit,
                    Event::DiscoveryFeedback {
                        text: arg.into(),
                        at,
                    },
                )?;
                println!("Your feedback is saved in this private journal.");
            }
            continue;
        }
        if queue.is_none()
            && matches!(
                command,
                "/teacher"
                    | "/assign"
                    | "/correct"
                    | "/redirect"
                    | "/measure"
                    | "/experiment"
                    | "/review-result"
                    | "/offline"
            )
        {
            println!("Supervision is unavailable for this visit.");
            continue;
        }
        if let Some(q) = &queue {
            if command == "/continue"
                && !visit.lesson.meaning_due()
                && visit.lesson.asking_meaning().is_none()
                && !visit.lesson.label_check_due()
                && !visit.lesson.asking_name()
                && open_review(q, revision, &session, &mut journal, &mut visit)?
            {
                continue;
            }
            if command == "/quit"
                && preparation::consolidate()
                && !discovery::active(&visit)
                && !visit.history.is_empty()
            {
                optional_context(
                    "closing review",
                    q.schedule(revision, session_time()? + 600)
                        .and_then(|_| supervisor::spawn(workspace_dir.as_deref().unwrap())),
                );
            }
            match command {
                "/offline" => {
                    match arg.parse::<u64>() {
                        Ok(m) if (30..=10080).contains(&m) => {
                            optional_context("offline window", q.offline(&session, m));
                            offline_window = Some(m * 60);
                            println!("Offline work window · {m} minutes");
                        }
                        _ => println!("Use /offline MINUTES (30 through 10080)."),
                    }
                    continue;
                }
                "/teacher" => {
                    for row in optional_context("review history", q.history()).unwrap_or_default() {
                        println!(
                            "Review {} · {}\n{}\nReason · {}",
                            row["rev"],
                            display(row["visit"].as_str().unwrap_or("")),
                            display(row["review"]["teacher_status"].as_str().unwrap_or("")),
                            display(row["review"]["rationale"].as_str().unwrap_or(""))
                        );
                        for e in row["review"]["evidence"].as_array().into_iter().flatten() {
                            println!(
                                "  {} · {} · {} · {}",
                                display(
                                    e["source"]
                                        .as_str()
                                        .unwrap_or(row["visit"].as_str().unwrap_or(""))
                                ),
                                e["turn"],
                                display(e["skill"].as_str().unwrap_or("")),
                                display(e["quote"].as_str().unwrap_or(""))
                            );
                        }
                    }
                    for note in optional_context("teacher notes", q.notes()).unwrap_or_default() {
                        println!(
                            "Teacher {} · {}",
                            note["kind"],
                            display(note["body"].as_str().unwrap_or(""))
                        );
                    }
                    if let Some(error) =
                        optional_context("review status", q.last_failure()).flatten()
                    {
                        println!("Last review attempt · {}", display(&error));
                    }
                    continue;
                }
                "/redirect" => {
                    let fields: Vec<_> = arg.split('|').map(str::trim).collect();
                    if fields.len() != 3
                        || fields.iter().any(|x| x.is_empty())
                        || arg.len() > 4000
                        || visit.pending.is_some()
                    {
                        println!(
                            "Use /redirect OBJECTIVE | TASK | SUFFICIENT after the pending reply."
                        );
                        continue;
                    }
                    if optional_context("teacher direction", q.note(command, arg, revision))
                        .is_none()
                    {
                        continue;
                    }
                    append(
                        &mut journal,
                        &mut visit,
                        Event::SupervisorPlan {
                            replace_prompt: true,
                            revision,
                            objective: fields[0].into(),
                            task: fields[1].into(),
                            criterion: fields[2].into(),
                        },
                    )?;
                    println!("{}", display(fields[1]));
                    continue;
                }
                "/correct" | "/assign" | "/experiment" | "/measure" | "/review-result" => {
                    match q.note(command, arg, revision) {
                        Ok(id) => println!("Saved entry {id}."),
                        Err(e) => eprintln!("{}", display(&e.to_string())),
                    };
                    continue;
                }
                _ => {}
            }
        }
        match command {
            "/predict" | "/outcome" | "/pilot" => {
                match pilot::note(command, arg, &visit, &session, workspace_dir.as_deref()) {
                    Ok(note) => {
                        let msg = display(&serde_json::to_string(&note).unwrap());
                        let at = session_time()?;
                        append(&mut journal, &mut visit, Event::Pilot { at, note })?;
                        println!("Recorded {msg}");
                    }
                    Err(e) => eprintln!("{e}"),
                }
                continue;
            }
            "/trace" => {
                if arg.contains('#') {
                    let detail = if let Some(w) = workspace.as_mut() {
                        w.trace_at(arg, true)
                            .unwrap_or_else(|e| display(&e.to_string()))
                    } else if let Some(n) = arg
                        .strip_prefix(&format!("{session}#"))
                        .and_then(|s| s.parse().ok())
                    {
                        judgment::trace(&visit, n, &[], true)
                    } else {
                        "Use a source from this journal.".into()
                    };
                    println!("{detail}");
                    continue;
                }
                let n = if arg.is_empty() {
                    Some(visit.history.len())
                } else {
                    arg.parse().ok()
                };
                let later = optional_context(
                    "workspace trace",
                    workspace
                        .as_mut()
                        .map(|w| w.sync().and_then(|_| w.external_findings(&session)))
                        .transpose(),
                )
                .flatten()
                .unwrap_or_default();
                println!("{}", judgment::trace(&visit, n.unwrap_or(0), &later, true));
                continue;
            }
            "/show"
                if arg
                    .rsplit_once('#')
                    .is_some_and(|(name, _)| name == session) =>
            {
                if let Some((words, reply)) = arg
                    .rsplit_once('#')
                    .and_then(|(_, n)| n.parse::<usize>().ok())
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| visit.history.get(i))
                {
                    println!(
                        "You › {}\nLuna › {}",
                        display(words.as_deref().unwrap_or("(lesson opening)")),
                        display(reply)
                    );
                } else {
                    println!("No recorded turn with that identifier.");
                }
                continue;
            }
            "/goal" if arg.is_empty() => {
                println!("{}", "Use /goal followed by the new study goal.");
                continue;
            }
            "/help" => {
                terminal::help();
                continue;
            }
            "/lessons" => {
                terminal::lessons();
                continue;
            }
            "/lab" => {
                if let Err(e) = terminal::catalog(arg) {
                    eprintln!("{}", display(&e));
                }
                continue;
            }
            "/lesson" if !material::CARDS.iter().any(|(id, _)| *id == arg) => {
                terminal::lessons();
                continue;
            }
            "/reload" => {
                if journal.is_none() {
                    println!("A recorded visit is required to reload and resume.");
                    continue;
                }
                eprintln!(
                    "Could not reload the tutor: {}",
                    display(&installation.reload())
                );
                continue;
            }
            _ => {}
        }
        if matches!(command, "/skills" | "/plan" | "/review" | "/continue") {
            let skills = if let Some(w) = &mut workspace {
                optional_context("skill history", w.sync().and_then(|_| w.skills()))
                    .unwrap_or_default()
            } else {
                vec![]
            };
            match command {
                "/skills" => {
                    if let Err(e) = terminal::list(&skills, arg) {
                        eprintln!("{e}");
                    }
                    continue;
                }
                "/plan" => {
                    if let Some(d) = visit.lesson.discovery.as_mut().filter(|d| d.active()) {
                        d.merge(&prior_profile()?);
                    }
                    if let Some(a) = journal.as_ref().and_then(|j| j.assignment.as_ref()) {
                        println!(
                            "Assignment objective · {}\nSubmission criterion · {}",
                            display(a["objective"].as_str().unwrap_or("")),
                            display(a["criterion"].as_str().unwrap_or(""))
                        );
                    }
                    let external = optional_context(
                        "earlier findings",
                        workspace
                            .as_ref()
                            .map(|w| w.external_findings(&session))
                            .transpose(),
                    )
                    .flatten()
                    .unwrap_or_default();
                    let plan = terminal::plan(&visit, &skills, &external);
                    if env::var_os("ROBOTUTOR_DESKTOP").is_some() {
                        let mut sources = visit
                            .lesson
                            .discovery
                            .as_ref()
                            .map(|d| d.correctable_sources())
                            .unwrap_or_default();
                        sources.extend(
                            judgment::known(&visit, &external)
                                .into_iter()
                                .map(|f| (f.id, f.proposal.quote)),
                        );
                        sources.extend(
                            visit
                                .practice_history
                                .iter()
                                .filter(|e| {
                                    !visit
                                        .practice_corrections
                                        .iter()
                                        .any(|c| c.source == e.source)
                                })
                                .map(|e| (e.source.clone(), e.feedback.clone())),
                        );
                        println!(
                            "\x1ePLAN {}",
                            json!({"text":plan,"turn":visit.history.len(),"sources":sources})
                        )
                    } else {
                        println!("{plan}");
                    }
                    if let Some((_, r)) = optional_context(
                        "supervisor plan",
                        queue.as_ref().map(|q| q.latest()).transpose(),
                    )
                    .flatten()
                    .flatten()
                    {
                        println!("Last supervisor update\n{}", display(&r.plan()));
                    }
                    continue;
                }
                "/review" if skills.is_empty() => {
                    println!("Open a lesson first to choose a review target.");
                    continue;
                }
                "/review" if !arg.is_empty() && terminal::number(arg, skills.len()).is_err() => {
                    println!("Choose a number from /skills.");
                    continue;
                }
                "/continue"
                    if !visit.lesson.meaning_due()
                        && visit.lesson.asking_meaning().is_none()
                        && !visit.lesson.label_check_due()
                        && !visit.lesson.asking_name()
                        && !skills.iter().any(|s| {
                            s.due
                                && s.target.label()
                                    == study::Target::from_lesson(&visit.lesson).label()
                        })
                        && !visit.lesson.exercise.is_empty() =>
                {
                    println!("Open task · {}", display(visit.lesson.active_step()));
                    continue;
                }
                "/continue" if visit.lesson.objective.is_empty() => {
                    println!("What would you like to learn?");
                    continue;
                }
                _ => {}
            }
        }
        if workspace.is_none() && matches!(command, "/recall" | "/sources" | "/show") {
            println!("Cross-visit recall is unavailable here. /trace shows this visit’s work.");
            continue;
        }
        if let Some(w) = &mut workspace {
            if text == "/sources" {
                let Some(mut links) = optional_context(
                    "source records",
                    w.sync()
                        .and_then(|_| w.links(&format!("{}#{}", session, visit.history.len()))),
                ) else {
                    continue;
                };
                if links.is_empty() {
                    if let Some(s) = &visit.from {
                        links.push(s.source.clone());
                    }
                }
                println!(
                    "{}",
                    if links.is_empty() {
                        "No earlier turns supplied.".into()
                    } else {
                        links.join("\n")
                    }
                );
                continue;
            }
            if let Some(id) = text.strip_prefix("/show ") {
                match w.sync().and_then(|_| w.lookup(id.trim())) {
                    Ok(Some(h)) => {
                        println!(
                            "{} · {}\nYou › {}\nLuna › {}",
                            h.source,
                            h.day.as_deref().unwrap_or("undated"),
                            display(h.learner.as_deref().unwrap_or("(lesson opening)")),
                            display(&h.tutor)
                        );
                        println!(
                            "{}",
                            w.trace_at(&h.source, false).map_err(|e| e.to_string())?
                        );
                        let links = w.links(&h.source).map_err(|e| e.to_string())?;
                        if !links.is_empty() {
                            println!("Prior turns supplied · {}", links.join(", "));
                        }
                    }
                    Ok(None) => println!("No recorded turn with that identifier."),
                    Err(e) => eprintln!("Recall unavailable: {e}"),
                }
                continue;
            }
            if let Some(query) = text.strip_prefix("/recall ") {
                let recall_connection = match connection(&mut config) {
                    Ok(c) => c,
                    Err(error) => {
                        eprintln!("Recall unavailable: {}", display(&error));
                        continue;
                    }
                };
                match w.recall(recall_connection, query.trim(), &session, usize::MAX / 2) {
                    Ok(hits) if hits.is_empty() => println!("No matching recorded turns."),
                    Ok(hits) => {
                        for h in hits {
                            println!(
                                "{} · {}\nYou › {}\nLuna › {}\n",
                                h.source,
                                h.day.as_deref().unwrap_or("undated"),
                                display(
                                    h.learner
                                        .as_deref()
                                        .unwrap_or("(no learner excerpt in this match)")
                                ),
                                display(&h.tutor)
                            );
                        }
                    }
                    Err(e) => eprintln!("Recall unavailable: {e}"),
                }
                continue;
            }
        }
        match text {
            "/quit" => break,
            "" => continue,
            "/retry" if visit.interrupted_input().is_some() => {
                let text = visit.interrupted_input().unwrap().to_owned();
                append(&mut journal, &mut visit, Event::ResumedInterrupted { text })?;
            }
            "/retry"
                if visit.pending.is_none()
                    && !visit.lesson.awaiting_topic_opening()
                    && !visit.lesson.label_check_due()
                    && !visit.lesson.meaning_due()
                    && !recall::legacy_due(&visit)
                    && !(visit.history.is_empty() && visit.lesson.selected.is_some()) =>
            {
                println!("There is no turn awaiting a reply.");
                continue;
            }
            _ => {}
        }
        if visit.pending.is_some() && discovery::active(&visit) && text != "/retry" {
            println!("Your message is still pending. Use /retry to finish it first.");
            continue;
        }
        if let Some(q) = &queue {
            if optional_context("supervisor update", q.supersede(revision)).is_none() {
                queue = None;
            }
        }
        let kind = if submission.is_some() || visit.submitted_work.is_some() {
            "practice"
        } else if command == "/retry" {
            "return"
        } else if matches!(command, "/amend" | "/correct" | "/goal") {
            "correction"
        } else if discovery::active(&visit) {
            "discovery"
        } else {
            "ordinary"
        };
        background.interrupt();
        if let Some((work, answer)) = submission {
            let turn = visit.history.len();
            append(
                &mut journal,
                &mut visit,
                Event::PracticeAnswer {
                    turn,
                    work,
                    text: answer,
                },
            )?;
        } else if text != "/retry" {
            append(
                &mut journal,
                &mut visit,
                Event::Learner { text: text.into() },
            )?;
        }
        offline_window = None;
        let before = visit.history.len();
        if !practice::respond(&mut visit, &mut journal, &session, &prepared, &mut config)?
            && (shadow
                || !preparation::deliver(&mut visit, &mut journal, queue.as_ref(), &mut config)?)
        {
            respond(
                &mut config,
                &mut journal,
                &mut visit,
                &mut workspace,
                &session,
                shadow,
            )?;
        }
        metrics::stage("render", || io::stdout().flush()).map_err(|e| e.to_string())?;
        let delivered = visit.history.len() > before;
        if kind == "discovery"
            && delivered
            && !shadow
            && (env::var_os("ROBOTUTOR_DESKTOP").is_none()
                || env::var_os("ROBOTUTOR_NATIVE").is_some())
            && journal.as_ref().is_some_and(|j| j.assignment.is_none())
        {
            if let (Some(folder), Some(seed)) =
                (workspace_dir.as_deref(), course::agreed_seed(&visit.lesson))
            {
                if let Err(error) = course::prepare_private(folder, &seed) {
                    eprintln!("Private course preparation unavailable: {error}");
                }
            }
        }
        let ambiguous = visit
            .lesson
            .practice_evidence
            .as_ref()
            .is_some_and(|e| e.decision == "provisional")
            || visit
                .decisions
                .last()
                .and_then(|r| r.as_ref())
                .and_then(|r| r.assessment.as_ref())
                .is_some_and(|a| {
                    matches!(a.method.as_str(), "disagreement" | "verifier_unavailable")
                });
        let kind = if ambiguous && delivered {
            "ambiguous"
        } else {
            kind
        };
        if let Some(mut sample) = metrics::end(
            kind,
            delivered,
            !visit.lesson.active_step().is_empty() || discovery::active(&visit),
        ) {
            sample.entry = Some(
                if first_submission {
                    if fresh {
                        "cold"
                    } else {
                        "return"
                    }
                } else {
                    "ongoing"
                }
                .into(),
            );
            first_submission = false;
            append(&mut journal, &mut visit, Event::Timing { sample })?;
        }
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{}", display(&error));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
