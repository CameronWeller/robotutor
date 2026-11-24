//! One new goal for which the reviewed outline exists and the chosen lesson does not.
use super::Course;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::{
    curriculum::{
        author_input::AuthorInput,
        creation::{Batch, Candidate, Error, Work},
        goal_rehearsal::{run, with_author_input, Options, View},
        Span,
    },
    material::document_id,
    workshop::{Focus, Workshop},
};

const UNIT: u32 = 105;
const OWN: &str =
    "I can stay until six at our volunteer meeting. How can I leave without deciding for everyone?";
const CARD: &str = "This fictional meeting ends at six. Both volunteers may pause or leave and may suggest returning to the question another day. A line can state the speaker's own limit and leave another person room to decline. There is no required decision to stay or leave.";

fn job(
    course: &Course,
    passage: &str,
    shared: Option<&BTreeSet<Span>>,
) -> Result<(Work, BTreeSet<Span>, AuthorInput), String> {
    let bytes = passage.as_bytes();
    let pin = Span {
        document: document_id(bytes),
        start: 0,
        end: bytes.len() as u32,
    };
    if shared.is_some_and(|allowed| !allowed.contains(&pin)) {
        return Err("no shared-author grant for this exact excerpt".into());
    }
    let pins = BTreeSet::from([pin]);
    let work = course.frame.commission(
        &course.plan,
        course.frame.review_digest(&course.plan),
        UNIT,
        &pins,
    )?;
    let input = course.frame.author_input(
        &course.plan,
        course.frame.review_digest(&course.plan),
        work,
        &pins,
        &BTreeMap::from([(pin.document, bytes)]),
    )?;
    Ok((work, pins, input))
}

/// An expressly chosen private reflection. It produces no assessor input or shared author job.
pub(super) fn reflection(words: &str) -> Result<Option<String>, String> {
    if words.trim().is_empty() {
        return Ok(None);
    }
    let draft = Workshop::new(words, Focus::NextStep).map_err(str::to_owned)?;
    Ok(Some(format!("“{}”\nWhich part is your decision to make? What might the other person choose differently? You can keep this choice, change it, or stop.",draft.draft())))
}

pub(super) fn examine(course: &Course) -> Result<(), String> {
    let options = Options {
        unit: Some(UNIT),
        ..Options::default()
    };
    println!(
        "Requested target:\n{}",
        run(course.raw, &course.pin, &options)?
    );
    let alternative = course.sample()?;
    println!("With explicit agreement to a different activity:\n{alternative}");
    let mut sends = 0;
    let author = with_author_input(
        course.raw,
        &course.pin,
        &Options {
            view: View::AuthorUnit,
            ..options
        },
        |_| {
            sends += 1;
            Ok("would draft".into())
        },
    );
    println!("Current catalog, requested draft: {author:?}; author callbacks={sends}");

    // First experiment: even a correctly hashed piece of text may be private.
    let (_, _, unfiltered) = job(course, OWN, None)?;
    let private_in_shared = unfiltered.wire().contains(OWN);
    // New, invented teacher passage supplied and licensed for this lab case only.
    let allowed = BTreeSet::from([Span {
        document: document_id(CARD.as_bytes()),
        start: 0,
        end: CARD.len() as u32,
    }]);
    let guarded_private = job(course, OWN, Some(&allowed))
        .err()
        .ok_or("private document crossed the gate")?;
    let (work, pins, input) = job(course, CARD, Some(&allowed))?;
    let body = json!({"lesson":"Try naming what you can do and returning the other person's choice to them.",
        "worked":"I can stay until six. Would you like to pause here or keep talking with the others?",
        "practice":"Write one possible line for a volunteer with another commitment.",
        "check":"At a different meeting, you want to stop discussing a decision. Write a response that leaves each person's options clear.",
        "criterion":"Identify a limit the speaker can control and leave the other person's response open.",
        "rubric":"Accept different choices and registers, including leaving now. Ask which sentence establishes the speaker's own limit."});
    let pack = input.decode(&json!({"job":input.job(),"pack":body}))?;
    let mut batch = Batch::new(work.release, [work]).map_err(|e| format!("{e:?}"))?;
    batch
        .stage(vec![Candidate {
            work,
            author: 17,
            pack,
        }])
        .map_err(|e| format!("{e:?}"))?
        .into_iter()
        .next()
        .ok_or("draft not staged")?
        .map_err(|e| format!("{e:?}"))?;
    println!("Digest-only private text present in shared author input={private_in_shared}; scoped admission={guarded_private}");
    println!("Authorized teacher card present={}, private writing present={}, unsigned candidate unavailable={}",
        input.wire().contains(CARD),input.wire().contains(OWN),batch.available(work,&pins)==Err(Error::Unapproved));

    let workshop = Workshop::new(OWN, Focus::NextStep).map_err(str::to_owned)?;
    let feedback = workshop.feedback();
    println!(
        "Generic private coaching: {}\n{}",
        &workshop.draft()[feedback.span],
        feedback.experiment
    );
    for (intention, reply) in [
        (
            "stay",
            "I could offer five more minutes because I care about finishing together.",
        ),
        (
            "leave",
            "I could leave at six because keeping my next commitment matters to me.",
        ),
    ] {
        let rendered = reflection(reply)?.ok_or("missing reflection")?;
        println!("Private values exploration, {intention}: {rendered}");
    }
    println!("Pass: reflection shown={}", reflection("")?.is_some());
    Ok(())
}
