use super::{Config, DEFAULT_JEV, DEFAULT_LUNA};
use std::env;
use std::io::IsTerminal;
#[cfg(not(any(windows, target_os = "macos")))]
use std::io::Write;
#[cfg(not(any(windows, target_os = "macos")))]
use std::process::{Command, Output, Stdio};

#[cfg(not(any(windows, target_os = "macos")))]
const ATTRIBUTES: [&str; 4] = ["service", "robotutor", "provider", "openrouter"];

#[cfg(not(any(windows, target_os = "macos")))]
fn with_input(cmd: &mut Command, value: &[u8]) -> Result<Output, String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Unable to start credential helper.")?;
    let sent = child.stdin.take().unwrap().write_all(value);
    let output = child
        .wait_with_output()
        .map_err(|_| "Credential helper did not finish.")?;
    sent.map_err(|_| "Unable to send credential to helper.")?;
    Ok(output)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn lookup() -> Result<Option<String>, String> {
    let out = Command::new("secret-tool")
        .arg("lookup")
        .args(ATTRIBUTES)
        .output()
        .map_err(|_| "Unable to read the login keyring through secret-tool.")?;
    if out.status.success() {
        let value = String::from_utf8(out.stdout).map_err(|_| "Invalid credential in keyring.")?;
        return Ok(Some(value.trim().into()));
    }
    if out.status.code() == Some(1) && out.stderr.is_empty() {
        return Ok(None);
    }
    Err("Unable to read the Robotutor credential from the login keyring.".into())
}

fn config(key: &str) -> Result<Config, String> {
    Config::new(key.into(), DEFAULT_JEV.into(), DEFAULT_LUNA.into())
}

pub fn existing() -> Result<Option<Config>, String> {
    if let Ok(key) = env::var("OPENROUTER_API_KEY") {
        if !key.trim().is_empty() {
            return config(key.trim()).map(Some);
        }
    }
    lookup()?.as_deref().map(config).transpose()
}

pub fn connect() -> Result<Config, String> {
    if let Some(saved) = existing()? {
        return Ok(saved);
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "Start a terminal lesson to save an OpenRouter key, or set OPENROUTER_API_KEY.".into(),
        );
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        let key = rpassword::prompt_password("OpenRouter key to save for Robotutor: ")
            .map_err(|_| "Unable to open hidden key prompt.")?;
        let key = key.trim();
        let cfg = config(key)?;
        cfg.verify_key()?;
        entry()?
            .set_password(key)
            .map_err(|_| "Unable to save the Robotutor credential.")?;
        if lookup()?.as_deref() != Some(key) {
            return Err("Could not verify the saved Robotutor credential.".into());
        }
        Ok(cfg)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let entry = Command::new("systemd-ask-password")
            .args([
                "--timeout=0",
                "--echo=no",
                "OpenRouter key to save in your login keyring",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|c| c.wait_with_output())
            .map_err(|_| "Unable to open the hidden key prompt.")?;
        if !entry.status.success() {
            return Err("Key entry cancelled.".into());
        }
        let key = std::str::from_utf8(&entry.stdout)
            .map_err(|_| "Invalid OpenRouter credential.")?
            .trim();
        let connection = config(key)?;
        connection.verify_key()?;
        let stored = with_input(
            Command::new("secret-tool")
                .args(["store", "--label=Robotutor OpenRouter API key"])
                .args(ATTRIBUTES),
            key.as_bytes(),
        )?;
        if !stored.status.success() || lookup()?.as_deref() != Some(key) {
            return Err("Could not verify the saved OpenRouter key in the login keyring.".into());
        }
        println!("Saved the verified OpenRouter key in your login keyring.");
        Ok(connection)
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("Robotutor", "OpenRouter")
        .map_err(|_| "Unable to open the OS credential store.".into())
}
#[cfg(any(windows, target_os = "macos"))]
pub(super) fn lookup() -> Result<Option<String>, String> {
    match entry()?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => {
            Err("Unable to read the Robotutor credential from the OS credential store.".into())
        }
    }
}
