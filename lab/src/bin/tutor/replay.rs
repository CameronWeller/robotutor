//! Read-only inspection. The single-journal form can use captured external inputs offline.
//! The workspace form also resolves those inputs against their originating learner journals.
use crate::{
    judgment::{self, Finding},
    session::{self, Visit},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use tutor_lab::platform::PrivateOpen;

pub fn journals(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !tutor_lab::platform::private_directory(dir).map_err(|e| e.to_string())? {
        return Err("Select a private workspace.".into());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|s| s != "jsonl") {
            continue;
        }
        if fs::OpenOptions::new()
            .read(true)
            .open_private(&path)
            .is_ok()
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn show(name: &str, visit: &Visit, later: &[Finding]) {
    for n in 1..=visit.history.len() {
        println!(
            "Turn {name}#{n}\n{}",
            judgment::render_trace(visit, n, later, true, crate::trace_view::Layout::Plain)
        );
    }
    let external = visit
        .decisions
        .iter()
        .flatten()
        .next_back()
        .map(|r| r.inputs.external_findings.as_slice())
        .unwrap_or_default();
    let model = json!({"active_findings":judgment::known(visit,external),"lesson":visit.lesson,"pending":visit.pending});
    println!(
        "Learner record at the end of {name}\n{}",
        serde_json::to_string_pretty(&model).unwrap()
    );
    println!(
        "Verified {} recorded decisions across {} delivered turns.",
        visit.decisions.iter().flatten().count(),
        visit.history.len()
    );
}

pub fn run(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        let visit = session::replay(path)?;
        show(
            path.file_stem().and_then(|n| n.to_str()).unwrap_or("visit"),
            &visit,
            &[],
        );
        return Ok(());
    }
    let mut visits = BTreeMap::new();
    for path in journals(path)? {
        let name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .ok_or("Invalid journal filename.")?
            .to_owned();
        visits.insert(name, session::replay(&path)?);
    }
    let findings: Vec<_> = visits.values().flat_map(Visit::findings).cloned().collect();
    let by_id: BTreeMap<_, _> = findings.iter().map(|f| (&f.id, f)).collect();
    for (name, visit) in &visits {
        for (index, receipt) in visit
            .decisions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Some((i, r.as_ref()?)))
        {
            if receipt.inputs.source != format!("{name}#{}", index + 1) {
                return Err("Recorded source and workspace filename differ.".into());
            }
            for finding in &receipt.inputs.external_findings {
                if by_id.get(&finding.id).copied() != Some(finding) {
                    return Err(format!("Missing or changed finding {}", finding.id));
                }
            }
            for (source, text) in &receipt.inputs.external_cues {
                let saved = source.rsplit_once('#').and_then(|(name, n)| {
                    visits
                        .get(name)?
                        .history
                        .get(n.parse::<usize>().ok()?.checked_sub(1)?)
                });
                if saved.is_none_or(|(_, t)| t != text) {
                    return Err(format!("Missing or changed tutor cue at {source}"));
                }
            }
        }
    }
    for (name, visit) in &visits {
        show(name, visit, &findings);
    }
    println!(
        "Current workspace findings\n{}",
        serde_json::to_string_pretty(&judgment::active(&findings)).unwrap()
    );
    Ok(())
}
