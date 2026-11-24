//! Audit an editorial comparison of a learner's goal and source cards before planning units.
//! An excerpt can establish a topic lead. Scope, performance, minutes and mastery need other evidence.
use super::program::{setting_key, Frame};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn list<'a>(object: &'a Value, field: &str, max: usize) -> Result<&'a [Value], String> {
    object[field]
        .as_array()
        .filter(|v| v.len() <= max)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("expected at most {max} items in {field}"))
}

fn word<'a>(object: &'a Value, field: &str, max: usize) -> Result<&'a str, String> {
    object[field]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= max)
        .ok_or_else(|| format!("missing or oversized {field}"))
}

fn facet_id(value: &Value) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| "expected a positive facet ID".to_owned())
}

struct Binding {
    facets: BTreeSet<u32>,
    held: bool,
}

struct Registry<'a> {
    evidence: BTreeMap<&'a str, Binding>,
    aims: BTreeMap<&'a str, u32>,
    obligations: BTreeSet<u32>,
    challenges: BTreeMap<u32, &'a str>,
    source_count: usize,
}

impl<'a> Registry<'a> {
    fn read(material: &'a Value) -> Result<Self, String> {
        let cards = list(material, "sources", 24)?;
        let mut evidence = BTreeMap::new();
        let mut publishers = BTreeMap::new();
        let mut urls = BTreeSet::new();
        let mut obligations = BTreeSet::new();
        for card in cards {
            let publisher = word(card, "publisher", 120)?;
            let url = word(card, "url", 1024)?;
            let held = card["held"].as_bool().ok_or("missing source visibility")?;
            if !url.starts_with("https://") || !urls.insert(url) {
                return Err("a source needs a unique HTTPS locator".into());
            }
            if publishers
                .insert(publisher, held)
                .is_some_and(|previous| previous != held)
            {
                return Err(format!(
                    "held and visible cards share publisher {publisher}"
                ));
            }
            let labels = list(card, "labels", 64)?;
            if labels.is_empty() {
                return Err(format!("empty source card at {url}"));
            }
            for label in labels {
                let key = word(label, "key", 96)?;
                word(label, "text", 180)?;
                let mut facets = BTreeSet::new();
                for id in list(label, "facets", 16)? {
                    if !facets.insert(facet_id(id)?) {
                        return Err(format!("duplicate mapping for {key}"));
                    }
                }
                if facets.is_empty() {
                    return Err(format!("unmapped source label {key}"));
                }
                obligations.extend(&facets);
                if evidence.insert(key, Binding { facets, held }).is_some() {
                    return Err(format!("duplicate evidence key {key}"));
                }
            }
        }
        if !publishers.values().any(|held| !held) {
            return Err("no source cards are visible to the first proposal".into());
        }
        let mut aims = BTreeMap::new();
        for aim in list(material, "owner_aims", 64)? {
            let key = word(aim, "key", 96)?;
            let id = facet_id(&aim["facet"])?;
            word(aim, "reason", 512)?;
            if evidence.contains_key(key) || aims.insert(key, id).is_some() {
                return Err(format!("ambiguous basis key {key}"));
            }
            obligations.insert(id);
        }
        let mut challenges = BTreeMap::new();
        for challenge in list(material, "challenges", 64)? {
            let id = facet_id(&challenge["facet"])?;
            let key = word(challenge, "evidence", 96)?;
            let prompt = word(challenge, "question", 512)?;
            if !evidence
                .get(key)
                .is_some_and(|b| b.held && b.facets.contains(&id))
            {
                return Err(format!(
                    "challenge for area {id} lacks mapped held-back evidence"
                ));
            }
            if challenges.insert(id, prompt).is_some() {
                return Err(format!("duplicate challenge for area {id}"));
            }
        }
        if challenges.is_empty() {
            return Err("a separate coverage challenge is required".into());
        }
        Ok(Self {
            evidence,
            aims,
            obligations,
            challenges,
            source_count: cards.len(),
        })
    }

    fn verify_key(
        &self,
        id: u32,
        key: &str,
        owner: bool,
        held_available: bool,
    ) -> Result<(), String> {
        if owner {
            return if self.aims.get(key) == Some(&id) {
                Ok(())
            } else {
                Err(format!("unregistered scope addition for area {id}"))
            };
        }
        let binding = self
            .evidence
            .get(key)
            .ok_or_else(|| format!("unknown source label {key}"))?;
        if !binding.facets.contains(&id) || (binding.held && !held_available) {
            return Err(format!(
                "invalid source label {key} for area {id} at this stage"
            ));
        }
        Ok(())
    }

    fn apply(
        &self,
        rows: &[Value],
        held_available: bool,
        candidate: &mut BTreeMap<u32, String>,
        provisional: &mut BTreeSet<u32>,
    ) -> Result<(), String> {
        for row in rows {
            let id = facet_id(&row["facet"])?;
            let name = word(row, "name", 256)?;
            if candidate.insert(id, name.to_owned()).is_some() {
                return Err(format!("area {id} appears twice in proposed map"));
            }
            // A proposal may reference a pinned fact or a registered owner aim. It cannot supply a quote or URL.
            let fields = row.as_object().ok_or("expected a candidate area")?;
            if fields.len() != 3
                || (fields.contains_key("owner_aim") == fields.contains_key("evidence"))
            {
                return Err(format!("area {id} must name exactly one basis kind"));
            }
            if fields.contains_key("owner_aim") {
                let key = word(row, "owner_aim", 96)?;
                self.verify_key(id, key, true, held_available)?;
                provisional.insert(id);
            } else {
                let citations = list(row, "evidence", 12)?;
                if citations.is_empty() {
                    return Err(format!("area {id} lacks supporting labels"));
                }
                let mut used = BTreeSet::new();
                for citation in citations {
                    let key = citation.as_str().ok_or("expected an evidence key")?;
                    if !used.insert(key) {
                        return Err(format!("repeated source label {key} for area {id}"));
                    }
                    self.verify_key(id, key, false, held_available)?;
                }
            }
        }
        Ok(())
    }
}

/// Initial discovery sees the confirmed goal and visible labels. Editorial crosswalks, candidate
/// maps, held-out cards and challenge questions stay outside this packet.
pub fn initial_packet(value: &Value) -> Result<Value, String> {
    let material = &value["material"];
    Registry::read(material)?;
    let mut cards = Vec::new();
    for card in list(material, "sources", 24)? {
        if card["held"] == false {
            let labels: Vec<_> = list(card, "labels", 64)?
                .iter()
                .map(|l| json!({"key":l["key"],"text":l["text"]}))
                .collect();
            cards.push(json!({"publisher":card["publisher"],"url":card["url"],"labels":labels}));
        }
    }
    let scope = &value["scope"];
    let key = setting_key(scope)?;
    let aims: Vec<_> = list(material, "owner_aims", 64)?
        .iter()
        .map(|a| json!({"key":a["key"], "reason":a["reason"]}))
        .collect();
    Ok(json!({"goal": word(value,"goal",2048)?, "scope": {
        "purpose":word(scope,"purpose",2048)?, (key):word(scope,key,2048)?,
        "horizon":word(scope,"horizon",2048)?}, "source_cards":cards, "owner_aims":aims,
        "instructions":"Propose up to 64 areas for this learner. Give each a name and a visible source label key or owner aim key. Use null for an area that still needs a source or owner rationale. An editor will resolve names and assign IDs.",
        "reply_schema":{"type":"object", "required":["areas"], "additionalProperties":false,
            "properties":{"areas":{"type":"array","maxItems":64,"items":{
                "type":"object","required":["name","basis"],"additionalProperties":false,
                "properties":{"name":{"type":"string","minLength":1,"maxLength":256},
                              "basis":{"type":["string","null"],"minLength":1,"maxLength":96}}}}}}}))
}

#[derive(Debug)]
pub struct MappedReply {
    /// Accepted rows use the same format as the first_map array in discover().
    pub first_map: Vec<Value>,
    /// Preserve the raw suggestion for acquisition or editorial review.
    pub pending: Vec<Value>,
    /// Every conflicting editorial choice remains visible. The corresponding ID stays out of first_map.
    pub conflicts: Vec<Value>,
}

/// The callback represents an explicit editorial decision about the supplied name and basis.
/// Returning two assignments splits an integrated course. Returning none leaves the suggestion open.
pub fn map_first_reply(
    brief: &Value,
    reply: &Value,
    mut assign: impl FnMut(&str, &str) -> Vec<(u32, String)>,
) -> Result<MappedReply, String> {
    let registry = Registry::read(&brief["material"])?;
    if reply.as_object().map(|fields| fields.len()) != Some(1) {
        return Err("a discovery reply must contain only areas".into());
    }
    let mut mapped = MappedReply {
        first_map: Vec::new(),
        pending: Vec::new(),
        conflicts: Vec::new(),
    };
    let mut grouped = BTreeMap::<u32, BTreeMap<String, BTreeSet<&str>>>::new();
    for raw in list(reply, "areas", 64)? {
        let name = word(raw, "name", 256)?;
        let fields = raw.as_object().ok_or("expected a proposed area")?;
        if fields.len() != 2 || !fields.contains_key("basis") {
            return Err("a raw area must contain only name and basis".into());
        }
        if raw["basis"].is_null() {
            mapped.pending.push(raw.clone());
            continue;
        }
        let key = word(raw, "basis", 96)?;
        let owner_aim = registry.aims.contains_key(key);
        if !owner_aim && !registry.evidence.get(key).is_some_and(|label| !label.held) {
            return Err(format!("basis {key} was not in the first packet"));
        }
        let assignments = assign(name, key);
        if assignments.is_empty() {
            mapped.pending.push(raw.clone());
        }
        if assignments.len() > 64 {
            return Err("editorial assignments exceed the map limit".into());
        }
        for (id, name) in assignments {
            registry.verify_key(id, key, owner_aim, false)?;
            if name.trim().is_empty() || name.len() > 256 {
                return Err(format!("invalid editorial name for area {id}"));
            }
            grouped
                .entry(id)
                .or_default()
                .entry(name)
                .or_default()
                .insert(key);
        }
        if grouped.len() > 64 {
            return Err("editorial assignments exceed the map limit".into());
        }
    }
    for (id, choices) in grouped {
        let keys: BTreeSet<_> = choices.values().flatten().copied().collect();
        let conflicting_basis =
            keys.len() > 1 && keys.iter().any(|key| registry.aims.contains_key(key));
        if choices.len() > 1 || conflicting_basis {
            mapped
                .conflicts
                .push(json!({"facet":id,"conflicting_names":choices.len()>1,
                "conflicting_basis":conflicting_basis,"choices":choices}));
            continue;
        }
        for (name, keys) in choices {
            let row = match keys.iter().find(|key| registry.aims.contains_key(**key)) {
                Some(key) => json!({"facet":id,"name":name,"owner_aim":key}),
                None => json!({"facet":id,"name":name,"evidence":keys}),
            };
            mapped.first_map.push(row);
        }
    }
    // Reuse the audit used by replay. An assignment to an unsupported area cannot advance.
    registry.apply(
        &mapped.first_map,
        false,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )?;
    Ok(mapped)
}

#[derive(Debug)]
pub struct DiscoveryReport {
    pub source_count: usize,
    pub candidate: BTreeMap<u32, String>,
    pub initial_missing: Vec<u32>,
    pub initial_challenges: BTreeMap<u32, String>,
    pub repaired_missing: Vec<u32>,
    pub provisional: Vec<u32>,
}

impl DiscoveryReport {
    /// Eligibility to present this comparison to an editor. Curricular completeness is an open judgment.
    pub fn ready_for_editorial_review(&self) -> bool {
        self.repaired_missing.is_empty()
    }
}

/// Replay the two authored maps in a trusted, local scenario.
pub fn rehearse(value: &Value, frame: &Frame) -> Result<DiscoveryReport, String> {
    compare_maps(value, frame, value)
}

/// Bind the result of source discovery to the scope, IDs and names that unit planning will use.
pub fn compare_maps(brief: &Value, frame: &Frame, maps: &Value) -> Result<DiscoveryReport, String> {
    let s = &frame.scope;
    let scope = &brief["scope"];
    let key = setting_key(scope)?;
    if key != frame.setting_key() {
        return Err("discovery and planning scopes differ".into());
    }
    for (actual, expected) in [
        (&brief["goal"], &s.goal),
        (&scope["purpose"], &s.purpose),
        (&scope[key], &s.jurisdiction),
        (&scope["horizon"], &s.horizon),
    ] {
        if actual.as_str() != Some(expected.as_str()) {
            return Err("discovery and planning scopes differ".into());
        }
    }
    let report = discover(brief, maps)?;
    let names: BTreeMap<_, _> = frame
        .facets
        .iter()
        .map(|f| (f.id, f.name.as_str()))
        .collect();
    let frame_ids = names.keys().copied().collect();
    let obligations: BTreeSet<_> = report
        .candidate
        .keys()
        .chain(&report.repaired_missing)
        .copied()
        .collect();
    if obligations != frame_ids {
        return Err(format!(
            "source/owner obligations and planning frame disagree on areas {:?}",
            obligations
                .symmetric_difference(&frame_ids)
                .collect::<Vec<_>>()
        ));
    }
    for (id, name) in &report.candidate {
        if names.get(id) != Some(&name.as_str()) {
            return Err(format!("area {id} differs from planning frame: {name}"));
        }
    }
    Ok(report)
}

/// No pre-existing planning frame is needed. The editor registers the topic bindings in the
/// trusted brief. Only first_map and additions are read from the untrusted, editorially mapped drafts.
pub fn discover(brief: &Value, maps: &Value) -> Result<DiscoveryReport, String> {
    word(brief, "goal", 2048)?;
    for field in ["purpose", setting_key(&brief["scope"])?, "horizon"] {
        word(&brief["scope"], field, 2048)?;
    }
    let r = Registry::read(&brief["material"])?;
    let mut candidate = BTreeMap::new();
    let mut provisional = BTreeSet::new();
    r.apply(
        list(maps, "first_map", 64)?,
        false,
        &mut candidate,
        &mut provisional,
    )?;
    let omissions = |areas: &BTreeMap<u32, String>| {
        r.obligations
            .iter()
            .filter(|id| !areas.contains_key(id))
            .copied()
            .collect()
    };
    let initial_missing = omissions(&candidate);
    let initial_challenges = r
        .challenges
        .iter()
        .filter(|(id, _)| !candidate.contains_key(id))
        .map(|(&id, &question)| (id, question.to_owned()))
        .collect();
    r.apply(
        list(maps, "additions", 64)?,
        true,
        &mut candidate,
        &mut provisional,
    )?;
    Ok(DiscoveryReport {
        source_count: r.source_count,
        initial_missing,
        initial_challenges,
        repaired_missing: omissions(&candidate),
        candidate,
        provisional: provisional.into_iter().collect(),
    })
}
