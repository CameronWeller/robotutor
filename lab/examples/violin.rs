mod goal_program;

fn main() -> std::process::ExitCode {
    goal_program::main_for("violin", include_str!("../fixtures/goals/violin.json"))
}
