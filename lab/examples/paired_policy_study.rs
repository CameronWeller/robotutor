//! Replay the next agenda from a synthetic participant's actual display and response.
//! Run with a study manifest. Outputs are local, and all source text comes from a pinned pack.
use serde_json::{json, Value};
use std::{env, fs, path::Path};
use tutor_lab::{
    curriculum::goal_evidence::check_quantity_text, material::document_id, protocol::Judgment,
};

fn load(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn field<'a>(object: &'a Value, key: &str) -> Result<&'a str, String> {
    object[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing text for {key}"))
}

fn course(domain: &str) -> Result<Value, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/goals");
    let pins = load(&dir.join("reviews.json"))?;
    let pin = field(&pins, domain)?;
    let bytes = fs::read_to_string(dir.join(format!("{domain}.json")))
        .map_err(|e| e.to_string())?
        .replace("\r\n", "\n");
    let hash: String = document_id(bytes.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hash != pin {
        return Err("course differs from reviewed release".into());
    }
    serde_json::from_str(&bytes).map_err(|e| e.to_string())
}

/// An explicit assessment of one quoted quantity in prose. The selector is a
/// study annotation. The reviewed quantity judge checks it against the edition.
fn accepted_quantity(pack: &Value, response: &str, quote: Option<&str>) -> Result<bool, String> {
    let Some(quote) = quote else {
        return Ok(false);
    };
    let expected = pack["quantity"]["value"]
        .as_i64()
        .ok_or("release has no quantitative criterion")?;
    if !response.contains(quote)
        || check_quantity_text(expected, field(&pack["quantity"], "unit")?, quote)
            != Judgment::Correct
    {
        return Err("quoted quantity is absent or fails the released criterion".into());
    }
    Ok(true)
}

fn visit(pack: &Value, arm: &Value, receipt: &Value) -> Result<Value, String> {
    let check = field(pack, "check")?;
    let display = field(&receipt["packet"], "display")?;
    let response = field(&receipt["response"], "response")?;
    let contact = field(arm, "contact")?;
    if !matches!(contact, "absent" | "possible" | "verbatim")
        || (contact == "verbatim") != display.contains(check)
    {
        return Err("contact annotation disagrees with the delivered question".into());
    }
    let verified = accepted_quantity(pack, response, arm["quantity_quote"].as_str())?;
    if verified && contact != "verbatim" {
        return Err("quantity cannot count against an unconfirmed question".into());
    }
    // This experiment has one reserved item. A familiar showing is a useful
    // practice option when a second independent check cannot be released.
    let (action, introduction, material) = if verified {
        (
            "offer_reviewed_practice",
            "The quantity you reported matched the last check. Here is a follow-up practice task.",
            field(pack, "practice")?,
        )
    } else if contact == "absent" {
        (
            "open_unseen_check",
            "Here is the reserved question. You can explain your first step before feedback.",
            check,
        )
    } else {
        (
            "offer_familiar_practice",
            "You may have seen this one. You can use it for practice or wait for a different question.",
            check,
        )
    };
    Ok(json!({
        "id": field(arm, "id")?,
        "memory": {"item": pack["unit"], "contact":contact, "quantity_matched":verified, "help":"unknown"},
        "day_30_action":action,
        "day_30_display":format!("{introduction}\n\n{material}")
    }))
}

fn replay(manifest: &Path) -> Result<Value, String> {
    let study = load(manifest)?;
    let released = course(field(&study, "course")?)?;
    let pack = &released["return_pack"];
    let root = manifest.parent().ok_or("missing directory")?;
    let mut results = Vec::new();
    for arm in study["arms"].as_array().ok_or("missing arms")? {
        let receipt = load(&root.join(field(arm, "receipt")?))?;
        let packet = receipt["packet"]
            .as_object()
            .ok_or("missing participant packet")?;
        if packet.keys().any(|key| {
            !matches!(
                key.as_str(),
                "your_goal" | "your_prior_experience" | "your_prior_words" | "today" | "display"
            )
        }) {
            return Err("unexpected channel to the simulated participant".into());
        }
        let teacher = load(&root.join(field(arm, "teacher")?))?;
        let prose = field(&teacher["response"], "display")?;
        let display = match arm["attach_check"]
            .as_bool()
            .ok_or("missing host decision")?
        {
            true => format!("{prose}\n\n{}", field(pack, "check")?),
            false => prose.to_owned(),
        };
        if field(&receipt["packet"], "display")? != display {
            return Err("participant did not receive the recorded host display".into());
        }
        results.push(visit(pack, arm, &receipt)?);
    }
    Ok(json!({"release_unit":pack["unit"],"arms":results}))
}

fn main() -> Result<(), String> {
    let name = env::args_os().nth(1).ok_or("supply a study manifest")?;
    let report = replay(Path::new(&name))?;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_number_requires_the_annotated_response_and_visible_item() {
        let pack =
            json!({"check":"Question", "practice":"Practice", "quantity":{"value":-6,"unit":"kg"}});
        let receipt =
            json!({"packet":{"display":"Question"}, "response":{"response":"I got -6 kg."}});
        let mut arm = json!({"id":"synthetic", "contact":"absent", "quantity_quote":"-6 kg"});
        assert!(visit(&pack, &arm, &receipt).is_err());
        arm["contact"] = json!("verbatim");
        assert_eq!(
            visit(&pack, &arm, &receipt).unwrap()["day_30_action"],
            "offer_reviewed_practice"
        );
        arm["quantity_quote"] = json!("6 kg");
        assert!(visit(&pack, &arm, &receipt).is_err());
    }
}
