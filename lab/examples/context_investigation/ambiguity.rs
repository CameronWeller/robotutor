//! Bound what any deterministic route could distinguish on these authored cases.
//! Each key is the complete transcript of outgoing provider requests.
use super::{Focus, Grant, Recorder, Selection, Turn, Workshop, LINE};
use std::collections::BTreeMap;

#[derive(Default)]
struct Signals {
    traces: BTreeMap<String, BTreeMap<&'static str, usize>>,
    bytes: usize,
    calls: usize,
}

impl Signals {
    fn observe(&mut self, requests: Recorder, intended: &'static str) {
        let key = serde_json::to_string(&requests.requests).expect("serialized requests");
        *self
            .traces
            .entry(key)
            .or_default()
            .entry(intended)
            .or_default() += 1;
        self.bytes += requests
            .requests
            .iter()
            .map(|r| r.to_string().len())
            .sum::<usize>();
        self.calls += requests.requests.len();
    }

    fn indistinguishable_errors(&self) -> usize {
        self.traces
            .values()
            .map(|labels| {
                labels.values().sum::<usize>() - labels.values().max().copied().unwrap_or(0)
            })
            .sum()
    }
}

// Simulate an explicit selection and, where opted in, a later learner response.
fn messages(draft: &str, start: usize, reply: Option<&str>) -> Result<Recorder, String> {
    let work = Workshop::new(draft, Focus::Clarity)?;
    let grant = Grant {
        artifact: (2, 14),
        epoch: 1,
        revision: work.revision(),
        focus: work.focus(),
        selected: start..draft.len(),
    };
    let mut selection = Selection::new(grant.artifact, &work, grant.clone())?;
    let mut seen = Recorder::default();
    if let Some(answer) = reply {
        let Turn::Ask(_, question) = selection.coach(
            || Some(grant.clone()),
            None,
            |role, r| seen.answer(role, r, "clarify_0"),
        )?
        else {
            return Err("clarification did not open".into());
        };
        selection.coach(
            || Some(grant.clone()),
            Some((&question, answer)),
            |role, r| seen.answer(role, r, "coach_0"),
        )?;
    } else {
        selection.coach(
            || Some(grant.clone()),
            None,
            |role, r| seen.answer(role, r, "coach_0"),
        )?;
    }
    Ok(seen)
}

pub(super) fn compare() -> Result<(), String> {
    let cases = [
        (
            "Élodie will host the meeting.",
            "Élodie.",
            "Someone on the team.",
        ),
        (
            "Élodie will host the meeting.",
            "The two ushers.",
            "Someone on the team.",
        ),
        (
            "The two ushers will host the meeting.",
            "The two ushers.",
            "The two ushers.",
        ),
        (
            "The two ushers will host the meeting.",
            "Élodie.",
            "Élodie.",
        ),
        ("Élodie has the only key.", "Élodie.", "Élodie."),
        (
            "The two ushers have the only key.",
            "The two ushers.",
            "The two ushers.",
        ),
    ];
    let mut methods: [Signals; 4] = Default::default();
    for (lead, intended, response) in cases {
        let draft = format!("{lead}\n{LINE}");
        let start = lead.len() + 1;
        for (signals, (start, reply)) in methods.iter_mut().zip([
            (start, None),
            (0, None),
            (start, Some(intended)),
            (start, Some(response)),
        ]) {
            signals.observe(messages(&draft, start, reply)?, intended);
        }
    }
    for (name, method) in [
        "line alone",
        "lead and line",
        "direct clarification",
        "two vague clarifications",
    ]
    .iter()
    .zip(&methods)
    {
        println!("{name}: {} distinct request histories, at least {} of 6 wrong forced identities; {} calls, {} bytes total",
            method.traces.len(),method.indistinguishable_errors(),method.calls,method.bytes);
    }
    assert_eq!(methods.map(|s| s.indistinguishable_errors()), [3, 2, 0, 1]);
    Ok(())
}
