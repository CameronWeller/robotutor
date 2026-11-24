use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;
use url::{Host, Url};

pub fn internet(raw: &str) -> Option<Url> {
    let u = Url::parse(raw).ok()?;
    let Host::Domain(host) = u.host()? else {
        return None;
    };
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || !host.contains('.')
        || host.ends_with('.')
        || [".local", ".localhost", ".internal", ".test", ".lan"]
            .iter()
            .any(|s| host.ends_with(s))
    {
        return None;
    }
    Some(u)
}

pub fn destination(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty() || text.len() > 2048 {
        return None;
    }
    if text.contains("://") {
        return internet(text).map(Into::into);
    }
    if !text.contains(char::is_whitespace) && text.contains('.') {
        return internet(&format!("https://{text}")).map(Into::into);
    }
    let mut u = Url::parse("https://www.google.com/search").ok()?;
    u.query_pairs_mut().append_pair("q", text);
    Some(u.into())
}

pub struct Source {
    pub address: String,
    pub article: bool,
}
pub fn source(raw: &str) -> Option<Source> {
    let mut u = internet(raw)?;
    let host = u.host_str()?;
    let path = u.path();
    let path_lower = path.to_ascii_lowercase();
    if host.split('.').any(|s| {
        matches!(
            s,
            "mail"
                | "accounts"
                | "auth"
                | "login"
                | "signin"
                | "banking"
                | "payments"
                | "drive"
                | "docs"
                | "classroom"
                | "myaccount"
                | "portal"
        )
    }) || path_lower.split('/').any(|s| {
        [
            "login",
            "signin",
            "sign-in",
            "account",
            "oauth",
            "authorize",
            "checkout",
            "inbox",
            "profile",
            "settings",
            "payment",
            "password",
        ]
        .iter()
        .any(|k| s.starts_with(k))
    }) {
        return None;
    }
    let article = (host.ends_with(".wikipedia.org")
        && path.starts_with("/wiki/")
        && !path.contains(':')
        && !path_lower.contains("%3a"))
        || (host == "developer.mozilla.org" && path.contains("/docs/"))
        || (host == "science.nasa.gov"
            && [
                "/earth/",
                "/universe/",
                "/solar-system/",
                "/mission/",
                "/resource/",
            ]
            .iter()
            .any(|p| path.starts_with(p)))
        || (host == "arxiv.org" && (path.starts_with("/abs/") || path.starts_with("/html/")))
        || (host == "openstax.org" && path.starts_with("/books/") && path.contains("/pages/"));
    let address = if article {
        u.set_query(None);
        u.set_fragment(None);
        u.to_string()
    } else {
        u.origin().ascii_serialization()
    };
    Some(Source { address, article })
}

fn bounded(s: &str, limit: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}
pub fn observation(source: &Source, metadata: &Value) -> Value {
    let title = if source.article {
        bounded(metadata["title"].as_str().unwrap_or(""), 160)
    } else {
        String::new()
    };
    let headings: Vec<_> = if source.article {
        metadata["headings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| bounded(s, 100))
            .filter(|s| !s.is_empty())
            .take(6)
            .collect()
    } else {
        vec![]
    };
    json!({"resource":source.address, "title":title, "available_headings":headings})
}
pub struct Notebook(Connection);
impl Notebook {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS research(id INTEGER PRIMARY KEY, minute INTEGER NOT NULL, source TEXT NOT NULL UNIQUE, evidence TEXT NOT NULL);")?;
        let book = Self(db);
        book.expire()?;
        Ok(book)
    }
    fn expire(&self) -> rusqlite::Result<()> {
        self.0.execute(
            "DELETE FROM research WHERE minute < unixepoch()/60-43200",
            [],
        )?;
        Ok(())
    }
    pub fn record(&mut self, evidence: &Value) -> rusqlite::Result<()> {
        self.expire()?;
        let tx = self.0.transaction()?;
        tx.execute(
            "INSERT INTO research(minute,source,evidence) VALUES(unixepoch()/60,?1,?2)
            ON CONFLICT(source) DO UPDATE SET minute=excluded.minute,evidence=excluded.evidence",
            params![evidence["resource"].as_str(), evidence.to_string()],
        )?;
        tx.execute("DELETE FROM research WHERE id NOT IN (SELECT id FROM research ORDER BY minute DESC,id DESC LIMIT 200)",[])?;
        tx.commit()
    }
    pub fn recent(&self) -> rusqlite::Result<Vec<Value>> {
        self.expire()?;
        let mut stmt = self
            .0
            .prepare("SELECT evidence FROM research ORDER BY minute DESC,id DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|s| Ok(serde_json::from_str(&s?).unwrap_or(Value::Null)))
            .collect()
    }
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.0.execute("DELETE FROM research", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_and_expired_sources_stay_out_of_review() {
        assert!(
            !source("https://en.wikipedia.org/wiki/Special:Search?search=secret")
                .unwrap()
                .article
        );
        assert!(source("https://private.example.org/settings/security").is_none());
        assert!(
            !source("https://science.nasa.gov.evil.example/solar-system/")
                .unwrap()
                .article
        );
        let mut book = Notebook::open(Path::new(":memory:")).unwrap();
        let evidence = observation(
            &source("https://arxiv.org/abs/1234.5678?tracking=yes").unwrap(),
            &json!({"title":"Study","headings":["Purpose"]}),
        );
        book.record(&evidence).unwrap();
        book.0.execute("UPDATE research SET minute=0", []).unwrap();
        assert!(book.recent().unwrap().is_empty());
    }
    #[test]
    fn minimal_evidence() {
        let m = json!({"title":"private search","headings":["sensitive"],"form":"secret"});
        let other = source("https://example.org/student/123?token=secret#me").unwrap();
        assert_eq!(
            observation(&other, &m),
            json!({"resource":"https://example.org","title":"","available_headings":[]})
        );
        assert!(source("https://accounts.google.com/o/oauth2").is_none());
        assert!(source("https://example.org/oauth/callback?code=123").is_none());
        assert!(internet("https://user:password@example.org").is_none());
        assert!(internet("https://127.0.0.1/x").is_none());
        assert!(destination("file:///etc/passwd").is_none());
        let wiki = source("https://en.wikipedia.org/wiki/Orbit?ref=secret#here").unwrap();
        assert_eq!(wiki.address, "https://en.wikipedia.org/wiki/Orbit");
        let mut book = Notebook::open(Path::new(":memory:")).unwrap();
        let ev = observation(&wiki, &m);
        book.record(&ev).unwrap();
        book.record(&ev).unwrap();
        assert_eq!(book.recent().unwrap(), vec![ev]);
        book.clear().unwrap();
        assert!(book.recent().unwrap().is_empty());
    }
}
