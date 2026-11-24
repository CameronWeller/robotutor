mod goal_program;
#[path = "writer/meaning.rs"]
mod meaning;

fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--meaning-session") {
        return match meaning::run() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(reason) => {
                eprintln!("{reason}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    goal_program::main_for("writer", include_str!("../fixtures/goals/writer.json"))
}
