//! A local release check for a specific way worked arithmetic can cue fresh practice.
use serde_json::{json, Value};

fn numeric_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.'
}

fn numbers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c| !numeric_char(c))
        .map(|s| s.trim_end_matches('.'))
        .filter(|s| !s.is_empty())
}

fn calculations(text: &str) -> impl Iterator<Item = (&str, char, &str)> {
    text.match_indices(['×', '÷', '*'])
        .filter_map(move |(i, op)| {
            let left = text[..i].trim_end().rsplit(|c| !numeric_char(c)).next()?;
            let right = text[i + op.len()..]
                .trim_start()
                .split(|c| !numeric_char(c))
                .next()?
                .trim_end_matches('.');
            if left.parse::<f64>().is_err() || right.parse::<f64>().is_err() {
                return None;
            }
            Some((left, op.chars().next()?, right))
        })
}

pub fn arithmetic_cue(packet: &Value) -> Option<Value> {
    let draft = &packet["draft"];
    if packet["task"] != "review" || !matches!(draft["move"].as_str(), Some("advance" | "practice"))
    {
        return None;
    }
    let exercise = draft["exercise"].as_str()?;
    let teaching = draft["bridge_and_instruction"]
        .as_str()
        .or_else(|| draft["feedback"].as_str())?;
    let previous = [
        packet["open_exercise"].as_str().unwrap_or(""),
        packet["learner_message"].as_str().unwrap_or(""),
    ]
    .join(" ");
    let (a, op, b) = calculations(teaching).find(|&(a, op, b)| {
        numbers(exercise).any(|n| n == a)
            && numbers(exercise).any(|n| n == b)
            && numbers(exercise).all(|n| n == a || n == b)
            && !(numbers(&previous).any(|n| n == a) && numbers(&previous).any(|n| n == b))
            && !calculations(exercise).any(|c| c == (a, op, b))
    })?;
    Some(
        json!({"verdict":"revise", "reason":format!("The teaching text gives {a} {op} {b} using the fresh exercise's numbers. Preserve the challenge. Work a different example or leave the calculation for the learner."),
        "criterion":"", "mode":"unassessable"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catches_the_observed_map_cue_and_preserves_legitimate_instruction() {
        let mut p = json!({"task":"review","learner_message":"5 cm gives 20 km.","open_exercise":"4 km per centimetre",
            "draft":{"move":"advance","bridge_and_instruction":"28 km contains 28 ÷ 4 groups of 4 km.","exercise":"At 4 km per centimetre, how long is a 28 km trail on the map?"}});
        assert_eq!(arithmetic_cue(&p).unwrap()["verdict"], "revise");
        p["draft"]["bridge_and_instruction"] = json!("The previous trail gave 5 × 4 = 20 km.");
        assert!(arithmetic_cue(&p).is_none());
        p["draft"]["bridge_and_instruction"] = json!("For a 12 km trail, use 12 ÷ 4.");
        assert!(arithmetic_cue(&p).is_none());
        p["draft"]["bridge_and_instruction"] = json!("28 ÷ 4 represents four-kilometre groups.");
        p["draft"]["exercise"] = json!("Explain why 28 ÷ 4 represents the map distance.");
        assert!(arithmetic_cue(&p).is_none());
        p["draft"]["move"] = json!("guide");
        assert!(arithmetic_cue(&p).is_none());
    }
}
