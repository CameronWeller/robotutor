#[path = "../examples/goal_program/chat.rs"]
mod chat;

use serde_json::Value;
use std::io::Cursor;
use tutor_lab::curriculum::goal_rehearsal::Options;

const WRITER: &str = include_str!("../fixtures/goals/writer.json");
const PINS: &str = include_str!("../fixtures/goals/reviews.json");

fn talk(input: &str, course: Option<chat::Course<'_>>, start: chat::Start) -> String {
    let mut output = Vec::new();
    chat::session(
        &mut Cursor::new(input.as_bytes()),
        &mut output,
        course,
        start,
        None,
    )
    .unwrap();
    String::from_utf8(output).unwrap()
}

fn writer_course(input: &str, start: chat::Start) -> String {
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let options = Options::default();
    let course = chat::Course {
        fixture: WRITER,
        pin: pins["writer"].as_str().unwrap(),
        options: &options,
    };
    talk(input, Some(course), start)
}

#[test]
fn writer_can_park_an_exercise_in_their_own_words_and_resume_the_same_question() {
    let fixture: Value = serde_json::from_str(WRITER).unwrap();
    let first = &fixture["first_pack"];
    let check = first["check"].as_str().unwrap();
    let later_check = fixture["return_pack"]["check"].as_str().unwrap();
    let published_answer = first["attempts"]["demonstrated"]["response"]
        .as_str()
        .unwrap();
    let input = format!(
        "Can we work on my own writing instead?\nvoice\nUnder the pier it was quiet.\nI counted the silver rivets.\n.end\n/resume\n{published_answer}\n/quit\n"
    );
    let output = writer_course(&input, chat::Start::Course);
    let (_, after_opening) = output
        .split_once(check)
        .expect("original question was offered");
    let (_, after_coaching) = after_opening
        .split_once("I counted the silver rivets.")
        .expect("coaching was grounded in the learner's passage");
    let (_, after_resume) = after_coaching
        .split_once(check)
        .expect("same question on resume");
    assert!(after_resume.contains("Guided practice"));
    assert!(!after_resume.contains(
        first["attempts"]["demonstrated"]["feedback"]
            .as_str()
            .unwrap()
    ));
    assert!(!output.contains(later_check));
}

#[test]
fn direct_workshop_comments_on_each_submitted_revision_and_keeps_export_opt_in() {
    let output = talk(
        "clarity\nThe festival opened.\nThey were watching the pelicans.\n.end\n/revise\nThe parade paused.\nIt was spilling across the quay.\n.end\n/quit\n",
        None,
        chat::Start::Workshop,
    );
    let (_, revised) = output
        .split_once("They were watching the pelicans.")
        .expect("feedback incorporated the second line of the original draft");
    assert!(revised.contains("It was spilling across the quay."));
    assert!(!output.contains("\"messages\""));
}

#[test]
fn ending_during_a_paste_never_creates_an_attempt_or_a_return_lesson() {
    let fixture: Value = serde_json::from_str(WRITER).unwrap();
    let first = &fixture["first_pack"];
    for input in [
        "",
        "/work\nclarity\n.end\n",
        "/work\nrhythm\nI have half a line",
    ] {
        let output = writer_course(input, chat::Start::Course);
        assert!(output.contains(first["check"].as_str().unwrap()));
        for branch in ["demonstrated", "revisit", "unobserved"] {
            assert!(!output.contains(first["attempts"][branch]["feedback"].as_str().unwrap()));
        }
        assert!(!output.contains(fixture["return_pack"]["check"].as_str().unwrap()));
    }
}

#[test]
fn a_workshop_started_from_an_already_displayed_prompt_does_not_repeat_the_question() {
    let fixture: Value = serde_json::from_str(WRITER).unwrap();
    let output = writer_course(
        "rhythm\nOne boat.\nThen the entire harbor answered.\n.end\n/quit\n",
        chat::Start::Switch,
    );
    assert!(output.contains("Local rhythm exercise"));
    assert!(!output.contains(fixture["first_pack"]["check"].as_str().unwrap()));
    assert!(!output.contains(fixture["return_pack"]["check"].as_str().unwrap()));
}

#[test]
fn explicitly_exported_request_and_multiline_reply_apply_only_to_the_current_draft() {
    let draft = "Clouds crossed the balcony.\nI kept counting.";
    let input = format!("voice\n{draft}\n.end\n/request test-model\n");
    let exported = talk(&input, None, chat::Start::Workshop);
    let request = exported
        .lines()
        .find_map(|line| {
            let value: Value = serde_json::from_str(line).ok()?;
            (value["model"] == "test-model").then_some(value)
        })
        .expect("explicit model request printed as JSON");
    let packet: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(packet["draft"], draft);
    assert_eq!(packet["focus"], "voice");
    let suggestion = "Try ending the paragraph on the counting.";
    let reply = serde_json::json!({
        "job": packet["job"],
        "critique": {
            "quote": "I kept counting.",
            "suggestion": suggestion,
            "question": "Whose count should the reader hear?"
        }
    });
    let capture = serde_json::to_string_pretty(&reply).unwrap();
    let input = format!("{input}/replay\n{capture}\n.end\n/revise\nThe clouds cleared.\n.end\n/replay\n{capture}\n.end\n/quit\n");
    let output = talk(&input, None, chat::Start::Workshop);
    assert_eq!(
        output.matches(suggestion).count(),
        1,
        "only the first revision can display this advice"
    );
    assert!(output.contains("does not match this draft revision and focus"));
}

#[test]
fn missing_material_and_project_notes_lead_to_private_work_without_a_course_check() {
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let options = Options {
        unit: Some(103),
        ..Options::default()
    };
    let course = chat::Course {
        fixture: WRITER,
        pin: pins["writer"].as_str().unwrap(),
        options: &options,
    };
    let output = talk(
        "Could we work on my project instead?\nnext-step\nI assume the hall has twelve tables.\n.end\n/resume\n/quit\n",
        Some(course), chat::Start::Course,
    );
    assert!(output.contains("select and authorize a passage"));
    assert!(output.contains("Local next-step exercise"));
    assert!(output.contains("“I assume the hall has twelve tables.”"));
    assert_eq!(output.matches("Unit 103 ·").count(), 2);
    assert!(!output.contains("Independent exercise"));
}

#[test]
fn ambiguous_request_can_be_clarified_and_canceling_keeps_the_original_opening() {
    let fixture: Value = serde_json::from_str(WRITER).unwrap();
    let check = fixture["first_pack"]["check"].as_str().unwrap();
    let output = writer_course(
        "Could I show you something else?\n/work\nvoice\n.cancel\n/answer Can you critique my poem?\n",
        chat::Start::Course,
    );
    assert!(output.contains("Would you like coaching on your own passage?"));
    assert!(output.contains("The question above is still open."));
    assert_eq!(output.matches(check).count(), 1);
    assert!(!output.contains("as guided practice"));
    assert!(!output.contains("Local voice exercise"));
}

mod course_reply_tests {
    use super::chat::close_course_question;
    use super::{Options, Value, PINS, WRITER};
    use serde_json::json;
    use std::io::{BufRead, Cursor};
    use tutor_lab::curriculum::live_turn;
    use tutor_lab::openrouter::{Role, DEFAULT_JEV, DEFAULT_LUNA};

    fn prompt() -> live_turn::Coaching {
        let pins: Value = serde_json::from_str(PINS).unwrap();
        live_turn::coach(WRITER, pins["writer"].as_str().unwrap(),
            &Options::default(), "At the entry.", false, (DEFAULT_JEV,DEFAULT_LUNA), |role,req| {
                Ok(match role {
                    Role::Jev => {
                        let key = req["questions"].as_object().unwrap().keys().next().unwrap();
                        json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":"clarify"}}})
                    }
                    Role::Luna => {
                        let turn: Value = serde_json::from_str(req["messages"][1]["content"].as_str().unwrap()).unwrap();
                        let text = json!({"job":turn["job"],"quote":"the entry","question":"Which entry do you mean?"});
                        json!({"model":DEFAULT_LUNA,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":text.to_string()}}]})
                    }
                })
            }).unwrap().coaching.unwrap()
    }

    #[test]
    fn skipping_or_ending_the_displayed_question_makes_no_further_calls() {
        for input in ["", "/quit\n", "\n", "/unknown\n/quit\n"] {
            let mut output = Vec::new();
            close_course_question(
                prompt(),
                &mut Cursor::new(input),
                &mut output,
                None,
                |_, _, _| panic!("no reply was submitted"),
            )
            .unwrap();
            assert!(String::from_utf8(output)
                .unwrap()
                .contains("Which entry do you mean?"));
        }
    }

    #[test]
    fn the_explicit_reply_closes_once_and_leaves_the_following_line_unread() {
        let mut input = Cursor::new("/reply Can we work on my writing?\nThis is another answer.\n");
        let mut output = Vec::new();
        let mut calls = 0;
        close_course_question(prompt(), &mut input, &mut output, None, |_,role,req|{
            calls+=1;
            assert_eq!(role,Role::Jev);
            assert_eq!(req["state"]["coaching_exchange"]["reply"],"Can we work on my writing?");
            let key=req["questions"].as_object().unwrap().keys().next().unwrap();
            Ok(json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":"local_only"}}}))
        }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            input.lines().next().transpose().unwrap().as_deref(),
            Some("This is another answer.")
        );
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("This coaching turn is complete."));
    }
    #[test]
    fn switching_after_submission_enters_the_workshop_without_sending_a_reflection() {
        for request in ["/work", "Can we work on my own writing instead?"] {
            let mut output = Vec::new();
            let script = format!("{request}\n/resume\n");
            close_course_question(
                prompt(),
                &mut Cursor::new(script),
                &mut output,
                None,
                |_, _, _| panic!("switch reached course inference"),
            )
            .unwrap();
            let visible = String::from_utf8(output).unwrap();
            assert!(visible.contains("What would you like to explore in your draft?"));
            assert!(visible.contains("Your course exercise was already submitted."));
            assert!(!visible.contains("Independent exercise"));
        }
    }
}

#[test]
fn help_before_submission_selects_released_teaching_and_preserves_the_grade_boundary() {
    use serde_json::json;
    use tutor_lab::openrouter::{Role, DEFAULT_JEV};
    use tutor_lab::workshop::course::CourseTurn;

    let pins: Value = serde_json::from_str(PINS).unwrap();
    let fixture: Value = serde_json::from_str(WRITER).unwrap();
    let options = Options::default();
    let course = chat::Course {
        fixture: WRITER,
        pin: pins["writer"].as_str().unwrap(),
        options: &options,
    };
    for (input, route, guided) in [
        ("Can you show me a worked example?", "worked_example", true),
        ("Example?", "worked_example", true),
        (
            "Where in the brief does it describe the entrance?",
            "source_passage",
            true,
        ),
        ("I'm stuck.", "clarify_help", false),
        (
            "/help Which words support naming the entrance?",
            "source_passage",
            true,
        ),
        ("/help I'm stuck", "clarify_help", false),
        ("/help I'll try on my own", "local_only", false),
    ] {
        let mut lane = CourseTurn::opened();
        let mut output = Vec::new();
        let mut calls = 0;
        let help = chat::course_help_request(input).unwrap();
        chat::show_course_help(
            &course,
            &mut lane,
            input,
            &mut output,
            Some(DEFAULT_JEV),
            |_, role, request| {
                calls += 1;
                assert_eq!(role, Role::Jev);
                let state = &request["state"];
                assert_eq!(state["help_request"], help);
                assert!(
                    state.get("submitted_answer").is_none()
                        && state.get("local_feedback").is_none()
                );
                let questions = request["questions"].as_object().unwrap();
                let (key, question) = questions.iter().next().unwrap();
                let choices = question["criteria"].as_object().unwrap();
                assert!(!choices.contains_key("revise"));
                for available in [
                    "worked_example",
                    "source_passage",
                    "clarify_help",
                    "local_only",
                ] {
                    assert!(choices.contains_key(available));
                }
                Ok(json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":route}}}))
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(lane.practice_only(), guided);
        let rendered = String::from_utf8(output).unwrap();
        assert!(!rendered.contains("would need a reviewer"));
        assert_eq!(
            rendered.contains(fixture["first_pack"]["check"].as_str().unwrap()),
            guided
        );
        if route == "worked_example" {
            assert!(rendered.contains(fixture["first_pack"]["worked"].as_str().unwrap()));
        }
        if route == "source_passage" {
            assert!(rendered.contains(fixture["first_pack"]["excerpt"].as_str().unwrap()));
        }
    }
    assert_eq!(
        chat::course_help_request("/answer Can you show me a worked example?"),
        None
    );
    assert_eq!(
        chat::course_help_request("My example shows an open window."),
        None
    );
    assert_eq!(
        chat::course_help_request("Could I show you something else?"),
        None
    );
}

#[test]
fn unsubmitted_help_never_promotes_a_stale_answer_or_sends_ambiguous_commands() {
    use tutor_lab::openrouter::DEFAULT_JEV;
    use tutor_lab::workshop::course::CourseTurn;
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let options = Options::default();
    let course = chat::Course {
        fixture: WRITER,
        pin: pins["writer"].as_str().unwrap(),
        options: &options,
    };
    let mut lane = CourseTurn::opened();
    let mut out = Vec::new();
    chat::show_course_help(
        &course,
        &mut lane,
        "",
        &mut out,
        Some(DEFAULT_JEV),
        |_, _, _| panic!("empty help requested a model"),
    )
    .unwrap();
    let mut calls = 0;
    chat::show_course_help(&course,&mut lane,"How do I begin?",&mut out,Some(DEFAULT_JEV),|_,_,_|{
        calls+=1;
        Ok(serde_json::json!({"model":DEFAULT_JEV,"answers":{"prior":{"type":"choice","choice":"worked_example"}}}))
    }).unwrap();
    assert_eq!(calls, 1);
    assert!(!lane.practice_only());
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("Your exercise is still open."));
    assert!(!output.contains("Try the same exercise"));
}

#[test]
fn choosing_the_named_card_after_clarification_uses_no_more_inference() {
    use serde_json::json;
    use tutor_lab::openrouter::{Role, DEFAULT_JEV};
    use tutor_lab::workshop::course::CourseTurn;
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let fixture: Value = serde_json::from_str(WRITER).unwrap();
    let options = Options::default();
    let course = chat::Course {
        fixture: WRITER,
        pin: pins["writer"].as_str().unwrap(),
        options: &options,
    };
    for (command, field) in [("/help example", "worked"), ("/help source", "excerpt")] {
        let mut lane = CourseTurn::opened();
        let mut output = Vec::new();
        let mut calls = 0;
        for input in ["/help I'm stuck", command] {
            chat::show_course_help(&course,&mut lane,input,&mut output,Some(DEFAULT_JEV),|_,role,req|{
                calls += 1;
                assert_eq!(role,Role::Jev);
                assert_eq!(req["state"]["help_request"], "I'm stuck", "the explicit choice must execute locally");
                let key=req["questions"].as_object().unwrap().keys().next().unwrap();
                Ok(json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":"clarify_help"}}}))
            }).unwrap();
        }
        assert_eq!(calls, 1);
        assert!(lane.practice_only());
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Both open locally."));
        assert!(output.contains(fixture["first_pack"][field].as_str().unwrap()));

        let offline = writer_course(
            &format!("{command}\n/answer My first draft.\n"),
            chat::Start::Course,
        );
        assert!(offline.contains("Try the same exercise as guided practice."));
        assert!(offline.contains("Guided practice\nYou can keep this exercise private."));
    }
    let broken = chat::Course {
        fixture: WRITER,
        pin: "wrong",
        options: &options,
    };
    let mut lane = CourseTurn::opened();
    let mut output = Vec::new();
    chat::show_course_help(
        &broken,
        &mut lane,
        "/help example",
        &mut output,
        Some(DEFAULT_JEV),
        |_, _, _| panic!("pinned edition was invalid"),
    )
    .unwrap();
    assert!(!lane.practice_only());
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains(fixture["first_pack"]["worked"].as_str().unwrap()));
}

#[test]
fn only_the_exact_commands_bypass_the_live_help_menu() {
    use tutor_lab::openrouter::DEFAULT_JEV;
    use tutor_lab::workshop::course::CourseTurn;
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let options = Options::default();
    let course = chat::Course {
        fixture: WRITER,
        pin: pins["writer"].as_str().unwrap(),
        options: &options,
    };
    for (input, expected) in [
        ("/help source please", "source please"),
        ("/help /help example", "/help example"),
        ("Source?", "Source?"),
    ] {
        let mut calls = 0;
        chat::show_course_help(&course,&mut CourseTurn::opened(),input,&mut Vec::new(),Some(DEFAULT_JEV),|_,_,req|{
            calls+=1;
            assert_eq!(req["state"]["help_request"],expected);
            let key=req["questions"].as_object().unwrap().keys().next().unwrap();
            Ok(serde_json::json!({"model":DEFAULT_JEV,"answers":{(key):{"type":"choice","choice":"local_only"}}}))
        }).unwrap();
        assert_eq!(calls, 1);
    }
}

#[test]
fn closing_example_ends_without_guided_prompt_or_later_assessment() {
    const CHEM: &str = include_str!("../fixtures/goals/chemical-engineering.json");
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let options = Options::default();
    let course = chat::Course {
        fixture: CHEM,
        pin: pins["chemical-engineering"].as_str().unwrap(),
        options: &options,
    };
    let out = talk(
        "/example-and-stop\n/answer +10 kg\n",
        Some(course),
        chat::Start::Course,
    );
    let pack: Value = serde_json::from_str(CHEM).unwrap();
    let first = &pack["first_pack"];
    let example = first["worked"].as_str().unwrap();
    let question = first["check"].as_str().unwrap();
    assert!(out.contains("/example-and-stop to finish with one example"));
    assert_eq!(out.matches(question).count(), 1);
    assert!(out.ends_with(&format!("Worked example\n{example}\nWe can stop here.\n")));
    assert!(!out.contains("Your number and unit match the requested quantity."));
    // The lower-level session can receive a selected unit. A withdrawn later pack
    // yields a preview with no active independent question or permitted replay.
    let withheld = Options {
        unit: pack["return_pack"]["unit"].as_u64().map(|u| u as u32),
        withdraw_return: true,
        ..Options::default()
    };
    let held = chat::Course {
        fixture: CHEM,
        pin: pins["chemical-engineering"].as_str().unwrap(),
        options: &withheld,
    };
    let held_output = talk(
        "/example-and-stop\n/answer +10 kg\n",
        Some(held),
        chat::Start::Course,
    );
    assert!(held_output.ends_with(
        "There is no open exercise with an available worked example. We can stop here.\n"
    ));
    assert!(!held_output.contains(pack["return_pack"]["worked"].as_str().unwrap()));
    let broken = chat::Course {
        fixture: CHEM,
        pin: "outdated release",
        options: &options,
    };
    let mut output = Vec::new();
    chat::final_example(&broken, false, &mut output).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "The worked example is unavailable for this opening. We can stop here.\n"
    );
}
