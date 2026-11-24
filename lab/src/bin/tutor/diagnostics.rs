//! Diagnostics use only the workspaces explicitly supplied by their owner.
//! A shared session is a standalone export of visible exchanges.
use crate::{session, supervisor::Queue};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap, error::Error, ffi::OsString, fs::OpenOptions, io::Write, path::Path,
};
use tutor_lab::platform::PrivateOpen;

pub fn share(source: &Path, dest: &Path) -> Result<(), Box<dyn Error>> {
    let v = session::replay(source)?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open_private(dest)?;
    serde_json::to_writer_pretty(
        &mut f,
        &json!({"format":"shared-session/1","exchanges":v.history,"pending_learner_message":v.pending}),
    )?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    Ok(())
}

#[derive(Default, serde::Serialize)]
struct Counts {
    enrolled: usize,
    returned: usize,
    return_unknown: usize,
    enjoyment_positive: usize,
    enjoyment_observed: usize,
    enjoyment_missing: usize,
    delayed_unaided_success: usize,
    delayed_unaided_observed: usize,
    delayed_unaided_missing: usize,
}
pub fn report(dirs: &[OsString]) -> Result<Value, Box<dyn Error>> {
    let mut variants = BTreeMap::<String, Counts>::new();
    let mut cases = vec![];
    let mut reviewed = 0;
    let mut probes = 0;
    let mut missed = 0;
    let mut total_turns = 0;
    let mut supported = 0;
    let mut contradicted = 0;
    let mut delayed_skills = 0;
    let mut tracked_skills = 0;
    let mut journals_without_review = 0;
    let mut seen = std::collections::HashSet::new();
    for (index, d) in dirs.iter().enumerate() {
        let dir = Path::new(d).canonicalize()?;
        if !seen.insert(dir.clone()) {
            return Err("Repeated workspace".into());
        }
        let q = Queue::open(&dir)?;
        let notes = q.all_notes()?;
        let mut workspace = crate::memory::Workspace::open(&dir)?;
        workspace.sync()?;
        let skills = workspace.skills()?;
        tracked_skills += skills.len();
        delayed_skills += skills
            .iter()
            .filter(|s| s.durable && s.changed_form)
            .count();
        let history = q.history()?;
        journals_without_review += crate::replay::journals(&dir)?
            .iter()
            .filter(|p| {
                !history
                    .iter()
                    .any(|r| r["visit"].as_str() == p.file_stem().and_then(|s| s.to_str()))
            })
            .count();
        for r in history {
            reviewed += 1;
            let rep = &r["review"];
            let repeat = rep["repeated_probes"].as_array().unwrap();
            let omissions = rep["missed_demonstrations"].as_array().unwrap();
            probes += repeat.len();
            missed += omissions.len();
            total_turns += r["turns"].as_u64().unwrap_or(0);
            let rev = r["rev"].as_i64().unwrap();
            let outcome = notes.iter().rev().find(|n| {
                n["kind"] == "/review-result"
                    && n["body"]
                        .as_str()
                        .and_then(|s| s.split('|').next())
                        .and_then(|s| s.trim().parse::<i64>().ok())
                        == Some(rev)
            });
            if let Some(o) = outcome {
                match o["body"]
                    .as_str()
                    .unwrap()
                    .split('|')
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                {
                    "supported" => supported += 1,
                    "contradicted" => contradicted += 1,
                    _ => {}
                }
            }
            cases.push(json!({"workspace":index+1,"visit":r["visit"],"revision":rev,"model":r["model"],"version":r["version"],"sufficient_evidence":rep["evidence"].as_array().into_iter().flatten().filter(|e|e["sufficient_for_skill"]==true).map(|e|json!({"source":e["source"],"turn":e["turn"],"skill":e["skill"]})).collect::<Vec<_>>(),"repeated_probes":repeat,"missed_demonstrations":omissions,"counterexample_candidate":repeat.is_empty()||outcome.is_some_and(|n|n["body"].as_str().unwrap().contains("|contradicted|")),"later_evidence":outcome}));
        }
        for experiment in notes.iter().filter(|n| n["kind"] == "/experiment") {
            let fields: Vec<_> = experiment["body"]
                .as_str()
                .unwrap()
                .split('|')
                .map(str::trim)
                .collect();
            let id = experiment["id"].as_i64().unwrap();
            let c = variants.entry(fields[0].into()).or_default();
            c.enrolled += 1;
            let mut enjoyment = None;
            let mut delayed = None;
            let mut returned = q.returned_since(experiment["at"].as_i64().unwrap())?;
            for n in notes.iter().filter(|n| n["kind"] == "/measure") {
                let f: Vec<_> = n["body"]
                    .as_str()
                    .unwrap()
                    .split('|')
                    .map(str::trim)
                    .collect();
                if f[0].parse::<i64>().ok() != Some(id) {
                    continue;
                }
                match f[1] {
                    "enjoyment" => enjoyment = Some(f[2] == "positive"),
                    "return" => returned = true,
                    "delayed-unaided" => delayed = Some(f[2] == "success"),
                    _ => {}
                }
            }
            c.returned += usize::from(returned);
            c.return_unknown += usize::from(!returned);
            c.enjoyment_observed += usize::from(enjoyment.is_some());
            c.enjoyment_missing += usize::from(enjoyment.is_none());
            c.enjoyment_positive += usize::from(enjoyment == Some(true));
            c.delayed_unaided_observed += usize::from(delayed.is_some());
            c.delayed_unaided_missing += usize::from(delayed.is_none());
            c.delayed_unaided_success += usize::from(delayed == Some(true));
        }
    }
    Ok(
        json!({"journals_without_review":journals_without_review,"tracked_skills":tracked_skills,"durable_skills_with_changed_form":delayed_skills,"reviewed_snapshots":reviewed,"reviewed_turn_opportunities_including_revisited_history":total_turns,"repeated_probes":probes,"missed_demonstrations":missed,"review_supported":supported,"review_contradicted":contradicted,"review_outcome_missing":reviewed-supported-contradicted,"cases":cases,"personal_experiments_by_version":variants,"nonreturn_cause":"unknown"}),
    )
}
