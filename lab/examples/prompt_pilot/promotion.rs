use super::*;
use serde_json::Value;
use std::{fs::File, io::Write};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    previous: String,
    next: String,
    approved_by: String,
    decision: String,
    report_sha256: String,
}

fn history(dir: &Path) -> Result<(usize, String)> {
    let mut reference = "luna-001".to_owned();
    let mut count = 0;
    if dir.is_dir() {
        let mut files = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        files.retain(|f| f.path().extension().is_some_and(|e| e == "json"));
        files.sort_by_key(|f| f.file_name());
        for file in files {
            count += 1;
            let record: Approval = serde_json::from_str(&read(&file.path())?)?;
            if file.file_name() != format!("{count:06}.json").as_str()
                || record.previous != reference
            {
                return Err("Broken canonical decision history".into());
            }
            reference = record.next;
        }
    }
    Ok((count, reference))
}
pub fn canonical(dir: &Path) -> Result<String> {
    Ok(history(dir)?.1)
}

pub fn record(dir: &Path, approval_path: &Path, f: &Frozen, report: &Value) -> Result<()> {
    let approval: Approval = serde_json::from_str(&read(approval_path)?)?;
    let (count, current) = history(dir)?;
    if f.manifest["mode"] != "live"
        || current != approval.previous
        || current != f.settings.baseline
        || approval.next != f.settings.candidate
        || approval.approved_by.trim().is_empty()
        || approval.decision.trim().is_empty()
        || approval.report_sha256 != hash(&serde_json::to_vec_pretty(report)?)
    {
        return Err("Approval must identify the current baseline, compared candidate, decision maker, decision and replayed report hash".into());
    }
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut file = File::create_new(&tmp)?;
    let result = (|| -> Result<()> {
        serde_json::to_writer_pretty(&mut file, &approval)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        // Publish a complete immutable entry. A competing decision cannot replace it.
        fs::hard_link(&tmp, dir.join(format!("{:06}.json", count + 1)))?;
        Ok(())
    })();
    fs::remove_file(tmp)?;
    result
}
