//! Compare a learner's time budget and the available instruction for a reviewed goal.
#[allow(
    dead_code,
    reason = "this intake opens only the existing standalone workshop"
)]
#[path = "goal_program/chat.rs"]
mod chat;

#[path = "curriculum_investigation/applied.rs"]
mod applied;
#[path = "curriculum_investigation/assessment.rs"]
mod assessment;
#[path = "curriculum_investigation/missing.rs"]
mod missing;
#[path = "curriculum_investigation/returns.rs"]
mod returns;

use serde_json::Value;
use std::{
    collections::BTreeMap,
    env,
    io::{self, BufRead, Write},
};
use tutor_lab::{
    curriculum::{
        availability::{introductory_sample, Availability, MaterialForecast},
        creation::MAX_ROWS,
        goal_rehearsal::{run, Options},
        program::{Frame, Proposal, Unit},
        rehearsal_release::publish,
    },
    material::document_id,
};

const REVIEWS: &str = include_str!("../fixtures/goals/reviews.json");

struct Course {
    raw: &'static str,
    pin: String,
    data: Value,
    frame: Frame,
    plan: Proposal,
}

impl Course {
    fn load(subject: &str) -> Result<Self, String> {
        let raw = match subject {
            "writer" => include_str!("../fixtures/goals/writer.json"),
            "violin" => include_str!("../fixtures/goals/violin.json"),
            "worship" => include_str!("../fixtures/goals/worship.json"),
            "better-person" => include_str!("../fixtures/goals/better-person.json"),
            "chemical-engineering" => include_str!("../fixtures/goals/chemical-engineering.json"),
            _ => {
                return Err(
                    "choose writer, violin, worship, better-person or chemical-engineering".into(),
                )
            }
        };
        let pins: Value = serde_json::from_str(REVIEWS).map_err(|e| e.to_string())?;
        let pin = pins[subject]
            .as_str()
            .ok_or("no pinned scenario")?
            .to_owned();
        let actual: String = document_id(raw.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != pin {
            return Err("scenario changed since review".into());
        }
        let data: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        let frame = Frame::from_json(&data)?;
        let plan = Proposal::from_json(&data["proposal"])?;
        Ok(Self {
            raw,
            pin,
            data,
            frame,
            plan,
        })
    }

    fn proposal(&self) {
        println!(
            "Possible purpose: {}\nSetting: {}\nExample horizon: {}",
            self.frame.scope.purpose, self.frame.scope.jurisdiction, self.frame.scope.horizon
        );
    }

    fn first_unit(&self) -> Result<&Unit, String> {
        let id = self.data["first_pack"]["unit"]
            .as_u64()
            .ok_or("no first lesson released")?;
        self.plan
            .units()
            .iter()
            .find(|unit| u64::from(unit.id) == id && unit.requires.is_empty())
            .ok_or_else(|| "first lesson must be eligible for a new learner".into())
    }

    fn sample(&self) -> Result<String, String> {
        introductory_sample(&self.data, &self.frame, &self.plan)
    }

    fn visit(&self, minutes: u32) -> Result<String, String> {
        if minutes == 0 {
            return Ok("Stop here. You can come back when you have time.".into());
        }
        let unit = self.first_unit()?;
        if minutes < unit.minutes {
            return Ok(format!(
                "This full unit is planned for {} minutes. Here is an optional worked example.\n{}",
                unit.minutes,
                self.sample()?
            ));
        }
        run(
            self.raw,
            &self.pin,
            &Options {
                unit: Some(unit.id),
                ..Options::default()
            },
        )
    }

    fn materials(&self) -> Result<MaterialForecast<'_>, String> {
        MaterialForecast::from_pinned(&self.data, &self.frame, &self.plan)
    }

    // Inspect all overlapping units before dividing work between the author and
    // source reviewer. The existing creation window caps one query at MAX_ROWS.
    fn prepare(
        &self,
        elapsed: u32,
        width: u32,
        slots: usize,
    ) -> Result<Vec<(u32, Availability)>, String> {
        let budget = self
            .frame
            .calendar
            .ok_or("missing calendar")?
            .forward_minutes();
        let schedule = self
            .frame
            .sequence(&self.plan, budget)
            .map_err(|e| format!("{e:?}"))?;
        let by_id: BTreeMap<_, _> = self
            .plan
            .units()
            .iter()
            .map(|unit| (unit.id, unit))
            .collect();
        let mut offset = 0;
        let mut ends = BTreeMap::new();
        for id in schedule.scheduled {
            offset += by_id[&id].minutes;
            ends.insert(id, offset);
        }
        let stop = elapsed.saturating_add(width).min(offset);
        let mut cursor = elapsed;
        let materials = self.materials()?;
        let mut chosen = Vec::new();
        while cursor < stop && chosen.len() < slots {
            let ids = self
                .frame
                .creation_window(&self.plan, budget, cursor, stop - cursor, MAX_ROWS)
                .map_err(|e| format!("{e:?}"))?;
            let last = ids.last().ok_or("nonempty calendar interval has no unit")?;
            cursor = *ends.get(last).ok_or("unit outside scheduled interval")?;
            for id in ids {
                let available = materials.status(id)?;
                if matches!(available, Availability::Brief | Availability::NeedsSource) {
                    chosen.push((id, available));
                    if chosen.len() == slots {
                        break;
                    }
                }
            }
        }
        Ok(chosen)
    }
}

const CHOICES: &str = "Choose sample for one worked example, visit MINUTES for today, forecast MINUTES for a course outline, work for your own piece, or quit.";

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let subject = args.next().ok_or("supply a goal name")?;
    if subject == "return-options" && args.len() == 0 {
        return returns::run();
    }
    let course = Course::load(&subject)?;
    let script = args.collect::<Vec<_>>().join(" ");
    if subject == "worship"
        && matches!(
            script.as_str(),
            "source-voices granted" | "source-voices held"
        )
    {
        return worship_sources(&course, script.ends_with("granted"));
    }
    if script == "applied-evidence" {
        return applied::examine(&course, &subject);
    }
    if script == "reflection" && subject == "better-person" {
        println!("What might you choose to do in your situation? Write one private line, or press Enter to pass.");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        std::io::Read::take(io::stdin().lock(), 4097)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        if line.len() > 4096 {
            return Err("private reflection exceeds 4096 bytes".into());
        }
        if let Some(text) = missing::reflection(line.trim_end_matches(['\r', '\n']))? {
            println!("{text}");
        }
        return Ok(());
    }
    if script == "missing-lesson" && subject == "better-person" {
        return missing::examine(&course);
    }
    if script == "editorial-review" && subject == "writer" {
        return assessment::examine(&course);
    }
    if matches!(
        script.as_str(),
        "compare-checks reviewed" | "compare-checks reuse"
    ) {
        println!(
            "{}",
            assessment::compare_checks(&course, &subject, script.ends_with("reuse"))?
        );
        return Ok(());
    }
    let interactive = script.is_empty();
    let mut input = io::stdin().lock();
    let mut choice = script;
    if interactive {
        println!(
            "{}\n{CHOICES}",
            course.data["intake_question"]
                .as_str()
                .ok_or("missing intake question")?
        );
    }
    loop {
        if interactive {
            choice.clear();
            io::stdout().flush().map_err(|e| e.to_string())?;
            if std::io::Read::take(&mut input, 4097)
                .read_line(&mut choice)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Ok(());
            }
            if choice.len() > 4096 {
                return Err("intake choice exceeds 4096 bytes".into());
            }
        }
        let words: Vec<_> = choice.split_whitespace().collect();
        match words.as_slice() {
            ["quit" | "/quit"] => return Ok(()),
            ["sample"] => println!("{}", course.sample()?),
            [mode @ ("visit" | "forecast"), minutes] => {
                let minutes = minutes
                    .parse::<u32>()
                    .map_err(|_| "expected unsigned available minutes")?;
                if *mode == "visit" {
                    println!("{}", course.visit(minutes)?);
                } else {
                    course.proposal();
                    println!("{}", course.materials()?.forecast(minutes)?);
                }
            }
            ["prepare", start, width, slots] => {
                let elapsed = start
                    .parse()
                    .map_err(|_| "expected elapsed forward minutes")?;
                let width = width
                    .parse()
                    .map_err(|_| "expected lookahead in forward minutes")?;
                let slots = slots
                    .parse()
                    .map_err(|_| "expected available staff slots")?;
                let chosen = course.prepare(elapsed, width, slots)?;
                println!("{} preparation actions in this forecast.", chosen.len());
                for (id, available) in chosen {
                    println!("Unit {id} · {}", available.label());
                }
            }
            ["work"] => {
                return chat::session(
                    &mut input,
                    &mut io::stdout().lock(),
                    None,
                    chat::Start::Workshop,
                    None,
                )
            }
            _ => {
                println!("What would help today? You can leave the longer course open.\n{CHOICES}");
                if interactive {
                    continue;
                }
                return Ok(());
            }
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editorial_comparisons_show_the_prior_prompt_only_in_the_authorized_brief() {
        for subject in ["writer", "chemical-engineering"] {
            let course = Course::load(subject).unwrap();
            let prior = &course.data["first_pack"];
            for reuse in [false, true] {
                let result: Value = serde_json::from_str(
                    &assessment::compare_checks(&course, subject, reuse).unwrap(),
                )
                .unwrap();
                let original = &result["original_requests"]["jev"]["state"];
                let changed = &result["comparison_requests"];
                let commission: Value = serde_json::from_str(
                    changed["luna"]["messages"][1]["content"].as_str().unwrap(),
                )
                .unwrap();
                assert!(original["commission"].get("comparison").is_none());
                let comparison = &commission["comparison"];
                assert_eq!(comparison.as_object().unwrap().len(), 3);
                assert_eq!(comparison["prior_published_check"], prior["check"]);
                assert_eq!(
                    comparison["shared_criterion"],
                    course.data["criterion_claim"]
                );
                assert_eq!(commission, changed["jev"]["state"]["commission"]);
                assert_eq!(original["candidate"], changed["jev"]["state"]["candidate"]);
                for field in ["scope_reply", "attempts", "practice_response"] {
                    assert!(!changed.to_string().contains(field));
                }
                assert_eq!(result["editor_report_status"], "unpublished");
                assert_eq!(result["learner_read"], "Unapproved");
                if subject == "chemical-engineering" {
                    assert_eq!(
                        result["public_tank_answer_on_candidate"],
                        if reuse { "Correct" } else { "Incorrect" }
                    );
                }
            }
        }
    }

    #[test]
    fn single_visit_and_calendar_use_distinct_time_budgets() {
        let c = Course::load("writer").unwrap();
        let calendar = c.frame.calendar.unwrap();
        assert!(c
            .frame
            .sequence(&c.plan, calendar.forward_within(25).unwrap())
            .unwrap()
            .scheduled
            .is_empty());
        assert!(c.visit(25).unwrap().contains("Guided practice"));
        assert_eq!(
            c.visit(0).unwrap(),
            "Stop here. You can come back when you have time."
        );
        assert!(c.visit(24).unwrap().contains("optional worked example"));
    }

    #[test]
    fn preparing_a_window_skips_released_units_before_limiting_jobs() {
        let c = Course::load("writer").unwrap();
        assert!(c.prepare(0, 25, 1).unwrap().is_empty());
        assert!(c.prepare(0, 75, 0).unwrap().is_empty());
        assert_eq!(
            c.prepare(0, 75, 1).unwrap(),
            vec![(102, Availability::Brief)]
        );
        let c = Course::load("chemical-engineering").unwrap();
        let jobs = c.prepare(0, u32::MAX, usize::MAX).unwrap();
        assert_eq!(jobs.len(), 17);
        assert!(jobs.iter().all(|(_, a)| *a == Availability::NeedsSource));
        assert!(c.prepare(u32::MAX, 10, 1).unwrap().is_empty());
    }

    #[test]
    fn samples_only_show_reviewed_worked_material_and_its_attribution() {
        for subject in [
            "writer",
            "violin",
            "chemical-engineering",
            "better-person",
            "worship",
        ] {
            let c = Course::load(subject).unwrap();
            let sample = c.sample().unwrap();
            let record = &c.data["first_pack"];
            for key in ["worked", "locator", "source_url"] {
                assert!(
                    sample.contains(record[key].as_str().unwrap()),
                    "{subject}: {key}"
                );
            }
            for key in ["check", "rubric"] {
                assert!(
                    !sample.contains(record[key].as_str().unwrap()),
                    "{subject}: {key}"
                );
            }
            assert!(!sample.contains(c.data["scope_reply"].as_str().unwrap()));
        }
    }
}

/// The shared lesson draws only on the named publisher. An optional private account
/// enters the local comparison after release, without changing the author job or rubric.
fn worship_sources(course: &Course, separately_permitted: bool) -> Result<(), String> {
    use tutor_lab::workshop::{Focus, Workshop};
    let record = tutor_lab::rehearsal::goal_choice::parish_record();
    let source_url = record["source_url"]
        .as_str()
        .ok_or("missing parish source")?;
    let excerpt = record["excerpt"].as_str().ok_or("missing parish excerpt")?;
    println!("You may enter a family account to compare locally, or press Enter to use a fictional example.");
    let mut words = String::new();
    std::io::Read::take(io::stdin().lock(), 4097)
        .read_line(&mut words)
        .map_err(|e| e.to_string())?;
    if words.len() > 4096 {
        return Err("private account exceeds 4096 bytes".into());
    }
    let own = words.trim();
    let private = if own.is_empty() {
        None
    } else {
        Some(Workshop::new(own, Focus::NextStep).map_err(str::to_owned)?)
    };
    if separately_permitted {
        let published = publish(
            &course.frame,
            &course.plan,
            course.frame.review_digest(&course.plan),
            &record,
        )?;
        let author_hash = document_id(published.request().as_bytes());
        eprintln!(
            "Shared author job: {}",
            author_hash
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let material = published.material(true)?;
        println!(
            "{}
“{excerpt}”
{source_url}
{}
{}",
            material.lesson, material.worked, material.practice
        );
    } else {
        eprintln!("No parish author request.");
        println!("The parish excerpt is unavailable in this rehearsal. Here is a reading exercise from a separately released source.
{}", course.sample()?);
    }
    if let Some(workshop) = private {
        println!(
            "You described your family's practice this way: “{}”",
            workshop.draft()
        );
    }
    match (separately_permitted,own.is_empty()) {
        (true,false) => println!("Which detail comes from your household? Which word in the parish guidance leaves room for a different practice? You may leave your account here."),
        (true,true) => println!("In a fictional gathering, a leader names someone in need and the room stays quiet. What did the description tell you about this group?"),
        (false,_) => println!("In the fictional example, which words came from the psalm and which detail came from the imagined scene?"),
    }
    Ok(())
}
