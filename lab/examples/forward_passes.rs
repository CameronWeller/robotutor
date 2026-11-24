//! Persist and replay counterexample-driven simulator corrections. Each committed
//! journal is a complete JSON Lines snapshot. Readers open it without taking a lock.
use serde_json::Value;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tutor_lab::campaign::forward::Experiment;

const USAGE: &str = "forward_passes run JOURNAL N | verify JOURNAL | explain JOURNAL PASS | simulate JOURNAL DOMAIN SEED | report JOURNAL";
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn sibling(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let mut name = path
        .file_name()
        .ok_or("a journal filename is required")?
        .to_owned();
    name.push(suffix);
    Ok(path.with_file_name(name))
}

fn directory(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

// Replacing the journal changes its inode, so the lock must live at a stable
// sibling path. Keep that file in place; dropping the handle releases the OS lock.
fn writer_lock(path: &Path) -> Result<File, String> {
    let lock_path = sibling(path, ".forward.lock")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| format!("open writer lock {}: {e}", lock_path.display()))?;
    lock.try_lock()
        .map_err(|e| format!("acquire writer lock {}: {e}", lock_path.display()))?;
    Ok(lock)
}

struct Journal {
    header: Value,
    rows: Vec<Value>,
    bytes: Vec<u8>,
}

impl Journal {
    fn new(header: Value) -> Result<Self, String> {
        let mut bytes = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        Ok(Self {
            header,
            rows: Vec::new(),
            bytes,
        })
    }

    fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        let body = bytes
            .strip_suffix(b"\n")
            .ok_or("empty or incomplete journal: missing final newline")?;
        let mut records = Vec::new();
        for (i, line) in body.split(|&b| b == b'\n').enumerate() {
            let number = i + 1;
            if line.iter().all(u8::is_ascii_whitespace) {
                return Err(format!("blank journal record at line {number}"));
            }
            let record: Value = serde_json::from_slice(line)
                .map_err(|e| format!("invalid JSON at line {number}: {e}"))?;
            if !record.is_object() {
                return Err(format!("expected one JSON object at line {number}"));
            }
            if i > 0 && record["pass"].as_u64() != Some(i as u64) {
                return Err(format!("expected pass {i} at line {number}"));
            }
            records.push(record);
        }
        let mut items = records.into_iter();
        let header = items.next().ok_or("journal has no header")?;
        Ok(Self {
            header,
            rows: items.collect(),
            bytes,
        })
    }

    fn load(path: &Path) -> Result<Option<Self>, String> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("inspect {}: {e}", path.display())),
            Ok(meta) if !meta.is_file() => {
                return Err(format!(
                    "expected a regular journal file: {}",
                    path.display()
                ))
            }
            Ok(_) => {}
        }
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Self::parse(bytes).map(Some)
    }

    fn append(&mut self, path: &Path, row: Value) -> Result<(), String> {
        let pass = self.rows.len() + 1;
        if row["pass"].as_u64() != Some(pass as u64) {
            return Err(format!(
                "engine emitted an out-of-sequence record at pass {pass}"
            ));
        }
        let mut line = serde_json::to_vec(&row).map_err(|e| e.to_string())?;
        line.push(b'\n');
        let mut staged = StagedFile::new(path)?;
        staged
            .file
            .write_all(&self.bytes)
            .and_then(|()| staged.file.write_all(&line))
            .map_err(|e| format!("stage pass {pass}: {e}"))?;
        staged.publish(path)?;
        self.bytes.extend(line);
        self.rows.push(row);
        Ok(())
    }
}

// A crash before rename can leave a uniquely named staging file. It is never
// used as committed history. Other writers neither select nor remove that file.
struct StagedFile {
    path: PathBuf,
    file: File,
}

impl StagedFile {
    fn new(journal: &Path) -> Result<Self, String> {
        loop {
            let n = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            let path = sibling(journal, &format!(".stage.{}.{n}", std::process::id()))?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            match options.open(&path) {
                Ok(file) => return Ok(Self { path, file }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("create staging file {}: {e}", path.display())),
            }
        }
    }

    fn publish(self, path: &Path) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => self
                .file
                .set_permissions(meta.permissions())
                .map_err(|e| format!("preserve journal permissions: {e}"))?,
            Ok(_) => {
                return Err(format!(
                    "refuse to replace non-regular journal {}",
                    path.display()
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("inspect journal before publish: {e}")),
        }
        self.file
            .sync_all()
            .map_err(|e| format!("sync staged journal: {e}"))?;
        tutor_lab::platform::replace_staged(directory(path), &self.path, path)
            .map_err(|e| format!("commit staged journal: {e}"))
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

enum Operation<'a> {
    Run(usize),
    Verify,
    Report,
    Explain(usize),
    Simulate { domain: &'a str, seed: u64 },
}

fn positive_count(value: &OsStr, error: &'static str) -> Result<usize, String> {
    value
        .to_str()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .ok_or_else(|| error.to_owned())
}

// These records have already been fully regenerated from the current experiment.
fn explain(r: &Value) -> String {
    fn words(v: &Value) -> String {
        v.as_str().expect("verified label").replace('_', " ")
    }
    fn witness(w: &Value) -> String {
        format!(
            "{} seed {}, {}. The simulator predicted “{}”. The reference returned “{}”.",
            words(&w["domain"]),
            w["seed"],
            words(&w["step"]),
            words(&w["predicted"]),
            words(&w["observed"])
        )
    }
    fn comparison(label: &str, score: &Value) -> String {
        format!("{label} errors changed from {} to {} across {} observations ({} fixed, {} newly wrong).",
            score["errors_before"], score["errors_candidate"], score["observations"], score["fixed"], score["new_errors"])
    }
    let design = &r["design"];
    let guards: Vec<_> = design["predicates"]
        .as_array()
        .expect("verified predicates")
        .iter()
        .map(|p| format!("“{}” is {}", words(&p["feature"]), p["value"]))
        .collect();
    let edit = if design["op"] == "remove" {
        "Remove the exception"
    } else {
        "Put this exception first"
    };
    let sim = &r["simulation"];
    let a = &r["analysis"];
    let disposition = if a["promoted"] == true {
        "Retained this edit"
    } else {
        "Rejected this edit and kept the previous model"
    };
    let reason = match a["reason"].as_str().expect("verified reason") {
        "witness_still_wrong" => "it still mispredicted the selected case",
        "reopens_retained_counterexample" => "it broke an earlier protected correction",
        "new_false_clear_on_fixed_suite" => {
            "it introduced a new false clearance on the fixed suite"
        }
        "fixed_suite_loss" => "it increased total errors on the fixed suite",
        "fresh_trajectory_loss" => "it increased total errors on the fresh trajectory",
        "reduces_development_error" => {
            "it reduced total development errors and passed the acceptance checks"
        }
        "simpler_at_equal_error" => {
            "it used fewer rules at the same development error and passed the acceptance checks"
        }
        "no_development_gain" => {
            "it neither reduced development error nor simplified the model at equal error"
        }
        reason => reason,
    };
    let next = &a["next_counterexample"];
    let followup = if next.is_null() {
        "No remaining development witness was found.".to_owned()
    } else {
        format!("Investigate {}", witness(next))
    };
    format!(
        "Pass {}

Plan
Investigate {}

Design
{edit} for {}. When {}, its prediction is “{}”. The candidate predicts “{}” for the selected case.

Simulate
{}
{}
The candidate introduced {} new false clearances on the fixed suite.

Analyze
{disposition} because {reason}.
{followup}",
        r["pass"],
        witness(&r["plan"]["counterexample"]),
        words(&design["domain"]),
        guards.join(" and "),
        words(&design["outcome"]),
        words(&sim["candidate_prediction"]),
        comparison("Fixed regression suite", &sim["fixed_regression"]),
        comparison(
            &format!(
                "Fresh {} trajectory (seed {})",
                words(&sim["fresh_domain"]),
                sim["fresh_seed"]
            ),
            &sim["fresh"]
        ),
        sim["new_false_clears_on_fixed_suite"]
    )
}

fn command(args: &[std::ffi::OsString]) -> Result<(), String> {
    let (op, path) = match args {
        [mode, path, count] if mode == OsStr::new("run") => (
            Operation::Run(positive_count(
                count,
                "N must be a positive count of additional passes",
            )?),
            Path::new(path),
        ),
        [mode, path] if mode == OsStr::new("verify") => (Operation::Verify, Path::new(path)),
        [mode, path] if mode == OsStr::new("report") => (Operation::Report, Path::new(path)),
        [mode, path, pass] if mode == OsStr::new("explain") => (
            Operation::Explain(positive_count(
                pass,
                "PASS must be a positive round number",
            )?),
            Path::new(path),
        ),
        [mode, path, domain, seed] if mode == OsStr::new("simulate") => (
            Operation::Simulate {
                domain: domain
                    .to_str()
                    .ok_or("DOMAIN must be a UTF-8 workload name")?,
                seed: seed
                    .to_str()
                    .and_then(|n| n.parse().ok())
                    .ok_or("SEED must be an unsigned 64-bit integer")?,
            },
            Path::new(path),
        ),
        _ => return Err(USAGE.into()),
    };
    let _lock = if matches!(op, Operation::Run(_)) {
        Some(writer_lock(path)?)
    } else {
        None
    };
    let engine = Experiment::new()?;
    let mut journal = match Journal::load(path)? {
        Some(stored) => stored,
        None if matches!(op, Operation::Run(_)) => Journal::new(engine.header())?,
        None => return Err(format!("journal does not exist: {}", path.display())),
    };
    let mut session = engine.restore(&journal.header, &journal.rows)?;
    let print_json = |result: Value| -> Result<(), String> {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
        );
        Ok(())
    };
    match op {
        Operation::Verify => println!("Verified {} passes.", journal.rows.len()),
        Operation::Report => return print_json(engine.report(&session)?),
        Operation::Simulate { domain, seed } => {
            return print_json(engine.simulate(&session, domain, seed)?)
        }
        Operation::Explain(round) => {
            let row = journal.rows.get(round - 1).ok_or_else(|| {
                format!(
                    "pass {round} is missing from the {} committed passes",
                    journal.rows.len()
                )
            })?;
            println!("{}", explain(row));
        }
        Operation::Run(add) => {
            let previous = journal.rows.len();
            let target = previous
                .checked_add(add)
                .filter(|n| u32::try_from(*n).is_ok())
                .ok_or("requested total exceeds the experiment's 32-bit pass counter")?;
            for next in previous + 1..=target {
                let row = engine
                    .step(&mut session)
                    .map_err(|e| format!("pass {next}: {e}"))?;
                journal.append(path, row)?;
            }
            eprintln!(
                "Committed {add} {} to {} ({} total).",
                if add == 1 { "pass" } else { "passes" },
                path.display(),
                journal.rows.len()
            );
        }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match command(&std::env::args_os().skip(1).collect::<Vec<_>>()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            loop {
                let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("tutor-forward-test-{}-{id}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("create isolated test directory: {e}"),
                }
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn partial_blank_duplicate_and_missing_rows_never_become_history() {
        for malformed in [
            "",
            "\n",
            "{}\n\n",
            "{}\n{",
            "{}\n{\"pass\":1}",
            "{}\nnull\n",
            "{}\n{\"pass\":2}\n",
            "{}\n{\"pass\":1}\n{\"pass\":1}\n",
            "{}\n{\"pass\":1}\n{\"pass\":3}\n",
        ] {
            assert!(
                Journal::parse(malformed.as_bytes().to_vec()).is_err(),
                "{malformed:?}"
            );
        }
    }

    #[test]
    fn lock_survives_replacement_and_uncommitted_staging_leaves_previous_revision() {
        let temp = Scratch::new();
        let path = temp.0.join("run.jsonl");
        assert!(Journal::load(&path).unwrap().is_none());
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
        let lock = writer_lock(&path).unwrap();
        let mut journal = Journal::new(json!({"corpus": 1})).unwrap();
        journal.append(&path, json!({"pass": 1})).unwrap();
        let mut old_reader = File::open(&path).unwrap();
        let old_bytes = fs::read(&path).unwrap();
        {
            let mut partial = StagedFile::new(&path).unwrap();
            partial.file.write_all(b"incomplete").unwrap();
            assert_eq!(fs::read(&path).unwrap(), old_bytes);
        }
        assert!(writer_lock(&path).is_err());
        journal.append(&path, json!({"pass": 2})).unwrap();
        assert!(writer_lock(&path).is_err());
        let mut snapshot = Vec::new();
        std::io::Read::read_to_end(&mut old_reader, &mut snapshot).unwrap();
        assert_eq!(snapshot, old_bytes);
        drop(lock);
        let _second_writer = writer_lock(&path).unwrap();
        let reopened = Journal::load(&path).unwrap().unwrap();
        assert_eq!(reopened.rows, [json!({"pass": 1}), json!({"pass": 2})]);
        assert_eq!(reopened.bytes, journal.bytes);
    }
    #[test]
    fn continued_run_matches_uninterrupted_history_and_rejects_changed_measurements() {
        let temp = Scratch::new();
        let split = temp.0.join("split.jsonl").into_os_string();
        let whole = temp.0.join("whole.jsonl").into_os_string();
        let run =
            |path: &std::ffi::OsString, n: &str| command(&["run".into(), path.clone(), n.into()]);
        run(&split, "1").unwrap();
        run(&split, "2").unwrap();
        run(&whole, "3").unwrap();
        assert_eq!(fs::read(&split).unwrap(), fs::read(&whole).unwrap());
        assert!(run(&split, "4294967295").is_err());
        assert_eq!(fs::read(&split).unwrap(), fs::read(&whole).unwrap());
        command(&["verify".into(), split.clone()]).unwrap();
        command(&["explain".into(), split.clone(), "3".into()]).unwrap();
        assert!(command(&["explain".into(), split.clone(), "4".into()]).is_err());
        let mut journal = Journal::load(Path::new(&split)).unwrap().unwrap();
        journal.rows[0]["simulation"]["fresh"]["observations"] = json!(123456);
        let mut altered = Vec::new();
        for record in std::iter::once(&journal.header).chain(&journal.rows) {
            serde_json::to_writer(&mut altered, record).unwrap();
            altered.push(b'\n');
        }
        fs::write(&split, &altered).unwrap();
        assert!(command(&["verify".into(), split.clone()]).is_err());
        assert!(run(&split, "1").is_err());
        assert_eq!(fs::read(split).unwrap(), altered);
    }
}
