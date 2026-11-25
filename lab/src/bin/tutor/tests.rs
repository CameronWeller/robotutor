use super::*;
use lesson::Lesson;
use session::{Event, Journal, Visit};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use tutor_lab::platform::PrivateOpen;

fn route(action: &str, evidence: &str) -> Value {
    json!({"action":action,"evidence":evidence,"note":"Review only the learner's submitted words.","target_stage":-1})
}
fn active() -> Lesson {
    let mut lesson = Lesson::default();
    lesson.selected = Some("washington-civil".into());
    lesson.objective = "Infer what the admission establishes.".into();
    lesson.exercise = "What follows from the admission and what remains open?".into();
    lesson
}
fn advanced(exercise: &str) -> Value {
    json!({"turn":{"move":"advance","bridge_and_instruction":"Here is the reasoning for the next skill.",
        "exercise":exercise,"next_objective":"Compare the record against a legal standard."}})
}
fn accepted() -> Value {
    json!({"verdict":"accept","reason":""})
}
fn play(
    lesson: &mut Lesson,
    text: Option<&str>,
    replies: Vec<Value>,
) -> Result<(String, Vec<Value>), String> {
    let mut packets = vec![];
    let mut replies = replies.into_iter();
    let answer = lesson.respond(text, &mut |packet, _| {
        packets.push(packet.clone());
        let mut v = replies.next().expect("unexpected model call");
        if v["turn"]["move"] == "teach" && v["turn"]["outline"].is_null() {
            v["turn"]["outline"] = json!(["Opening skill", "Next skill"]);
        }
        Ok(v)
    })?;
    assert!(replies.next().is_none());
    Ok((answer, packets))
}
#[test]
fn partial_answer_retires_no_skill_and_only_quotes_the_learner() {
    let mut lesson = active();
    let coach = json!({"turn":{"move":"coach","feedback":"You identified delivery.","exercise":"Where does your colleague's conclusion exceed that admission?"}});
    let attempt = "They agreed on delivery.";
    let old = lesson.exercise.clone();
    let (_, calls) = play(
        &mut lesson,
        Some(attempt),
        vec![
            route("partial", "damage"),
            route("partial", "delivery"),
            coach,
            accepted(),
        ],
    )
    .unwrap();
    assert!(calls[1]["repair"].is_string());
    assert_eq!(calls[2]["open_exercise"], old);
    assert_eq!(calls[2]["route"]["evidence"], "delivery");
    assert_eq!(lesson.stage, 0);
    assert!(lesson.closed.is_empty());
    assert_eq!(lesson.exercise, old);
    assert!(lesson.active_step().contains("colleague"));
}
#[test]
fn probability_probe_keeps_the_original_task_and_marks_followup_as_guided() {
    let case: Value = serde_json::from_str(include_str!("../../../fixtures/conditional-probability-v1.json")).unwrap();
    let mut lesson = Lesson::default();
    lesson.topic = "Probability".into();
    lesson.objective = case["capability"].as_str().unwrap().into();
    lesson.exercise = case["initial"]["prompt"].as_str().unwrap().into();
    let answer = case["initial"]["answer"].as_str().unwrap();
    let coach = json!({"turn":{"move":"coach","feedback":"You counted the overlap.","exercise":case["probe"]["prompt"]}});
    play(&mut lesson, Some(answer), vec![route("partial", answer), coach, accepted()]).unwrap();
    assert_eq!(lesson.exercise, case["initial"]["prompt"]);
    assert_eq!(lesson.active_step(), case["probe"]["prompt"].as_str().unwrap());
    assert!(lesson.assisted);
    assert_eq!(lesson.prior_attempts, [answer]);
    assert!(!lesson.observation.as_ref().unwrap().assisted);
}

#[test]
fn reviewed_method_cue_marks_the_new_exercise_as_assisted() {
    let mut lesson = active();
    let mut draft = advanced("Which source distinguishes the accounts?");
    draft["turn"]["bridge_and_instruction"] = json!("Compare the timestamps on the two photographs.");
    let mut verdict = accepted();
    verdict["method_cue"] = json!("Compare the timestamps");
    play(&mut lesson, Some("Give me a harder one."), vec![route("harder", ""), draft, verdict]).unwrap();
    assert!(lesson.assisted);
}

#[test]
fn changing_nouns_is_rejected_and_skipping_never_awards_credit() {
    let mut lesson = active();
    let old = lesson.exercise.clone();
    let (answer,calls)=play(&mut lesson,Some("This is the same question again."),vec![
        route("harder",""),advanced("What do the two parties to a laptop delivery agree on?"),
        json!({"verdict":"revise","reason":"Same admitted-or-disputed classification in a new setting."}),
        advanced("Which source could distinguish the two accounts, and how?"),accepted()]).unwrap();
    assert!(calls[1]["target"].as_str().unwrap().contains("evidence"));
    assert!(calls[3]["revision"]["issue"].is_string());
    let assigned = material::card("washington-civil", 1).unwrap()["practice"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(answer.ends_with(&assigned));
    assert!(!calls[1].to_string().contains(&assigned));
    assert_eq!(calls[2]["draft"]["exercise"], assigned);
    assert!(lesson.challenge);
    assert_eq!(lesson.closed[0].exercise, old);
    assert!(lesson.closed[0].learner_evidence.is_empty());
    let (_, calls) = play(
        &mut lesson,
        Some("Where do we go from here?"),
        vec![
            route("next", ""),
            advanced("Does the record resolve a material fact?"),
            accepted(),
        ],
    )
    .unwrap();
    assert!(calls[1]["target"]
        .as_str()
        .unwrap()
        .contains("material fact"));
    assert_eq!(lesson.stage, 2);
}
#[test]
fn review_failure_keeps_the_delivered_exercise_available_for_retry() {
    let mut lesson = active();
    let before = serde_json::to_value(&lesson).unwrap();
    let bad =
        json!({"verdict":"revise","reason":"Answered the learner's question before assigning it."});
    assert!(play(
        &mut lesson,
        Some("/next"),
        vec![
            advanced("repeat"),
            bad.clone(),
            advanced("another repeat"),
            bad.clone(),
            advanced("still unsuitable"),
            bad
        ]
    )
    .is_err());
    assert_eq!(serde_json::to_value(lesson).unwrap(), before);
}
#[test]
fn questions_preserve_the_task_and_explanation_mode_survives_a_next_turn() {
    let mut lesson = active();
    let old = lesson.exercise.clone();
    let step = "Name one fact the witness could establish from firsthand observation.";
    let answer = json!({"turn":{"move":"reply","response":"A deposition is sworn testimony.","next_step":step}});
    let (visible, calls) = play(
        &mut lesson,
        Some("What are those?"),
        vec![route("question", ""), answer, accepted()],
    )
    .unwrap();
    assert!(visible.ends_with(step));
    assert_eq!(calls[1]["open_exercise"], old);
    assert_eq!(lesson.exercise, old);
    let explain = json!({"turn":{"move":"explain","objective":"Weigh evidence","explanation":"A photo can show condition.",
        "worked_example":"A dated photo brackets the damage.","next_step":"Trace the date on the example photo back to the delivery.","next_objective":"Reason about disputed evidence."}});
    play(
        &mut lesson,
        Some("Just explain for a while."),
        vec![route("explain_only", ""), explain.clone(), accepted()],
    )
    .unwrap();
    assert!(lesson.exercise.is_empty());
    let (visible, calls) = play(&mut lesson, Some("/next"), vec![explain, accepted()]).unwrap();
    assert!(visible.ends_with("Trace the date on the example photo back to the delivery."));
    assert_eq!(calls[0]["explanation_only"], true);
    assert!(lesson.exercise.is_empty());
}
#[test]
fn broad_goals_require_teaching_and_dont_assume_a_jurisdiction() {
    let mut lesson = Lesson::default();
    let teach = json!({"turn":{"move":"teach","objective":"Compute probability","explanation":"Count equally likely outcomes.",
        "worked_example":"A fair coin has two faces.","exercise":"What is the probability of rolling an even number on a fair die?","next_objective":"Condition on a clue."}});
    let (_, calls) = play(
        &mut lesson,
        Some("I want to learn probability"),
        vec![
            route("start", ""),
            json!({"turn":{"move":"reply","response":"What interests you?"}}),
            teach,
            accepted(),
        ],
    )
    .unwrap();
    assert!(calls[2]["repair"].is_string());
    assert!(calls[1]["reference_if_relevant"].is_null());
    assert!(calls[1]["available_cards"]
        .get("washington-civil")
        .is_none());
    let (_,calls)=play(&mut lesson,Some("What if I get sued in Washington State?"),vec![route("use_source_or_topic",""),
        json!({"turn":{"move":"read","card":"washington-civil"}}),
        json!({"turn":{"move":"teach","objective":"Read the admission","explanation":"Look at the admission.",
            "worked_example":"The witness admits arriving.","exercise":"What does the admission prove?","next_objective":"Weigh evidence."}}),accepted()]).unwrap();
    assert!(calls[2]["current_card"]["sources"]["CR 8"].is_string());
    assert_eq!(lesson.stage, 0);
    let answer = json!({"turn":{"move":"reply","response":"That could refer to a different jurisdiction.","next_step":"Name the court you want to study."}});
    let (_, calls) = play(
        &mut Lesson::default(),
        Some("Washington, D.C.?"),
        vec![route("question", ""), answer, accepted()],
    )
    .unwrap();
    assert!(calls[1].get("reference_if_relevant").is_none());
}
#[test]
fn reviewed_material_changes_task_and_excludes_simulated_answers() {
    for &(name, _) in material::CARDS {
        let initial = material::card(name, 0).unwrap();
        let next = material::card(name, 1).unwrap();
        assert_ne!(initial["objective"], next["objective"]);
        assert_ne!(initial["practice"], next["practice"]);
        for key in [
            "scope_reply",
            "first_pack",
            "return_pack",
            "attempt",
            "learner_answer",
            "steps",
        ] {
            assert!(initial.get(key).is_none());
            assert!(next.get(key).is_none());
        }
        assert!(material::card(name, 8).unwrap().get("practice").is_none());
    }
}
#[test]
fn only_selected_visible_turns_enter_the_bounded_request() {
    let history: Vec<_> = (0..22)
        .map(|i| (Some(format!("attempt {i}")), format!("reply {i}")))
        .collect();
    let body = request(&history, &json!({"task":"review"}), &json!({})).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 30);
    assert_eq!(messages[1]["content"], "attempt 8");
    assert_eq!(messages[28]["content"], "reply 21");
    assert!(body.get("tools").is_none());
    let fresh = request(&[], &json!({"task":"route"}), &json!({})).unwrap();
    assert_eq!(fresh["messages"].as_array().unwrap().len(), 2);
}
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn file() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "robotutor-visit-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    tutor_lab::platform::create_private_dir(&dir, false).unwrap();
    dir.join("lesson.jsonl")
}
#[test]
fn journal_recovers_pending_input_and_state_and_excludes_a_second_writer() {
    let path = file();
    let mut visit = Visit::default();
    let mut writer = Some(Journal::open(&path, &mut visit).unwrap());
    for e in [
        Event::Open {
            version: 1,
            selected: None,
        },
        Event::Learner {
            text: "first".into(),
        },
        Event::Delivered {
            judgment: None,
            text: "displayed".into(),
            lesson: Box::new(active()),
            sources: vec![],
        },
        Event::Learner {
            text: "second".into(),
        },
        Event::Failed {
            message: "request interrupted".into(),
        },
    ] {
        append(&mut writer, &mut visit, e).unwrap();
    }
    assert!(Journal::open(&path, &mut Visit::default()).is_err());
    assert!(tutor_lab::platform::private_handle(&fs::File::open(&path).unwrap()).unwrap());
    drop(writer);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"event\"")
        .unwrap();
    let mut restored = Visit::default();
    let recovered = Journal::open(&path, &mut restored).unwrap();
    drop(recovered);
    assert_eq!(restored.pending.as_deref(), Some("second"));
    assert_eq!(restored.history[0].1, "displayed");
    assert_eq!(restored.lesson.exercise, active().exercise);
    assert!(fs::read(&path).unwrap().ends_with(b"\n"));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn unrelated_private_file_is_preserved() {
    let path = file();
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open_private(&path)
        .unwrap()
        .write_all(b"private notes")
        .unwrap();
    assert!(Journal::open(&path, &mut Visit::default()).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"private notes");
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn content_review_only_receives_the_current_task_even_when_coaching_revises_the_plan() {
    for kind in ["coach", "advance", "guide"] {
        let packet = json!({"task":"review","target":"Compare two sources.",
            "working_plan":{"previous_next":"Design a follow-up investigation."},
            "draft":{"move":kind,"exercise":"Which account has independent support?",
                     "next_objective":"Design a follow-up investigation."}});
        let body = request(&[], &packet, &json!({})).unwrap();
        let wire: Value = serde_json::from_str(
            body["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(wire["target"], packet["target"]);
        assert_eq!(wire["draft"]["exercise"], packet["draft"]["exercise"]);
        assert!(wire.get("working_plan").is_none());
        assert!(!wire
            .to_string()
            .contains("Design a follow-up investigation."));
        assert!(packet["draft"]["next_objective"].is_string());
    }
}

#[test]
fn rejected_coaching_drafts_preserve_the_submission_and_deliver_a_step_in_the_open_case() {
    let mut lesson = active();
    lesson.next_objective = "Look for corroboration".into();
    let original = lesson.exercise.clone();
    let words = "I would look at the timing.";
    let draft = json!({"turn":{"move":"coach","feedback":"UNRELEASED","exercise":"UNRELEASED"}});
    let revise = json!({"verdict":"revise","reason":"Assumes an answer."});
    let (reply, _) = play(
        &mut lesson,
        Some(words),
        vec![
            route("partial", words),
            draft.clone(),
            revise.clone(),
            draft,
            revise,
        ],
    )
    .unwrap();
    assert!(reply.contains(words) && !reply.contains("UNRELEASED"));
    assert!(reply.ends_with(lesson.active_step()) && lesson.active_step().ends_with('?'));
    assert_eq!(lesson.exercise, original);
    assert_eq!(lesson.next_objective, "Look for corroboration");
    assert_eq!(lesson.prior_attempts, [words]);
    let observed = lesson.observation.unwrap();
    assert_eq!(observed.quote, words);
    assert_eq!(observed.action, "partial");
}

#[test]
fn free_subject_rewrites_a_renamed_problem_before_delivery() {
    let mut lesson = active();
    lesson.selected = None;
    lesson.next_objective = "Choose evidence between two explanations.".into();
    let (visible, packets) = play(
        &mut lesson,
        Some("/harder"),
        vec![
            advanced("What did the laptop owner admit?"),
            json!({"verdict":"revise","reason":"Only the nouns changed."}),
            advanced("Which document can separate those explanations and why?"),
            accepted(),
        ],
    )
    .unwrap();
    assert!(packets[2]["revision"].is_object());
    assert!(visible.contains("Which document"));
    assert!(!visible.contains("laptop"));
}

#[test]
fn return_check_uses_released_answer_key_and_changes_the_form() {
    use crate::study::Target;
    let mut lesson = Lesson::default();
    lesson.selected = Some("chemical-engineering".into());
    lesson.objective = "Water inventory".into();
    let first = Target::from_lesson(&lesson);
    let prompt = lesson
        .probe(&first, "", &[], &mut |_, _| {
            panic!("released task needs no model")
        })
        .unwrap();
    let config = Config::new(
        "offline-test".into(),
        tutor_lab::openrouter::DEFAULT_JEV.into(),
        tutor_lab::openrouter::DEFAULT_LUNA.into(),
    )
    .unwrap();
    let verdict = assessment::verify(&lesson, "+10 kg", "repair", &config, None);
    assert_eq!(
        (verdict.method.as_str(), verdict.action.as_str()),
        ("exact_key", "complete")
    );
    let first_claim = Target::from_lesson(&lesson);
    let changed = lesson
        .probe(
            &first_claim,
            &prompt,
            &[prompt.to_lowercase()],
            &mut |_, _| panic!("second released form"),
        )
        .unwrap();
    assert_ne!(changed, prompt);
    assert_eq!(first_claim.key(), Target::from_lesson(&lesson).key());
    let result = assessment::verify(&lesson, "-6 kg", "complete", &config, None);
    assert_eq!(result.method, "exact_key");
    assert_ne!(result.family, verdict.family);
}
#[test]
fn text_cannot_verify_an_unobserved_performance() {
    let config = Config::new(
        "offline-test".into(),
        tutor_lab::openrouter::DEFAULT_JEV.into(),
        tutor_lab::openrouter::DEFAULT_LUNA.into(),
    )
    .unwrap();
    let mut lesson = Lesson::default();
    lesson.selected = Some("violin".into());
    let target = study::Target::from_lesson(&lesson);
    lesson
        .probe(&target, "", &[], &mut |_, _| panic!())
        .unwrap();
    let result = assessment::verify(
        &lesson,
        "I played it all perfectly",
        "complete",
        &config,
        None,
    );
    assert_eq!(result.method, "unobserved");
    assert_ne!(result.action, "complete");
}

#[test]
fn unavailable_verification_leaves_the_written_assessment_open() {
    let config = Config::new(
        "offline-test".into(),
        tutor_lab::openrouter::DEFAULT_JEV.into(),
        tutor_lab::openrouter::DEFAULT_LUNA.into(),
    ).unwrap();
    let mut lesson = active();
    lesson.rubric = Some(assessment::Rubric {
        criterion: "Identify the admitted fact.".into(),
        mode: "written".into(),
    });
    let result = assessment::verify(&lesson, "Delivery.", "complete", &config, Some(std::time::Instant::now()));
    assert_eq!(result.method, "verifier_unavailable");
    assert_eq!(result.action, "support");
}

#[test]
fn misquoted_evidence_recovers_the_original_submission_without_losing_the_turn() {
    let mut lesson = active();
    let words = "I used the signed receipt, including its date.";
    let (_text,requests)=play(&mut lesson,Some(words),vec![route("partial","nonexistent"),route("partial","invented"),json!({"turn":{"move":"coach","feedback":"You selected the receipt.","exercise":"What does its date establish?"}}),accepted()]).unwrap();
    assert_eq!(lesson.observation.unwrap().quote, words);
    assert_eq!(requests[2]["route"]["evidence"], words);
}

#[test]
fn clarification_and_legacy_generated_plan_cannot_replace_a_declared_goal() {
    let mut visit = Visit::default();
    visit
        .apply(Event::Open {
            version: 1,
            selected: None,
        })
        .unwrap();
    let mut entry = serde_json::to_value(Lesson::default()).unwrap();
    entry["objective"] = json!("Apply one rule to the given facts");
    entry["next_objective"] = json!("Compare two plausible interpretations");
    entry["exercise"] = json!("Which fact supports the exception?");
    entry["last_decision"] = route("start", "");
    visit
        .apply(Event::Learner {
            text: "I want to study legal reasoning".into(),
        })
        .unwrap();
    visit
        .apply(Event::Delivered {
            judgment: None,
            text: "Apply the rule.".into(),
            lesson: Box::new(serde_json::from_value(entry.clone()).unwrap()),
            sources: vec![],
        })
        .unwrap();
    let question = "Is there an acronym for this?";
    entry["last_decision"] = route("question", "");
    entry["topic"] = json!(question);
    visit
        .apply(Event::Learner {
            text: question.into(),
        })
        .unwrap();
    visit
        .apply(Event::Delivered {
            judgment: None,
            text: "IRAC is one method.".into(),
            lesson: Box::new(serde_json::from_value(entry).unwrap()),
            sources: vec![],
        })
        .unwrap();
    visit
        .apply(Event::Planned {
            topic: "Learn IRAC".into(),
            outline: vec!["Complete all four stages".into()],
        })
        .unwrap();
    assert_eq!(
        visit.lesson.declared_goal.as_deref(),
        Some("I want to study legal reasoning")
    );
    assert_eq!(visit.lesson.objective, "Apply one rule to the given facts");
    assert_eq!(
        visit.lesson.next_objective,
        "Compare two plausible interpretations"
    );
    assert!(visit.lesson.outline.is_empty());
    let reply = json!({"turn":{"move":"reply","response":"One method is IRAC.","next_step":"Identify the fact that supports the exception."}});
    let (_, requests) = play(
        &mut visit.lesson,
        Some(question),
        vec![route("use_source_or_topic", ""), reply, accepted()],
    )
    .unwrap();
    assert_eq!(requests[1]["route"]["action"], "question");
    assert_eq!(
        visit.lesson.declared_goal.as_deref(),
        Some("I want to study legal reasoning")
    );
}

#[test]
fn clarification_delivers_a_doable_step_and_the_learner_can_answer_it_directly() {
    let mut lesson = active();
    let original_rubric = assessment::Rubric {
        criterion: "Use only what the witness observed.".into(),
        mode: "written".into(),
    };
    lesson.rubric = Some(original_rubric.clone());
    let original = lesson.exercise.clone();
    let step = "Name the one fact that witness actually saw.";
    let body = "Firsthand means the witness personally observed the event.";
    let no_step = json!({"turn":{"move":"reply","response":body}});
    let vague = json!({"turn":{"move":"reply","response":body,"next_step":"Reflect on that."}});
    let fixed = json!({"turn":{"move":"reply","response":body,"next_step":step}});
    let reject =
        json!({"verdict":"revise","reason":"Specify what to identify in the witness account."});
    let (visible, calls) = play(
        &mut lesson,
        Some("Firsthand?"),
        vec![
            route("question", ""),
            no_step,
            vague,
            reject,
            fixed,
            accepted(),
        ],
    )
    .unwrap();
    assert_eq!(visible, format!("{body}\n\n{step}"));
    assert!(calls[2]["repair"].is_string());
    assert_eq!(calls[3]["draft"]["next_step"], "Reflect on that.");
    assert!(calls[4]["revision"].is_object());
    assert_eq!(lesson.exercise, original);
    assert_eq!(
        lesson.rubric.as_ref().unwrap().criterion,
        original_rubric.criterion
    );
    let mut restored: Lesson =
        serde_json::from_value(serde_json::to_value(&lesson).unwrap()).unwrap();
    let (reminder, _) = play(&mut restored, Some("Okay."), vec![]).unwrap();
    assert_eq!(reminder, step);
    assert!(restored.closed.is_empty());
    assert!(restored.observation.is_none());
    assert_eq!(restored.stage, 0);
    let answer = "She saw the delivery.";
    let coach = json!({"turn":{
        "move":"coach",
        "feedback":"Yes, she saw the delivery.",
        "exercise":"Which part is still an inference?"
    }});
    let (_, packets) = play(
        &mut lesson,
        Some(answer),
        vec![route("partial", "the delivery"), coach, accepted()],
    )
    .unwrap();
    assert_eq!(packets[0]["last_requested_step"], step);
    assert_eq!(lesson.observation.as_ref().unwrap().exercise, original);
    let (reminder, _) = play(&mut lesson, Some("Okay!"), vec![route("next", "")]).unwrap();
    assert_eq!(reminder, "Which part is still an inference?");
    let (reminder, _) = play(
        &mut lesson,
        Some("That clears it up."),
        vec![route("acknowledge", "")],
    )
    .unwrap();
    assert_eq!(reminder, lesson.active_step());
    assert!(lesson.closed.is_empty());
    assert!(lesson.observation.is_none());
}

#[test]
fn an_explicit_pause_waits_until_the_learner_says_ready() {
    let mut lesson = active();
    let (visible, _) = play(
        &mut lesson,
        Some("Let's stop here."),
        vec![route("pause", "")],
    )
    .unwrap();
    assert!(visible.contains("Just say “ready”"));
    assert!(lesson.exercise.is_empty());
    assert!(!lesson.awaiting_topic_opening());
    assert_eq!(
        lesson
            .route(Some("I’m ready!"), true, true, &mut |_, _| panic!())
            .unwrap()
            .action,
        "resume_practice"
    );
}

fn guided_turn() -> Value {
    json!({"turn":{
        "move":"guide",
        "feedback":"Your guess that the witness saw the box is a sensible starting point. Seeing the box leaves its contents open.",
        "why_it_matters":"Someone relying on the witness deserves to know what they saw for themselves.",
        "exercise":"If the witness watched the box being opened, would you trust their account of its contents more or less?"
    }})
}

#[test]
fn a_request_for_support_models_the_old_case_and_accepts_a_guess_without_grading() {
    let mut lesson = active();
    let objective = lesson.objective.clone();
    let previous = lesson.exercise.clone();
    let guided = guided_turn();
    let (visible, _) = play(
        &mut lesson,
        Some("Maybe the box? I don't see the point."),
        vec![route("support", ""), guided.clone(), accepted()],
    )
    .unwrap();
    assert!(visible.contains(guided["turn"]["why_it_matters"].as_str().unwrap()));
    assert!(visible.ends_with(guided["turn"]["exercise"].as_str().unwrap()));
    assert_eq!(lesson.closed[0].exercise, previous);
    assert_eq!(lesson.closed[0].reason, "support");
    assert!(lesson.observation.is_none() && lesson.rubric.is_none());
    let mut restored: Lesson =
        serde_json::from_value(serde_json::to_value(&lesson).unwrap()).unwrap();
    let practice = json!({"turn":{"move":"practice","feedback":"Seeing inside gives that witness something more to report.","exercise":"What could a witness learn just by hearing a parcel land?"}});
    let mut replies = [route("partial", "More"), practice, accepted()].into_iter();
    let visible = restored
        .respond_checked(
            Some("More"),
            &mut |_, _| Ok(replies.next().unwrap()),
            &mut |_, _, _| panic!("an invited guess must not be graded"),
        )
        .unwrap();
    assert!(visible.ends_with("What could a witness learn just by hearing a parcel land?"));
    assert!(restored.observation.is_none());
    assert_eq!(restored.closed.last().unwrap().reason, "discuss_guess");
    assert_eq!(restored.objective, objective);
    assert_eq!(restored.stage, 0);
}

#[test]
fn a_second_incomplete_attempt_gets_a_demonstration_even_after_reopening_an_old_journal() {
    let mut lesson = active();
    let criterion = "Distinguish seeing delivery from seeing the contents.";
    lesson.rubric = Some(assessment::Rubric {
        criterion: criterion.into(),
        mode: "written".into(),
    });
    let original = lesson.exercise.clone();
    let first = "The witness saw the delivery.";
    let coach = json!({"turn":{"move":"coach","feedback":"The delivery is something the witness saw.","exercise":"What did the witness see inside?"}});
    let rewritten =
        json!({"verdict":"accept","criterion":"Name every item in the parcel.","mode":"written"});
    play(
        &mut lesson,
        Some(first),
        vec![route("partial", "delivery"), coach, rewritten],
    )
    .unwrap();
    assert_eq!(lesson.rubric.as_ref().unwrap().criterion, criterion);
    assert_eq!(lesson.exercise, original);
    let mut old = serde_json::to_value(&lesson).unwrap();
    old.as_object_mut().unwrap().remove("prior_attempts");
    let mut restored: Lesson = serde_json::from_value(old).unwrap();
    restored.recover_metadata(Some(&active()), Some(first));
    assert_eq!(restored.prior_attempts, [first]);
    let second = "Probably the seal?";
    let mut replies = [route("repair", "seal"), guided_turn(), accepted()].into_iter();
    let msg = restored
        .respond_checked(
            Some(second),
            &mut |packet, _| {
                if packet["task"] == "deliver" {
                    assert_eq!(packet["route"]["action"], "support");
                    assert_eq!(packet["prior_attempts"], json!([first]));
                    assert_eq!(packet["prior_criterion"]["criterion"], criterion);
                }
                Ok(replies.next().unwrap())
            },
            &mut |state, _, _| {
                assert_eq!(state.active_step(), "What did the witness see inside?");
                assert_eq!(state.prior_attempts, [first]);
                assessment::Verdict::provisional("partial")
            },
        )
        .unwrap();
    assert!(msg.contains("Seeing the box leaves its contents open."));
    assert_eq!(restored.observation.as_ref().unwrap().action, "partial");
    assert_eq!(restored.closed[0].exercise, original);
    assert!(restored.prior_attempts.is_empty());
}

#[test]
fn support_at_intake_and_during_an_explanation_keeps_practice_closed() {
    let mut music = Lesson::default();
    music.objective = "Hear a pulse through a rest".into();
    music.explanation_only = true;
    for (mut lesson, learner, explanation, step) in [
        (
            Lesson::default(),
            "I'm lost.",
            "We can start with just one thing.",
            "What were you trying to do when you got stuck?",
        ),
        (
            music,
            "I lose the beat when the sound stops.",
            "A rest leaves space while the beat carries on.",
            "Count one, two, three, four and let only the third count be silent.",
        ),
    ] {
        let mode = lesson.explanation_only;
        let objective = lesson.objective.clone();
        let reply = json!({"turn":{"move":"reply","response":explanation,"next_step":step}});
        let (visible, packets) = play(
            &mut lesson,
            Some(learner),
            vec![route("support", ""), reply, accepted()],
        )
        .unwrap();
        assert!(visible.ends_with(step));
        assert_eq!(packets[0]["explanation_only"], mode);
        assert_eq!(lesson.explanation_only, mode);
        assert_eq!(lesson.objective, objective);
        assert!(
            lesson.closed.is_empty() && lesson.exercise.is_empty() && lesson.observation.is_none()
        );
        assert_eq!(
            serde_json::to_value(&lesson).unwrap()["awaiting_guess"],
            false
        );
    }
}

#[test]
fn twice_rejected_new_question_can_yield_a_reviewed_walkthrough_without_another_guess() {
    let mut lesson = Lesson::default();
    lesson.objective = "Compare fractions".into();
    lesson.exercise = "Compare three quarters with five eighths.".into();
    let original = lesson.exercise.clone();
    let guide = json!({"turn":{"move":"guide","feedback":"Picture equal pieces.","why_it_matters":"The same piece size lets you count them.","exercise":"Which portion is larger?"}});
    let revise = json!({"verdict":"revise","reason":"The guess repeats the demonstration."});
    let step = "Trace the three quarter marks to the sixth eighth mark on the same strip.";
    let reply = json!({"turn":{"move":"reply","response":"Folding each quarter in half makes two eighths.","next_step":step}});
    let (visible, packets) = play(
        &mut lesson,
        Some("I'm lost."),
        vec![
            route("support", ""),
            guide.clone(),
            revise.clone(),
            guide,
            revise,
            reply,
            accepted(),
        ],
    )
    .unwrap();
    assert_eq!(packets[1]["worked_exercise"], original);
    assert_eq!(packets[5]["worked_exercise"], original);
    assert_eq!(packets[6]["explanation_only"], true);
    assert!(visible.ends_with(step));
    assert_eq!(lesson.closed.len(), 1);
    assert!(lesson.exercise.is_empty() && lesson.observation.is_none());
    assert!(!lesson.explanation_only);
    assert_eq!(
        serde_json::to_value(&lesson).unwrap()["awaiting_guess"],
        false
    );
}

#[test]
fn course_plan_marks_the_assessed_unit_and_keeps_return_checks_scoped_to_their_claim() {
    use study::{Observation, Target};
    let mut visit = Visit::default();
    visit.lesson.selected = Some("chemical-engineering".into());
    let title_at = |stage| {
        material::card("chemical-engineering", stage).unwrap()["objective"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = title_at(0);
    let second = title_at(1);
    visit.lesson.objective = first.clone();
    let first_target = Target::from_lesson(&visit.lesson);
    let mut observation = Observation {
        target: first_target.clone(),
        action: "complete".into(),
        validation: "exact_key".into(),
        ..Default::default()
    };
    visit
        .assessments
        .insert(first_target.key(), observation.clone());
    visit.lesson.stage = 1;
    visit.lesson.objective = second.clone();
    let plan = terminal::plan(&visit, &[], &[]);
    assert!(plan.contains(&format!("Demonstrated · {first}")));
    assert!(plan.contains(&format!("{first} · practiced successfully")));
    assert!(plan.contains(&format!("{second} · current")));

    visit.assessments.clear();
    observation.target.claim = Some("Read one quantity".into());
    observation.target.objective = "Read one quantity".into();
    visit
        .assessments
        .insert(observation.target.key(), observation);
    let plan = terminal::plan(&visit, &[], &[]);
    assert!(plan.contains("Demonstrated · Read one quantity"));
    assert!(!plan.contains(&format!("{first} · practiced successfully")));
}

#[test]
fn the_model_can_choose_a_new_objective_while_the_reviewer_checks_the_chosen_one() {
    let mut packet = json!({
        "task":"deliver", "route":{"action":"complete"}, "working_plan":{"previous_next":"Count groups"},
        "previous_objective":"Compare group sizes", "target":"Count groups"
    });
    let messages = |p: &Value| -> Value {
        let req = request(&[], p, &json!({})).unwrap();
        serde_json::from_str(
            req["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
    };
    let teacher = messages(&packet);
    assert!(teacher.get("target").is_none());
    assert_eq!(
        teacher["working_plan"]["just_demonstrated"],
        "Compare group sizes"
    );
    packet["task"] = json!("review");
    packet["target"] = json!("Use probabilities to choose an option");
    assert_eq!(messages(&packet)["target"], packet["target"]);
}

#[test]
fn malformed_course_and_routing_can_defer_without_marking_an_answer_correct() {
    let mut attempts = 0;
    let course = course::Course::create("Compare maps", &Lesson::default(), &[], &mut |_, _| {
        attempts += 1;
        Ok(json!({"milestones":[]}))
    })
    .unwrap();
    assert!(course.is_none() && attempts == 2);
    let mut lesson = active();
    let old = lesson.exercise.clone();
    let (reply,_)=play(&mut lesson,Some("Could you clarify?"),vec![
        json!({"broken":true}),json!({"broken":true}),
        json!({"turn":{"move":"reply","response":"Start from what the witness could hear.","next_step":"Which part was audible?"}}),accepted()
    ]).unwrap();
    assert!(reply.contains("audible") && lesson.observation.is_none());
    assert_eq!(lesson.exercise, old);
}

#[test]
fn a_partial_journal_write_resumes_only_identical_completed_steps_of_the_pending_input() {
    let path = file();
    let request = json!({"messages":["one"],"schema":"original"});
    {
        let mut visit = Visit::default();
        let mut journal = Some(Journal::open(&path, &mut visit).unwrap());
        append(
            &mut journal,
            &mut visit,
            Event::Open {
                version: 1,
                selected: None,
            },
        )
        .unwrap();
        append(
            &mut journal,
            &mut visit,
            Event::Learner {
                text: "my question".into(),
            },
        )
        .unwrap();
        let work = recovery::TurnWork::new(Default::default(), journal.as_mut());
        work.get_or_run("route", "model", &request, || Ok(json!("completed")))
            .unwrap();
    }
    // Simulate a crash between the last fsynced record and the next newline.
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(br#"{"event":"turn_step""#)
        .unwrap();
    let mut visit = Visit::default();
    let mut journal = Some(Journal::open(&path, &mut visit).unwrap());
    assert_eq!(visit.pending.as_deref(), Some("my question"));
    let work = recovery::TurnWork::new(std::mem::take(&mut visit.checkpoints), journal.as_mut());
    assert_eq!(
        work.get_or_run("route", "model", &request, || panic!(
            "repeated a committed provider call"
        ))
        .unwrap(),
        "completed"
    );
    assert_eq!(
        work.get_or_run("route", "different model", &request, || Ok(json!("new")))
            .unwrap(),
        "new"
    );
    assert_eq!(
        work.get_or_run("route", "model", &json!({"schema":"changed"}), || Ok(
            json!("changed")
        ))
        .unwrap(),
        "changed"
    );
    visit.checkpoints = work.finish();
    append(
        &mut journal,
        &mut visit,
        Event::Learner {
            text: "new question".into(),
        },
    )
    .unwrap();
    assert_eq!(visit.checkpoints.len(), 0);
    drop(journal);
    assert_eq!(
        session::replay(&path).unwrap().pending.as_deref(),
        Some("new question")
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn rejected_events_never_enter_the_journal_or_consume_pending_recovery() {
    let path = file();
    let mut visit = Visit::default();
    let mut journal = Some(Journal::open(&path, &mut visit).unwrap());
    append(
        &mut journal,
        &mut visit,
        Event::Open {
            version: 1,
            selected: None,
        },
    )
    .unwrap();
    for text in ["original", "\x1b/retry"] {
        append(
            &mut journal,
            &mut visit,
            Event::Learner { text: text.into() },
        )
        .unwrap();
    }
    let committed = fs::read(&path).unwrap();
    for invalid in [
        Event::Open {
            version: 1,
            selected: None,
        },
        Event::ResumedInterrupted {
            text: "wrong".into(),
        },
    ] {
        assert!(append(&mut journal, &mut visit, invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), committed);
        assert_eq!(visit.interrupted_input(), Some("original"));
    }
    let resume = || Event::ResumedInterrupted {
        text: "original".into(),
    };
    assert!(visit
        .commit(resume(), 0, |_| Err("write failed".into()))
        .is_err());
    assert_eq!(visit.interrupted_input(), Some("original"));
    append(&mut journal, &mut visit, resume()).unwrap();
    drop(journal);
    assert_eq!(
        session::replay(&path).unwrap().pending.as_deref(),
        Some("original")
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

mod question_purpose;
