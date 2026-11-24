#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
#[cfg(windows)]
mod browser;
#[cfg(windows)]
mod google;
#[cfg(windows)]
mod notebook;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("robotutor-research {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    #[cfg(windows)]
    return browser::run();
    #[cfg(not(windows))]
    Err("The research browser requires Windows WebView2.".into())
}
