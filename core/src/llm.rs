//! Model calls through OpenRouter's chat completions endpoint.
//!
//! Every reply is cached on disk by a hash of (model, messages, params). Re-running a
//! simulation therefore replays the same replies at zero cost, which makes live experiments
//! reproducible. A hard budget (ROBOTUTOR_BUDGET_USD, default $5) stops spending.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Routing table. Data, not code paths: a vendor change is a one-line diff that must pass
/// the grader calibration and swap tests before it serves.
pub mod route {
    /// T1: renders the chosen move. Fast, cheap.
    pub const RENDER: &str = "anthropic/claude-haiku-5.5";
    /// T2: two graders from different vendors, run blind and in parallel.
    pub const GRADE_A: &str = "anthropic/claude-haiku-5.5";
    pub const GRADE_B: &str = "openai/gpt-6-luna";
    /// Simulated learner persona: a third vendor, so it shares no habits with the graders.
    pub const LEARNER: &str = "deepseek/deepseek-v4.1-flash";
    /// T3: offline crawl/foundry and adjudication.
    pub const FRONTIER: &str = "anthropic/claude-sonnet-5.5";
    /// Optional advisor / router under test.
    pub const JEV: &str = "typesafe/jev-router";
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub text: String,
    pub model: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub ms: u64,
    pub cached: bool,
}

static SPENT_MICRO: AtomicU64 = AtomicU64::new(0);

pub fn spent_usd() -> f64 {
    SPENT_MICRO.load(Ordering::Relaxed) as f64 / 1e6
}

fn budget_usd() -> f64 {
    std::env::var("ROBOTUTOR_BUDGET_USD").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0)
}

fn cache_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cache").join("llm");
    std::fs::create_dir_all(&d).ok();
    d
}

pub struct Call<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub user: &'a str,
    pub max_tokens: u32,
    pub json: bool,
    pub temperature: f64,
    /// Distinguishes deliberate repeats of the same prompt (e.g. sampling several learners).
    pub nonce: u64,
}

pub fn chat(c: &Call) -> Result<Reply, String> {
    let mut body = json!({
        "model": c.model,
        "messages": [
            {"role": "system", "content": c.system},
            {"role": "user", "content": c.user}
        ],
        "max_tokens": c.max_tokens,
        "temperature": c.temperature,
        "usage": {"include": true},
        "reasoning": {"effort": "low"},
    });
    if c.json {
        body["response_format"] = json!({"type": "json_object"});
    }
    let key = hex(&Sha256::digest(format!("{}\u{0}{}", body, c.nonce).as_bytes()));
    let path = cache_dir().join(format!("{key}.json"));
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(mut r) = serde_json::from_str::<Reply>(&s) {
            r.cached = true;
            return Ok(r);
        }
    }
    if spent_usd() >= budget_usd() {
        return Err(format!("budget of ${:.2} reached", budget_usd()));
    }
    let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| "OPENROUTER_API_KEY not set".to_string())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(90)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut last_err = String::new();
    for attempt in 0..3 {
        let t0 = Instant::now();
        let resp = agent
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", &format!("Bearer {api_key}"))
            .header("X-Title", "robotutor-core-lab")
            .send_json(&body);
        let mut resp = match resp {
            Ok(r) => r,
            Err(e) => {
                last_err = e.to_string();
                std::thread::sleep(Duration::from_millis(800 * (attempt + 1)));
                continue;
            }
        };
        let status = resp.status().as_u16();
        let v: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
        if status != 200 {
            last_err = format!("HTTP {status}: {}", v["error"]["message"].as_str().unwrap_or("?"));
            if status == 429 || status >= 500 {
                std::thread::sleep(Duration::from_millis(1500 * (attempt + 1)));
                continue;
            }
            return Err(last_err);
        }
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        let u = &v["usage"];
        let r = Reply {
            text,
            model: v["model"].as_str().unwrap_or(c.model).to_string(),
            tokens_in: u["prompt_tokens"].as_u64().unwrap_or(0),
            tokens_out: u["completion_tokens"].as_u64().unwrap_or(0),
            cost_usd: u["cost"].as_f64().unwrap_or(0.0),
            ms: t0.elapsed().as_millis() as u64,
            cached: false,
        };
        SPENT_MICRO.fetch_add((r.cost_usd * 1e6) as u64, Ordering::Relaxed);
        std::fs::write(&path, serde_json::to_string(&r).unwrap()).ok();
        return Ok(r);
    }
    Err(last_err)
}

/// Parse the first JSON object in a reply (models sometimes wrap JSON in prose or fences).
pub fn json_of(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
