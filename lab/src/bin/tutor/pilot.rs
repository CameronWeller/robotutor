//! Private prospective pilot records and shadow observation of existing lesson journals.
use crate::{
    judgment, request,
    session::{Event, Visit},
};
use serde_json::json;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    path::Path,
};
use tutor_lab::openrouter::{assistant_text, credentials, Role};
use tutor_lab::platform::PrivateOpen;

/// Sample deliveries in the caller's order, including ordinary turns with zero findings.
/// The output is deliberately separate from all input journals and the learner index.
pub fn shadow_audit(output: &Path, journals: &[std::ffi::OsString]) -> Result<(), String> {
    let connection = credentials::connect()?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open_private(output)
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    let mut all = Vec::new();
    let mut previous_tutors: Vec<(String, String)> = Vec::new();
    for path in journals {
        let path = Path::new(path);
        let prior = OpenOptions::new()
            .read(true)
            .open_private(path)
            .map_err(|e| e.to_string())?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid journal name.")?;
        let mut visit = Visit::default();
        for line in BufReader::new(prior).lines() {
            let event: Event = serde_json::from_str(&line.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if let Event::Delivered { text, .. } = &event {
                let source = format!("{name}#{}", visit.history.len() + 1);
                if let Some(words) = visit.pending.as_deref().filter(|w| !w.starts_with('/')) {
                    let proposals = judgment::observe(
                        &visit,
                        &source,
                        &judgment::active(&all),
                        &mut |packet, schema| {
                            let receipt = connection
                                .complete(Role::Luna, &request(&visit.history, packet, schema)?)?;
                            serde_json::from_str(assistant_text(&receipt)?)
                                .map_err(|e| e.to_string())
                        },
                    )?;
                    let cues = previous_tutors
                        .iter()
                        .filter(|(_, t)| {
                            proposals["findings"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .any(|p| {
                                    matches!(p["kind"].as_str(), Some("method" | "prompted_method"))
                                        && p["detail"]
                                            .as_str()
                                            .is_some_and(|m| judgment::mentions(t, m))
                                })
                        })
                        .cloned()
                        .collect();
                    let input = judgment::Inputs {
                        version: judgment::VERSION.into(),
                        source: source.clone(),
                        shadow: true,
                        external_findings: all.clone(),
                        external_cues: cues,
                        probe: None,
                        proposals,
                    };
                    let (accepted, rejected) =
                        judgment::validate(&input, &visit, &judgment::active(&all));
                    serde_json::to_writer(&mut file,&json!({"source":source,"task":judgment::task(&visit.lesson),"learner":words,
                        "preceding_step":visit.lesson.active_step(),"actual_reply":text,"proposals":input.proposals,"accepted":accepted,"rejected":rejected})).map_err(|e|e.to_string())?;
                    file.write_all(b"\n")
                        .and_then(|_| file.sync_data())
                        .map_err(|e| e.to_string())?;
                    all.extend(accepted);
                    count += 1;
                    if count == 10 {
                        println!("Saved ten shadow opportunities in the private review file.");
                        return Ok(());
                    }
                }
                previous_tutors.push((source, text.clone()));
            }
            visit.apply(event)?;
        }
    }
    println!("Saved {count} shadow opportunities in the private review file.");
    Ok(())
}

/// A forecast names a fresh task before its outcome. Subsequent rows are append-only.
/// Journal locking orders learner actions and prevents concurrent writers in the same visit.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Note {
    Review {
        source: String,
        category: String,
        reason: String,
    },
    Prediction {
        id: String,
        skill: String,
        expected: String,
        criterion: String,
        after_turn: usize,
    },
    Outcome {
        id: String,
        source: String,
        result: String,
        help: String,
        task_form: String,
        evidence: String,
        observed_at: u64,
    },
}
type Notes = Vec<(u64, Note)>;

fn notes_in(path: &Path, exclude: Option<&str>) -> Result<Notes, String> {
    if !path.is_dir() {
        return Ok(crate::session::replay(path)?.notes);
    }
    let mut notes = Vec::new();
    for path in crate::replay::journals(path)? {
        if path.file_stem().and_then(|s| s.to_str()) == exclude {
            continue;
        }
        notes.extend(crate::session::replay(&path)?.notes);
    }
    notes.sort_by_key(|(at, _)| *at);
    Ok(notes)
}

pub fn note(
    command: &str,
    arg: &str,
    visit: &Visit,
    session: &str,
    dir: Option<&Path>,
) -> Result<Note, String> {
    let n = visit.history.len();
    let parts: Vec<_> = arg.split('|').map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err("Fill every field separated by |.".into());
    }
    let usage = "Use /pilot [turn|]category|reason, /predict skill|expected result|fresh application criterion, or /outcome prediction-id|result|task form|intervening help|evidence.";
    match (command, parts.as_slice()) {
        ("/pilot", [category, reason]) | ("/pilot", [_, category, reason]) => {
            let categories = [
                "advanced",
                "unrelated-question",
                "useful",
                "unsupported",
                "ignored-correction",
                "redundant",
                "missed",
                "neutral",
            ];
            let (source_session, index, available) = if parts.len() == 3 {
                if let Some((name, index)) = parts[0].rsplit_once('#') {
                    let length = if name == session {
                        n
                    } else {
                        let folder =
                            dir.ok_or("Select a source within the same private workspace.")?;
                        let journal = crate::replay::journals(folder)?
                            .into_iter()
                            .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(name))
                            .ok_or("Unknown visit.")?;
                        crate::session::replay(&journal)?.history.len()
                    };
                    (name, Some(index), length)
                } else {
                    (session, Some(parts[0]), n)
                }
            } else {
                (session, None, n)
            };
            let ordinal = if let Some(index) = index {
                index
                    .parse::<usize>()
                    .map_err(|_| "Choose a delivered turn number in this visit.")?
            } else {
                n
            };
            if ordinal == 0 || ordinal > available || !categories.contains(category) {
                return Err("Choose a delivered turn and a supported review category.".into());
            }
            Ok(Note::Review {
                source: format!("{source_session}#{ordinal}"),
                category: category.to_string(),
                reason: reason.to_string(),
            })
        }
        ("/predict", [skill, expected, criterion]) => {
            if visit.pending.is_some() {
                return Err("The learner has already submitted this turn. Record a forecast before their next application.".into());
            }
            if !["independent", "with-help"].contains(expected) {
                return Err("Choose independent or with-help for the prediction.".into());
            }
            Ok(Note::Prediction {
                id: format!("{session}/p{}", visit.notes.len() + 1),
                skill: skill.to_string(),
                expected: expected.to_string(),
                criterion: criterion.to_string(),
                after_turn: n,
            })
        }
        ("/outcome", [id, result, form, help, evidence]) => {
            if !["independent", "with-help", "unobserved"].contains(result)
                || !["changed-case", "same-task"].contains(form)
            {
                return Err(
                    "Choose independent, with-help or unobserved and changed-case or same-task."
                        .into(),
                );
            }
            let mut notes = dir
                .map(|p| notes_in(p, Some(session)))
                .transpose()?
                .unwrap_or_default();
            notes.extend_from_slice(&visit.notes);
            let (forecast_at, prediction) = notes
                .iter()
                .find(|(_, note)| matches!(note, Note::Prediction{id: key,..} if key == id))
                .ok_or("Unknown prediction.")?;
            let Note::Prediction { after_turn, .. } = prediction else {
                unreachable!()
            };
            let same_visit = id.rsplit_once('/').is_some_and(|(name, _)| name == session);
            let later = if same_visit {
                n > *after_turn
            } else {
                visit.last_delivery_at > *forecast_at
            };
            let learner_work = visit
                .history
                .last()
                .and_then(|(words, _)| words.as_deref())
                .is_some_and(|words| !words.starts_with('/'));
            if !later || !learner_work {
                return Err("Record a subsequent learner application before its outcome.".into());
            }
            if notes
                .iter()
                .any(|(_, note)| matches!(note, Note::Outcome{id:key,..} if key == id))
            {
                return Err("This prediction already has an outcome.".into());
            }
            Ok(Note::Outcome {
                id: id.to_string(),
                source: format!("{session}#{n}"),
                result: result.to_string(),
                help: help.to_string(),
                task_form: form.to_string(),
                evidence: evidence.to_string(),
                observed_at: visit.last_delivery_at,
            })
        }
        _ => Err(usage.into()),
    }
}
pub fn summarize(notes: &[(u64, Note)]) -> serde_json::Value {
    let mut rows = Vec::new();
    let mut delayed_transfer = Vec::new();
    let mut unobserved = Vec::new();
    let mut errors = Vec::new();
    let mut forecast_skills = std::collections::BTreeSet::new();
    let mut observed_at_four_weeks = std::collections::BTreeSet::new();
    for (at, n) in notes {
        if let Note::Prediction {
            id,
            skill,
            expected,
            criterion,
            ..
        } = n
        {
            forecast_skills.insert(skill);
            let outcomes: Vec<_> = notes
                .iter()
                .filter_map(|(_, n)| match n {
                    Note::Outcome { id: k, .. } if k == id => Some(n),
                    _ => None,
                })
                .collect();
            if let [Note::Outcome {
                result,
                help,
                source,
                task_form,
                evidence,
                observed_at,
                ..
            }] = outcomes.as_slice()
            {
                let days = observed_at.saturating_sub(*at) / 86400;
                if result == "unobserved" {
                    unobserved.push(id);
                } else if result != expected {
                    errors.push(id);
                }
                if days >= 28 && result != "unobserved" && task_form == "changed-case" {
                    observed_at_four_weeks.insert(skill);
                }
                if days >= 28 && result == "independent" && task_form == "changed-case" {
                    delayed_transfer.push(source);
                }
                rows.push(json!({"id":id,"skill":skill,"criterion":criterion,"prediction":expected,"observed":result,
                    "source":source,"intervening_help":help,"task_form":task_form,"reviewer_evidence":evidence,"forecast_at":at,"observed_at":observed_at,"elapsed_days":days}));
            } else {
                unobserved.push(id);
                rows.push(
                    json!({"id":id,"skill":skill,"criterion":criterion,"prediction":expected,
                    "observed":if outcomes.is_empty(){"unobserved"}else{"conflicting outcomes"}}),
                );
            }
        }
    }
    let reviews: Vec<_> = notes
        .iter()
        .filter(|(_, n)| matches!(n, Note::Review { .. }))
        .collect();
    json!({"reviews":reviews,"predictions":rows,"reviewer_recorded_independent_transfer_at_four_weeks_or_later":delayed_transfer,
        "prediction_errors":errors,"unobserved_predictions":unobserved,"skills_without_a_four_week_changed_case":forecast_skills.difference(&observed_at_four_weeks).collect::<Vec<_>>()})
}
/// Record prospective notes without opening a lesson or triggering tutoring on return.
pub fn record_notes(path: &Path) -> Result<(), String> {
    let dir = path.parent().ok_or("Select a private workspace journal.")?;
    if !crate::replay::journals(dir)?.iter().any(|p| p == path) {
        return Err("Select an existing private journal.".into());
    }
    let session = path
        .file_stem()
        .and_then(|n| n.to_str())
        .ok_or("Invalid session.")?;
    let mut visit = Visit::default();
    let mut journal = Some(crate::Journal::open(path, &mut visit)?);
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let (command, fields) = line
            .split_once(' ')
            .ok_or("Expected a pilot command and fields.")?;
        let note = note(command, fields, &visit, session, Some(dir))?;
        crate::append(
            &mut journal,
            &mut visit,
            Event::Pilot {
                at: crate::session_time()?,
                note,
            },
        )?;
    }
    Ok(())
}

pub fn report(path: &Path) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&summarize(&notes_in(path, None)?)).unwrap()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_four_week_report_distinguishes_transfer_prediction_error_and_absence() {
        let mut visit = Visit::default();
        let prediction = note(
            "/predict",
            "fact selection|independent|Identify a relevant new fact",
            &visit,
            "march",
            None,
        )
        .unwrap();
        visit.notes.push((100, prediction));
        assert!(note(
            "/outcome",
            "march/p1|independent|changed-case|none|selected a fact",
            &visit,
            "march",
            None
        )
        .is_err());
        visit.history.push((
            Some("The warning makes notice plausible.".into()),
            "Discuss the warning.".into(),
        ));
        visit.last_delivery_at = 100 + 28 * 86400;
        let outcome=note("/outcome","march/p1|independent|changed-case|one intervening explanation|selected a new relevant fact",&visit,"march",None).unwrap();
        visit.notes.push((visit.last_delivery_at, outcome));
        let pending = note(
            "/predict",
            "compare two accounts|with-help|Name the discrepancy",
            &visit,
            "march",
            None,
        )
        .unwrap();
        visit.notes.push((visit.last_delivery_at, pending));
        let report = summarize(&visit.notes);
        assert_eq!(report["unobserved_predictions"], json!(["march/p3"]));
        assert_eq!(
            report["reviewer_recorded_independent_transfer_at_four_weeks_or_later"],
            json!(["march#1"])
        );
        if let Note::Outcome { result, .. } = &mut visit.notes[1].1 {
            *result = "with-help".into();
        }
        let report = summarize(&visit.notes);
        assert_eq!(report["prediction_errors"], json!(["march/p1"]));
        assert_eq!(
            report["reviewer_recorded_independent_transfer_at_four_weeks_or_later"],
            json!([])
        );
    }
}
