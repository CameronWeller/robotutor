use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};
use url::Url;

fn random() -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn callback(line: &str, redirect: &Url, expected: &str) -> Option<Result<String, &'static str>> {
    let mut words = line.split_whitespace();
    if words.next()? != "GET" {
        return None;
    }
    let target = words.next()?;
    if !target.starts_with("/?") {
        return None;
    }
    let request = redirect.join(target).ok()?;
    let mut params = request.query_pairs();
    if params.clone().filter(|(k, _)| k == "state").count() != 1
        || !params.clone().any(|(k, v)| k == "state" && v == expected)
    {
        return None;
    }
    if params.clone().any(|(k, _)| k == "error") {
        return Some(Err("Google sign-in was canceled."));
    }
    Some(
        params
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned())
            .filter(|v| !v.is_empty())
            .ok_or("Google did not return a sign-in code."),
    )
}

pub fn sign_in() -> Result<String, &'static str> {
    let path = env::var_os("ROBOTUTOR_GOOGLE_CLIENT_FILE").ok_or(
        "Google sign-in has not been enabled for this installation. You can continue researching.",
    )?;
    let cfg: Value = serde_json::from_slice(
        &fs::read(path).map_err(|_| "Could not open the configured Google client file.")?,
    )
    .map_err(|_| "Invalid Google client file.")?;
    let id = cfg["installed"]["client_id"]
        .as_str()
        .filter(|s| s.ends_with(".apps.googleusercontent.com"))
        .ok_or("A Google Desktop OAuth client is required.")?;
    let secret = cfg["installed"]["client_secret"]
        .as_str()
        .ok_or("Google Desktop client configuration is incomplete.")?;
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|_| "Could not open the local Google callback.")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "Could not start the local callback.")?;
    let redirect = Url::parse(&format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let state = random();
    let verifier = random();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut consent = Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    consent.query_pairs_mut().extend_pairs([
        ("client_id", id),
        ("redirect_uri", redirect.as_str()),
        ("response_type", "code"),
        ("scope", "openid email"),
        ("prompt", "select_account"),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", &state),
    ]);
    webbrowser::open(consent.as_str())
        .map_err(|_| "Could not open the default browser for Google sign-in.")?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let code = loop {
        if Instant::now() >= deadline {
            return Err("Google sign-in timed out. You can try again.");
        }
        let (mut stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(80));
                continue;
            }
            Err(_) => return Err("Google callback failed."),
        };
        if stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .is_err()
        {
            continue;
        }
        let mut line = String::new();
        if BufReader::new((&stream).take(8192))
            .read_line(&mut line)
            .is_err()
        {
            continue;
        }
        let result = callback(&line, &redirect, &state);
        let response = if result.is_some() {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let _=write!(stream,"HTTP/1.1 {response}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nReturn to Robotutor for your sign-in result.");
        if let Some(value) = result {
            break value?;
        }
    };
    let client: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let token: Value = client
        .post("https://oauth2.googleapis.com/token")
        .send_form([
            ("code", code.as_str()),
            ("client_id", id),
            ("client_secret", secret),
            ("redirect_uri", redirect.as_str()),
            ("code_verifier", &verifier),
            ("grant_type", "authorization_code"),
        ])
        .map_err(|_| "Google could not exchange the sign-in code.")?
        .body_mut()
        .read_json()
        .map_err(|_| "Invalid Google token response.")?;
    let bearer = token["access_token"]
        .as_str()
        .ok_or("Google did not issue an access token.")?;
    let user: Value = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .header("Authorization", format!("Bearer {bearer}"))
        .call()
        .map_err(|_| "Could not verify the Google account.")?
        .body_mut()
        .read_json()
        .map_err(|_| "Invalid Google account response.")?;
    if user["sub"].as_str().is_none_or(str::is_empty) || user["email_verified"] != true {
        return Err("Google did not verify this account.");
    }
    user["email"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or("Google did not provide an account email.")
}

#[test]
fn only_matching_callbacks_complete_sign_in() {
    let url = Url::parse("http://127.0.0.1:1234/").unwrap();
    assert_eq!(
        callback("GET /?state=ok&code=test HTTP/1.1", &url, "ok"),
        Some(Ok("test".into()))
    );
    assert!(callback("GET /?state=wrong&code=test HTTP/1.1", &url, "ok").is_none());
    assert!(callback("GET /?state=ok&state=wrong&code=test HTTP/1.1", &url, "ok").is_none());
    assert!(
        callback("GET /?state=ok&error=access_denied HTTP/1.1", &url, "ok")
            .unwrap()
            .is_err()
    );
}
