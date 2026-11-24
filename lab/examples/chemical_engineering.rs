mod goal_program;

fn main() -> std::process::ExitCode {
    goal_program::main_for(
        "chemical-engineering",
        include_str!("../fixtures/goals/chemical-engineering.json"),
    )
}
