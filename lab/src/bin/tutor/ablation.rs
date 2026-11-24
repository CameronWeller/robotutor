//! Four inference policies on one frozen journal and one subsequently revealed answer.
//! This experiment never writes a learner journal or awards credit.
use crate::{lesson::object, session, Config};
use serde_json::{json, Value};
use std::{path::Path, time::Instant};
use tutor_lab::openrouter::{assistant_text, credentials, Role};

struct Budget {
    remaining: u64,
    records: Vec<Value>,
    cfg: Config,
}
impl Budget {
    fn call(
        &mut self,
        stage: &str,
        instruction: &str,
        data: Value,
        schema: Value,
        cap: u64,
    ) -> Result<Value, String> {
        let req = json!({"messages":[{"role":"system","content":instruction},{"role":"user","content":data.to_string()}],"max_tokens":cap,
            "provider":{"require_parameters":true},"response_format":{"type":"json_schema","json_schema":{"name":"study_decision","strict":true,"schema":schema}}});
        // Byte length upper-bounds tokens for ordinary byte-tokenized text. Reserve
        // extra room for transport framing and reject overflow in the actual receipt.
        if req.to_string().len() as u64 + cap + 256 > self.remaining {
            return Err(format!("{stage} exceeds remaining inference budget"));
        }
        let start = Instant::now();
        let r = self.cfg.complete(Role::Luna, &req)?;
        let usage = &r["usage"];
        let count = usage["total_tokens"]
            .as_u64()
            .ok_or("Unmetered policy call")?;
        self.remaining = self
            .remaining
            .checked_sub(count)
            .ok_or("Provider exceeded the study budget")?;
        self.records.push(json!({"stage":stage,"model":r["model"],"usage":usage,"elapsed_ms":start.elapsed().as_millis()}));
        serde_json::from_str(assistant_text(&r)?).map_err(|e| e.to_string())
    }
}

pub fn run(policy: &str, path: &Path, response: &Path, limit: u64) -> Result<Value, String> {
    if !matches!(policy, "history" | "upstream" | "downstream" | "combined") {
        return Err("Choose history, upstream, downstream or combined.".into());
    }
    let visit = session::replay(path)?;
    if visit.pending.is_some() {
        return Err("Select the saved prefix immediately before submission.".into());
    }
    let mut b = Budget {
        remaining: limit,
        records: vec![],
        cfg: credentials::connect()?,
    };
    let string = json!({"type":"string"});
    let mut context = json!({"history":visit.history,"objective":visit.lesson.objective,"exercise":visit.lesson.active_step(),"criterion":visit.lesson.rubric,"course":visit.lesson.course});
    if matches!(policy, "downstream" | "combined") {
        let summary=b.call("consolidation","Consolidate only observed work from this completed history. Link each claim to a numbered source turn. Distinguish learner reports, assistance, uncertainty, and corrections. Preserve what changes the next instruction.",context.clone(),
            object(json!({"observations":{"type":"array","items":object(json!({"turn":{"type":"integer"},"quote":string,"interpretation":string}))},"open_question":string})),650)?;
        let verified: Vec<_> = summary["observations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| {
                let Some(i) = o["turn"].as_u64().and_then(|n| n.checked_sub(1)) else {
                    return false;
                };
                visit
                    .history
                    .get(i as usize)
                    .and_then(|(w, _)| w.as_ref())
                    .is_some_and(|w| {
                        o["quote"]
                            .as_str()
                            .is_some_and(|q| !q.is_empty() && w.contains(q))
                    })
            })
            .collect();
        context.as_object_mut().unwrap().remove("history");
        context["previous_evidence"] =
            json!({"observations":verified,"open_question":summary["open_question"]});
    }
    if matches!(policy, "upstream" | "combined") {
        let form = object(json!({"task":string,"criterion":string}));
        let moves=b.call("preparation","Prepare a challenge and a support branch for the current objective before seeing any answer. Give a complete task and correct criterion for each.",context.clone(),object(json!({"challenge":form,"support":form})),700)?;
        let reviewed=b.call("review","Check both candidates independently for a correct key or rubric, clarity, accessibility and fit to this objective. Accept only usable candidates.",json!({"lesson":context,"candidates":moves}),object(json!({"challenge":{"type":"boolean"},"support":{"type":"boolean"}})),400)?;
        for branch in ["challenge", "support"] {
            if reviewed[branch] == true {
                context["reviewed_branches"][branch] = moves[branch].clone()
            }
        }
    }
    let answer = std::fs::read_to_string(response).map_err(|e| e.to_string())?;
    context["new_submission"] = json!(answer.trim());
    let decision=b.call("foreground","Read the actual submission. Give feedback justified by its words. Choose the next actionable exercise for the current goal. Use a reviewed branch only if it fits. Check the evidence for independence. Grant provisional credit only. Return the intended action, an exact evidence quote and the concrete delivered exercise.",context,
        object(json!({"action":{"type":"string","enum":["advance","support","clarify"]},"quote":string,"feedback":string,"exercise":string})),650)?;
    let quoted = decision["quote"]
        .as_str()
        .is_some_and(|q| !q.is_empty() && answer.contains(q));
    Ok(
        json!({"policy":policy,"total_token_limit":limit,"total_tokens_used":limit-b.remaining,"stages":b.records,"decision":decision,"quote_verified":quoted}),
    )
}
