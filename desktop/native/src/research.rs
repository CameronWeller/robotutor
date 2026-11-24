use crate::runtime::{command, Result, Runtime};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    env,
    path::{Path, PathBuf},
    process::{Child, Stdio},
};

fn workspace(folder: &Path, visit: &str) -> PathBuf {
    folder.join(".research").join(visit)
}

#[derive(Default)]
pub struct Window(Option<Child>);
impl Window {
    pub fn launch(&mut self, rt: &Runtime, folder: &Path, visit: &str) -> Result<()> {
        if let Some(child) = &mut self.0 {
            if child.try_wait()?.is_none() {
                return Err("The research browser for this lesson is already open.".into());
            }
        }
        let dir = workspace(folder, visit);
        rt.secure(&dir)?;
        let binary = env::current_exe()?.with_file_name(format!("robotutor-research{}", env::consts::EXE_SUFFIX));
        if !binary.is_file() {
            return Err("Build the research browser using the Robotutor desktop launcher.".into());
        }
        self.0 = Some(
            command(binary)
                .arg(dir)
                .arg("--parent-pipe")
                .stdin(Stdio::piped())
                .spawn()?,
        );
        Ok(())
    }
    pub fn finished_with_error(&mut self) -> bool {
        let Some(child) = self.0.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => false,
            Ok(Some(status)) => {
                self.0.take();
                !status.success()
            }
            Err(_) => {
                self.close();
                true
            }
        }
    }
    pub fn close(&mut self) {
        if let Some(mut child) = self.0.take() {
            // Give WebView2 time to close its private profile after the parent pipe closes.
            drop(child.stdin.take());
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                if !matches!(child.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn read_resources(folder: &Path, visit: &str) -> Result<Vec<Value>> {
    let path = workspace(folder, visit).join("sources.sqlite3");
    if !path.is_file() {
        return Ok(vec![]);
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement=db.prepare("SELECT evidence FROM research WHERE minute >= unixepoch()/60-43200 ORDER BY minute DESC,id DESC LIMIT 200")?;
    let mut resources = vec![];
    for entry in statement.query_map([], |r| r.get::<_, String>(0))? {
        resources.push(serde_json::from_str(&entry?)?);
    }
    Ok(resources)
}
