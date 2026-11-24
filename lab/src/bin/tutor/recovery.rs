//! Private, turn-scoped checkpoints. The ordinary delivery receipt remains
//! authoritative and is independently replayed before publishing the reply.
use crate::session::{Event, Journal};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub key: String,
    pub output: Value,
}

#[derive(Default)]
pub struct Checkpoints(BTreeMap<String, Value>);
impl Checkpoints {
    pub fn insert(&mut self, step: Step) {
        // Debug notifications share the journal format but cannot resume a model call.
        if !step.key.starts_with("debug/") {
            self.0.insert(step.key, step.output);
        }
    }
    pub fn retry_after_failure(&mut self) {
        // Keep decisions about the submission. A retry may need a different draft
        // or interpretation after exhausting content or schema repairs.
        self.0.retain(|key, _| key.starts_with("assessment/"));
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

pub struct TurnWork<'a> {
    saved: RefCell<Checkpoints>,
    journal: RefCell<Option<&'a mut Journal>>,
}
impl<'a> TurnWork<'a> {
    pub fn new(saved: Checkpoints, journal: Option<&'a mut Journal>) -> Self {
        Self {
            saved: RefCell::new(saved),
            journal: RefCell::new(journal),
        }
    }
    pub fn finish(self) -> Checkpoints {
        self.saved.into_inner()
    }
    pub fn record(&self, event: &Event) -> Result<(), String> {
        if let Some(journal) = self.journal.borrow_mut().as_mut() {
            journal.save(event)?;
        }
        Ok(())
    }
    pub fn model_attempt(&self, stage: &str, model: &str, attempt: usize) -> Result<(), String> {
        self.record(&Event::TurnStep {
            step: Step {
                key: format!("debug/{stage}"),
                output: serde_json::json!({"stage":stage,"attempt":attempt,"model":model}),
            },
        })
    }
    pub fn get_or_run(
        &self,
        stage: &str,
        model: &str,
        request: &Value,
        run: impl FnOnce() -> Result<Value, String>,
    ) -> Result<Value, String> {
        let key = format!(
            "{stage}/{:x}",
            Sha256::digest(serde_json::to_vec(&(model, request)).map_err(|e| e.to_string())?)
        );
        if let Some(output) = self.saved.borrow().0.get(&key) {
            self.record(&Event::TurnStep {
                step: Step {
                    key: format!("debug/{stage}"),
                    output: serde_json::json!({"stage":stage, "state":"reused","checkpoint":key}),
                },
            })?;
            return Ok(output.clone());
        }
        let output = run()?;
        if stage == "assessment" && output["method"] == "verifier_unavailable" {
            return Ok(output);
        }

        let step = Step {
            key,
            output: output.clone(),
        };
        self.record(&Event::TurnStep { step: step.clone() })?;
        self.saved.borrow_mut().insert(step);
        Ok(output)
    }
}

// Only retry explicit temporary service refusals. A timed-out POST can have
// reached the service and is left for explicit resumption.
fn temporary_refusal(error: &str) -> bool {
    ["HTTP 502;", "HTTP 503;", "HTTP 504;"]
        .iter()
        .any(|status| error.starts_with(&format!("OpenRouter returned {status}")))
}
pub fn model_output(
    mut run: impl FnMut() -> Result<Value, String>,
    mut before_retry: impl FnMut(),
) -> Result<Value, String> {
    match run() {
        Err(error)
            if temporary_refusal(&error) || error == "Could not read Luna's structured reply." =>
        {
            before_retry();
            run()
        }
        result => result,
    }
}

pub fn interrupted(message: &str) -> bool {
    let detail = message
        .split_once(": ")
        .map_or(message, |(_, detail)| detail);
    detail.starts_with("OpenRouter") || detail.starts_with("could not start curl")
}

pub fn while_waiting<T>(stage: &str, interactive: bool, run: impl FnOnce() -> T) -> T {
    if !interactive {
        return run();
    }
    std::thread::scope(|scope| {
        let (done, signal) = std::sync::mpsc::channel::<()>();
        scope.spawn(move || {
            let start = std::time::Instant::now();
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                signal.recv_timeout(std::time::Duration::from_secs(12))
            {
                eprintln!(
                    "Luna · Waiting for {stage} ({}s)…",
                    start.elapsed().as_secs()
                );
            }
        });
        let result = run();
        drop(done);
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_rate_limit_and_uncertain_billing_are_not_blindly_retried() {
        for error in [
            "OpenRouter returned HTTP 401; check the API key and its model permissions",
            "OpenRouter returned HTTP 429; check account and provider rate limits",
            "OpenRouter exchange failed or timed out; billing outcome is unknown",
        ] {
            let r = model_output(
                || Err(error.into()),
                || panic!("duplicate provider request"),
            );
            assert_eq!(r.unwrap_err(), error);
        }
        let mut calls = 0;
        let r = model_output(
            || {
                calls += 1;
                if calls == 1 {
                    Err("Could not read Luna's structured reply.".into())
                } else {
                    Ok(serde_json::json!({"ok":true}))
                }
            },
            || {},
        );
        assert_eq!(calls, 2);
        assert!(r.unwrap()["ok"].as_bool().unwrap());
    }
}
