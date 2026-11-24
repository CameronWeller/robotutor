//! Opt-in replay of three fictional first drafts through Luna's actual review and repair.
use super::*;

#[test]
#[ignore = "makes live Luna requests with fictional lesson material"]
fn review_and_repair_question_demands() {
    let config = Config::from_env().unwrap();
    let quote = "Visitors may enter after noon unless the keeper has raised the blue flag.";
    let cases = [
        (
            "Find out only whether I can choose the horizontal axis for pedal turns versus wheel turns.",
            "Choose the input quantity for a graph.",
            "Time belongs on the horizontal axis when we track temperature as time passes.",
            "Which quantity belongs on the horizontal axis for wheel turns as pedal turns increase? Plot three points and calculate the slope.",
            true,
        ),
        (
            "I want to predict a graph value and justify my prediction using its trend.",
            "Use a linear graph trend to support a prediction.",
            "A plant gains 2 cm each day. Starting at 5 cm, the trend predicts 7 cm after one day.",
            "A graph of distance against time has points (1, 4), (2, 8), and (3, 12). Predict the distance at time 5 if the trend continues, and justify your prediction from the graph.",
            false,
        ),
        (
            "Help me interpret the exception in this exact fictional park rule. Include the rule in my exercise so I can read it there: Visitors may enter after noon unless the keeper has raised the blue flag.",
            "Apply an exception in the supplied rule.",
            "A library allows snacks except in the archive. A visitor in the archive cannot eat a snack there.",
            "At one o’clock the blue flag is raised. Under the posted rule, may the visitors enter?",
            true,
        ),
    ];
    for (goal, objective, worked, exercise, needs_revision) in cases {
        let mut lesson = Lesson::default();
        let mut first = Some(json!({"turn": {
            "move":"teach", "objective":objective, "outline":[objective,"Compare two cases"],
            "explanation":"Use the information in the case to make your decision.",
            "worked_example":worked, "exercise":exercise, "next_objective":"Compare two cases"
        }}));
        let mut verdicts = Vec::new();
        let delivered = lesson
            .respond(Some(goal), &mut |packet, schema| {
                if packet["task"] == "route" {
                    return Ok(route("start", ""));
                }
                if packet["task"] == "deliver" {
                    if let Some(draft) = first.take() {
                        return Ok(draft);
                    }
                }
                let receipt = config.complete(Role::Luna, &request(&[], packet, schema)?)?;
                let response: Value =
                    serde_json::from_str(assistant_text(&receipt)?).map_err(|e| e.to_string())?;
                if packet["task"] == "review" {
                    eprintln!("{}: {}", response["verdict"], response["reason"]);
                    verdicts.push(response["verdict"].clone());
                }
                Ok(response)
            })
            .unwrap();
        eprintln!("{objective}\n{delivered}");
        assert_eq!(
            verdicts.first().unwrap(),
            if needs_revision { "revise" } else { "accept" }
        );
        assert_eq!(verdicts.last().unwrap(), "accept");
        if !needs_revision {
            assert_eq!(lesson.exercise, exercise);
        } else if goal.contains(quote) {
            assert!(lesson.exercise.contains(quote));
        } else {
            assert!(lesson.exercise.to_lowercase().contains("horizontal"));
            assert!(!lesson.exercise.to_lowercase().contains("slope"));
        }
    }
}
