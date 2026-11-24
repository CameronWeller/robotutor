use super::{short, Hit, Result, Workspace};
use rusqlite::params;
use std::collections::HashSet;
const LIMIT: usize = 3;
impl Workspace {
    pub(super) fn search(
        &self,
        query: &str,
        vector: Option<&[f32]>,
        session: &str,
        recent_from: usize,
    ) -> Result<Vec<Hit>> {
        let (month, year) = when(query);
        let words = tokens(query);
        let mut stmt = self.db.prepare(
            "SELECT t.source,p.body,p.vector,t.day,p.learner,p.tutor,t.digest FROM turns t JOIN passages p USING(source)
            WHERE (visit!=?1 OR ordinal<?2) AND (?3 IS NULL OR substr(day,6,2)=?3)
            AND (?4 IS NULL OR substr(day,1,4)=?4)",
        )?;
        let mut rows = stmt.query(params![session, recent_from as i64, month, year])?;
        let mut ranks = Vec::new();
        while let Some(row) = rows.next()? {
            let passage: String = row.get(1)?;
            let common = words.intersection(&tokens(&passage)).count();
            let lexical = common as f32 / words.len().max(1) as f32;
            let semantic = match (vector, row.get_ref(2)?.as_blob_or_null()?) {
                (Some(q), Some(v)) if v.len() == q.len() * 4 => q
                    .iter()
                    .zip(v.chunks_exact(4))
                    .map(|(a, b)| a * f32::from_le_bytes(b.try_into().unwrap()))
                    .sum(),
                _ => 0.0,
            };
            let relevant = semantic >= 0.30 || (common > 0 && lexical >= 0.25);
            if relevant || month.is_some() || year.is_some() {
                let hit = Hit {
                    source: row.get(0)?,
                    day: row.get(3)?,
                    learner: row.get(4)?,
                    tutor: row.get(5)?,
                    digest: row.get(6)?,
                };
                ranks.push((relevant, semantic + 0.35 * lexical, hit, passage));
            }
        }
        ranks.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.total_cmp(&a.1))
                .then_with(|| a.2.source.cmp(&b.2.source))
        });
        let has_matches = ranks.first().is_some_and(|r| r.0);
        let mut hits = Vec::new();
        let mut seen_sources = HashSet::new();
        let mut seen_passages = HashSet::new();
        for (relevant, _, hit, passage) in ranks {
            if hits.len() == LIMIT || (has_matches && !relevant) {
                break;
            }
            if seen_sources.contains(&hit.source) || seen_passages.contains(&passage) {
                continue;
            }
            seen_sources.insert(hit.source.clone());
            seen_passages.insert(passage);
            hits.push(hit);
        }
        // Follow one recorded context edge from the strongest match. Respect the time filter.
        if let Some(first) = hits.first() {
            for id in self.links(&first.source)? {
                if seen_sources.contains(&id) {
                    continue;
                }
                if let Some(mut h) = self.lookup(&id)? {
                    let day = h.day.as_deref().unwrap_or("");
                    if month.as_deref().is_none_or(|m| day.get(5..7) == Some(m))
                        && year.as_deref().is_none_or(|y| day.get(..4) == Some(y))
                        && !id.rsplit_once('#').is_some_and(|(s, i)| {
                            s == session && i.parse::<usize>().is_ok_and(|n| n >= recent_from)
                        })
                    {
                        h.learner = h.learner.map(|s| short(&s, 850));
                        h.tutor = short(&h.tutor, 1200);
                        hits.push(h);
                        break;
                    }
                }
            }
        }
        Ok(hits)
    }
}
fn tokens(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|s| {
            s.len() > 2
                && !matches!(
                    s.as_str(),
                    "what"
                        | "which"
                        | "when"
                        | "where"
                        | "that"
                        | "this"
                        | "with"
                        | "and"
                        | "the"
                        | "was"
                        | "were"
                        | "our"
                        | "from"
                        | "remember"
                        | "about"
                        | "you"
                        | "did"
                        | "how"
                        | "have"
                        | "could"
                        | "for"
                        | "can"
                        | "back"
                        | "session"
                        | "discuss"
                        | "discussed"
                )
        })
        .collect()
}
fn when(query: &str) -> (Option<String>, Option<String>) {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let mut months = HashSet::new();
    let mut years = HashSet::new();
    let words: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    for (i, w) in words.iter().enumerate() {
        let lower = w.to_ascii_lowercase();
        if let Some(month) = MONTHS
            .iter()
            .position(|&m| m == lower || (lower.len() == 3 && m.starts_with(&lower)))
        {
            if lower != "may"
                || *w == "May"
                || i > 0
                    && matches!(
                        words[i - 1].to_ascii_lowercase().as_str(),
                        "in" | "during" | "last" | "from" | "of"
                    )
            {
                months.insert(format!("{:02}", month + 1));
            }
        }
        if w.len() == 4 && w.parse::<i32>().is_ok_and(|n| (1900..=2199).contains(&n)) {
            years.insert(w.to_string());
        }
    }
    (
        if months.len() == 1 {
            months.into_iter().next()
        } else {
            None
        },
        if years.len() == 1 {
            years.into_iter().next()
        } else {
            None
        },
    )
}
