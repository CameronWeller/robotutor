//! Rehearse the path from a vague learner goal to a first lesson. The example consumes
//! fixed, fictional editorial decisions so that a changed draft cannot approve itself.
#[path = "law_program/first_lesson.rs"]
mod first_lesson;

use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tutor_lab::{
    curriculum::{
        author_input::AuthorInput,
        creation::{Batch, Candidate, ReviewerGrant, Work, MAX_ROWS},
        discovery, live_author,
        program::{Frame, Intake, Proposal},
        Document, Outcome, Progress, ReviewedUnit, Slot, Span,
    },
    material::document_id,
};

const SURVEY: &str = include_str!("../fixtures/law-survey.json");
const DISCOVERY: &str = include_str!("../fixtures/law-discovery.json");
const REVIEW: &str = include_str!("../fixtures/law-review.json");
const USAGE: &str = "law_program [--intake | --jurisdiction NAME | --calendar-minutes N | --discovery-request | --author-request [--openrouter] | --candidate FILE | --learner presentment|signature]";

enum Run {
    Full,
    Intake,
    Jurisdiction(String),
    Calendar(u32),
    Discovery,
    Author,
    LiveAuthor,
    Candidate(PathBuf),
    Learner(first_lesson::View),
}

fn mode() -> Result<Run, String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => Ok(Run::Full),
        [s] if s == "--intake" => Ok(Run::Intake),
        [s] if s == "--discovery-request" => Ok(Run::Discovery),
        [s] if s == "--author-request" => Ok(Run::Author),
        [a, b]
            if (a == "--author-request" && b == "--openrouter")
                || (a == "--openrouter" && b == "--author-request") =>
        {
            Ok(Run::LiveAuthor)
        }
        [s, v] if s == "--calendar-minutes" => Ok(Run::Calendar(v.parse().map_err(|_| USAGE)?)),
        [s, v] if s == "--jurisdiction" && !v.trim().is_empty() => Ok(Run::Jurisdiction(v.clone())),
        [s, v] if s == "--candidate" => Ok(Run::Candidate(v.into())),
        [s, v] if s == "--learner" && v == "presentment" => {
            Ok(Run::Learner(first_lesson::View::LearnerCorrect))
        }
        [s, v] if s == "--learner" && v == "signature" => {
            Ok(Run::Learner(first_lesson::View::LearnerSignature))
        }
        _ => Err(USAGE.into()),
    }
}

fn field<'a>(row: &'a Value, key: &str) -> Result<&'a str, String> {
    row[key]
        .as_str()
        .ok_or_else(|| format!("missing text for {key}"))
}

fn hex(hash: Document) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn check_pin(digest: Document, review: &Value, name: &str, problem: &str) -> Result<(), String> {
    if hex(digest) != field(review, name)? {
        return Err(problem.into());
    }
    Ok(())
}

fn read_candidate(path: &Path) -> Result<Value, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(65537).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read author reply: {e}"))?;
    if bytes.len() > 65536 {
        return Err("author reply exceeds 64 KiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid author reply: {e}"))
}

fn prepare_job(
    frame: &Frame,
    plan: &Proposal,
    record: &Value,
    review: &Value,
) -> Result<(Work, BTreeSet<Span>, AuthorInput), String> {
    let id = u32::try_from(record["unit"].as_u64().ok_or("missing first-unit ID")?)
        .map_err(|_| "invalid unit ID")?;
    let excerpt = field(record, "excerpt")?;
    let source_id = document_id(excerpt.as_bytes());
    let cited = json!([
        field(record, "source_url")?,
        field(record, "locator")?,
        source_id
    ]);
    check_pin(
        document_id(cited.to_string().as_bytes()),
        review,
        "source_pin",
        "the cited source, locator or excerpt changed after review",
    )?;
    let live = BTreeSet::from([Span {
        document: source_id,
        start: 0,
        end: u32::try_from(excerpt.len()).map_err(|_| "excerpt too large")?,
    }]);
    let plan_digest = frame.review_digest(plan);
    check_pin(
        plan_digest,
        review,
        "program_digest",
        "the changed program needs an independent outline review",
    )?;
    let work = frame.commission(plan, plan_digest, id, &live)?;
    let corpus = BTreeMap::from([(source_id, excerpt.as_bytes())]);
    let job = frame.author_input(plan, plan_digest, work, &live, &corpus)?;
    Ok((work, live, job))
}

fn release_first_lesson(
    frame: &Frame,
    plan: &Proposal,
    record: &Value,
    review: &Value,
    mode: &Run,
) -> Result<(), String> {
    let (work, live, job) = prepare_job(frame, plan, record, review)?;
    if matches!(mode, Run::LiveAuthor) {
        println!("{}", live_author::dispatch(&job)?);
        return Ok(());
    }
    if matches!(mode, Run::Author) {
        println!("{}", job.wire());
        return Ok(());
    }
    let reply = match mode {
        Run::Candidate(path) => read_candidate(path)?,
        _ => {
            let mut body = serde_json::Map::new();
            for name in [
                "lesson",
                "worked",
                "practice",
                "check",
                "criterion",
                "rubric",
            ] {
                body.insert(name.into(), json!(field(record, name)?));
            }
            json!({"job":job.job(),"pack":body})
        }
    };
    let pack = job.decode(&reply)?;
    let mut batch =
        Batch::new(frame.revision, [work]).map_err(|e| format!("commission failed: {e:?}"))?;
    let author = review["author"]
        .as_u64()
        .ok_or("missing simulated author")?;
    let reviewer = review["reviewer"]
        .as_u64()
        .ok_or("missing simulated reviewer")?;
    let receipt = batch
        .stage(vec![Candidate { work, author, pack }])
        .map_err(|e| format!("batch failed: {e:?}"))?
        .remove(0)
        .map_err(|e| format!("draft failed: {e:?}"))?;
    if batch.available(work, &live).is_ok() {
        return Err("unreviewed draft unexpectedly available".into());
    }
    if hex(receipt) != field(review, "draft_receipt")? {
        if matches!(mode, Run::Candidate(_)) {
            println!("The replacement passed draft validation. Hold it for an independent review of the new material. No lesson was served.");
            return Ok(());
        }
        return Err("the reference material changed after its simulated review".into());
    }
    batch
        .approve(
            work,
            receipt,
            &ReviewerGrant {
                actor: reviewer,
                work,
            },
            &live,
        )
        .map_err(|e| format!("approval failed: {e:?}"))?;
    let released = batch
        .available(work, &live)
        .map_err(|e| format!("release unavailable: {e:?}"))?;
    let endpoint = work.objective;
    let first = plan
        .units()
        .iter()
        .find(|u| u.id == endpoint.0)
        .ok_or("unknown first unit")?;
    if !first.requires.is_empty() {
        return Err(
            "this first-visit demonstration needs a reviewed entry unit with no prerequisites"
                .into(),
        );
    }
    let unit = ReviewedUnit::build(
        frame.revision,
        vec![Outcome {
            id: endpoint,
            requires: BTreeSet::new(),
            support: live.clone(),
        }],
    )
    .map_err(|e| format!("invalid local unit: {e:?}"))?;
    let progress = Progress {
        revision: frame.revision,
        checkpoints: Default::default(),
        postponed: Default::default(),
    };
    let next = unit
        .next(endpoint, &progress, 0, &BTreeSet::new(), Slot::Forward)
        .map_err(|e| format!("{e:?}"))?;
    if next != tutor_lab::curriculum::Decision::Teach(endpoint) {
        return Err("the first lesson does not have an eligible teaching decision".into());
    }
    let view = match mode {
        Run::Learner(view) => *view,
        _ => {
            println!("\nOne of {} units has a pack released by the simulated reviewer. Current instructional decision {next:?}.",plan.units().len());
            first_lesson::View::Walkthrough
        }
    };
    first_lesson::teach(
        released,
        field(record, "source_url")?,
        field(record, "locator")?,
        field(record, "excerpt")?,
        view,
    )?;
    if matches!(mode, Run::Learner(_)) {
        return Ok(());
    }
    let gone = live.iter().map(|s| s.document).collect();
    let next = unit
        .next(endpoint, &progress, 7, &gone, Slot::Forward)
        .map_err(|e| format!("{e:?}"))?;
    if batch.available(work, &BTreeSet::new()).is_ok() {
        return Err("withdrawn source still served".into());
    }
    println!("\nSource-withdrawal alternative on day 7 · {next:?}. The outline survives and that lesson is held.");
    Ok(())
}

fn run(mode: Run) -> Result<(), String> {
    let data: Value = serde_json::from_str(SURVEY).map_err(|e| e.to_string())?;
    let review: Value = serde_json::from_str(REVIEW).map_err(|e| e.to_string())?;
    let evidence: Value = serde_json::from_str(DISCOVERY).map_err(|e| e.to_string())?;
    let frame = Frame::from_json(&data)?;
    let transcript = !matches!(
        mode,
        Run::Author | Run::LiveAuthor | Run::Discovery | Run::Learner(_)
    );
    if transcript && frame.intake(&frame.scope.goal, None) == Intake::AskForScope {
        println!("Learner · “{}”", frame.scope.goal);
        println!("Tutor · Where would you like to start geographically? Are you here for a broad education, a particular problem, or an exam? How much time would you like to spend?");
    }
    if matches!(mode, Run::Intake) {
        return Ok(());
    }
    let mut confirmed = frame.scope.clone();
    if let Run::Jurisdiction(jurisdiction) = &mode {
        confirmed.jurisdiction = jurisdiction.clone();
    }
    if transcript {
        println!(
            "\nSimulated learner confirmation\n{}\n{}\n{}",
            confirmed.purpose, confirmed.jurisdiction, confirmed.horizon
        );
    }
    if frame.intake(&frame.scope.goal, Some(&confirmed)) != Intake::Confirmed {
        println!("Commission a breadth map for the confirmed setting before assigning jurisdiction-specific lessons.");
        return Ok(());
    }
    if matches!(mode, Run::Discovery) {
        println!("{}", discovery::initial_packet(&evidence)?);
        return Ok(());
    }
    let discovery = discovery::rehearse(&evidence, &frame)?;
    if transcript {
        println!(
            "\nDiscovery · {} source cards. First draft missing strands {:?}.",
            discovery.source_count, discovery.initial_missing
        );
        for prompt in discovery.initial_challenges.values() {
            println!("Independent challenge · {prompt}");
        }
        println!(
            "After repair, outstanding omissions {:?}; editor-proposed additions {:?}.",
            discovery.repaired_missing, discovery.provisional
        );
    }
    if !discovery.ready_for_editorial_review() {
        return Err("the proposed scope still has open breadth gaps".into());
    }
    check_pin(
        document_id(evidence.to_string().as_bytes()),
        &review,
        "discovery_digest",
        "the proposed breadth map differs from the simulated scope agreement",
    )?;
    let plan = Proposal::from_json(&data["proposal"])?;
    let calendar = frame.calendar.ok_or("law simulation needs a calendar")?;
    let total = if let Run::Calendar(m) = &mode {
        *m
    } else {
        calendar.total_minutes()
    };
    let forward = calendar
        .forward_within(total)
        .ok_or("a longer horizon needs a new scope decision")?;
    let reserve = total - forward;
    let schedule = frame
        .sequence(&plan, forward)
        .map_err(|e| format!("outline held · {e:?}"))?;
    if transcript {
        let ids: BTreeSet<_> = schedule.scheduled.iter().copied().collect();
        let covered = frame
            .facets
            .iter()
            .filter(|f| {
                plan.units()
                    .iter()
                    .filter(|u| u.facet == f.id && ids.contains(&u.id))
                    .map(|u| u.minutes)
                    .sum::<u32>()
                    >= f.min_minutes
            })
            .count();
        println!("\nForecast · {total} calendar minutes, {reserve} protected, {} forward minutes allocated.", schedule.used_minutes);
        println!("{} of {} proposed units fit. {covered} of {} strands reach their time floor in this window.",ids.len(),plan.units().len(),frame.facets.len());
        for (i, id) in schedule.scheduled.iter().enumerate() {
            let u = plan
                .units()
                .iter()
                .find(|u| u.id == *id)
                .expect("audited unit");
            println!("{:2}. {} · {} · {} min", i + 1, u.id, u.title, u.minutes);
        }
        if !schedule.deferred.is_empty() {
            println!("Deferred unit IDs · {:?}", schedule.deferred);
        }
    }
    let window = frame
        .creation_window(&plan, forward, 0, 160, MAX_ROWS)
        .map_err(|e| format!("invalid window: {e:?}"))?;
    if transcript {
        println!("First 160 forward minutes · prepare or reuse material for unit IDs {window:?}.");
    }
    let Some(&first) = window.first() else {
        println!("Offer to extend the window or agree a narrower first objective. No unit has been scheduled.");
        return Ok(());
    };
    if data["first_pack"]["unit"].as_u64() != Some(u64::from(first)) {
        println!("Unit {first} needs a new evidence-backed teaching pack.");
        return Ok(());
    }
    if matches!(mode, Run::Learner(_)) {
        println!("Your U.S. law survey · lesson 1 of {} planned units\nToday you’ll read Article I and decide which part of a headline its words support. The next planned unit asks whether a valid rule can be unjust. Its readings are still in preparation.\n",plan.units().len());
    }
    release_first_lesson(&frame, &plan, &data["first_pack"], &review, &mode)
}

#[cfg(test)]
mod walkthrough {
    use super::*;

    #[test]
    fn reviewed_law_edition_still_runs_from_intake_to_feedback_and_withdrawal() {
        run(Run::Full).unwrap();
    }

    #[test]
    fn a_changed_source_locator_or_performance_cannot_inherit_review() {
        let data: Value = serde_json::from_str(SURVEY).unwrap();
        let review: Value = serde_json::from_str(REVIEW).unwrap();
        let frame = Frame::from_json(&data).unwrap();
        let plan = Proposal::from_json(&data["proposal"]).unwrap();
        let mut record = data["first_pack"].clone();
        record["locator"] = json!("Article II");
        assert!(prepare_job(&frame, &plan, &record, &review).is_err());

        let mut change = plan.wire();
        change["units"][0]["performance"] = json!("Evaluate a newspaper's account of a state law.");
        let edited = Proposal::from_json(&change).unwrap();
        assert!(prepare_job(&frame, &edited, &data["first_pack"], &review).is_err());
    }
}

fn main() -> ExitCode {
    match mode().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
