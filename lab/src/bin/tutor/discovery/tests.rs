use super::*;

fn response(action: &str, reply: &str) -> Value {
    json!({"action":action,"subject":"Algebra","area":"Graphs","areas":["Ratios","Graphs","Equations"],"findings":[],"reply":reply,"reason":"Connect this question to your bike stand.","start":{"objective":"Graph a ratio","exercise":"Plot the turns for two cranks.","criterion":"Label the axes.","why":"You explained the ratio and wanted help translating it to a graph.","sources":["current"]}})
}

fn finding(kind: &str, area: &str, quote: &str) -> Value {
    json!({"kind":kind,"area":area,"quote":quote,"reading":format!("Discussed {area}"),"replaces":[]})
}

fn turn(lesson: &mut Lesson, words: &str, id: &str, proposal: Value) -> Result<String, String> {
    let mut proposal = proposal;
    if let Some(fields) = proposal.as_object_mut() {
        let action = fields.remove("action");
        let start = fields.remove("start");
        let early_request = fields.remove("early_request").unwrap_or(json!(""));
        fields.insert(
            "decision".into(),
            json!({"action":action,"start":start,"early_request":early_request}),
        );
    }
    lesson
        .discovery
        .clone()
        .unwrap()
        .next(lesson, words, id, false, &mut |_, _| Ok(proposal.clone()))
}

fn fresh() -> Lesson {
    let mut lesson = Lesson::default();
    lesson.discovery = Some(Discovery::new("Algebra".into(), Profile::default()));
    lesson
}

#[test]
fn reflection_and_an_explicit_choice_guard_each_transition() {
    let mut l = fresh();
    assert!(turn(
        &mut l,
        "I repair bicycles.",
        "one#1",
        response("diagnostic", "Plot a point.")
    )
    .is_err());
    let mut reflection = response("reflect", "You learned by repairing bicycles.");
    reflection["findings"] = json!([finding("reported", "", "I repair bicycles.")]);
    turn(&mut l, "I repair bicycles.", "one#1", reflection).unwrap();
    assert!(turn(
        &mut l,
        "perhaps",
        "one#2",
        response("diagnostic", "Plot a point.")
    )
    .is_err());
    turn(
        &mut l,
        "looks right",
        "one#2",
        response("diagnostic", "Plot a point."),
    )
    .unwrap();
    let mut propose = response("propose", "Graph this next.");
    propose["findings"] = json!([finding("demonstrated", "Graphs", "(2,6)")]);
    assert!(turn(&mut l, "(2,6)", "one#3", propose.clone()).is_err());
    propose["findings"] = json!([]);
    propose["start"]["sources"] = json!(["one#1.1"]);
    let text = turn(&mut l, "/start-now", "one#3", propose).unwrap();
    assert!(text.contains("Unexplored · Equations"));
    assert!(l.exercise.is_empty());
    turn(&mut l, "yes", "one#4", Value::Null).unwrap();
    assert!(l.exercise.is_empty());
    let saved = serde_json::to_string(&l).unwrap();
    let mut l: Lesson = serde_json::from_str(&saved).unwrap();
    let text = turn(&mut l, "/start", "later#1", Value::Null).unwrap();
    assert!(text.ends_with("Plot the turns for two cranks."));
    assert_eq!(l.active_step(), "Plot the turns for two cranks.");
    assert_eq!(l.discovery.unwrap().phase, Phase::Instruction);
}

#[test]
fn declined_unfamiliar_hinted_contradictory_and_corrected_evidence_remains_distinguishable() {
    let mut l = fresh();
    let mut reflection = response("reflect", "I’ll leave school aside.");
    reflection["findings"] = json!([finding("declined", "", "I’d rather skip school.")]);
    turn(&mut l, "I’d rather skip school.", "v#1", reflection).unwrap();
    turn(
        &mut l,
        "looks right",
        "v#2",
        response("diagnostic", "Which axis shows crank turns?"),
    )
    .unwrap();
    let mut cue = response(
        "hint",
        "Try putting the input on the horizontal axis. Where does 1 go?",
    );
    cue["findings"] = json!([finding("unfamiliar", "Graphs", "New to me")]);
    turn(&mut l, "New to me. A hint?", "v#3", cue).unwrap();
    let mut check = response("diagnostic", "What point represents two crank turns?");
    check["findings"] = json!([finding("demonstrated", "Graphs", "(1,3)")]);
    turn(&mut l, "(1,3)", "v#4", check).unwrap();
    let mut follow = response("follow", "What does the second coordinate mean?");
    follow["findings"] = json!([finding("difficulty", "Graphs", "(6,2)")]);
    turn(&mut l, "(6,2)", "v#5", follow).unwrap();
    let d = l.discovery.as_ref().unwrap();
    assert_eq!(d.profile.facts["v#4.1"].kind, "cued");
    assert_eq!(d.profile.facts["v#5.1"].kind, "difficulty");
    assert!(d.profile.facts["v#4.1"]
        .task
        .as_ref()
        .unwrap()
        .contains("input"));
    assert!(d.account().contains("Unexplored · Equations"));
    let mut first = response("propose", "A starting point.");
    first["start"]["sources"] = json!(["v#5.1"]);
    turn(&mut l, "/start-now", "v#6", first).unwrap();
    let old = l.clone();
    let mut repair = response("explore", "Let’s check the relationship in words.");
    let mut corrected = finding(
        "correction",
        "Graphs",
        "I reversed the labels while typing.",
    );
    corrected["replaces"] = json!(["v#5.1"]);
    repair["findings"] = json!([corrected]);
    turn(&mut l, "I reversed the labels while typing.", "v#7", repair).unwrap();
    let d = l.discovery.as_ref().unwrap();
    assert_eq!(d.phase, Phase::Subject);
    assert!(d.proposal.is_none());
    assert!(d.account().contains("Earlier account, since corrected"));
    let mut other = old.discovery.unwrap();
    other.merge(&d.profile);
    assert_eq!(other.phase, Phase::Subject);
    let mut changed = response("propose", "Explain your relationship.");
    changed["start"]["exercise"] = json!("Explain why wheel turns triple.");
    changed["start"]["sources"] = json!(["v#7.1"]);
    turn(&mut l, "/start-now", "v#8", changed).unwrap();
    turn(&mut l, "start", "v#9", Value::Null).unwrap();
    assert_eq!(l.exercise, "Explain why wheel turns triple.");
}

#[test]
fn an_ordinary_proposal_accounts_for_the_subject_and_delivers_only_after_acceptance() {
    let mut l = fresh();
    l.discovery.as_mut().unwrap().phase = Phase::Subject;
    let mut p = response("propose", "Let’s review.");
    p["findings"] = json!([
        finding("reported", "Ratios", "I know ratios"),
        finding("unfamiliar", "Graphs", "graphs are new"),
        finding("declined", "Equations", "let’s leave equations for later")
    ]);
    turn(
        &mut l,
        "I know ratios, graphs are new, and let’s leave equations for later.",
        "v#1",
        p,
    )
    .unwrap();
    let state = l.discovery.as_ref().unwrap();
    assert_eq!(state.phase, Phase::Review);
    assert!(state.account().contains("Unexplored · Equations"));
    turn(&mut l, "start", "v#2", Value::Null).unwrap();
    assert_eq!(l.active_step(), "Plot the turns for two cranks.");
}

#[test]
fn reports_do_not_become_demonstrations_and_inferences_require_a_real_quote() {
    let mut l = fresh();
    let mut reply = response("follow", "How did you learn?");
    reply["findings"] = json!([finding("demonstrated", "Ratios", "I can use ratios")]);
    assert!(turn(&mut l, "I can use ratios", "v#1", reply.clone()).is_err());
    reply["findings"] = json!([finding("reported", "Ratios", "invented evidence")]);
    assert!(turn(&mut l, "I can use ratios", "v#1", reply).is_err());
    assert!(l.discovery.unwrap().profile.facts.is_empty());
}

#[test]
fn another_subject_reuses_the_personal_account_and_scopes_subject_evidence() {
    let mut l = fresh();
    let mut reflection = response("reflect", "You learn by fixing bicycles.");
    reflection["findings"] = json!([finding("reported", "", "I fix bicycles")]);
    turn(&mut l, "I fix bicycles", "old#1", reflection).unwrap();
    turn(
        &mut l,
        "looks right",
        "old#2",
        response("diagnostic", "Where is the crank count on your graph?"),
    )
    .unwrap();
    let mut follow = response("follow", "How did you decide?");
    follow["findings"] = json!([finding("demonstrated", "Graphs", "Horizontal axis")]);
    turn(&mut l, "Horizontal axis", "old#3", follow).unwrap();
    let mut next = l.discovery.unwrap();
    next.change_subject("Music");
    assert!(next.account().contains("I fix bicycles"));
    assert!(!next.account().contains("Horizontal axis"));
    next.change_subject("Algebra");
    assert!(next.account().contains("Observed independently (Graphs)"));
    assert!(next.account().contains("Unexplored · Equations"));
}

#[test]
fn an_explicit_correction_withdraws_the_dependent_proposal_without_a_provider_call() {
    let mut l = fresh();
    let mut p = response("propose", "Try a graph.");
    p["findings"] = json!([finding("reported", "", "I can plot points")]);
    // Interpret the learner's request to start before the subject map is complete.
    p["action"] = json!("provisional");
    p["early_request"] = json!("Can we begin teaching now?");
    turn(
        &mut l,
        "I can plot points. Can we begin teaching now?",
        "v#1",
        p,
    )
    .unwrap();
    let msg = turn(
        &mut l,
        "/amend v#1.1 I needed a hint to plot those points.",
        "v#2",
        Value::Null,
    )
    .unwrap();
    assert!(msg.contains("I needed a hint"));
    let d = l.discovery.as_ref().unwrap();
    assert_eq!(d.phase, Phase::Subject);
    assert!(d.proposal.is_none());
    assert_eq!(d.profile.facts["v#2.1"].replaces, vec!["v#1.1"]);
}

#[test]
fn an_early_personal_request_can_span_the_subject_clarification() {
    let mut lesson = Lesson::default();
    lesson.discovery = Some(Discovery::new(String::new(), Profile::default()));
    let mut proposal = response("provisional", "We can start now.");
    proposal["subject"] = json!("");
    proposal["early_request"] = json!("Please start teaching now");
    let question = turn(&mut lesson, "Please start teaching now", "v#1", proposal).unwrap();
    assert!(question.contains("Which subject"));
    assert!(lesson.discovery.as_ref().unwrap().awaiting_subject_to_start);
    let mut proposal = response("propose", "We can try a graph.");
    proposal["start"]["sources"] = json!([]);
    turn(&mut lesson, "Algebra", "v#2", proposal).unwrap();
    assert_eq!(lesson.discovery.as_ref().unwrap().phase, Phase::Review);
    assert!(lesson.exercise.is_empty());
}

#[test]
fn opening_offers_small_moves_without_inferring_ability() {
    let mut lesson = fresh();
    assert_eq!(lesson.discovery.as_ref().unwrap().quick_choices().len(), 3);
    let first = turn(&mut lesson, "A pattern", "one#1", Value::Null).unwrap();
    assert_eq!(first, PATTERN);
    assert!(lesson
        .discovery
        .as_ref()
        .unwrap()
        .quick_choices()
        .is_empty());
    let next = turn(&mut lesson, "8", "one#2", Value::Null).unwrap();
    assert!(next.contains("before 2"));
    assert!(lesson.discovery.as_ref().unwrap().profile.facts.is_empty());

    let mut word = fresh();
    turn(&mut word, "A word puzzle", "one#1", Value::Null).unwrap();
    assert!(turn(&mut word, "raindrop", "one#2", Value::Null)
        .unwrap()
        .contains("raindrop"));

    let mut topic = fresh();
    assert_eq!(
        turn(&mut topic, "My own topic", "one#1", Value::Null).unwrap(),
        TOPIC
    );
}

const GUIDE: &str = "Make a card that helps new bike-club members predict wheel turns from pedal turns. Try it with a reader, then revise it using their feedback.";
const GUIDE_CRITERION: &str = "Explain the ratio correctly and report what a new member could do with the card, including a revision based on their feedback.";
const FIRST_DRAFT: &str = "The wheel turns three times for each pedal turn. Two pedal turns give six wheel turns. I have not tried the card with a reader yet.";
const REVISION: &str = "Jo tried my card but reversed the columns. I added pedal and wheel labels. Using the revised card, Jo correctly predicted nine wheel turns for three pedal turns.";

// Persist the last exchange of each fictional visit and resume from its journal.
fn reopened(lesson: Lesson, day: u64, words: &str, reply: &str) -> Lesson {
    use crate::session::{replay, Event, Journal, Visit};
    let folder = std::env::temp_dir().join(format!("robotutor-charter-{}", uuid::Uuid::new_v4()));
    tutor_lab::platform::create_private_dir(&folder, false).unwrap();
    let path = folder.join(format!("day-{day}.jsonl"));
    let mut journal = Journal::open(&path, &mut Visit::default()).unwrap();
    let reason = lesson.discovery.as_ref().unwrap().reason.clone();
    let at = day * 86_400;
    for event in [
        Event::Open {
            version: 1,
            selected: None,
        },
        Event::Learner { text: words.into() },
        Event::Delivered {
            text: reply.into(),
            lesson: Box::new(lesson),
            sources: vec![],
            judgment: None,
        },
    ] {
        journal.save_at(&event, at).unwrap();
    }
    drop(journal);
    let restored = replay(&path).unwrap();
    assert_eq!(restored.discovery_trace.get(&1), Some(&reason));
    assert_eq!(restored.history[0], (Some(words.into()), reply.into()));
    std::fs::remove_file(&path).unwrap();
    #[cfg(windows)]
    std::fs::remove_file(path.with_extension("jsonl.lock")).unwrap();
    std::fs::remove_dir(folder).unwrap();
    restored.lesson
}

// Model proposals are scripted. Evidence classification, correction, proposal
// acceptance, the frozen rubric, and the teaching context use the tutor code.
fn bike_club_visits() -> Lesson {
    let mut lesson = fresh();
    assert_eq!(
        turn(&mut lesson, "A pattern", "day01#1", Value::Null).unwrap(),
        PATTERN
    );
    let intake = lesson.discovery.as_ref().unwrap();
    assert!(intake.profile.facts.is_empty());
    assert!(intake.proposal.is_none());

    let mut lesson = reopened(lesson, 1, "A pattern", PATTERN);
    let words = "I want to show the bike club how gears work.";
    let mut reflect = response("reflect", "You want to explain gears to the bike club.");
    reflect["findings"] = json!([finding("reported", "", words)]);
    turn(&mut lesson, words, "day02#1", reflect).unwrap();
    turn(
        &mut lesson,
        "looks right",
        "day02#2",
        response(
            "diagnostic",
            "There are 12 turns. How many turns does the wheel make?",
        ),
    )
    .unwrap();
    let mut reading = response("follow", "We could practice converting turns.");
    let mut difficulty = finding("difficulty", "Graphs", "I can't tell.");
    difficulty["reading"] = json!("May need help converting turns.");
    reading["findings"] = json!([difficulty]);
    turn(&mut lesson, "I can't tell.", "day02#3", reading).unwrap();
    let mut flawed = response("propose", "Try computing the wheel turns.");
    flawed["start"]["exercise"] = json!("The bike makes 12 turns. How many wheel turns is that?");
    flawed["start"]["sources"] = json!(["day02#3.1"]);
    turn(&mut lesson, "/start-now", "day02#4", flawed).unwrap();
    let clarification = "Twelve turns of which part? The question leaves that out.";
    let mut repair = response("propose", "You're right. I left out what was turning.");
    let mut corrected = finding("correction", "Graphs", clarification);
    corrected["replaces"] = json!(["day02#3.1"]);
    repair["reason"] = json!("The original question omitted the input quantity.");
    repair["start"]["why"] = json!("Naming both parts makes the question answerable.");
    repair["findings"] = json!([corrected]);
    repair["start"]["sources"] = json!(["current"]);
    repair["start"]["exercise"] = json!(
        "One pedal turn gives three wheel turns. How many wheel turns follow two pedal turns?"
    );
    let repaired = turn(&mut lesson, clarification, "day02#5", repair).unwrap();
    assert!(repaired.contains("One pedal turn gives three wheel turns"));
    assert!(repaired.contains("Earlier account, since corrected"));
    assert_eq!(
        lesson
            .discovery
            .as_ref()
            .unwrap()
            .proposal
            .as_ref()
            .unwrap()
            .sources,
        ["day02#5.1"]
    );

    let mut lesson = reopened(lesson, 2, clarification, &repaired);
    turn(
        &mut lesson,
        "Let's explore first.",
        "day03#1",
        response("explore", "Let's try the graph."),
    )
    .unwrap();
    turn(
        &mut lesson,
        "Ready",
        "day03#2",
        response(
            "diagnostic",
            "One pedal turn gives three wheel turns. What point shows two pedal turns?",
        ),
    )
    .unwrap();
    turn(
        &mut lesson,
        "A hint, please.",
        "day03#3",
        response(
            "hint",
            "Put pedal turns first. Two groups of three give six. Which point is that?",
        ),
    )
    .unwrap();
    let mut follow = response(
        "follow",
        "We can use that diagram again or try a different application.",
    );
    follow["findings"] = json!([finding("demonstrated", "Graphs", "(2,6)")]);
    let reply = turn(&mut lesson, "(2,6)", "day03#4", follow).unwrap();
    assert_eq!(
        lesson.discovery.as_ref().unwrap().profile.facts["day03#4.1"].kind,
        "cued"
    );

    let mut lesson = reopened(lesson, 3, "(2,6)", &reply);
    let choice = "May we begin the bike-club card now?";
    let mut project = response("provisional", "Try the card, or keep exploring the graph.");
    project["early_request"] = json!(choice);
    project["findings"] = json!([finding("reported", "", choice)]);
    project["start"] = json!({"objective":"Explain a gear ratio to a new club member", "exercise":GUIDE,
        "criterion":GUIDE_CRITERION,"why":"A new rider needs to know which part is turning.","sources":["current"]});
    let offer = turn(&mut lesson, choice, "day31#1", project).unwrap();
    assert!(offer.contains(REVIEW));
    assert!(lesson.exercise.is_empty());
    let mut alternative = lesson.clone();
    turn(
        &mut alternative,
        "Let's keep exploring.",
        "day31#2",
        response("explore", "Which gear should we look at?"),
    )
    .unwrap();
    assert!(alternative.exercise.is_empty());
    assert!(alternative.discovery.as_ref().unwrap().proposal.is_none());
    let start = turn(&mut lesson, "start", "day31#2", Value::Null).unwrap();
    assert_eq!(lesson.exercise, GUIDE);
    let plan = lesson.discovery.as_ref().unwrap().teaching_context();
    let observations = plan["observations"].as_array().unwrap();
    assert!(observations
        .iter()
        .any(|e| e["source"] == "day02#5.1" && e["learner_quote"] == clarification));
    assert!(observations.iter().any(|e| e["source"] == "day03#4.1"
        && e["kind"] == "cued"
        && e["tutor_prompt"].as_str().unwrap().contains("Two groups")));
    assert!(observations.iter().all(|e| e["source"] != "day02#3.1"));
    reopened(lesson, 31, "start", &start)
}

#[test]
fn educational_charter_across_five_visits() {
    let mut lesson = bike_club_visits();
    assert_eq!(lesson.rubric.as_ref().unwrap().criterion, GUIDE_CRITERION);
    let mut requests = vec![
        json!({"action":"partial","evidence":"I have not tried the card with a reader yet.","note":"The calculation is correct. Audience use is still open.","target_stage":-1}),
        json!({"turn":{"move":"coach","feedback":"Your example predicts six wheel turns. What remains open is whether the labels work for a new rider.","exercise":"What happened when a rider used the card?"}}),
        json!({"verdict":"accept","reason":""}),
    ].into_iter();
    let feedback = lesson
        .respond(Some(FIRST_DRAFT), &mut |_, _| Ok(requests.next().unwrap()))
        .unwrap();
    assert!(feedback.contains("rider"));
    assert_eq!(lesson.exercise, GUIDE);
    assert_eq!(lesson.observation.as_ref().unwrap().action, "partial");
    assert_eq!(lesson.rubric.as_ref().unwrap().criterion, GUIDE_CRITERION);

    let revised_evidence = "Jo correctly predicted nine wheel turns";
    let mut revision = vec![
        json!({"action":"complete","evidence":revised_evidence,"note":"The reader used the revised labels successfully.","target_stage":-1}),
        json!({"turn":{"move":"advance","bridge_and_instruction":"The labels helped Jo tell the columns apart. A different gear changes the relationship.", "exercise":"How would you adapt the card for a gear with two wheel turns per pedal turn?", "next_objective":"Compare two gear ratios."}}),
        json!({"verdict":"accept","reason":""}),
    ].into_iter();
    lesson
        .respond(Some(REVISION), &mut |_, _| Ok(revision.next().unwrap()))
        .unwrap();
    let observed = lesson.observation.as_ref().unwrap();
    assert_eq!(observed.quote, revised_evidence);
    assert_eq!(observed.action, "complete");
    assert!(observed.assisted);
    assert_eq!(observed.validation, "luna_only");
}

#[test]
#[ignore = "makes two live Jev checks of fictional audience feedback"]
fn educational_charter_audience_evidence() {
    let lesson = bike_club_visits();
    let config = tutor_lab::openrouter::Config::from_env().unwrap();
    let draft = crate::assessment::verify(&lesson, FIRST_DRAFT, "partial", &config, None);
    assert_eq!(draft.method, "independent_agreement");
    let mut revised = lesson;
    revised.prior_attempts.push(FIRST_DRAFT.into());
    let result = crate::assessment::verify(&revised, REVISION, "complete", &config, None);
    assert_eq!(result.method, "independent_agreement");
}
