//! Explicit live transport for the lab's two inference roles. Jev returns named decisions;
//! Luna returns chat completions. Callers own admission, semantic checks and any receipts.
pub mod credentials;
mod decision;
mod trailing;
pub use trailing::wake as wake_astra;
pub mod experiment;
pub use decision::DecisionMenu;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use std::{
    env,
    io::{Read, Write},
    process::{Command, Stdio},
};

const REQUEST_LIMIT: usize = 128 * 1024;
const RESPONSE_LIMIT: usize = 1024 * 1024;
pub const DEFAULT_JEV: &str = "typesafe/jev-1.14";
pub const DEFAULT_LUNA: &str = "openai/gpt-6-luna";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Jev,
    Luna,
}

impl Role {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Jev => "https://openrouter.ai/api/alpha/decisions",
            Self::Luna => "https://openrouter.ai/api/v1/chat/completions",
        }
    }
}

/// Credentials have no Debug or serialization implementation. The secret only enters
/// the HTTPS client through an anonymous pipe.
pub struct Config {
    key: String,
    jev: String,
    luna: String,
}

fn valid_model(s: &str) -> bool {
    let part = |v: &str| {
        v.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    };
    s.len() <= 200
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "openrouter/auto" | "openrouter/free" | "openrouter/bodybuilder"
        )
        && s.split_once('/')
            .is_some_and(|(owner, model)| part(owner) && part(model))
}

// Canonical endpoint names published by OpenRouter for these two models. A new
// version needs explicit identification; a matching family or date prefix is insufficient.
fn canonical(model: &str) -> &str {
    match model {
        "openai/gpt-6.2-astra-20261117" => "openai/gpt-6.2-astra",
        "typesafe/jev-1.14-20261014" => DEFAULT_JEV,
        "openai/gpt-6-luna-20260922" => DEFAULT_LUNA,
        other => other,
    }
}

pub fn matches_reported(requested: &str, reported: &str) -> bool {
    valid_model(requested) && canonical(requested) == canonical(reported)
}

// Collect usage before response validation so a billed but unusable reply still counts.
// Nesting assigns each exchange to its innermost measured operation.
thread_local! {
    static USAGE: std::cell::RefCell<Option<Vec<Value>>> = const { std::cell::RefCell::new(None) };
}
pub fn with_usage<T>(operation: impl FnOnce() -> T) -> (T, Vec<Value>) {
    let outer = USAGE.replace(Some(Vec::new()));
    let result = operation();
    (result, USAGE.replace(outer).unwrap_or_default())
}

pub fn assistant_text(capture: &Value) -> Result<&str, String> {
    let choice = capture["choices"]
        .as_array()
        .filter(|c| c.len() == 1)
        .and_then(|c| c.first())
        .ok_or("expected one OpenRouter completion")?;
    let message = &choice["message"];
    if !capture["error"].is_null()
        || !choice["error"].is_null()
        || choice["finish_reason"] != "stop"
        || message["role"] != "assistant"
        || !message["refusal"].is_null()
        || message
            .get("tool_calls")
            .is_some_and(|calls| !calls.is_null() && calls.as_array().is_none_or(|v| !v.is_empty()))
        || !message["function_call"].is_null()
    {
        return Err("OpenRouter did not return a completed text-only assistant reply".into());
    }
    message["content"]
        .as_str()
        .filter(|s| s.len() <= RESPONSE_LIMIT && !s.trim().is_empty())
        .ok_or_else(|| "OpenRouter returned empty or oversized assistant text".into())
}

/// Accept one of the caller's declared categories from a completed native decision.
/// Probability fields are advisory. The caller interprets the category in its own domain.
pub fn decision_choice<'a>(
    capture: &'a Value,
    model: &str,
    question: &str,
    allowed: &[&str],
) -> Result<&'a str, String> {
    reported_model(capture, model)?;
    typed_choice(capture, question).and_then(|choice| {
        allowed
            .contains(&choice)
            .then_some(choice)
            .ok_or_else(|| "OpenRouter returned an unrecognized decision choice".into())
    })
}

fn typed_choice<'a>(capture: &'a Value, question: &str) -> Result<&'a str, String> {
    let answer = &capture["answers"][question];
    if answer["type"] != "choice" {
        return Err("OpenRouter returned no typed answer for the requested choice".into());
    }
    answer["choice"]
        .as_str()
        .ok_or_else(|| "OpenRouter returned a choice without a label".into())
}

fn https_client() -> Command {
    // -q is first so local curlrc cannot enable traces, follow redirects or add headers.
    // Preserve the operator's networking and CA configuration. Neither learner data,
    // the provider credential nor TLS key logging belongs in the child environment.
    let mut curl = Command::new("curl");
    curl.env_clear();
    for key in [
        "PATH",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "CURL_CA_BUNDLE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = env::var_os(key) {
            curl.env(key, value);
        }
    }
    curl
}

impl Config {
    /// Send the exact, already frozen text request for a fictional pilot. The
    /// pilot records even unusable provider responses before it interprets them.
    pub fn pilot_completion(&self, request: &Value) -> Result<Value, String> {
        if request["model"] != "openai/gpt-6-luna-20260922" {
            return Err("Unexpected pilot model".into());
        }
        let mut bytes = Vec::new();
        serde_json::to_writer(Limited(&mut bytes), request)
            .map_err(|_| "Pilot request too large")?;
        self.exchange(
            Role::Luna.endpoint(),
            Some(std::str::from_utf8(&bytes).map_err(|_| "Invalid pilot request")?),
            None,
        )
    }
    pub fn from_env() -> Result<Self, String> {
        let optional = |name, default: &str| match env::var(name) {
            Ok(s) => Ok(s),
            Err(env::VarError::NotPresent) => Ok(default.to_owned()),
            Err(env::VarError::NotUnicode(_)) => Err(format!("invalid {name}")),
        };
        Self::new(
            match env::var("OPENROUTER_API_KEY") {
                Ok(key) => key,
                Err(env::VarError::NotPresent)
                    if env::var_os("ROBOTUTOR_USE_LOGIN_KEYRING").is_some() =>
                {
                    credentials::lookup()?.ok_or("Save an OpenRouter key in the terminal first.")?
                }
                Err(_) => return Err("set OPENROUTER_API_KEY for live inference".into()),
            },
            optional("OPENROUTER_JEV_MODEL", DEFAULT_JEV)?,
            optional("OPENROUTER_LUNA_MODEL", DEFAULT_LUNA)?,
        )
    }

    pub fn new(key: String, jev: String, luna: String) -> Result<Self, String> {
        if key.is_empty()
            || key.len() > 1024
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return Err("invalid OpenRouter bearer credential".into());
        }
        if !valid_model(&jev)
            || !valid_model(&luna)
            || canonical(&jev).eq_ignore_ascii_case(canonical(&luna))
        {
            return Err("configure two distinct concrete OpenRouter model IDs".into());
        }
        Ok(Self { key, jev, luna })
    }

    pub fn model(&self, role: Role) -> &str {
        match role {
            Role::Jev => &self.jev,
            Role::Luna => &self.luna,
        }
    }

    /// Verify a newly supplied key before storing it. Discard the account metadata.
    pub fn verify_key(&self) -> Result<(), String> {
        let mut child = https_client()
            .args([
                "-q",
                "--silent",
                "--proto",
                "=https",
                "--max-time",
                "20",
                "--config",
                "-",
                "--output",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
                "--write-out",
                "%{http_code}",
                "https://openrouter.ai/api/v1/key",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Could not start OpenRouter key verification.")?;
        let sent = writeln!(
            child.stdin.take().unwrap(),
            "header = \"Authorization: Bearer {}\"",
            self.key
        );
        let out = child
            .wait_with_output()
            .map_err(|_| "OpenRouter key verification failed.")?;
        if sent.is_err() || !out.status.success() || out.stdout != b"200" {
            return Err("OpenRouter could not verify the supplied key.".into());
        }
        Ok(())
    }

    /// Make exactly one application request. An interrupted exchange may have been billed.
    /// A workflow can inject a closure with this signature for offline operation.
    pub fn complete(&self, role: Role, request: &Value) -> Result<Value, String> {
        self.complete_by(role, request, None)
    }

    pub fn complete_by(
        &self,
        role: Role,
        request: &Value,
        deadline: Option<Instant>,
    ) -> Result<Value, String> {
        let model = self.model(role);
        let body = payload(role, model, request)?;
        let capture = self.exchange(role.endpoint(), Some(&body), deadline)?;
        validate_completion(role, model, &capture)?;
        if role == Role::Jev {
            let questions = request["questions"]
                .as_object()
                .expect("validated decision request");
            let answers = capture["answers"]
                .as_object()
                .expect("validated decision response");
            if questions.keys().ne(answers.keys()) {
                return Err("OpenRouter decision keys differ from the request".into());
            }
        }
        if role == Role::Luna {
            if let Err(e) = trailing::record(&body, &capture) {
                eprintln!("Astra review capture unavailable: {e}");
            }
        }
        Ok(capture)
    }
    pub fn supervise(&self, request: &Value) -> Result<(String, Value), String> {
        let model = env::var("OPENROUTER_SUPERVISOR_MODEL")
            .map_err(|_| "Paid supervision requires an explicitly configured supervisor model")?;
        if !valid_model(&model)
            || matches_reported(&model, &self.luna)
            || matches_reported(&model, &self.jev)
        {
            return Err("Choose a distinct concrete supervisor model".into());
        }
        let body = payload(Role::Luna, &model, request)?;
        let capture = self.post(Role::Luna.endpoint(), &body)?;
        validate_completion(Role::Luna, &model, &capture)?;
        Ok((model, capture))
    }
    fn post(&self, endpoint: &str, body: &str) -> Result<Value, String> {
        self.exchange(endpoint, Some(body), None)
    }
    fn key_info(&self, deadline: Instant) -> Result<Value, String> {
        self.exchange("https://openrouter.ai/api/v1/key", None, Some(deadline))
    }
    fn exchange(
        &self,
        endpoint: &str,
        body: Option<&str>,
        deadline: Option<Instant>,
    ) -> Result<Value, String> {
        let max = Duration::from_secs(45);
        let remaining = deadline
            .map_or(max, |end| {
                end.saturating_duration_since(Instant::now()).min(max)
            })
            .as_millis();
        if remaining == 0 {
            return Err("OpenRouter request deadline elapsed".into());
        }
        let seconds = format!("{}.{:03}", remaining / 1000, remaining % 1000);
        let mut curl = https_client();
        let mut child = curl
            .args([
                "-q",
                "--silent",
                "--fail",
                "--proto",
                "=https",
                "--max-redirs",
                "0",
                "--retry",
                "0",
                "--connect-timeout",
                "10",
                "--max-time",
                &seconds,
                "--max-filesize",
                &RESPONSE_LIMIT.to_string(),
                "--header",
                "Content-Type: application/json",
                "--write-out",
                "\n%{http_code}",
                "--config",
                "-",
                endpoint,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "could not start curl for OpenRouter")?;
        let exchange = (|| {
            let mut input = child
                .stdin
                .take()
                .ok_or("OpenRouter input pipe unavailable")?;
            writeln!(input, "header = \"Authorization: Bearer {}\"", self.key)
                .map_err(|_| "could not send request to curl")?;
            if let Some(body) = body {
                let escaped = body.replace('\\', "\\\\").replace('"', "\\\"");
                writeln!(input, "data-raw = \"{escaped}\"")
                    .map_err(|_| "could not send request to curl")?;
            }
            drop(input);
            let mut output = Vec::new();
            child
                .stdout
                .take()
                .ok_or("OpenRouter output pipe unavailable")?
                .take((RESPONSE_LIMIT + 5) as u64)
                .read_to_end(&mut output)
                .map_err(|_| "could not read the OpenRouter response")?;
            if output.len() > RESPONSE_LIMIT + 4 {
                return Err("OpenRouter response exceeded its byte limit");
            }
            Ok(output)
        })();
        if exchange.is_err() {
            let _ = child.kill();
        }
        let status = child
            .wait()
            .map_err(|_| "could not reap the OpenRouter request")?;
        let result = decode_json(status.code(), &exchange?);
        if body.is_some() {
            USAGE.with_borrow_mut(|active| {
                if let Some(calls) = active {
                    calls.push(
                        result
                            .as_ref()
                            .ok()
                            .and_then(|r| r.get("usage"))
                            .cloned()
                            .unwrap_or(Value::Null),
                    );
                }
            });
        }
        result
    }

    /// Embeddings share the existing authenticated transport, separate from the teaching model.
    pub fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
        self.embed_by(inputs, None)
    }
    pub fn embed_by(
        &self,
        inputs: &[String],
        deadline: Option<Instant>,
    ) -> Result<Vec<Vec<f32>>, String> {
        if inputs.is_empty() || inputs.len() > 32 {
            return Err("Expected a bounded embedding batch.".into());
        }
        let body=json!({"model":EMBEDDING_MODEL,"dimensions":EMBEDDING_DIM,"encoding_format":"float","input":inputs}).to_string();
        if body.len() > REQUEST_LIMIT {
            return Err("Embedding request exceeds its byte limit.".into());
        }
        let receipt = self.exchange(
            "https://openrouter.ai/api/v1/embeddings",
            Some(&body),
            deadline,
        )?;
        decode_embeddings(&receipt, inputs.len())
    }
}

// Bound serialization of an incoming Value before cloning it to add transport settings.
struct Limited<'a>(&'a mut Vec<u8>);
impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > REQUEST_LIMIT - self.0.len() {
            return Err(std::io::Error::other("body too large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn payload(role: Role, model: &str, request: &Value) -> Result<String, String> {
    let mut bytes = Vec::new();
    serde_json::to_writer(Limited(&mut bytes), request)
        .map_err(|_| "OpenRouter request exceeds its byte limit")?;
    let fields = request
        .as_object()
        .ok_or("expected an OpenRouter request object")?;
    let allowed: &[&str] = match role {
        Role::Jev => &["model", "state", "questions"],
        Role::Luna => &[
            "model",
            "messages",
            "max_tokens",
            "response_format",
            "temperature",
            "provider",
            "reasoning",
            "stream",
        ],
    };
    if fields.keys().any(|key| !allowed.contains(&key.as_str()))
        || fields
            .get("model")
            .is_some_and(|m| m.as_str() != Some(model))
    {
        return Err("OpenRouter request conflicts with the configured role or endpoint".into());
    }
    let mut body = request.clone();
    body["model"] = json!(model);
    match role {
        Role::Jev => {
            if !(request["state"].is_string()
                || request["state"].is_object()
                || request["state"].is_array())
                || request["questions"]
                    .as_object()
                    .is_none_or(|q| q.is_empty() || q.len() > 16)
            {
                return Err(
                    "Jev requires state and one to sixteen named decision questions".into(),
                );
            }
        }
        Role::Luna => {
            let messages = request["messages"]
                .as_array()
                .ok_or("Luna requires text messages")?;
            if messages.is_empty()
                || messages.len() > 32
                || messages.iter().any(|m| {
                    m.as_object().is_none_or(|obj| obj.len() != 2)
                        || !matches!(m["role"].as_str(), Some("system" | "user" | "assistant"))
                        || !m["content"].is_string()
                })
                || request["max_tokens"]
                    .as_u64()
                    .is_none_or(|n| !(1..=8192).contains(&n))
                || fields
                    .get("stream")
                    .is_some_and(|v| v.as_bool() != Some(false))
            {
                return Err("Luna requires bounded text messages and a non-streaming output budget from 1 to 8192 tokens".into());
            }
            let reasoning = json!({"effort":"low","exclude":true});
            let mut provider = fields.get("provider").cloned().unwrap_or_else(|| json!({}));
            if fields.get("reasoning").is_some_and(|v| v != &reasoning)
                || provider.as_object().is_none_or(|obj| {
                    obj.iter().any(|(k, v)| match k.as_str() {
                        "require_parameters" => v.as_bool() != Some(true),
                        "allow_fallbacks" => !v.is_boolean(),
                        _ => true,
                    })
                })
            {
                return Err("Luna request conflicts with low reasoning or provider routing".into());
            }
            // Provider-side routing between hosts of this same concrete model can use
            // the normal OpenRouter policy. A caller may explicitly disable it.
            provider["require_parameters"] = json!(true);
            body["reasoning"] = reasoning;
            body["provider"] = provider;
        }
    }
    bytes.clear();
    serde_json::to_writer(Limited(&mut bytes), &body)
        .map_err(|_| "OpenRouter request exceeds its byte limit")?;
    String::from_utf8(bytes).map_err(|_| "could not encode OpenRouter request".into())
}

pub(crate) fn reported_model<'a>(capture: &'a Value, model: &str) -> Result<&'a str, String> {
    if !capture["error"].is_null() {
        return Err("OpenRouter reported an inference error".into());
    }
    capture["model"]
        .as_str()
        .filter(|id| matches_reported(model, id))
        .ok_or_else(|| "OpenRouter returned a different or missing model ID".into())
}

fn decode_json(code: Option<i32>, output: &[u8]) -> Result<Value, String> {
    let (body, suffix) = output.split_at(output.len().saturating_sub(4));
    let status = std::str::from_utf8(suffix)
        .ok()
        .and_then(|s| s.strip_prefix('\n'))
        .and_then(|s| s.parse::<u16>().ok());
    if let Some(status @ 300..=599) = status {
        let hint = match status {
            401 | 403 => "check the API key and its model permissions",
            402 => "check account credits and key spending limits",
            404 => "check the configured model and endpoint availability",
            429 => "check account and provider rate limits",
            300..=399 => "redirects are disabled",
            500..=599 => "check provider status before making another request",
            _ => "check the request schema and configured model",
        };
        return Err(format!("OpenRouter returned HTTP {status}; {hint}"));
    }
    if code == Some(63) {
        return Err("OpenRouter response exceeded its byte limit".into());
    }
    if code == Some(60) {
        return Err(
            "OpenRouter TLS verification failed; check the configured certificate authority".into(),
        );
    }
    if code != Some(0) || !matches!(status, Some(200..=299)) {
        return Err("OpenRouter exchange failed or timed out; billing outcome is unknown".into());
    }
    let capture: Value =
        serde_json::from_slice(body).map_err(|_| "OpenRouter returned invalid JSON")?;
    Ok(capture)
}

fn validate_completion(role: Role, model: &str, capture: &Value) -> Result<(), String> {
    reported_model(capture, model)?;
    match role {
        Role::Jev if capture["answers"].as_object().is_none_or(|a| a.is_empty()) => {
            return Err("OpenRouter returned no native decisions".into())
        }
        Role::Luna => {
            assistant_text(capture)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
fn decode(role: Role, model: &str, code: Option<i32>, output: &[u8]) -> Result<Value, String> {
    let capture = decode_json(code, output)?;
    validate_completion(role, model, &capture)?;
    Ok(capture)
}

pub const EMBEDDING_MODEL: &str = "openai/text-embedding-3-small";
pub const EMBEDDING_DIM: usize = 256;
fn decode_embeddings(receipt: &Value, count: usize) -> Result<Vec<Vec<f32>>, String> {
    // OpenRouter hosts can return the canonical provider slug or its bare model name.
    if !receipt["error"].is_null()
        || !matches!(
            receipt["model"].as_str(),
            Some(EMBEDDING_MODEL | "text-embedding-3-small")
        )
    {
        return Err("OpenRouter returned a different or missing embedding model.".into());
    }
    let items = receipt["data"]
        .as_array()
        .filter(|a| a.len() == count)
        .ok_or("Incomplete embedding batch.")?;
    let mut ordered = vec![None; count];
    for item in items {
        let slot = item["index"]
            .as_u64()
            .and_then(|i| ordered.get_mut(i as usize))
            .ok_or("Invalid embedding index.")?;
        let vector: Vec<f32> = serde_json::from_value(item["embedding"].clone())
            .map_err(|_| "Invalid embedding values.")?;
        let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if slot.is_some() || vector.len() != EMBEDDING_DIM || !norm.is_finite() || norm <= 0.0 {
            return Err("Invalid embedding dimension, norm, or duplicate index.".into());
        }
        *slot = Some(vector.into_iter().map(|x| x / norm).collect());
    }
    ordered
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "Missing embedding.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_receipts_keep_input_order_and_require_the_requested_vector_space() {
        let mut x = vec![0.0; EMBEDDING_DIM];
        x[0] = 2.0;
        let mut y = vec![0.0; EMBEDDING_DIM];
        y[1] = 3.0;
        let mut receipt = json!({"model":EMBEDDING_MODEL,"data":[{"index":1,"embedding":y},{"index":0,"embedding":x}]});
        let ordered = decode_embeddings(&receipt, 2).unwrap();
        assert_eq!(ordered[0][0], 1.0);
        assert_eq!(ordered[1][1], 1.0);
        receipt["model"] = json!("some/other-model");
        assert!(decode_embeddings(&receipt, 2).is_err());
        receipt["model"] = json!(EMBEDDING_MODEL);
        receipt["data"][1]["index"] = json!(1);
        assert!(decode_embeddings(&receipt, 2).is_err());
        receipt["data"][1]["index"] = json!(0);
        receipt["data"][1]["embedding"] = json!([1.0]);
        assert!(decode_embeddings(&receipt, 2).is_err());
    }

    fn config() -> Config {
        Config::new("test-key".into(), DEFAULT_JEV.into(), DEFAULT_LUNA.into()).unwrap()
    }

    fn chat() -> Value {
        json!({"max_tokens":512,"messages":[{"role":"user","content":"one example, please"}]})
    }

    fn reply() -> Value {
        json!({"model":DEFAULT_LUNA,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"whole reply"}}]})
    }

    #[test]
    fn credentials_and_explicit_model_profiles_are_validated_without_echoing_values() {
        for key in [
            "",
            "private\nheader: injection",
            "contains spaces",
            "opaque\"value",
        ] {
            assert!(Config::new(key.into(), DEFAULT_JEV.into(), DEFAULT_LUNA.into()).is_err());
        }
        for model in [
            "m",
            "a//b",
            "../bad",
            "@preset/coach",
            "openrouter/auto",
            "bad/sub:free",
            "openai/gpt-6-luna-20260922",
        ] {
            assert!(Config::new("test-key".into(), model.into(), DEFAULT_LUNA.into()).is_err());
        }
        let cfg = config();
        assert_eq!(cfg.model(Role::Jev), DEFAULT_JEV);
        assert!(matches_reported(DEFAULT_JEV, "typesafe/jev-1.14-20261014"));
        assert!(matches_reported(DEFAULT_LUNA, "openai/gpt-6-luna-20260922"));
        assert!(!matches_reported(
            DEFAULT_LUNA,
            "openai/gpt-6-luna-20991231"
        ));
        assert!(!matches_reported(DEFAULT_LUNA, DEFAULT_JEV));
    }

    #[test]
    fn each_role_uses_its_native_fields_and_the_configured_model() {
        let decision = json!({"state":{"goal":"read a law"},"questions":{"review":{"type":"choice","instructions":"Select a result","criteria":{"keep":"sufficient","revise":"missing facts"}}}});
        let body: Value =
            serde_json::from_str(&payload(Role::Jev, DEFAULT_JEV, &decision).unwrap()).unwrap();
        assert_eq!(body["model"], DEFAULT_JEV);
        assert_eq!(body.as_object().unwrap().len(), 3);
        assert!(payload(Role::Luna, DEFAULT_LUNA, &decision).is_err());
        assert!(payload(Role::Jev, DEFAULT_JEV, &chat()).is_err());
        let body: Value =
            serde_json::from_str(&payload(Role::Luna, DEFAULT_LUNA, &chat()).unwrap()).unwrap();
        assert_eq!(body["reasoning"], json!({"effort":"low","exclude":true}));
        assert_eq!(body["provider"], json!({"require_parameters":true}));
        assert_eq!(body["messages"], chat()["messages"]);
        for (key, value) in [
            ("model", json!(DEFAULT_JEV)),
            ("stream", json!(true)),
            ("models", json!(["backup/auto"])),
            ("reasoning", json!({"effort":"high"})),
            ("provider", json!({"require_parameters":false})),
            ("max_tokens", json!(8193)),
            ("tools", json!([{"type":"function"}])),
        ] {
            let mut bad = chat();
            bad[key] = value;
            // Invalid envelopes fail before a curl process is started.
            assert!(config().complete(Role::Luna, &bad).is_err());
        }
        let mut too_big = chat();
        too_big["messages"][0]["content"] = json!("x".repeat(REQUEST_LIMIT + 1));
        assert!(payload(Role::Luna, DEFAULT_LUNA, &too_big).is_err());
    }

    #[test]
    fn upstream_body_and_credential_stay_out_of_transport_errors() {
        let sensitive = "private-draft-text and test-key";
        for (code, suffix) in [
            (22, "401"),
            (22, "402"),
            (22, "429"),
            (0, "302"),
            (28, "000"),
        ] {
            let output = format!("{sensitive}\n{suffix}");
            let reason =
                decode(Role::Luna, DEFAULT_LUNA, Some(code), output.as_bytes()).unwrap_err();
            assert!(!reason.contains("private-draft") && !reason.contains("test-key"));
            if suffix != "000" {
                assert!(reason.contains(suffix));
            }
        }
        assert!(decode(Role::Luna, DEFAULT_LUNA, Some(0), b"bad-json\n200").is_err());
        assert!(decode(Role::Luna, DEFAULT_LUNA, Some(63), b"\n200")
            .unwrap_err()
            .contains("byte limit"));
        let mut bad = reply();
        bad["model"] = json!(DEFAULT_JEV);
        assert!(decode(
            Role::Luna,
            DEFAULT_LUNA,
            Some(0),
            format!("{bad}\n200").as_bytes()
        )
        .is_err());
    }

    #[test]
    fn recognized_decisions_and_finished_assistant_text_are_the_only_consumable_results() {
        let dec =
            json!({"model":DEFAULT_JEV,"answers":{"review":{"type":"choice","choice":"revise"}}});
        assert_eq!(
            decision_choice(&dec, DEFAULT_JEV, "review", &["keep", "revise"]).unwrap(),
            "revise"
        );
        assert!(decision_choice(&dec, DEFAULT_JEV, "review", &["keep"]).is_err());
        assert!(decision_choice(&dec, DEFAULT_JEV, "different", &["revise"]).is_err());
        assert!(decision_choice(&dec, DEFAULT_LUNA, "review", &["revise"]).is_err());
        assert_eq!(assistant_text(&reply()).unwrap(), "whole reply");
        for (path, value) in [
            ("/choices/0/finish_reason", json!("length")),
            ("/choices/0/message/role", json!("tool")),
            ("/choices/0/message/content", json!("  ")),
        ] {
            let mut bad = reply();
            *bad.pointer_mut(path).unwrap() = value;
            assert!(assistant_text(&bad).is_err());
        }
        for (field, value) in [
            ("tool_calls", json!([{}])),
            ("function_call", json!({})),
            ("refusal", json!("declined")),
        ] {
            let mut bad = reply();
            bad["choices"][0]["message"][field] = value;
            assert!(assistant_text(&bad).is_err());
        }
    }
}
