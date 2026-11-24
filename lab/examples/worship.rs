mod goal_program;

fn main() -> std::process::ExitCode {
    goal_program::main_for("worship", include_str!("../fixtures/goals/worship.json"))
}
