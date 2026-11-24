use super::{google, notebook};
use serde_json::{json, Value};
use std::{env, error::Error, fs, io, os::unix::process::CommandExt, path::PathBuf, process::Command};

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let first = args.next().ok_or("Missing research workspace")?;
    if first == "--notebook" {
        let workspace = PathBuf::from(args.next().ok_or("Missing notebook")?);
        let request: Value = serde_json::from_reader(io::stdin().lock())?;
        let raw = request["url"].as_str().unwrap_or("");
        let result = match request["action"].as_str() {
            Some("destination") => json!(notebook::destination(raw)),
            Some("source") => json!(notebook::source(raw).map(|s| s.article)),
            Some("google") => json!(google::sign_in()?),
            Some(action) => {
                let mut book = notebook::Notebook::open(&workspace.join("sources.sqlite3"))?;
                match action {
                    "list" => json!(book.recent()?),
                    "clear" => { book.clear()?; Value::Null }
                    "record" => {
                        if let Some(source) = notebook::source(raw) {
                            book.record(&notebook::observation(&source, &request["metadata"]))?;
                        }
                        Value::Null
                    }
                    _ => return Err("Unknown notebook operation".into()),
                }
            }
            None => return Err("Missing notebook operation".into()),
        };
        println!("{}", result);
        return Ok(());
    }
    let root = env::var_os("ROBOTUTOR_ROOT").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let client = root.join("desktop/chromium");
    let package = client.join("node_modules/electron");
    let executable = fs::read_to_string(package.join("path.txt"))
        .map_err(|_| "Install the pinned Chromium runtime with Robotutor.command")?;
    let mut command = Command::new(package.join("dist").join(executable.trim()));
    command.arg(&client).arg(first).args(args)
        .env("ROBOTUTOR_RESEARCH_HELPER", env::current_exe()?)
        .env_remove("ELECTRON_RUN_AS_NODE").env_remove("NODE_OPTIONS");
    Err(command.exec().into())
}
