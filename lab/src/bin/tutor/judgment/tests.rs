use super::*;

fn visit_and_inputs() -> (Visit, Inputs) {
    let mut v = Visit::default();
    v.lesson.objective = "Analyze damage".into();
    v.lesson.exercise = "What does the damage establish?".into();
    v.pending = Some("In IRAC, damage alone leaves fault open.".into());
    let finding = json!({"source":"a#1","speaker":"learner","kind":"method","quote":v.pending,
        "task":v.lesson.objective,"detail":"IRAC","target":""});
    (
        v,
        Inputs {
            version: VERSION.into(),
            source: "a#1".into(),
            shadow: false,
            external_findings: vec![],
            external_cues: vec![],
            probe: None,
            proposals: json!({"findings":[finding]}),
        },
    )
}
#[test]
fn unsupported_citations_and_assent_never_become_findings() {
    let (mut visit, mut input) = visit_and_inputs();
    for (field, value) in [
        ("source", "another#2"),
        ("speaker", "tutor"),
        ("task", "A different case"),
        ("quote", "Unsourced assertion"),
        ("detail", "new method"),
        ("kind", "mastery"),
    ] {
        let mut changed = input.clone();
        changed.proposals["findings"][0][field] = json!(value);
        let (found, rejected) = validate(&changed, &visit, &[]);
        assert!(found.is_empty() && rejected.len() == 1);
    }
    input
        .external_cues
        .push(("older#2".into(), "Use IRAC here.".into()));
    assert_eq!(
        validate(&input, &visit, &[]).0[0].proposal.kind,
        Kind::PromptedMethod
    );
    let mut previous_policy = input.clone();
    previous_policy.version = ELEVENTH_VERSION.into();
    assert!(validate(&previous_policy, &visit, &[]).0.is_empty());
    visit.pending = Some("yes exactly that".into());
    input.proposals["findings"][0]["kind"] = json!("strength");
    input.proposals["findings"][0]["quote"] = json!(visit.pending);
    assert!(validate(&input, &visit, &[]).0.is_empty());
}
#[test]
fn explicit_requests_and_invited_guesses_take_precedence_over_recognition() {
    let (mut visit, input) = visit_and_inputs();
    for requested in ["pause", "question", "support", "next"] {
        let d = select(&input, &visit, &Route::control(requested));
        assert_eq!(d.selected, requested);
        assert!(d.alternatives.iter().any(|a| a == "use_method"));
    }
    visit.lesson.awaiting_guess = true;
    assert_eq!(
        select(&input, &visit, &Route::control("complete")).selected,
        "discuss_guess"
    );
}
#[test]
fn the_draft_must_change_the_lesson() {
    let (visit, input) = visit_and_inputs();
    let d = select(&input, &visit, &Route::control("partial"));
    assert_eq!(d.selected, "use_method");
    for draft in [
        json!({"new_fact":"IRAC stands for Issue, Rule, Application and Conclusion.","exercise":"Who saw it?"}),
        json!({"new_fact":"The borrower had seen the crack.","exercise":"What does the damage establish?"}),
    ] {
        assert!(d
            .check_draft(&draft, &[], Some(visit.lesson.active_step()))
            .is_err());
    }
}
#[test]
fn later_work_can_support_narrow_or_contradict_without_erasing_the_original() {
    let (visit, mut input) = visit_and_inputs();
    let old = validate(&input, &visit, &[]).0.pop().unwrap();
    for kind in ["support", "narrow", "contradict"] {
        input.source = "b#1".into();
        input.proposals["findings"][0] = json!({"source":"b#1","speaker":"learner","kind":kind,
            "task":task(&visit.lesson),"quote":visit.pending,"detail":"This reading applies only to this case.","target":old.id});
        let (mut found, rejected) = validate(&input, &visit, std::slice::from_ref(&old));
        assert!(rejected.is_empty());
        found.push(old.clone());
        assert_eq!(
            active(&found).iter().any(|f| f.id == old.id),
            kind == "support"
        );
        assert_eq!(found.len(), 2);
    }
}

#[test]
fn shadow_candidates_do_not_change_lesson_or_learner_record() {
    let (mut visit, mut input) = visit_and_inputs();
    input.shadow = true;
    let mut calls = Vec::new();
    let (decision, lesson, text) = teach(
        &visit,
        &input,
        &mut |packet, schema| {
            let output = match packet["task"].as_str().unwrap() {
                "route" => json!({"action":"pause","evidence":"","note":"","target_stage":-1}),
                _ => panic!("unexpected generation on a pause"),
            };
            calls.push(Call::new(packet, schema, output.clone()));
            Ok(output)
        },
        &mut |_, _, _| panic!("a pause is ungraded"),
    )
    .unwrap();
    assert_eq!(decision.findings.len(), 1);
    visit
        .apply(crate::session::Event::Delivered {
            text,
            lesson: Box::new(lesson),
            sources: vec![],
            judgment: Some(Box::new(Receipt {
                inputs: input,
                decision,
                calls,
                assessment: None,
            })),
        })
        .unwrap();
    assert!(visit.findings().next().is_none());
    assert!(trace(&visit, 1, &[], false).is_empty());
    assert!(trace(&visit, 1, &[], true).contains("Shadow candidate only"));
}

#[test]
fn a_prompted_method_before_a_volunteered_method_does_not_name_the_wrong_one() {
    let (mut visit, mut input) = visit_and_inputs();
    let text = "CREAC or IRAC could frame why damage alone leaves fault open.";
    visit.pending = Some(text.into());
    input
        .external_cues
        .push(("previous#1".into(), "Try CREAC.".into()));
    let mut volunteered = input.proposals["findings"][0].clone();
    volunteered["quote"] = json!(text);
    let mut prompted = volunteered.clone();
    prompted["kind"] = json!("prompted_method");
    prompted["detail"] = json!("CREAC");
    for findings in [
        json!([prompted, volunteered]),
        json!([volunteered, prompted]),
    ] {
        input.proposals["findings"] = findings;
        let (decision, _, reply) = teach(&visit, &input, &mut |packet,_| Ok(match packet["task"].as_str().unwrap(){
            "route" => json!({"action":"partial","evidence":text,"note":"","target_stage":-1}),
            "deliver" => json!({"turn":{"move":"apply_method","new_fact":"A witness saw the wheel wobble earlier.","exercise":"What does that change?","next_objective":"Compare competing explanations."}}),
            "review" => json!({"verdict":"accept","criterion":"Connect the earlier wobble to uncertainty about cause.","mode":"written","reason":""}),
            other => panic!("unexpected task {other}"),
        }),&mut |_,_,_| panic!("method recognition alone is ungraded")).unwrap();
        assert_eq!(decision.findings.len(), 2);
        assert_eq!(decision.selected, "use_method");
        assert!(reply.starts_with("Using IRAC,"));
    }
}

#[test]
fn a_guided_followup_keeps_the_reviewed_criterion_for_the_next_submission() {
    let (visit, mut input) = visit_and_inputs();
    input
        .external_cues
        .push(("previous#1".into(), "Try IRAC.".into()));
    input.proposals["findings"][0]["kind"] = json!("prompted_method");
    let criterion = "Identify one fact connecting damage to the borrower's conduct.";
    let step = "Which fact about the handling of the bicycle could help establish fault?";
    let (decision, lesson, reply) = teach(
        &visit,
        &input,
        &mut |packet, _| {
            Ok(match packet["task"].as_str().unwrap() {
                "route" => {
                    json!({"action":"partial","evidence":visit.pending,"note":"","target_stage":-1})
                }
                "deliver" => json!({"turn":{"move":"guided_step","exercise":step,"next_objective":"Test a claim against a witness account."}}),
                "review" => {
                    json!({"verdict":"accept","criterion":criterion,"mode":"written","reason":""})
                }
                other => panic!("unexpected task {other}"),
            })
        },
        &mut |_, _, _| panic!("a guided step is ungraded"),
    )
    .unwrap();
    assert_eq!(decision.selected, "use_prompted_method");
    assert!(reply.ends_with(step));
    assert_eq!(lesson.active_step(), step);
    assert!(lesson.assisted);
    let rubric = lesson.rubric.unwrap();
    assert_eq!(rubric.mode, "written");
    assert_eq!(rubric.criterion, criterion);
}

/// Exercise the same receipts the terminal records, including replay of rejected next tasks.
fn completion_turn(visit: &Visit, input: Inputs, approve_draft: bool) -> (Receipt, Lesson, String) {
    let mut calls = vec![];
    let mut assessment = None;
    let (decision, lesson, text) = teach(visit, &input, &mut |packet, shape| {
        let output = match packet["task"].as_str().unwrap() {
            "route" => json!({"action":"complete", "evidence":visit.pending, "note":"", "target_stage":-1}),
            "deliver" => json!({"turn":{"move":"advance", "objective":"Date the first evidence of damage.", "bridge_and_instruction":"Compare the timing of the two photos.", "exercise":"Which photo places the crack before delivery?", "next_objective":"Compare accounts."}}),
            "review" if approve_draft => json!({"verdict":"accept","criterion":"Use the date of the photo.","mode":"written","reason":""}),
            "review" => json!({"verdict":"revise","reason":"The new task gives away the answer."}),
            other => panic!("unexpected task {other}"),
        };
        calls.push(Call::new(packet, shape, output.clone()));
        Ok(output)
    }, &mut |_, _, action| {
        let verdict = Verdict { method: "independent_agreement".into(), ..Verdict::provisional(action) };
        assessment = Some(verdict.clone());
        verdict
    }).unwrap();
    let receipt = Receipt {
        inputs: input,
        calls,
        assessment,
        decision,
    };
    receipt.verify(visit, &lesson, &text).unwrap();
    (receipt, lesson, text)
}

#[test]
fn completion_acknowledges_the_work_and_opens_an_answerable_next_step() {
    for (approve, card, assisted) in [
        (true, None, false),
        (false, None, true),
        (false, Some("writer"), false),
    ] {
        let (mut visit, mut input) = visit_and_inputs();
        input.proposals = json!({"findings":[]});
        visit.lesson.selected = card.map(str::to_owned);
        visit.lesson.assisted = assisted;
        visit.lesson.next_objective = "Compare dated evidence.".into();
        let (_, lesson, text) = completion_turn(&visit, input, approve);
        assert!(
            text.starts_with(if assisted {
                "You’ve got this step."
            } else {
                "It’s clear you get this part."
            }),
            "{text}"
        );
        assert!(lesson.observation.as_ref().unwrap().action == "complete");
        assert_eq!(lesson.closed.len(), 1);
        assert_eq!(lesson.stage, 1);
        assert!(!lesson.exercise.is_empty() && text.ends_with(lesson.active_step()));
        assert!(!text.contains("/continue") && !lesson.awaiting_topic_opening());
        if approve {
            assert!(lesson.rubric.is_some());
        } else if let Some(name) = card {
            let authored = crate::material::card(name, 1).unwrap();
            assert!(text.contains(authored["worked"].as_str().unwrap()));
            assert_eq!(lesson.exercise, authored["practice"]);
        } else {
            assert!(lesson.awaiting_guess && text.contains(&lesson.objective));
            assert!(!text.contains("Which photo"));
        }
    }
}

#[test]
fn a_saved_completion_opens_once_without_a_learner_command_or_a_second_grade() {
    let (mut visit, mut input) = visit_and_inputs();
    input.version = THIRD_VERSION.into();
    input.proposals = json!({"findings":[]});
    visit.lesson.next_objective = "Compare dated evidence.".into();
    let (old, lesson, text) = completion_turn(&visit, input.clone(), false);
    assert!(text.ends_with("Type /continue when you’re ready."));
    visit
        .apply(crate::session::Event::Delivered {
            text,
            lesson: Box::new(lesson),
            sources: vec![],
            judgment: Some(Box::new(old)),
        })
        .unwrap();
    assert!(visit.pending.is_none() && visit.lesson.awaiting_topic_opening());

    input.version = VERSION.into();
    input.source = "a#2".into();
    let (receipt, lesson, text) = completion_turn(&visit, input, true);
    assert!(receipt.assessment.is_none() && lesson.observation.is_none());
    assert_eq!(receipt.decision.delivered_action, "open_next_topic");
    assert_eq!(lesson.stage, visit.lesson.stage);
    assert_eq!(lesson.objective, visit.lesson.objective);
    assert_eq!(lesson.closed.len(), 1);
    assert!(
        !lesson.awaiting_topic_opening()
            && text.ends_with("Which photo places the crack before delivery?")
    );
    visit
        .apply(crate::session::Event::Delivered {
            text,
            lesson: Box::new(lesson),
            sources: vec![],
            judgment: Some(Box::new(receipt)),
        })
        .unwrap();
    assert_eq!(visit.history.last().unwrap().0, None);
}

#[test]
fn a_demonstrated_concept_changes_the_taught_objective_and_the_visible_plan() {
    let (mut visit, mut input) = visit_and_inputs();
    input.version = FIFTH_VERSION.into();
    visit.lesson.declared_goal = Some("Evaluate a claim".into());
    visit.lesson.topic = "Evaluate a claim".into();
    visit.lesson.outline = vec![
        visit.lesson.objective.clone(),
        "Define the method".into(),
        "List the steps".into(),
    ];
    visit.lesson.next_objective = visit.lesson.outline[1].clone();
    visit.lesson.assisted = true;
    let before = crate::terminal::plan(&visit, &[], &[]);
    assert!(before.contains("Next objective · Define the method"));
    let (receipt, lesson, text) = completion_turn(&visit, input, true);
    assert_eq!(receipt.decision.delivered_action, "complete");
    assert_eq!(lesson.objective, "Date the first evidence of damage.");
    assert_eq!(lesson.outline[1], lesson.objective);
    visit
        .apply(crate::session::Event::Delivered {
            text,
            lesson: Box::new(lesson),
            judgment: Some(Box::new(receipt)),
            sources: vec![],
        })
        .unwrap();
    let after = crate::terminal::plan(&visit, &[], &[]);
    assert!(after.contains("Demonstrated with support · Analyze damage"));
    assert!(after.contains("Current · Date the first evidence of damage."));
    assert!(after.contains("Next objective · Compare accounts."));
    assert!(!after.contains("Define the method") && !after.contains("List the steps"));
}

#[test]
fn partial_understanding_replans_the_followup_without_completing_the_topic() {
    let (mut visit, mut input) = visit_and_inputs();
    visit.lesson.next_objective = "Define fault".into();
    input.proposals["findings"][0]["kind"] = json!("strength");
    input.proposals["findings"][0]["detail"] = json!("Distinguishes damage from fault.");
    let mut calls = vec![];
    let verdict = Verdict {
        method: "independent_agreement".into(),
        ..Verdict::provisional("partial")
    };
    let (decision, lesson, text) = teach(&visit, &input, &mut |packet, shape| {
        let output = match packet["task"].as_str().unwrap() {
            "route" => json!({"action":"partial","note":"","target_stage":-1,"evidence":visit.pending}),
            "deliver" => {
                assert_eq!(packet["working_plan"]["previous_next"], "Define fault");
                json!({"turn":{"move":"coach","feedback":"You separated the two questions.","exercise":"Which witness saw it fall?","next_objective":"Compare eyewitness accounts."}})
            }
            "review" => json!({"verdict":"accept","reason":"","criterion":"Identify the witness.","mode":"written"}),
            other => panic!("{other}"),
        };
        calls.push(Call::new(packet, shape, output.clone()));
        Ok(output)
    }, &mut |_, _, _| verdict.clone()).unwrap();
    let receipt = Receipt {
        inputs: input.clone(),
        calls,
        assessment: Some(verdict),
        decision,
    };
    visit
        .apply(crate::session::Event::Delivered {
            text,
            lesson: Box::new(lesson),
            judgment: Some(Box::new(receipt)),
            sources: vec![],
        })
        .unwrap();
    let plan = crate::terminal::plan(&visit, &[], &[]);
    assert!(plan.contains("Working from · Distinguishes damage from fault."));
    assert!(plan.contains("Open task · Which witness saw it fall?"));
    assert!(plan.contains("Next objective · Compare eyewitness accounts."));
    assert!(!plan.contains("Demonstrated") && !plan.contains("Define fault"));

    visit.pending = Some("/next".into());
    input.source = "a#2".into();
    input.proposals = json!({"findings":[]});
    let (_, advanced, _) = teach(&visit, &input, &mut |p,_| Ok(match p["task"].as_str().unwrap() {
        "deliver" => {
            assert_eq!(p["target"], "Compare eyewitness accounts.");
            json!({"turn":{"move":"advance","bridge_and_instruction":"Consider two reports.","exercise":"Which witness could see the doorway?","next_objective":"Weigh corroboration."}})
        }
        "review" => json!({"verdict":"accept","reason":"","mode":"written","criterion":"Compare sightlines."}),
        other => panic!("{other}"),
    }), &mut |_,_,_| panic!()).unwrap();
    assert_eq!(advanced.objective, "Compare eyewitness accounts.");
}

#[test]
fn discussion_without_an_exercise_builds_one_course_and_carries_it_into_the_next_visit() {
    let (mut visit, mut input) = visit_and_inputs();
    visit.lesson.exercise.clear();
    visit.lesson.declared_goal = Some("Learn legal reasoning".into());
    visit.pending = Some("Both readings could change the outcome.".into());
    input.proposals = json!({"findings":[]});
    input.version = SIXTH_VERSION.into();
    let stages = [
        "Find authority",
        "Interpret a rule",
        "Build competing arguments",
        "Recommend a strategy",
    ];
    let mut calls = vec![];
    let mut generate = |packet: &Value, schema: &Value| {
        let answer = match packet["task"].as_str().unwrap() {
            "route" => {
                let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
                assert!(
                    actions.contains(&json!("discussion")) && !actions.contains(&json!("complete"))
                );
                // Even a provider ignoring the enum cannot grade an unassigned task.
                json!({"action":"complete","note":"","target_stage":-1,"evidence":"Both readings"})
            }
            "course_plan" => json!({"destination":"Advise a client in a new hypothetical dispute.",
                "milestones":stages.map(|skill| json!({"skill":skill,"application":format!("Apply {skill} in a fresh matter.")})), "focus":2}),
            "deliver" => {
                assert_eq!(packet["route"]["action"], "discussion");
                assert_eq!(packet["current_milestone"]["skill"], stages[2]);
                json!({"turn":{"move":"teach","objective":"Compare arguments","explanation":"Each side needs an interpretation.",
                    "worked_example":"A delivery rule can protect access or safety.","exercise":"Which reading best serves the rule here?",
                    "outline":["Compare arguments","Propose a strategy"],"next_objective":"Propose a strategy"}})
            }
            "review" => {
                json!({"verdict":"accept","reason":"","criterion":"Defend a reading","mode":"written"})
            }
            other => panic!("{other}"),
        };
        calls.push(Call::new(packet, schema, answer.clone()));
        Ok(answer)
    };
    let (decision, lesson, reply) = teach(&visit, &input, &mut generate, &mut |_, _, _| {
        panic!("discussion was graded")
    })
    .unwrap();
    assert!(lesson.observation.is_none());
    Receipt {
        inputs: input.clone(),
        decision,
        calls,
        assessment: None,
    }
    .verify(&visit, &lesson, &reply)
    .unwrap();
    let mut next = Visit::default();
    next.apply(crate::session::Event::Continued {
        from: crate::session::SourceRef {
            source: "yesterday#1".into(),
            digest: "test".into(),
        },
        lesson: Box::new(serde_json::from_value(serde_json::to_value(&lesson).unwrap()).unwrap()),
    })
    .unwrap();
    let plan = crate::terminal::plan(&next, &[], &[]);
    assert!(plan.contains("Advise a client") && plan.contains("4. Recommend a strategy"));
    next.pending = Some("/next".into());
    input.source = "today#1".into();
    let (_,mut later,_) = teach(&next,&input,&mut |p,_| Ok(match p["task"].as_str().unwrap() {
        "deliver" => {
            assert_eq!(p["current_milestone"]["skill"],stages[3]);
            assert_eq!(p["target"],stages[3]);
            json!({"turn":{"move":"advance","bridge_and_instruction":"The client must choose.","exercise":"Which option should the client take and why?","next_objective":"Defend the recommendation."}})
        }
        "review" => json!({"verdict":"accept","reason":"","criterion":"Compare the tradeoffs","mode":"written"}),
        other => panic!("unexpected second-visit call {other}")
    }), &mut |_,_,_|panic!("skip was graded")).unwrap();
    assert_eq!(later.course.as_ref().unwrap().focus, 3);
    later.open_supervised(
        "Compare options".into(),
        "Which option helps?".into(),
        "Explain the tradeoff".into(),
    );
    assert_eq!(
        later.course.unwrap().destination,
        lesson.course.unwrap().destination
    );
}

#[test]
fn knowing_how_to_argue_and_recalling_the_name_have_separate_evidence_on_return() {
    use crate::{
        method::NameRecall,
        session::{Event, SourceRef},
    };
    let (mut visit, mut input) = visit_and_inputs();
    input.version = EIGHTH_VERSION.into(); // Replay of the earlier name-only policy.
    visit.lesson.topic = "Legal reasoning".into();
    visit.lesson.declared_goal = Some(visit.lesson.topic.clone());
    input.external_cues = vec![("prior#1".into(), "Try IRAC on the first dispute.".into())];
    input.proposals = json!({"findings":[]});

    fn control(visit: &mut Visit, input: &mut Inputs, words: Option<&str>) -> String {
        visit.pending = words.map(str::to_owned);
        input.source = format!("test#{}", visit.history.len() + 1);
        let (decision, lesson, text) = teach(
            visit,
            input,
            &mut |_, _| panic!("control requested a model"),
            &mut |_, _, _| panic!("graded a control"),
        )
        .unwrap();
        assert!(lesson.observation.is_none());
        visit
            .apply(Event::Delivered {
                text: text.clone(),
                lesson: Box::new(lesson),
                sources: vec![],
                judgment: Some(Box::new(Receipt {
                    inputs: input.clone(),
                    calls: vec![],
                    decision,
                    assessment: None,
                })),
            })
            .unwrap();
        text
    }
    let old = visit.lesson.exercise.clone();
    assert!(control(
        &mut visit,
        &mut input,
        Some("What was that acronym called?")
    )
    .contains("Issue, Rule, Application, Conclusion"));
    assert_eq!(visit.lesson.exercise, old);
    assert!(!visit.lesson.recall_due());
    let case = control(&mut visit, &mut input, Some("/practice irac"));
    assert!(case.contains("strongest response"));
    let original_case = visit.lesson.exercise.clone();
    let original_criterion = visit.lesson.rubric.as_ref().unwrap().criterion.clone();
    assert!(original_criterion.contains("Either conclusion"));
    visit.pending = Some("The delay at the doorway affected access.".into());
    input.source = "test#3".into();
    let mut calls = Vec::new();
    let verdict = Verdict {
        method: "independent_agreement".into(),
        ..Verdict::provisional("partial")
    };
    let (decision,lesson,text)=teach(&visit,&input,&mut |p,s| {
        let v = match p["task"].as_str().unwrap() {
            "route"=>json!({"action":"partial","note":"","target_stage":-1,"evidence":"delay at the doorway"}),
            "course_plan"=>json!({"destination":"Advise a client","focus":1,"milestones":(["Find authority","Apply the rule","Answer an objection","Recommend a remedy"].map(|skill|json!({"skill":skill,"application":format!("Practice {skill}")})))}),
            "deliver"=>json!({"turn":{"move":"coach","feedback":"You connected the delay to access.","exercise":"What argument could Mira make about that delay?","next_objective":"Weigh a counterargument"}}),
            "review"=>json!({"verdict":"accept","reason":"","mode":"written","criterion":""}),
            _=>panic!("unexpected {p}"),
        };
        calls.push(Call::new(p,s,v.clone())); Ok(v)
    },&mut |_,_,_|verdict.clone()).unwrap();
    assert!(lesson.recall_due());
    visit
        .apply(Event::Delivered {
            text,
            lesson: Box::new(lesson),
            sources: vec![],
            judgment: Some(Box::new(Receipt {
                inputs: input.clone(),
                calls,
                decision,
                assessment: Some(verdict),
            })),
        })
        .unwrap();
    let assisted_step = visit.lesson.active_step().to_owned();
    for (answer, expected) in [
        ("I think it was IRAC.", NameRecall::Recalled),
        ("was it CRAC?", NameRecall::Reminded),
        ("skip", NameRecall::Skipped),
    ] {
        let mut next = Visit::default();
        next.apply(Event::Continued {
            lesson: Box::new(visit.lesson.clone()),
            from: SourceRef {
                source: "test#3".into(),
                digest: "test".into(),
            },
        })
        .unwrap();
        let question = control(&mut next, &mut input, None);
        assert!(question.contains("four-letter") && !mentions(&question, "IRAC"));
        let cue_free = question.to_lowercase();
        assert!(!cue_free.contains("issue") && !cue_free.contains("application"));
        assert!(control(&mut next, &mut input, Some(answer)).contains(&assisted_step));
        assert_eq!(next.lesson.name_recall, Some(expected));
        assert_eq!(next.lesson.exercise, original_case);
        assert_eq!(
            next.lesson.rubric.as_ref().unwrap().criterion,
            original_criterion
        );
        assert!(!next.lesson.recall_due());
        assert_eq!(next.assessments.len(), 1);
        assert!(next.assessments.values().all(|a| a.action == "partial"));
    }
}

#[test]
fn a_malformed_historical_retry_restores_only_the_interrupted_message() {
    use crate::session::Event;
    let mut visit = Visit::default();
    visit
        .apply(Event::Learner {
            text: "A discussion contribution".into(),
        })
        .unwrap();
    visit
        .apply(Event::Learner {
            text: "\u{1b}/retry".into(),
        })
        .unwrap();
    visit
        .apply(Event::Delivered {
            text: "A detour".into(),
            lesson: Box::new(Lesson::default()),
            judgment: None,
            sources: vec![],
        })
        .unwrap();
    assert_eq!(visit.interrupted_input(), Some("A discussion contribution"));
    visit
        .apply(Event::ResumedInterrupted {
            text: "A discussion contribution".into(),
        })
        .unwrap();
    assert!(visit.teaching_history().is_empty());
    assert_eq!(visit.history.len(), 1);
    assert_eq!(visit.pending.as_deref(), Some("A discussion contribution"));
    visit
        .apply(Event::Learner {
            text: "A newer question".into(),
        })
        .unwrap();
    assert!(visit.interrupted_input().is_none());
    assert_eq!(visit.teaching_history().len(), 1);
}

#[test]
fn a_coaching_step_can_pass_with_the_saved_plan_when_both_replans_repeat_a_skill() {
    let (mut visit, mut input) = visit_and_inputs();
    visit.lesson.next_objective = "Consider the other witness".into();
    input.proposals = json!({"findings":[]});
    let verdict = Verdict {
        method: "independent_agreement".into(),
        ..Verdict::provisional("partial")
    };
    let step = "Which fact would you use to check that inference?";
    let mut calls = Vec::new();
    let (decision, lesson, reply) = teach(
        &visit,
        &input,
        &mut |packet, schema| {
            let output = match packet["task"].as_str() {
                Some("route") => {
                    json!({"action":"partial","evidence":visit.pending,"note":"","target_stage":-1})
                }
                Some("deliver") => {
                    json!({"turn":{"move":"coach","feedback":"Let’s check that inference.",
                "exercise":step,"next_objective":visit.lesson.objective}})
                }
                Some("review") => {
                    assert_eq!(packet["draft"]["exercise"], step);
                    assert!(packet["draft"].get("next_objective").is_none());
                    json!({"verdict":"accept","reason":"","criterion":"","mode":"unassessable"})
                }
                _ => panic!("unexpected request"),
            };
            calls.push(Call::new(packet, schema, output.clone()));
            Ok(output)
        },
        &mut |_, _, _| verdict.clone(),
    )
    .unwrap();
    assert_eq!(calls.iter().filter(|c| c.task == "deliver").count(), 2);
    assert_eq!(calls.iter().filter(|c| c.task == "review").count(), 1);
    assert_eq!(lesson.next_objective, visit.lesson.next_objective);
    assert_eq!(lesson.exercise, visit.lesson.exercise);
    assert!(reply.ends_with(step));
    Receipt {
        inputs: input,
        decision,
        calls,
        assessment: Some(verdict),
    }
    .verify(&visit, &lesson, &reply)
    .unwrap();
}

#[test]
fn ordinary_work_prompts_for_meaning_across_subjects_and_preserves_the_application() {
    use crate::{recall::State, session::Event};
    for (term, reference, paraphrase) in [
        (
            "median",
            "the middle value in a sorted list",
            "sort the data and take the middle",
        ),
        (
            "PWM",
            "pulse width modulation",
            "it sets the on time of each pulse in a cycle",
        ),
    ] {
        let (mut visit, mut input) = visit_and_inputs();
        visit.pending = None;
        visit.lesson.objective = "Compare two measurements".into();
        visit.lesson.exercise = "Which observation supports your prediction?".into();
        let prior = visit.lesson.clone();
        visit
            .apply(Event::Recovered {
                exchanges: vec![(
                    Some("Remind me of the term.".into()),
                    format!("We used {term} for {reference}."),
                )],
                continuation: Box::new(prior),
            })
            .unwrap();
        input.source = "a#2".into();
        input.proposals =
            json!({"findings":[],"recall_candidate":{"term":term,"definition_quote":reference}});
        visit.pending = Some("The first measurement matches the prediction.".into());
        let step = "What observation might change your prediction?";
        let mut calls = Vec::new();
        let verdict = Verdict::provisional("partial");
        let (decision,lesson,reply)=teach(&visit,&input,&mut |p,s| {
            let output=match p["task"].as_str() {
                Some("route")=>json!({"action":"partial","note":"","target_stage":-1,"evidence":"first measurement"}),
                Some("deliver")=>json!({"turn":{"move":"coach","feedback":"You connected a measurement to the prediction.","exercise":step}}),
                Some("review")=>json!({"verdict":"accept","reason":"","criterion":"","mode":"unassessable"}),
                _=>panic!("unexpected stage {p}"),
            };
            calls.push(Call::new(p,s,output.clone())); Ok(output)
        },&mut |_,_,_|verdict.clone()).unwrap();
        assert!(reply.contains("connected a measurement") && reply.contains(term));
        assert!(!reply.contains(reference) && !reply.contains(step));
        assert!(reply.ends_with(&lesson.current_question()));
        assert_eq!(lesson.asking_meaning().unwrap().source, "a#1");
        assert_eq!(lesson.observation.as_ref().unwrap().action, "partial");
        visit
            .apply(Event::Delivered {
                text: reply,
                lesson: Box::new(lesson),
                judgment: Some(Box::new(Receipt {
                    inputs: input.clone(),
                    decision,
                    calls,
                    assessment: Some(verdict),
                })),
                sources: vec![],
            })
            .unwrap();
        for (answer, outcome, state) in [
            (paraphrase, "recalled", State::Recalled),
            ("I remember one part", "partial", State::Partial),
            ("skip", "", State::Skipped),
            ("hint", "", State::Reminded),
        ] {
            input.source = "a#3".into();
            input.proposals = json!({"findings":[]});
            visit.pending = Some(answer.into());
            let mut calls = vec![];
            let (d, l, t) = teach(
                &visit,
                &input,
                &mut |p, s| {
                    assert_eq!(p["task"], "check_meaning");
                    assert_eq!(p["reference"], reference);
                    assert!(!outcome.is_empty());
                    let v = json!({"result":outcome,"quote":answer});
                    calls.push(Call::new(p, s, v.clone()));
                    Ok(v)
                },
                &mut |_, _, _| panic!("meaning changed the case grade"),
            )
            .unwrap();
            assert!(l.observation.is_none());
            assert_eq!(l.recall_items[0].state, state);
            assert_eq!(l.exercise, visit.lesson.exercise);
            assert_eq!(l.active_step(), step);
            assert_eq!(l.current_question(), step);
            assert!(t.ends_with(step));
            Receipt {
                inputs: input.clone(),
                decision: d,
                calls,
                assessment: None,
            }
            .verify(&visit, &l, &t)
            .unwrap();
        }
    }
}

#[test]
fn instructional_advancement_preserves_a_partial_assessment_and_replays_the_new_exercise() {
    for (domain, operation, quote, skill, application) in [
        (
            "Statistics",
            "Read a median",
            "The middle value stays fixed",
            "Choose a robust summary",
            "Which summary would you use for skewed incomes, and why?",
        ),
        (
            "Engineering",
            "Balance a steady flow",
            "Inflow equals outflow",
            "Predict transient accumulation",
            "How would the stored volume change after the outlet closes?",
        ),
    ] {
        let (mut visit, mut input) = visit_and_inputs();
        visit.lesson.declared_goal = Some(domain.into());
        visit.lesson.objective = operation.into();
        visit.lesson.exercise = "Apply the familiar operation.".into();
        visit.pending = Some(quote.into());
        visit.lesson.course = Some(crate::course::Course {
            goal: domain.into(),
            destination: "Independent application".into(),
            milestones: vec![crate::course::Milestone {
                skill: operation.into(),
                application: "original exercise".into(),
            }],
            focus: 0,
            turns: 2,
        });
        input.proposals = json!({"findings":[],"progression":{"operation":operation,"quote":quote,"choice":"advance","reason":"The reasoning is explicit; a further example would repeat it.","next_skill":skill,"application":application}});
        let mut calls = vec![];
        let mut assessment = None;
        let (decision, lesson, reply)=teach(&visit,&input,&mut |p,shape| {
            let result=match p["task"].as_str().unwrap() {
                "route"=>json!({"action":"partial","evidence":quote,"note":"One detail remains.","target_stage":-1}),
                "deliver"=>{assert_eq!(p["target"],skill);json!({"turn":{"move":"advance","bridge_and_instruction":"Use the earlier result to make the following prediction.","exercise":application,"next_objective":"Check the limits"}})},
                "review"=>{assert_eq!(p["progression"]["operation"],operation);json!({"verdict":"accept","criterion":"Explain the new decision","mode":"written","reason":""})},
                other=>panic!("unexpected {other}")
            };calls.push(Call::new(p,shape,result.clone()));Ok(result)
        },&mut |_,_,action|{let v=Verdict::provisional(action);assessment=Some(v.clone());v}).unwrap();
        assert_eq!(lesson.observation.as_ref().unwrap().action, "partial");
        assert_eq!(
            lesson.course.as_ref().unwrap().current().unwrap().skill,
            skill
        );
        assert_eq!(lesson.active_step(), application);
        assert_eq!(
            lesson
                .course
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .application,
            application
        );
        assert!(reply.contains(application));
        assert_eq!(lesson.reasoning.last().unwrap().quote, quote);
        Receipt {
            inputs: input,
            decision,
            calls,
            assessment,
        }
        .verify(&visit, &lesson, &reply)
        .unwrap();
    }
}

#[test]
fn repeat_practice_and_real_gaps_remain_available_after_a_return() {
    let (mut visit, mut input) = visit_and_inputs();
    visit.lesson.assisted = true;
    let term = "Compare slopes";
    for (choice, quote, why) in [
        (
            "stay",
            "I divided by time twice",
            "The units contradict the proposed slope.",
        ),
        (
            "requested_practice",
            "Please give me another one",
            "The learner requested practice.",
        ),
        ("stay", "I used the same method", ""),
    ] {
        input.proposals = json!({"progression":{"operation":term,"quote":quote,"choice":choice,"reason":why,"next_skill":"Distinguish rates from totals","application":"Which units would rule out one interpretation?"}});
        assert!(crate::progression::read(&input, &visit.lesson, "different submission").is_none());
        let r = crate::progression::read(&input, &visit.lesson, quote).unwrap();
        assert!(r.assisted);
        assert_eq!(
            r.choice,
            if why.is_empty() {
                "distinguish"
            } else {
                choice
            }
        );
        visit.lesson.reasoning.push(r);
    }
    let mut returned = Visit::default();
    returned
        .apply(crate::session::Event::Continued {
            from: crate::session::SourceRef {
                source: "a#3".into(),
                digest: "sample".into(),
            },
            lesson: Box::new(visit.lesson),
        })
        .unwrap();
    returned.history = vec![(Some("later".into()), "an intervening exchange".into()); 16];
    returned.pending = Some("my next answer".into());
    observe(&returned, "b#1", &[], &mut |p, _| {
        let wire = crate::request(&returned.history, p, &json!({})).unwrap();
        let task: Value = serde_json::from_str(
            wire["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(task["earlier_exchanges_outside_window"], 2);
        assert_eq!(
            task["recent_reasoning"][0]["quote"],
            "I divided by time twice"
        );
        assert_eq!(task["recent_reasoning"][1]["choice"], "requested_practice");
        Ok(json!({"findings":[]}))
    })
    .unwrap();
}

#[test]
fn coaching_editor_preserves_feedback_and_leaves_unrelated_recall_for_later() {
    replay_coaching_repair(None, false);
    replay_coaching_repair(None, true);
}

#[test]
#[ignore = "uses live Luna to edit and review a fictional coaching turn"]
fn live_coaching_repair() {
    replay_coaching_repair(Some(crate::Config::from_env().unwrap()), false);
}

fn replay_coaching_repair(live: Option<crate::Config>, reject_final_step: bool) {
    let (mut visit, mut inputs) = visit_and_inputs();
    visit.lesson.objective = "Distinguish association from cause".into();
    visit.lesson.exercise = "Two trays are watered equally, but the sunny tray is also warmer. Which difference could explain its faster growth?".into();
    visit.pending =
        Some("I think sunlight caused the growth because that tray grew faster.".into());
    inputs.proposals = json!({"findings":[]});
    // Include a pre-upgrade recall item, with no recorded teaching objective.
    visit.lesson.recall_items = serde_json::from_value(json!([{
        "term":"Syncopation", "reference":"emphasis on a weak beat",
        "state":"due", "source":"earlier#1"
    }]))
    .unwrap();
    let feedback = "You used the difference in growth as evidence for sunlight. That comparison establishes an association. Temperature changed too, so the faster growth cannot tell us which change caused it.";
    let step = "What would you hold constant in a new comparison to isolate sunlight?";
    let mut rejected = 0;
    let mut calls = Vec::new();
    let verdict = Verdict::provisional("partial");
    let (decision, lesson, text) = teach(&visit, &inputs, &mut |p,s| {
        let output = match p["task"].as_str().unwrap() {
            "route" => json!({"action":"partial","evidence":"that tray grew faster","note":"","target_stage":-1}),
            "deliver" => json!({"turn":{"move":"coach","feedback":"You mentioned sunlight.","exercise":"What happened?"}}),
            "review" if rejected < 2 => {
                rejected += 1;
                json!({"verdict":"revise","reason":"Explain what the comparison establishes and ask about the confounding difference."})
            }
            "edit_turn" | "review" if live.is_some() => {
                let response = live.as_ref().unwrap().complete(crate::Role::Luna, &crate::request(&[], p, s)?)?;
                let text = crate::assistant_text(&response)?;
                eprintln!("{}: {text}", p["task"]);
                serde_json::from_str(text).map_err(|e| e.to_string())?
            }
            "edit_turn" => {
                assert!(p["editor_review"]["issue"].is_string());
                assert_eq!(p["learner_message"], visit.pending.as_deref().unwrap());
                json!({"turn":{"move":"coach","feedback":feedback,"exercise":step}})
            }
            "review" if reject_final_step => {
                assert_eq!(s["properties"]["standalone_feedback"]["type"], "string");
                json!({"verdict":"revise","reason":"Revise the next step.","standalone_feedback":feedback})
            }
            "review" => json!({"verdict":"accept","reason":"","criterion":"","mode":"unassessable"}),
            other => panic!("unexpected stage {other}"),
        };
        calls.push(Call::new(p,s,output.clone()));
        Ok(output)
    }, &mut |_,_,_| verdict.clone()).unwrap();
    if live.is_some() {
        eprintln!("Delivered coaching:\n{text}");
        assert!(text.to_lowercase().contains("sunlight") && !text.contains("Syncopation"));
    } else if reject_final_step {
        assert_eq!(text, feedback);
        assert_eq!(lesson.active_step(), visit.lesson.active_step());
    } else {
        assert_eq!(text, format!("{feedback}\n\n{step}"));
        assert_eq!(lesson.active_step(), step);
    }
    assert_eq!(lesson.exercise, visit.lesson.exercise);
    assert_eq!(lesson.recall_items[0].state, crate::recall::State::Waiting);
    Receipt {
        inputs,
        decision,
        calls,
        assessment: Some(verdict),
    }
    .verify(&visit, &lesson, &text)
    .unwrap();
}

#[test]
fn a_paraphrase_and_a_mistaken_observer_label_follow_the_recorded_method_cue() {
    for quote in [
        "IRAC separates damage from responsibility.",
        "I’d use IRAC here. Damage alone leaves fault open.",
    ] {
        for cued in [false, true] {
            let (mut v, mut input) = visit_and_inputs();
            v.pending = Some(quote.into());
            input.proposals["findings"][0]["quote"] = json!(quote);
            input.proposals["findings"][0]["kind"] =
                json!(if cued { "method" } else { "prompted_method" });
            if cued {
                input
                    .external_cues
                    .push(("earlier#1".into(), "Try IRAC on this case.".into()));
            }
            let decision = select(&input, &v, &Route::control("complete"));
            assert_eq!(
                decision.selected,
                if cued {
                    "use_prompted_method"
                } else {
                    "use_method"
                }
            );
            assert_eq!(decision.tutor_cue.is_some(), cued);
            assert!(decision.next_check.is_some());
        }
    }
}

#[test]
fn a_due_skill_can_be_checked_inside_the_next_course_application() {
    let (mut v, mut input) = visit_and_inputs();
    let application = "Compare two repair estimates.";
    v.lesson.course = Some(crate::course::Course {
        goal: "Repair a bicycle".into(),
        destination: "Choose a repair".into(),
        milestones: ["Identify the damage.", application]
            .iter()
            .map(|s| crate::course::Milestone {
                skill: (*s).into(),
                application: (*s).into(),
            })
            .collect(),
        focus: 0,
        turns: 1,
    });
    v.pending = Some("/next".into());
    input.proposals = json!({"findings":[]});
    input.probe = Some(Probe {
        source: Some("earlier#2".into()),
        target: Target::from_lesson(&v.lesson),
        previous: "How was the first bicycle damaged?".into(),
        tasks: vec![],
        application: Some(application.into()),
    });
    let (d,l,reply)=teach(&v,&input,&mut |packet,_|{
        assert_eq!(packet["new_application"],application);
        Ok(match packet["task"].as_str(){
            Some("deliver")=>json!({"exercise":"Which repair estimate accounts for the observed wheel damage?"}),
            Some("review")=>json!({"verdict":"accept","criterion":"Connect the observed damage to the proposed repair.","mode":"written","reason":""}),
            _=>panic!("Unexpected packet"),
        })
    }, &mut |_,_,_|panic!("Opening a fresh task supplies no grade")).unwrap();
    assert!(d.why.contains("earlier#2"));
    assert_eq!(l.course.unwrap().focus, 1);
    assert!(l.is_probe && !l.assisted && l.observation.is_none());
    assert!(reply.contains("repair estimate"));
}
