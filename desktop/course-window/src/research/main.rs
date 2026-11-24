#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
#[cfg(windows)]
mod browser;
mod google;
mod notebook;
#[cfg(unix)]
mod portable;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("robotutor-research {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    #[cfg(windows)]
    return browser::run();
    #[cfg(unix)]
    portable::run()
}
