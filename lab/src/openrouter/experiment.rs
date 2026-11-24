//! A single paid probe campaign. Keep its private ledger between invocations.
//! The lifetime limit on the temporary OpenRouter key is the provider-side
//! backstop. This adapter cannot terminate provider work already accepted.
use super::{payload, Config, Role, DEFAULT_JEV, DEFAULT_LUNA};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FORMAT: &str = "openrouter-probe-campaign/1";
const SHORT_CALLS: usize = 3;
const SUITE_CALLS: usize = 9;
const WINDOW_MS: u64 = 600_000;
const LEDGER_BYTES: u64 = 16_384;

fn now_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "campaign requires an accurate system clock")?
        .as_millis()
        .try_into()
        .map_err(|_| "invalid clock".into())
}

// Round externally reported dollars UP to nanodollars before comparing them
// with an operator's integer-microdollar ceiling. Reject strings and nulls.
fn nanodollars(value: &Value) -> Result<u64, String> {
    let n = value
        .as_f64()
        .filter(|v| (0.0..=1_000_000.0).contains(v))
        .ok_or("missing or invalid OpenRouter dollar amount")?;
    Ok((n * 1_000_000_000.0).ceil() as u64)
}

struct Ledger {
    start: u64,
    budget: u64,
    call_limit: usize,
    // Dispatch order identifies the one unresolved request after a restart.
    attempted: Vec<String>,
    pending: bool,
    billed_nanos: u64,
}

impl Ledger {
    fn begin(start: u64, budget: u64, call_limit: usize) -> Result<Self, String> {
        if budget == 0 || budget > 1_000_000_000_000 || start.checked_add(WINDOW_MS).is_none() {
            return Err("invalid campaign budget or clock".into());
        }
        if !matches!(call_limit, SHORT_CALLS | SUITE_CALLS) {
            return Err("unsupported campaign call limit".into());
        }
        Ok(Self {
            start,
            budget,
            call_limit,
            attempted: Vec::new(),
            pending: false,
            billed_nanos: 0,
        })
    }

    fn remaining_ms(&self, now: u64) -> Result<u64, String> {
        if now < self.start {
            return Err("campaign clock moved backwards".into());
        }
        Ok(WINDOW_MS.saturating_sub(now - self.start))
    }

    fn available_nanos(&self) -> u64 {
        (self.budget * 1000).saturating_sub(self.billed_nanos)
    }

    fn check_next(&self, id: &str) -> Result<(), String> {
        if self.pending {
            return Err("a previous request has uncertain billing; campaign stopped".into());
        }
        if self.attempted.len() >= self.call_limit {
            return Err(format!("{}-call campaign limit reached", self.call_limit));
        }
        if self.attempted.iter().any(|request| request == id) {
            return Err("this request has already been dispatched".into());
        }
        if self.available_nanos() == 0 {
            return Err("campaign credit ceiling reached".into());
        }
        Ok(())
    }

    fn preflight(&self, info: &Value, credit_only: bool) -> Result<(), String> {
        let data = &info["data"];
        if !data.get("limit_reset").is_some_and(Value::is_null) {
            return Err(
                "use a temporary OpenRouter key with a non-resetting lifetime credit cap".into(),
            );
        }
        let cap = nanodollars(&data["limit"])?;
        let left = nanodollars(&data["limit_remaining"])?;
        if cap == 0 || cap > self.budget * 1000 || left == 0 || left > cap {
            return Err("key needs a positive available balance and a lifetime cap within the campaign ceiling".into());
        }
        if left > self.available_nanos() {
            return Err("remaining key exposure plus recorded campaign charges exceeds the authorized total".into());
        }
        if !credit_only && data["include_byok_in_limit"] != true {
            return Err("funding is unverified: confirm an OpenRouter-credit-only account or use a key whose metadata includes BYOK in its cap".into());
        }
        Ok(())
    }

    fn advance(&mut self, row: &Value) -> Result<(), String> {
        match row[0].as_str() {
            Some("dispatch") if row.as_array().is_some_and(|r| r.len() == 2) => {
                let id = row[1]
                    .as_str()
                    .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or("invalid campaign dispatch")?;
                self.check_next(id)?;
                self.attempted.push(id.to_owned());
                self.pending = true;
            }
            Some("billed_nanos")
                if self.pending && row.as_array().is_some_and(|r| r.len() == 2) =>
            {
                self.billed_nanos = self
                    .billed_nanos
                    .checked_add(row[1].as_u64().ok_or("invalid receipt")?)
                    .ok_or("invalid cumulative charge")?;
                self.pending = false;
            }
            _ => return Err("invalid campaign record sequence".into()),
        }
        Ok(())
    }

    fn read(file: &mut File) -> Result<Self, String> {
        let mut bytes = Vec::new();
        file.take(LEDGER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > LEDGER_BYTES || bytes.last() != Some(&b'\n') {
            return Err("campaign ledger incomplete; refusing to dispatch".into());
        }
        let mut lines = bytes[..bytes.len() - 1].split(|b| *b == b'\n');
        let parse =
            |r: &[u8]| serde_json::from_slice::<Value>(r).map_err(|_| "invalid campaign ledger");
        let header = parse(lines.next().ok_or("missing campaign header")?)?;
        if header[0] != FORMAT {
            return Err("unsupported campaign header".into());
        }
        let call_limit = match header.as_array().map(Vec::as_slice) {
            // The original header had a fixed limit of three. A legacy
            // campaign must never gain dispatches merely by being reopened.
            Some([_, _, _]) => SHORT_CALLS,
            Some([_, _, _, limit]) => limit
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or("invalid campaign call limit")?,
            _ => return Err("unsupported campaign header".into()),
        };
        let mut ledger = Self::begin(
            header[1].as_u64().ok_or("invalid campaign epoch")?,
            header[2].as_u64().ok_or("invalid campaign ceiling")?,
            call_limit,
        )?;
        for row in lines {
            ledger.advance(&parse(row)?)?;
        }
        Ok(ledger)
    }

    fn summary(&self) -> Value {
        json!({"started_unix_ms":self.start, "deadline_unix_ms":self.start+WINDOW_MS,
            "budget_micros":self.budget,"billed_nanos":self.billed_nanos,"call_limit":self.call_limit,
            "attempts":self.attempted.len(),"uncertain_billing":self.pending})
    }
}

fn append(file: &mut File, value: &Value) -> Result<(), String> {
    let mut line = value.to_string().into_bytes();
    line.push(b'\n');
    file.seek(SeekFrom::End(0))
        .and_then(|_| file.write_all(&line))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

fn request_id(role: Role, model: &str, body: &Value) -> Result<String, String> {
    let wire = payload(role, model, body)?;
    if wire.len() > 16_384 {
        return Err("probe exceeds 16 KiB of model input".into());
    }
    match role {
        Role::Luna if body["max_tokens"].as_u64().is_some_and(|n| n <= 1024) => {}
        Role::Jev
            if body["questions"].as_object().is_some_and(|q| {
                q.len() == 1
                    && q.values().all(|v| {
                        v["type"] == "choice"
                            && v["criteria"].as_object().is_some_and(|c| {
                                (2..=16).contains(&c.len())
                                    && c.keys().all(|k| !k.is_empty() && k.len() <= 64)
                            })
                    })
            }) => {}
        _ => {
            return Err(
                "probe needs one bounded Jev choice or at most 1024 Luna output tokens".into(),
            )
        }
    }
    Ok(format!("{:x}", Sha256::digest(wire)))
}

pub struct Campaign<'a> {
    config: &'a Config,
    file: File,
    ledger: Ledger,
    deadline: Instant,
    credit_only: bool,
}

impl<'a> Campaign<'a> {
    /// Opening an existing file keeps its original start, ceiling, attempts and
    /// charges. Expired ledgers remain readable for capture recovery. A lock
    /// spans the full campaign invocation, including provider I/O.
    pub fn open(path: &Path, config: &'a Config, budget_micros: u64) -> Result<Self, String> {
        Self::open_with_limit(path, config, budget_micros, SHORT_CALLS)
    }

    /// The short probes, sparse hybrid, and finishing hybrid share nine calls,
    /// one original deadline and one credit ceiling for the entire suite.
    pub fn open_suite(path: &Path, config: &'a Config, budget_micros: u64) -> Result<Self, String> {
        Self::open_with_limit(path, config, budget_micros, SUITE_CALLS)
    }

    fn open_with_limit(
        path: &Path,
        config: &'a Config,
        budget_micros: u64,
        call_limit: usize,
    ) -> Result<Self, String> {
        if config.model(Role::Jev) != DEFAULT_JEV || config.model(Role::Luna) != DEFAULT_LUNA {
            return Err(format!(
                "use the exact probe model IDs {DEFAULT_JEV} and {DEFAULT_LUNA}"
            ));
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::custom_flags(
            &mut opts,
            windows_sys::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH,
        );
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let (mut file, new) = match opts.open(path) {
            Ok(f) => (f, true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|e| e.to_string())?,
                false,
            ),
            Err(e) => return Err(e.to_string()),
        };
        file.try_lock().map_err(|_| "campaign is already in use")?;
        let clock = now_ms()?;
        let ledger = if new {
            let ledger = Ledger::begin(clock, budget_micros, call_limit)?;
            append(
                &mut file,
                &json!([FORMAT, clock, budget_micros, call_limit]),
            )?;
            crate::platform::sync_created_entry(path).map_err(|e| e.to_string())?;
            ledger
        } else {
            Ledger::read(&mut file)?
        };
        if ledger.budget != budget_micros {
            return Err("existing campaign has a different authorized ceiling".into());
        }
        if ledger.call_limit != call_limit {
            return Err(format!(
                "existing campaign has a {}-call limit; requested {call_limit}",
                ledger.call_limit
            ));
        }
        let deadline = Instant::now() + Duration::from_millis(ledger.remaining_ms(now_ms()?)?);
        Ok(Self {
            file,
            ledger,
            config,
            deadline,
            credit_only: false,
        })
    }

    /// Inspect a closed ledger without a key or a network request.
    pub fn status(path: &Path) -> Result<Value, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        file.try_lock_shared()
            .map_err(|_| "campaign is currently in use")?;
        Ok(Ledger::read(&mut file)?.summary())
    }

    pub fn summary(&self) -> Value {
        self.ledger.summary()
    }

    /// A completed receipt can establish a charge even if its result file
    /// was lost. Uncertainty belongs to its specific last dispatch.
    pub fn request_billed(&self, role: Role, request: &Value) -> Result<bool, String> {
        let id = request_id(role, self.config.model(role), request)?;
        if self.ledger.pending && self.ledger.attempted.last() == Some(&id) {
            return Err("this request has uncertain billing; campaign stopped".into());
        }
        Ok(self.ledger.attempted.contains(&id))
    }

    /// An operator verified that neither selected model can incur separately
    /// billed provider charges on this account. This assertion expires at exit.
    pub fn confirm_credit_only_account(&mut self) {
        self.credit_only = true;
    }

    fn deadline(&self) -> Result<Instant, String> {
        let now = Instant::now();
        let end = self
            .deadline
            .min(now + Duration::from_millis(self.ledger.remaining_ms(now_ms()?)?));
        if end <= now {
            return Err("the ten-minute campaign has expired".into());
        }
        Ok(end)
    }

    fn record(&mut self, row: Value) -> Result<(), String> {
        // Advance first: even if syncing fails this process cannot refund an
        // uncertain dispatch or go on to a second inference.
        self.ledger.advance(&row)?;
        if let Err(e) = append(&mut self.file, &row) {
            self.ledger.pending = true;
            return Err(e);
        }
        Ok(())
    }

    pub fn complete(&mut self, role: Role, request: &Value) -> Result<Value, String> {
        let cfg = self.config;
        self.run(
            role,
            request,
            |end| cfg.key_info(end),
            |end| cfg.complete_by(role, request, Some(end)),
        )
    }

    fn run(
        &mut self,
        role: Role,
        request: &Value,
        inspect: impl FnOnce(Instant) -> Result<Value, String>,
        inference: impl FnOnce(Instant) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let id = request_id(role, self.config.model(role), request)?;
        self.ledger.check_next(&id)?;
        let info = inspect(self.deadline()?)?;
        self.ledger.preflight(&info, self.credit_only)?;
        self.deadline()?;
        self.record(json!(["dispatch", id]))?;
        let capture = inference(self.deadline()?)?;
        if capture["usage"]["is_byok"] == true || capture["is_byok"] == true {
            return Err("external provider billing reported; campaign stopped".into());
        }
        let cost = nanodollars(&capture["usage"]["cost"])
            .map_err(|_| "provider cost missing or ambiguous; campaign stopped")?;
        self.record(json!(["billed_nanos", cost]))?;
        if self.ledger.billed_nanos > self.ledger.budget * 1000 {
            return Err(
                "reported provider charge exceeds the campaign ceiling; campaign stopped".into(),
            );
        }
        Ok(capture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            Self(std::env::temp_dir().join(format!(
                "robotutor-paid-probe-{}-{}-{}",
                std::process::id(),
                now_ms().unwrap(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn cfg() -> Config {
        Config::new("test-key".into(), DEFAULT_JEV.into(), DEFAULT_LUNA.into()).unwrap()
    }
    fn msg(text: &str) -> Value {
        json!({"max_tokens":384,"messages":[{"role":"user","content":text}]})
    }
    fn key(remaining: f64) -> Value {
        json!({"data":{"limit":1.0,"limit_remaining":remaining,"limit_reset":null,"include_byok_in_limit":true}})
    }

    #[test]
    fn exact_same_campaign_survives_restart_rejects_duplicate_and_ends_after_three_calls() {
        let file = Temp::new();
        let cfg = cfg();
        let mut c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        let header = c.summary();
        assert!(Campaign::open(&file.0, &cfg, 1_000_000).is_err());
        for (i, (prompt, left)) in [("one", 1.0), ("two", 0.99), ("three", 0.98)]
            .into_iter()
            .enumerate()
        {
            let m = msg(prompt);
            let r = c
                .run(
                    Role::Luna,
                    &m,
                    |_| Ok(key(left)),
                    |end| {
                        assert!(end > Instant::now());
                        assert!(
                            end.saturating_duration_since(Instant::now())
                                <= Duration::from_secs(600)
                        );
                        Ok(json!({"usage":{"cost":0.01}}))
                    },
                )
                .unwrap();
            assert_eq!(r["usage"]["cost"], 0.01);
            drop(c);
            assert!(Campaign::open(&file.0, &cfg, 999_999).is_err());
            c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
            assert_eq!(c.summary()["attempts"], i + 1);
            assert_eq!(c.summary()["deadline_unix_ms"], header["deadline_unix_ms"]);
            assert!(c
                .run(
                    Role::Luna,
                    &m,
                    |_| panic!("duplicate preflight"),
                    |_| panic!("duplicate inference")
                )
                .is_err());
        }
        assert!(c
            .run(Role::Luna, &msg("four"), |_| panic!(), |_| panic!())
            .is_err());
        drop(c);
        let summary = Campaign::status(&file.0).unwrap();
        assert_eq!(summary["billed_nanos"], 30_000_000);
        assert_eq!(summary["uncertain_billing"], false);
        assert!(!std::fs::read_to_string(&file.0)
            .unwrap()
            .contains("test-key"));
    }

    #[test]
    fn the_provider_cap_and_remaining_balance_bound_every_dispatch() {
        let mut l = Ledger::begin(10, 1_000_000, SHORT_CALLS).unwrap();
        for info in [
            json!({"data":{"limit":null,"limit_remaining":null,"limit_reset":null}}),
            json!({"data":{"limit":1.01,"limit_remaining":0.01,"limit_reset":null}}),
            json!({"data":{"limit":1,"limit_remaining":1,"limit_reset":"daily"}}),
            json!({"data":{"limit":1,"limit_remaining":1}}),
            json!({"data":{"limit":1,"limit_remaining":0,"limit_reset":null}}),
            json!({"data":{"limit":1,"limit_remaining":2,"limit_reset":null}}),
            json!({"data":{"limit":1,"limit_remaining":1,"limit_reset":null,"byok_usage":0.01,"include_byok_in_limit":false}}),
        ] {
            assert!(l.preflight(&info, false).is_err());
        }
        assert!(l.preflight(&key(1.0), false).is_ok());
        l.billed_nanos = 500_000_000;
        assert!(l.preflight(&key(1.0), false).is_err());
        assert!(l.preflight(&key(0.5), false).is_ok());
        assert_eq!(nanodollars(&json!(0.0000000003)).unwrap(), 1);
        assert_eq!(l.remaining_ms(10).unwrap(), WINDOW_MS);
        assert_eq!(l.remaining_ms(10 + WINDOW_MS - 1).unwrap(), 1);
        assert_eq!(l.remaining_ms(10 + WINDOW_MS).unwrap(), 0);
        assert!(l.remaining_ms(9).is_err());
    }

    #[test]
    fn nine_call_suite_shares_clock_bill_and_dispatches_across_all_three_phases() {
        let path = Temp::new();
        let cfg = cfg();
        let original = Campaign::open_suite(&path.0, &cfg, 1_000_000)
            .unwrap()
            .summary();
        assert_eq!(original["call_limit"], 9);
        for (phase_index, phase) in ["short", "sparse", "finishing"].iter().enumerate() {
            let mut c = Campaign::open_suite(&path.0, &cfg, 1_000_000).unwrap();
            for checkpoint in 0..3 {
                let request = msg(&format!("{phase} checkpoint {checkpoint}"));
                c.run(
                    Role::Luna,
                    &request,
                    |_| Ok(key(0.85)),
                    |_| Ok(json!({"usage":{"cost":0.01}})),
                )
                .unwrap();
            }
            let s = c.summary();
            assert_eq!(s["attempts"], 3 * (phase_index + 1));
            assert_eq!(s["billed_nanos"], 30_000_000 * (phase_index + 1));
            assert_eq!(s["deadline_unix_ms"], original["deadline_unix_ms"]);
            assert_eq!(s["budget_micros"], original["budget_micros"]);
            drop(c);
            assert!(Campaign::open(&path.0, &cfg, 1_000_000).is_err());
            assert!(Campaign::open_suite(&path.0, &cfg, 2_000_000).is_err());
            assert_eq!(Campaign::status(&path.0).unwrap(), s);
        }
        let mut c = Campaign::open_suite(&path.0, &cfg, 1_000_000).unwrap();
        assert!(c
            .run(Role::Luna, &msg("tenth"), |_| panic!(), |_| panic!())
            .unwrap_err()
            .contains("9-call"));
    }

    #[test]
    fn legacy_and_explicit_short_campaigns_refuse_upgrades_and_invalid_headers() {
        let cfg = cfg();
        for legacy in [false, true] {
            let path = Temp::new();
            if legacy {
                let mut file = File::create_new(&path.0).unwrap();
                append(&mut file, &json!([FORMAT, now_ms().unwrap(), 1_000_000])).unwrap();
            } else {
                drop(Campaign::open(&path.0, &cfg, 1_000_000).unwrap());
            }
            let before = std::fs::read(&path.0).unwrap();
            assert!(Campaign::open_suite(&path.0, &cfg, 1_000_000).is_err());
            assert_eq!(std::fs::read(&path.0).unwrap(), before);
            let c = Campaign::open(&path.0, &cfg, 1_000_000).unwrap();
            assert_eq!(c.summary()["call_limit"], 3);
        }
        for invalid_cap in [json!(0), json!(4), json!(10), json!("9"), Value::Null] {
            let path = Temp::new();
            let mut file = File::create_new(&path.0).unwrap();
            append(
                &mut file,
                &json!([FORMAT, now_ms().unwrap(), 1_000_000, invalid_cap]),
            )
            .unwrap();
            assert!(Campaign::status(&path.0).is_err());
            assert!(Campaign::open_suite(&path.0, &cfg, 1_000_000).is_err());
        }
    }

    #[test]
    fn an_operator_can_verify_credit_only_funding_after_a_refused_free_preflight() {
        let file = Temp::new();
        let cfg = cfg();
        let mut c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        let mut info = key(1.0);
        info["data"]
            .as_object_mut()
            .unwrap()
            .remove("include_byok_in_limit");
        info["data"]["byok_usage"] = json!(0.0);
        let request = msg("first");
        assert!(c
            .run(
                Role::Luna,
                &request,
                |_| Ok(info.clone()),
                |_| panic!("unverified funding dispatched")
            )
            .is_err());
        assert_eq!(c.summary()["attempts"], 0);
        c.confirm_credit_only_account();
        c.run(
            Role::Luna,
            &request,
            |_| Ok(info),
            |_| Ok(json!({"usage":{"cost":0.01}})),
        )
        .unwrap();
        assert_eq!(c.summary()["attempts"], 1);
    }

    #[test]
    fn ambiguous_or_incomplete_receipts_never_open_a_retry() {
        let cfg = cfg();
        for response in [
            Err("timeout".into()),
            Ok(json!({})),
            Ok(json!({"usage":{"cost":"unknown"}})),
            Ok(json!({"usage":{"is_byok":true,"cost":0}})),
        ] {
            let file = Temp::new();
            let mut c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
            assert!(c
                .run(
                    Role::Luna,
                    &msg("private synthetic input"),
                    |_| Ok(key(1.0)),
                    |_| response
                )
                .is_err());
            drop(c);
            let mut resumed = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
            assert_eq!(resumed.summary()["uncertain_billing"], true);
            assert!(resumed
                .run(Role::Luna, &msg("another"), |_| panic!(), |_| panic!())
                .is_err());
            drop(resumed); // Windows enforces the exclusive lock on other file handles.
            assert!(!std::fs::read_to_string(&file.0)
                .unwrap()
                .contains("private synthetic"));
        }
        let file = Temp::new();
        let mut c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        assert!(c
            .run(
                Role::Luna,
                &msg("expensive"),
                |_| Ok(key(1.0)),
                |_| Ok(json!({"usage":{"cost":1.01}}))
            )
            .is_err());
        assert!(c
            .run(Role::Luna, &msg("again"), |_| panic!(), |_| panic!())
            .is_err());
        drop(c);
        OpenOptions::new()
            .append(true)
            .open(&file.0)
            .unwrap()
            .write_all(b"[")
            .unwrap();
        assert!(Campaign::open(&file.0, &cfg, 1_000_000).is_err());
    }

    #[test]
    fn spending_the_remaining_time_on_the_key_check_never_sends_the_inference() {
        let file = Temp::new();
        let cfg = cfg();
        let mut campaign = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        campaign.deadline = Instant::now() + Duration::from_millis(15);
        let result = campaign.run(
            Role::Luna,
            &msg("time"),
            |end| {
                std::thread::sleep(
                    end.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
                );
                Ok(key(1.0))
            },
            |_| panic!("paid request after expired key check"),
        );
        assert!(result.is_err());
        assert_eq!(campaign.summary()["attempts"], 0);
    }

    #[test]
    fn a_resumed_campaign_distinguishes_paid_zero_cost_uncertain_and_unseen_requests() {
        let file = Temp::new();
        let cfg = cfg();
        let paid = msg("delivered");
        let free = msg("zero cost");
        let lost = msg("ambiguous completion");
        let absent = msg("never sent");
        let mut c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        assert!(!c.request_billed(Role::Luna, &paid).unwrap());
        for (request, cost) in [(&paid, 0.01), (&free, 0.0)] {
            c.run(
                Role::Luna,
                request,
                |_| Ok(key(0.98)),
                |_| Ok(json!({"usage":{"cost":cost}})),
            )
            .unwrap();
        }
        assert!(c
            .run(
                Role::Luna,
                &lost,
                |_| Ok(key(0.98)),
                |_| Err("timeout".into())
            )
            .is_err());
        drop(c);
        let c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        assert!(c.request_billed(Role::Luna, &paid).unwrap());
        assert!(c.request_billed(Role::Luna, &free).unwrap());
        assert!(c.request_billed(Role::Luna, &lost).is_err());
        assert!(!c.request_billed(Role::Luna, &absent).unwrap());
        assert!(c.request_billed(Role::Jev, &paid).is_err());
    }

    #[test]
    fn alias_configuration_cannot_start_a_campaign_with_different_request_model_ids() {
        for (jev, luna) in [
            ("typesafe/jev-1.14-20261014", DEFAULT_LUNA),
            (DEFAULT_JEV, "openai/gpt-6-luna-20260922"),
        ] {
            let file = Temp::new();
            let cfg = Config::new("test-key".into(), jev.into(), luna.into()).unwrap();
            assert!(Campaign::open(&file.0, &cfg, 1_000_000).is_err());
            assert!(!file.0.exists());
        }
    }

    #[test]
    fn an_expired_campaign_can_recover_receipts_without_contacting_the_provider() {
        let cfg = cfg();
        let path = Temp::new();
        let completed = msg("settled before expiry");
        let id = request_id(Role::Luna, DEFAULT_LUNA, &completed).unwrap();
        let mut file = File::create_new(&path.0).unwrap();
        let expired_start = now_ms().unwrap() - WINDOW_MS;
        append(&mut file, &json!([FORMAT, expired_start, 1_000_000])).unwrap();
        append(&mut file, &json!(["dispatch", id])).unwrap();
        append(&mut file, &json!(["billed_nanos", 1000])).unwrap();
        drop(file);

        let mut c = Campaign::open(&path.0, &cfg, 1_000_000).unwrap();
        assert!(c.request_billed(Role::Luna, &completed).unwrap());
        let next = msg("undispatched");
        assert!(!c.request_billed(Role::Luna, &next).unwrap());
        assert!(c
            .run(
                Role::Luna,
                &next,
                |_| panic!("key lookup after expiry"),
                |_| panic!("inference after expiry")
            )
            .unwrap_err()
            .contains("expired"));
        assert_eq!(c.summary()["attempts"], 1);
    }

    #[test]
    fn oversized_unbounded_or_expired_requests_never_dispatch() {
        let cfg = cfg();
        let file = Temp::new();
        let mut c = Campaign::open(&file.0, &cfg, 1_000_000).unwrap();
        let mut huge = msg("raw text");
        huge["max_tokens"] = json!(1025);
        for (r, body) in [
            (Role::Luna, huge),
            (Role::Luna, msg(&"x".repeat(16_384))),
            (
                Role::Jev,
                json!({"state":{},"questions":{"a":{"type":"text"}}}),
            ),
        ] {
            assert!(c.run(r, &body, |_| panic!(), |_| panic!()).is_err());
        }
        let choice = json!({"state":"public prompt","questions":{"a":{"type":"choice","criteria":{"a":"one","b":"two"}}}});
        assert!(request_id(Role::Jev, DEFAULT_JEV, &choice).is_ok());
        c.deadline = Instant::now();
        assert!(c
            .run(Role::Jev, &choice, |_| panic!(), |_| panic!())
            .is_err());
        assert_eq!(c.summary()["attempts"], 0);
    }
}
