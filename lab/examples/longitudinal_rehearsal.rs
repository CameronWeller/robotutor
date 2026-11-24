//! A process boundary around the reusable rehearsal reducer. A final incomplete
//! record is an unpublished write. Every mutation compares the caller's head.
use serde_json::Value;
use std::{
    env,
    io::{self, Read},
    path::Path,
    process::ExitCode,
};
use tutor_lab::{
    protocol::Assistance,
    rehearsal::{journal, Action, Session},
};
const USAGE:&str="longitudinal_rehearsal init FILE | show FILE [EXPECTED_HEAD] | forecast FILE EXPECTED_HEAD DAY [TARGET_DAY] | apply FILE EXPECTED_HEAD open DAY|resume DAY|work|answer TEXT none|reported|unknown|reply ASSISTANCE (stdin)|course-aid format|worked|source|withdraw|replace|revoke|reissue|unknown|recover|varied-instances|diagnostic|diagnostic-answer TEXT ASSISTANCE|diagnostic-help|diagnostic-skip|practice-case warm|agreed|brief|ambiguous|practice-next limit|invitation|clear|practice-visit DAY|goal choose UNIT|goal visit|goal try ORIGINAL ALTERNATIVE|goal wait|goal clear|goal source held|granted|goal move UNIT repeat|practice|check|proceed|private|goal-write UNIT (response on stdin)|goal request UNIT|case-review CANDIDATE";
fn parse(args: &[String]) -> Result<Action, String> {
    let Some(op) = args.first().map(String::as_str) else {
        return Err(USAGE.into());
    };
    let arg = |i| args.get(i).map(String::as_str).ok_or(USAGE);
    let help = |s| match s {
        "none" => Ok(Assistance::NoneReported),
        "reported" => Ok(Assistance::Reported),
        "unknown" => Ok(Assistance::Unknown),
        _ => Err(USAGE),
    };
    Ok(match (op, args.len()) {
        ("course-aid", 2) => Action::CourseAid(arg(1)?.into()),
        ("reply", 2) => {
            let mut text = String::new();
            io::stdin()
                .take((tutor_lab::workshop::MAX_DRAFT_BYTES + 1) as u64)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            Action::Reply(text, help(arg(1)?)?)
        }
        ("case-review", 2) => Action::CaseReview(arg(1)?.into()),
        ("goal", 2..=4) => Action::Goal(args[1..].to_vec()),
        ("goal-write", 2) => {
            let mut text = String::new();
            io::stdin()
                .take(4097)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            Action::GoalWriting(arg(1)?.parse().map_err(|_| "invalid lesson unit")?, text)
        }
        ("practice-case", 2) => Action::PracticeCase(arg(1)?.into()),
        ("practice-next", 2) => Action::PracticeNext(arg(1)?.into()),
        ("practice-visit", 2) => {
            Action::PracticeVisit(arg(1)?.parse().map_err(|_| "invalid practice day")?)
        }
        ("open", 2) => Action::Open(arg(1)?.parse().map_err(|_| "invalid day")?),
        ("resume", 2) => Action::Resume(arg(1)?.parse().map_err(|_| "invalid day")?),
        ("answer", 3) => Action::Answer(arg(1)?.into(), help(arg(2)?)?),
        ("diagnostic-answer", 3) => Action::DiagnosticAnswer(arg(1)?.into(), help(arg(2)?)?),
        ("work", 1) => {
            let mut draft = String::new();
            io::stdin()
                .take((tutor_lab::workshop::MAX_DRAFT_BYTES + 1) as u64)
                .read_to_string(&mut draft)
                .map_err(|e| e.to_string())?;
            Action::Work(draft)
        }
        ("withdraw", 1) => Action::WithdrawSource,
        ("replace", 1) => Action::ReplaceSource,
        ("revoke", 1) => Action::RevokeGrant,
        ("reissue", 1) => Action::ReissueGrant,
        ("unknown", 1) => Action::UnknownContact,
        ("recover", 1) => Action::RecoverContact,
        ("varied-instances", 1) => Action::UseVariedInstances,
        ("diagnostic", 1) => Action::Diagnostic,
        ("diagnostic-help", 1) => Action::DiagnosticHelp,
        ("diagnostic-skip", 1) => Action::DiagnosticSkip,
        _ => return Err(USAGE.into()),
    })
}
fn run(args: &[String]) -> Result<Value, String> {
    if args.len() < 2 {
        return Err(USAGE.into());
    }
    let path = Path::new(&args[1]);
    match (args[0].as_str(), args.len()) {
        ("init", 2) => journal::create(path),
        ("show", 2..=3) => journal::read(path, args.get(2).map(String::as_str), Session::status),
        ("forecast", 4..=5) => journal::read(path, Some(&args[2]), |s| {
            let day = args[3].parse().map_err(|_| "invalid forecast day")?;
            let target = args
                .get(4)
                .map(|s| s.parse().map_err(|_| "invalid retention target"))
                .transpose()?;
            s.return_forecast(day, target)
        }),
        ("apply", n) if n >= 4 => journal::apply(path, &args[2], || parse(&args[3..])),
        _ => Err(USAGE.into()),
    }
}
fn main() -> ExitCode {
    match run(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, File},
        io::Write,
    };
    #[test]
    fn recovery_uses_only_complete_records_and_checks_the_callers_head_before_writing() {
        let root = env::temp_dir().join(format!("longitudinal-rehearsal-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let path = root.join("session");
        let call = |command: &str, extra: &[&str]| {
            let mut args = vec![command.to_string(), path.to_str().unwrap().into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        let start = call("init", &[]).unwrap();
        let old = start["head"].as_str().unwrap();
        let open = call("apply", &[old, "open", "30"]).unwrap();
        let head = open["head"].as_str().unwrap();
        let mut f = File::options().append(true).open(&path).unwrap();
        f.write_all(b"{\"partial\":").unwrap();
        f.sync_all().unwrap();
        drop(f);
        assert_eq!(call("show", &[head]).unwrap()["current_opening"]["task"], 1);
        let partial = fs::read(&path).unwrap();
        assert!(call("apply", &[old, "replace"]).is_err());
        assert_eq!(fs::read(&path).unwrap(), partial);
        call("apply", &[head, "replace"]).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("partial"));
        assert_eq!(call("show", &[]).unwrap()["revision"], 2);
        fs::remove_dir_all(root).unwrap();
    }
}
