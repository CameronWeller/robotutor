use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const CARDS: &[(&str, &str)] = &[
    ("writer", "Revise a notice for a first-time reader."),
    ("chemical-engineering", "Read a tank mass balance."),
    ("violin", "Keep a pulse through rests on an open string."),
    ("better-person", "Practise making room for someone who was interrupted."),
    ("worship", "Read Psalm 100 and try a private expression of thanks."),
    ("washington-civil", "Follow a fictional Washington superior court civil case from pleadings to disputed evidence."),
];

pub fn card(name: &str, stage: usize) -> Result<Value, String> {
    if name == "washington-civil" {
        let mut source: Value =
            serde_json::from_str(include_str!("../../../fixtures/washington-civil.json"))
                .map_err(|e| e.to_string())?;
        let steps = source.as_object_mut().unwrap().remove("steps").unwrap();
        if let Some(step) = steps.get(stage) {
            source
                .as_object_mut()
                .unwrap()
                .extend(step.as_object().unwrap().clone());
        }
        return Ok(source);
    }
    let course = course(name)?;
    if stage >= 2 {
        let first = course["first_pack"]["unit"].as_u64();
        let returning = course["return_pack"]["unit"].as_u64();
        let published: Vec<_> = course["proposal"]["units"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|u| [first, returning].contains(&u["id"].as_u64()))
            .filter_map(|u| u["performance"].as_str())
            .collect();
        let next = program(name)?
            .into_iter()
            .filter(|(title, _)| !published.contains(&title.as_str()))
            .nth(stage - 2);
        return Ok(match next {
            Some((title, status)) => {
                json!({"objective":title,"material_status":status,"scope":course["scope"]["goal"]})
            }
            None => json!({"scope":course["scope"]["goal"]}),
        });
    }
    let pack = &course[if stage == 0 {
        "first_pack"
    } else {
        "return_pack"
    }];
    let mut out = json!({"locator":pack["locator"],"excerpt":pack["excerpt"]});
    {
        let unit = course["proposal"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == pack["unit"])
            .ok_or("Missing objective in the lesson pack.")?;
        out["objective"] = unit["performance"].clone();
        for key in ["lesson", "worked", "practice"] {
            out[key] = pack[key].clone();
        }
    }
    Ok(out)
}

pub fn syllabus(name: &str) -> Result<Vec<String>, String> {
    (0..3)
        .map(|stage| card(name, stage).map(|c| c["objective"].as_str().map(str::to_owned)))
        .collect::<Result<Vec<_>, _>>()
        .map(|steps| steps.into_iter().flatten().collect())
}

fn course(name: &str) -> Result<Value, String> {
    let raw = match name {
        "writer" => include_str!("../../../fixtures/goals/writer.json"),
        "chemical-engineering" => include_str!("../../../fixtures/goals/chemical-engineering.json"),
        "violin" => include_str!("../../../fixtures/goals/violin.json"),
        "better-person" => include_str!("../../../fixtures/goals/better-person.json"),
        "worship" => include_str!("../../../fixtures/goals/worship.json"),
        _ => return Err("Unknown lesson card.".into()),
    }
    .replace("\r\n", "\n");
    let pins: Value = serde_json::from_str(include_str!("../../../fixtures/goals/reviews.json"))
        .map_err(|e| e.to_string())?;
    if pins[name] != format!("{:x}", Sha256::digest(raw.as_bytes())) {
        return Err("The lesson pack differs from its reviewed version.".into());
    }
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

pub struct Check {
    pub id: String,
    pub prompt: String,
    pub claim: String,
    pub rubric: String,
    pub mode: String,
    pub quantity: Option<(i64, String)>,
}
pub fn check(name: &str, stage: usize) -> Result<Option<Check>, String> {
    if name == "washington-civil" || stage >= 2 {
        return Ok(None);
    }
    let c = course(name)?;
    let p = &c[if stage == 0 {
        "first_pack"
    } else {
        "return_pack"
    }];
    Ok(Some(Check {
        claim: c["criterion_claim"]
            .as_str()
            .ok_or("Missing released claim")?
            .into(),
        id: format!("{name}/{stage}"),
        prompt: p["check"].as_str().ok_or("Missing released check")?.into(),
        mode: match c["evidence_mode"].as_str() {
            Some("witnessed_performance") => "witness",
            Some("quantity" | "written_artifact") => "written",
            _ => "unassessable",
        }
        .into(),
        rubric: p["rubric"]
            .as_str()
            .ok_or("Missing released rubric")?
            .into(),
        quantity: p["quantity"]["value"]
            .as_i64()
            .zip(p["quantity"]["unit"].as_str().map(str::to_owned)),
    }))
}

/// Expand the reviewed program using the same prerequisite, time and source checks as the lab.
pub fn program(name: &str) -> Result<Vec<(String, String)>, String> {
    use tutor_lab::curriculum::{
        availability::MaterialForecast,
        program::{Frame, Proposal},
    };
    if name == "washington-civil" {
        return Ok(syllabus(name)?
            .into_iter()
            .map(|s| (s, "released".into()))
            .collect());
    }
    let data = course(name)?;
    let frame = Frame::from_json(&data)?;
    let proposal = Proposal::from_json(&data["proposal"])?;
    let calendar = frame.calendar.ok_or("Missing course calendar.")?;
    let schedule = frame
        .sequence(&proposal, calendar.forward_minutes())
        .map_err(|e| format!("Invalid course schedule: {e:?}"))?;
    let sources = MaterialForecast::from_pinned(&data, &frame, &proposal)?;
    schedule
        .scheduled
        .into_iter()
        .map(|id| {
            let u = proposal
                .units()
                .iter()
                .find(|u| u.id == id)
                .ok_or("Missing scheduled unit")?;
            Ok((u.performance.clone(), sources.status(id)?.label().into()))
        })
        .collect()
}
