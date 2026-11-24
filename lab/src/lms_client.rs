//! Local learner API shared by the terminal and native desktop.
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
};
pub fn post(port: u16, body: &Value) -> Result<Value, String> {
    let url = format!("http://127.0.0.1:{port}/api");
    let mut cmd = Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .args([
            "-q",
            "--silent",
            "--fail",
            "--noproxy",
            "*",
            "--max-time",
            "3",
            "--header",
            "Content-Type: application/json",
            "--header",
            "X-Profile: learner",
            "--data-binary",
            "@-",
            &url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Local LMS unavailable.")?;
    let write = child
        .stdin
        .take()
        .unwrap()
        .write_all(body.to_string().as_bytes());
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if write.is_err() || !output.status.success() {
        return Err("The local LMS did not acknowledge the request.".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "Invalid LMS response.".into())
}
