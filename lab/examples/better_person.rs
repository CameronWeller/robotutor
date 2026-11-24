mod goal_program;

fn main() -> std::process::ExitCode {
    goal_program::main_for(
        "better-person",
        include_str!("../fixtures/goals/better-person.json"),
    )
}
