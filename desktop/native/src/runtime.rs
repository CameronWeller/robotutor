use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn command(binary: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(binary);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}
fn configured(name: &str, fallback: PathBuf) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap_or(fallback)
}
pub fn save(path: &Path, value: &Value, create: bool) -> Result<()> {
    let mut tmp = NamedTempFile::new_in(path.parent().ok_or("Missing workspace")?)?;
    serde_json::to_writer(&mut tmp, value)?;
    tmp.as_file_mut().sync_all()?;
    if create {
        tmp.persist_noclobber(path)?;
    } else {
        tmp.persist(path)?;
    }
    Ok(())
}
pub fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_reader(fs::File::open(path)?)?)
}

#[derive(Clone)]
pub struct Runtime {
    pub root: PathBuf,
    pub tutor: PathBuf,
    pub personal: PathBuf,
    pub assigned: PathBuf,
    pub state: PathBuf,
    pub port: u16,
    pub courses: PathBuf,
    pub testing: bool,
}
impl Runtime {
    pub fn open(rebuild: bool) -> Result<Self> {
        let root =
            crate::installation::project_root(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .canonicalize()?;
        let target = configured("CARGO_TARGET_DIR", root.join("lab/target"));
        let build = |var: &str, name: &str, manifest: &str| -> Result<PathBuf> {
            let filename = format!("{name}{}", env::consts::EXE_SUFFIX);
            let sibling = env::current_exe()?.with_file_name(&filename);
            let binary = configured(
                var,
                if sibling.is_file() {
                    sibling
                } else {
                    target.join("debug").join(filename)
                },
            );
            if !binary.is_file() || rebuild {
                let ok = command("cargo")
                    .env("CARGO_TARGET_DIR", &target)
                    .args(["build", "--locked", "--manifest-path"])
                    .arg(root.join(manifest))
                    .args(["--bin", name])
                    .status()?;
                if !ok.success() || !binary.is_file() {
                    return Err(format!("Could not build {name}").into());
                }
            }
            Ok(binary)
        };
        let tutor = build("ROBOTUTOR_BIN", "tutor", "lab/Cargo.toml")?;
        let service = build("ROBOTUTOR_LMS_BIN", "robotutor-lms", "lms/Cargo.toml")?;
        let courses = if cfg!(target_os = "linux") {
            PathBuf::from("xdg-open")
        } else {
            build(
                "ROBOTUTOR_COURSES_BIN",
                "robotutor-courses",
                "desktop/course-window/Cargo.toml",
            )?
        };
        let state_home = if let Some(x) = env::var_os("XDG_STATE_HOME") {
            PathBuf::from(x)
        } else if cfg!(windows) {
            PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("Missing LOCALAPPDATA")?)
        } else {
            PathBuf::from(env::var_os("HOME").ok_or("Missing home directory")?).join(
                if cfg!(target_os = "macos") {
                    "Library/Application Support"
                } else {
                    ".local/state"
                },
            )
        };
        let state = state_home.join("robotutor");
        let personal = configured("ROBOTUTOR_WORKSPACE", state.join("learner-tests"));
        let assigned = configured("ROBOTUTOR_ASSIGNMENTS", state.join("assigned"));
        let port = env::var("ROBOTUTOR_PORT")
            .ok()
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(if cfg!(windows) { 47322 } else { 47321 });
        let rt = Self {
            root,
            tutor,
            personal,
            assigned,
            state,
            port,
            courses,
            testing: false,
        };
        let db = configured("ROBOTUTOR_LMS_STATE", rt.state.join("lms"));
        for dir in [
            &rt.state,
            &rt.personal,
            &rt.assigned,
            &db,
            &rt.state.join("course-webview"),
        ] {
            rt.secure(dir)?;
        }
        if !rt.healthy() {
            let log = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(db.join("service.log"))?;
            let mut child = command(service)
                .env("ROBOTUTOR_PORT", port.to_string())
                .arg(db.join("courses.sqlite3"))
                .arg(rt.root.join("web"))
                .stdin(Stdio::null())
                .stderr(log.try_clone()?)
                .stdout(log)
                .spawn()?;
            let start = Instant::now();
            while !rt.healthy() {
                if child.try_wait()?.is_some() || start.elapsed() > Duration::from_secs(10) {
                    return Err("Local course service did not start".into());
                }
                thread::sleep(Duration::from_millis(80));
            }
        }
        Ok(rt)
    }
    fn healthy(&self) -> bool {
        command("curl")
            .args([
                "-q",
                "--silent",
                "--fail",
                "--noproxy",
                "*",
                "--max-time",
                "1",
            ])
            .arg(format!("http://127.0.0.1:{}/health", self.port))
            .output()
            .is_ok_and(|o| o.status.success() && o.stdout == b"robotutor-lms")
    }
    pub fn secure(&self, path: &Path) -> Result<()> {
        let res = command(&self.tutor)
            .arg("--init-state-dir")
            .arg(path)
            .output()
            .map_err(|e| format!("Could not run the tutor to verify workspace permissions: {e}"))?;
        if !res.status.success() {
            return Err(String::from_utf8_lossy(&res.stderr).into_owned().into());
        }
        Ok(())
    }
    pub fn api(&self, body: Value) -> Result<Value> {
        Ok(crate::lms_client::post(self.port, &body)?)
    }
    fn learner_state(&self, body: Value) -> Result<Value> {
        if self.testing {
            Ok(json!({"assignments":[]}))
        } else {
            self.api(body)
        }
    }
    pub fn profile(&self, preset: &str, reset: bool) -> Result<Self> {
        if !["new", "known", "returning"].contains(&preset) {
            return Err("Unknown test learner".into());
        }
        let root = configured("ROBOTUTOR_TEST_HOME", self.state.join("test-learners")).join(preset);
        self.secure(&root)?;
        // Windows LockFileEx needs a handle with ordinary read/write access.
        let lock = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(root.join(".lock"))?;
        lock.lock()?;
        let active = root.join("active");
        let gen = if active.is_file() && !reset {
            uuid::Uuid::parse_str(fs::read_to_string(&active)?.trim())?
        } else {
            let id = uuid::Uuid::new_v4();
            let dir = root.join(id.to_string());
            self.secure(&dir)?;
            let o = command(&self.tutor)
                .args(["--seed-test-learner", preset])
                .arg(dir.join("personal"))
                .output()?;
            if !o.status.success() {
                return Err(String::from_utf8_lossy(&o.stderr).into_owned().into());
            }
            self.secure(&dir.join("assigned"))?;
            let mut ptr = NamedTempFile::new_in(&root)?;
            writeln!(ptr, "{id}")?;
            ptr.as_file().sync_all()?;
            ptr.persist(active)?;
            id
        };
        let dir = root.join(gen.to_string());
        let mut rt = self.clone();
        rt.personal = dir.join("personal");
        rt.assigned = dir.join("assigned");
        rt.testing = true;
        Ok(rt)
    }
    pub fn choices(&self) -> Result<Vec<(String, String)>> {
        let mut visits = Vec::new();
        for entry in fs::read_dir(&self.personal)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|s| s == "jsonl")
            {
                visits.push((
                    entry.metadata()?.modified()?,
                    entry.file_name().to_string_lossy().into_owned(),
                ));
            }
        }
        visits.sort_by(|a, b| b.cmp(a));
        let mut choices: Vec<_> = visits
            .into_iter()
            .map(|(_, k)| {
                (
                    k.clone(),
                    format!("Personal · {}", k.trim_end_matches(".jsonl")),
                )
            })
            .collect();
        if choices.is_empty() {
            let day = chrono::Local::now().format("%Y-%m-%d");
            choices.push((format!("{day}.jsonl"), format!("Personal · {day}")));
        }
        if let Ok(state) = self.learner_state(json!({"action":"state"})) {
            for a in state["assignments"].as_array().into_iter().flatten() {
                let id = a["id"].as_u64().ok_or("Invalid assignment")?;
                let dir = self.assigned.join(id.to_string());
                self.secure(&dir)?;
                save(&dir.join("assignment.json"), a, false)?;
            }
        }
        for e in fs::read_dir(&self.assigned)? {
            let e = e?;
            let id = e.file_name().to_string_lossy().to_string();
            if e.file_type()?.is_dir() && id.parse::<u64>().is_ok() {
                let a = read(&e.path().join("assignment.json"))?;
                choices.push((
                    id,
                    format!("{} · {}", s(&a["activity"]["title"]), s(&a["status"])),
                ));
            }
        }
        Ok(choices)
    }
    pub fn visit(&self, key: &str) -> Result<(PathBuf, String)> {
        if let Ok(id) = key.parse::<u64>() {
            let dir = self.assigned.join(id.to_string());
            self.secure(&dir)?;
            let _ = self.api(json!({"action":"study","id":id}));
            let a = read(&dir.join("assignment.json"))?;
            let pinned = json!({"id":a["id"],"version":a["version"],"activity":a["activity"]});
            let binding = dir.join("lesson.assignment.json");
            if binding.exists() {
                if read(&binding)? != pinned {
                    return Err("Assignment differs from its saved binding".into());
                }
            } else {
                save(&binding, &pinned, true)?;
            }
            Ok((dir, "lesson.jsonl".into()))
        } else if Path::new(key).file_name().and_then(|s| s.to_str()) == Some(key)
            && key.ends_with(".jsonl")
        {
            Ok((self.personal.clone(), key.into()))
        } else {
            Err("Invalid lesson selection".into())
        }
    }
}
pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
pub struct Tutor {
    child: Child,
    input: Option<ChildStdin>,
    pub output: Receiver<String>,
}
impl Tutor {
    pub fn start(rt: &Runtime, folder: &Path, visit: &str) -> Result<Self> {
        let mut cmd = command(&rt.tutor);
        if env::var("ROBOTUTOR_ASTRA_REVIEW").as_deref() == Ok("1") {
            cmd.arg("--astra");
        }
        let mut child = cmd
            .env("ROBOTUTOR_DESKTOP", "1")
            .env("ROBOTUTOR_NATIVE", "1")
            .env("ROBOTUTOR_PORT", rt.port.to_string())
            .env("ROBOTUTOR_WORKSPACE", &rt.personal)
            .env("ROBOTUTOR_ASSIGNMENTS", &rt.assigned)
            .args(["--workspace"])
            .arg(folder)
            .arg("--session")
            .arg(visit)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take();
        let (tx, output) = mpsc::channel();
        for stream in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn io::Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stream)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            child,
            input,
            output,
        })
    }
    pub fn write(&mut self, v: &Value) -> Result<()> {
        let stdin = self.input.as_mut().ok_or("Tutor disconnected")?;
        serde_json::to_writer(&mut *stdin, v)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }
}
impl Drop for Tutor {
    fn drop(&mut self) {
        self.input.take();
        let start = Instant::now();
        while matches!(self.child.try_wait(), Ok(None)) {
            if start.elapsed() > Duration::from_secs(2) {
                let _ = self.child.kill();
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.wait();
    }
}
