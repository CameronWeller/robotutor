//! Live episodes: the real orchestrator and policy with real model calls for the renderer,
//! the two graders and a simulated learner whose hidden state we control.
//!
//! The learner model is told, per item, whether to answer correctly and which misconception
//! (if any) drives its error. That gives every answer a known label, so grader accuracy,
//! agreement, quote fidelity and misconception detection can be *measured*, then fed back
//! into `sim::Noise` for the long-horizon runs.

use crate::atlas::{Checker, Family, Goal};
use crate::ledger::{Agreement, Assist};
use crate::llm::{chat, json_of, route, Call, Reply};
use crate::policy::{Kind, Params, Rng};
use crate::session::{Input, Item, Output};
use crate::sim::{Hidden, Persona};
use crate::spec::{self, Recorder};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

// ---------- deterministic helpers ----------

/// Fill numeric placeholders in an Exact family's stem with seeded values.
pub fn instantiate(f: &Family, rng: &mut Rng) -> (String, BTreeMap<String, f64>) {
    let mut vars = BTreeMap::new();
    let mut s = f.stem.clone();
    if f.checker != Checker::Exact {
        return (s, vars);
    }
    while let (Some(a), Some(b)) = (s.find('{'), s.find('}')) {
        if b < a {
            break;
        }
        let name = s[a + 1..b].to_string();
        let v = match name.as_str() {
            "p" => (5 + rng.below(20)) as f64,
            "r" => (2 + rng.below(10)) as f64,
            _ => (10 * (2 + rng.below(30))) as f64,
        };
        vars.insert(name, v);
        s.replace_range(a..=b, &format!("{v}"));
    }
    (s, vars)
}

/// Evaluate an arithmetic key such as `f*(0.40-0.05)/(0.90-0.05)`. `None` if the key is prose.
pub fn eval_key(key: &str, vars: &BTreeMap<String, f64>) -> Option<f64> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
        v: &'a BTreeMap<String, f64>,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i] == b' ' {
                self.i += 1;
            }
        }
        fn expr(&mut self) -> Option<f64> {
            let mut a = self.term()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'+') => { self.i += 1; a += self.term()?; }
                    Some(b'-') => { self.i += 1; a -= self.term()?; }
                    _ => return Some(a),
                }
            }
        }
        fn term(&mut self) -> Option<f64> {
            let mut a = self.atom()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'*') => { self.i += 1; a *= self.atom()?; }
                    Some(b'/') => { self.i += 1; a /= self.atom()?; }
                    _ => return Some(a),
                }
            }
        }
        fn atom(&mut self) -> Option<f64> {
            self.ws();
            let c = *self.s.get(self.i)?;
            if c == b'(' {
                self.i += 1;
                let v = self.expr()?;
                self.ws();
                (self.s.get(self.i) == Some(&b')')).then(|| { self.i += 1; v })
            } else if c.is_ascii_digit() || c == b'.' {
                let st = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[st..self.i]).ok()?.parse().ok()
            } else if c.is_ascii_alphabetic() {
                let st = self.i;
                while self.i < self.s.len() && self.s[self.i].is_ascii_alphanumeric() {
                    self.i += 1;
                }
                self.v.get(std::str::from_utf8(&self.s[st..self.i]).ok()?).copied()
            } else {
                None
            }
        }
    }
    let mut p = P { s: key.as_bytes(), i: 0, v: vars };
    let v = p.expr()?;
    p.ws();
    (p.i == key.len()).then_some(v)
}

/// Pull the number a learner gave, if any, and compare within 2%.
pub fn numeric_match(answer: &str, want: f64) -> Option<bool> {
    let nums: Vec<f64> = answer
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .filter_map(|t| t.trim_matches('.').parse::<f64>().ok())
        .collect();
    if nums.is_empty() {
        return None;
    }
    Some(nums.iter().any(|&n| (n - want).abs() <= 0.02 * want.abs().max(1e-9)))
}

const PRAISE: [&str; 10] = [
    "correct!", "that's right", "exactly", "well done", "great job", "perfect", "spot on", "nailed it",
    "you got it", "excellent",
];

/// L9 lint: praise words in feedback for an answer graded wrong.
pub fn false_praise(verdict: Option<bool>, utterance: &str) -> bool {
    let u = utterance.to_lowercase();
    verdict == Some(false) && PRAISE.iter().any(|p| u.contains(p))
}

/// L8 lint: the rendered question gives away the key (numeric value, or a long key phrase).
pub fn leaks(key_value: Option<f64>, key_text: &str, utterance: &str) -> bool {
    if let Some(v) = key_value {
        if numeric_match(utterance, v) == Some(true) && v.abs() > 1.0 {
            return true;
        }
    }
    let u = utterance.to_lowercase();
    let words: Vec<&str> = key_text.split_whitespace().collect();
    words.len() >= 5 && words.windows(5).any(|w| u.contains(&w.join(" ").to_lowercase()))
}

// ---------- model roles ----------

const RENDER_SYS: &str = "You are the voice of a patient, exacting tutor. A separate system has \
already decided WHAT to do this turn; you only decide HOW to say it. Rules: (1) Do exactly the \
move you are given; never switch topics or add a second question. (2) When the move asks a \
question, end with exactly that one question and never reveal or hint at its answer. (3) When \
giving feedback, the verdict you are given is final: do not call a wrong answer right or a \
right answer wrong. Name specifically what in the learner's words was right or wrong. (4) Teach \
only what the node and its listed sources cover; if unsure, say so plainly. (5) Be warm but not \
flattering. No more than 120 words. Plain text, no markdown headings.";

fn render(packet: &Value, nonce: u64) -> Result<Reply, String> {
    chat(&Call {
        model: route::RENDER,
        system: RENDER_SYS,
        user: &packet.to_string(),
        max_tokens: 400,
        json: false,
        temperature: 0.4,
        nonce,
    })
}

const GRADE_SYS: &str = "You grade one learner answer against a key or rubric. You see only the \
question, the key and the answer. Return JSON only: {\"verdict\": \"correct\"|\"incorrect\"|\"unsure\", \
\"quote\": \"<a short span copied verbatim from the answer that your verdict rests on>\", \
\"misconception\": <index from the list, or null>, \"rationale\": \"<one sentence>\"}. Be charitable \
about wording and strict about reasoning. Use \"unsure\" when the answer is ambiguous.";

#[derive(Clone, Debug, Serialize)]
pub struct GradeOut {
    pub model: String,
    pub verdict: Option<bool>,
    pub quote_ok: bool,
    pub misconception: Option<usize>,
    pub rationale: String,
    pub cost: f64,
    pub ms: u64,
}

fn grade_one(model: &str, stem: &str, key: &str, misc: &[String], answer: &str, nonce: u64) -> Result<GradeOut, String> {
    let user = json!({"question": stem, "key": key, "misconceptions": misc, "answer": answer}).to_string();
    let r = chat(&Call { model, system: GRADE_SYS, user: &user, max_tokens: 300, json: true, temperature: 0.0, nonce })?;
    let v = json_of(&r.text).ok_or_else(|| format!("grader {model} returned no JSON"))?;
    let verdict = match v["verdict"].as_str() {
        Some("correct") => Some(true),
        Some("incorrect") => Some(false),
        _ => None,
    };
    let quote = v["quote"].as_str().unwrap_or("");
    // Deterministic anti-fabrication check: the evidence must be in the learner's words.
    let quote_ok = !quote.is_empty() && answer.to_lowercase().contains(&quote.to_lowercase().trim_matches('"').to_string());
    Ok(GradeOut {
        model: r.model,
        verdict,
        quote_ok,
        misconception: v["misconception"].as_u64().map(|m| m as usize).filter(|m| *m < misc.len()),
        rationale: v["rationale"].as_str().unwrap_or("").to_string(),
        cost: r.cost_usd,
        ms: r.ms,
    })
}

/// Two blind graders from different vendors, in parallel.
pub fn grade_pair(stem: &str, key: &str, misc: &[String], answer: &str, nonce: u64) -> (Result<GradeOut, String>, Result<GradeOut, String>) {
    std::thread::scope(|s| {
        let a = s.spawn(|| grade_one(route::GRADE_A, stem, key, misc, answer, nonce));
        let b = s.spawn(|| grade_one(route::GRADE_B, stem, key, misc, answer, nonce));
        (a.join().unwrap(), b.join().unwrap())
    })
}

/// Combine the pair. A fabricated quote demotes that grader to "unsure".
pub fn combine(a: &Result<GradeOut, String>, b: &Result<GradeOut, String>) -> (Option<bool>, Agreement, Vec<usize>) {
    let v = |g: &Result<GradeOut, String>| g.as_ref().ok().and_then(|g| if g.quote_ok { g.verdict } else { None });
    let hyps: Vec<usize> = [a, b].iter().filter_map(|g| g.as_ref().ok().and_then(|g| g.misconception)).collect();
    let mut hyps = hyps;
    hyps.sort();
    hyps.dedup();
    match (v(a), v(b)) {
        (Some(x), Some(y)) if x == y => (Some(x), Agreement::Agree, hyps),
        (Some(x), Some(_)) => (Some(x), Agreement::Disagree, hyps),
        (Some(x), None) | (None, Some(x)) => (Some(x), Agreement::Single, hyps),
        (None, None) => (None, Agreement::Single, hyps),
    }
}

fn describe(goal: &Goal, h: &Hidden) -> String {
    let mut s = String::new();
    for n in &goal.nodes {
        let k = h.skill[&n.id];
        let level = if k > 0.7 { "solid" } else if k > 0.35 { "shaky" } else { "new to you" };
        s.push_str(&format!("- {}: {}\n", n.name, level));
    }
    s
}

fn learner_answer(goal: &Goal, persona: &Persona, h: &Hidden, question: &str, intend: bool, misc: Option<&str>, nonce: u64) -> Result<Reply, String> {
    let sys = format!(
        "You are role-playing a real adult learner ({}) studying '{}'. Your knowledge:\n{}\
         Write the learner's reply to the tutor in 1-4 sentences, in a natural voice (no meta \
         commentary, never mention role-play). {}",
        persona.name,
        goal.statement,
        describe(goal, h),
        match (intend, misc) {
            (true, _) => "This time you answer CORRECTLY, showing the reasoning a person at your level would show.".to_string(),
            (false, Some(m)) => format!("This time you answer INCORRECTLY because you genuinely believe this: '{m}'. Let that belief shape the answer."),
            (false, None) => "This time you answer INCORRECTLY in the way a person at your level plausibly would (a slip, a gap, or a half-remembered rule).".to_string(),
        }
    );
    chat(&Call { model: route::LEARNER, system: &sys, user: question, max_tokens: 250, json: false, temperature: 0.8, nonce })
}

// ---------- the episode ----------

#[derive(Clone, Debug, Default, Serialize)]
pub struct LiveMetrics {
    pub items: u32,
    pub rubric_items: u32,
    pub grader_correct: [u32; 2],
    pub grader_scored: [u32; 2],
    pub agree: u32,
    pub agree_correct: u32,
    pub disagree: u32,
    pub quote_fail: [u32; 2],
    pub exact_parsed: u32,
    pub exact_correct: u32,
    /// Wrong answers driven by a known misconception, and how often a grader named it.
    pub mis_driven: u32,
    pub mis_named: u32,
    pub mis_false: u32,
    pub leaks: u32,
    pub false_praise: u32,
    pub words_per_turn: f64,
    pub cost_usd: f64,
    pub render_ms: Vec<u64>,
    pub grade_ms: Vec<u64>,
}

pub struct Episode {
    pub transcript: String,
    pub metrics: LiveMetrics,
    pub violations: Vec<String>,
}

pub fn run_episode(goal: &Goal, persona: &Persona, params: &Params, sessions: u32, seed: u64) -> Episode {
    let mut rng = Rng(seed ^ 0x11);
    let mut h = Hidden::new(goal, persona);
    let mut rec = Recorder::new(seed);
    let mut m = LiveMetrics::default();
    let mut tx = format!("# Live episode: {} / {} (seed {seed})\n\n", goal.name, persona.name);
    let mut words = 0usize;
    let mut turns = 0usize;
    let mut nonce = seed * 1000;
    let mut t_day = 0.0;
    let say = |tx: &mut String, who: &str, text: &str| tx.push_str(&format!("**{who}:** {}\n\n", text.trim()));

    for s in 0..sessions {
        tx.push_str(&format!("## Session {} (day {:.1})\n\n", s + 1, t_day));
        rec.step(goal, params, Input::Open);
        let mut fatigue = 0u32;
        while rec.tutor.open {
            let outs = rec.step(goal, params, Input::Next);
            let why = rec.tutor.last_why;
            let mut item: Option<Item> = None;
            for o in &outs {
                match o {
                    Output::Teach { node, repair, why } => {
                        let n = goal.node(*node);
                        let packet = json!({"move": "teach", "why": why, "goal": goal.statement, "node": n.name,
                            "sources": n.sources, "misconception_to_repair": repair.map(|r| n.misconceptions[r].clone()),
                            "avoid": n.misconceptions});
                        nonce += 1;
                        match render(&packet, nonce) {
                            Ok(r) => {
                                m.cost_usd += r.cost_usd;
                                m.render_ms.push(r.ms);
                                words += r.text.split_whitespace().count();
                                turns += 1;
                                say(&mut tx, &format!("Tutor (teach, {why})"), &r.text);
                            }
                            Err(e) => say(&mut tx, "error", &e),
                        }
                        if let Some(mi) = repair {
                            if h.mis.contains(&(*node, *mi)) && rng.unit() < 0.6 {
                                h.mis.remove(&(*node, *mi));
                            }
                        }
                        h.teach(goal, *node, persona.learn_rate, t_day);
                    }
                    Output::Present { item: i, .. } => item = Some(i.clone()),
                    Output::Closed(w) => tx.push_str(&format!("*(session closed: {w})*\n\n")),
                    _ => {}
                }
            }
            let Some(item) = item else {
                if rec.tutor.open {
                    rec.step(goal, params, Input::Wait(3));
                }
                continue;
            };
            let fam = goal.families.iter().find(|f| f.id == item.family).unwrap().clone();
            let node = goal.node(item.node).clone();
            let (stem, vars) = instantiate(&fam, &mut rng);
            let key_value = eval_key(&fam.key, &vars);
            let packet = json!({"move": format!("{:?}", item.kind).to_lowercase(), "why": why, "goal": goal.statement,
                "node": node.name, "question_to_ask": stem, "cue_level": format!("{:?}", fam.cue),
                "note": "Fill any {placeholder} in braces with concrete, realistic content. Do not answer it."});
            nonce += 1;
            let q = match render(&packet, nonce) {
                Ok(r) => r,
                Err(e) => {
                    say(&mut tx, "error", &e);
                    break;
                }
            };
            m.cost_usd += q.cost_usd;
            m.render_ms.push(q.ms);
            words += q.text.split_whitespace().count();
            turns += 1;
            let leak = leaks(key_value, &fam.key, &q.text);
            m.leaks += leak as u32;
            say(&mut tx, &format!("Tutor ({:?}, {why})", item.kind), &q.text);
            if let Some(st) = rec.trace.last_mut() {
                st.leak = leak;
            }
            m.items += 1;

            // The learner: intended correctness sampled from the hidden state.
            let p_true = h.p_correct(goal, &fam, t_day, fatigue as f64 / 1000.0);
            let intend = rng.unit() < p_true;
            let held = fam.probes_misconception.filter(|mi| h.mis.contains(&(item.node, *mi)))
                .or_else(|| h.mis.iter().find(|(n, _)| *n == item.node).map(|(_, mi)| *mi));
            let misc_text = if intend { None } else { held.map(|mi| node.misconceptions[mi].clone()) };
            nonce += 1;
            let ans = match learner_answer(goal, persona, &h, &q.text, intend, misc_text.as_deref(), nonce) {
                Ok(r) => r,
                Err(e) => {
                    say(&mut tx, "error", &e);
                    break;
                }
            };
            m.cost_usd += ans.cost_usd;
            say(&mut tx, "Learner", &ans.text);
            rec.step(goal, params, Input::Answer { assist: Assist::None });

            let mut verdict = None;
            if fam.checker != Checker::SelfReport {
                let numeric = if fam.checker == Checker::Exact { key_value.and_then(|v| numeric_match(&ans.text, v)) } else { None };
                let (correct, agreement, hyps) = if let Some(ok) = numeric {
                    m.exact_parsed += 1;
                    m.exact_correct += (ok == intend) as u32;
                    (Some(ok), Agreement::Checker, vec![])
                } else {
                    m.rubric_items += 1;
                    let key = match key_value { Some(v) => format!("{} = {v:.4}", fam.key), None => fam.key.clone() };
                    nonce += 1;
                    let (a, b) = grade_pair(&q.text, &key, &node.misconceptions, &ans.text, nonce);
                    for (i, g) in [&a, &b].iter().enumerate() {
                        if let Ok(g) = g {
                            m.cost_usd += g.cost;
                            m.grade_ms.push(g.ms);
                            if !g.quote_ok {
                                m.quote_fail[i] += 1;
                            }
                            if let Some(v) = g.verdict {
                                m.grader_scored[i] += 1;
                                m.grader_correct[i] += (v == intend) as u32;
                            }
                        }
                    }
                    let (c, ag, hy) = combine(&a, &b);
                    match ag {
                        Agreement::Agree => {
                            m.agree += 1;
                            m.agree_correct += (c == Some(intend)) as u32;
                        }
                        Agreement::Disagree => m.disagree += 1,
                        _ => {}
                    }
                    if !intend {
                        if let Some(mi) = misc_text.as_ref().and(held) {
                            m.mis_driven += 1;
                            m.mis_named += hy.contains(&mi) as u32;
                        } else if !hy.is_empty() {
                            m.mis_false += 1;
                        }
                    }
                    tx.push_str(&format!(
                        "> graders: A {:?} / B {:?} → {:?} {:?}; hypotheses {:?}; truth {}\n\n",
                        a.as_ref().map(|g| (g.verdict, g.quote_ok)).ok(),
                        b.as_ref().map(|g| (g.verdict, g.quote_ok)).ok(),
                        c, ag, hy, intend
                    ));
                    (c, ag, hy)
                };
                verdict = correct;
                rec.step(goal, params, Input::Graded { correct, agreement, hyps });
            }
            // Feedback, anchored to the verdict.
            let packet = json!({"move": "feedback", "verdict": match verdict { Some(true) => "correct", Some(false) => "incorrect", None => "could not be judged reliably" },
                "question": q.text, "learner_answer": ans.text, "key": fam.key, "node": node.name,
                "self_report": fam.checker == Checker::SelfReport});
            nonce += 1;
            if let Ok(fb) = render(&packet, nonce) {
                m.cost_usd += fb.cost_usd;
                m.render_ms.push(fb.ms);
                words += fb.text.split_whitespace().count();
                turns += 1;
                let fp = false_praise(verdict, &fb.text);
                m.false_praise += fp as u32;
                if let Some(st) = rec.trace.last_mut() {
                    st.false_praise = fp;
                }
                say(&mut tx, "Tutor (feedback)", &fb.text);
            }
            // Hidden learning, same assumptions as the long-horizon sim.
            let r = crate::ledger::recall(h.stability[&item.node], t_day - h.last[&item.node]);
            if intend {
                h.retrieved(item.node, r, true, false);
                h.learn(goal, item.node, crate::sim::LEARN_OK, persona.learn_rate, t_day);
            } else {
                h.retrieved(item.node, r, false, misc_text.is_some() || p_true > 0.5);
                let amt = if item.kind == Kind::Challenge { 0.15 * h.shallow[&item.node] + crate::sim::LEARN_MISS } else { crate::sim::LEARN_MISS };
                h.learn(goal, item.node, amt, persona.learn_rate, t_day);
            }
            fatigue += if intend { 60 } else { 100 };
            rec.step(goal, params, Input::Wait(2));
            if fatigue >= persona.fatigue_quit {
                rec.step(goal, params, Input::Close);
                tx.push_str("*(learner stopped: tired)*\n\n");
            }
        }
        let gap = persona.gap_days * (0.5 + rng.unit());
        t_day += gap;
        rec.step(goal, params, Input::Wait((gap * 1440.0) as u64));
    }
    m.words_per_turn = words as f64 / turns.max(1) as f64;
    let trace = rec.finish().to_vec();
    let mut violations = spec::check(&trace);
    violations.extend(spec::window_violations(&trace, params.audit_every, true));
    Episode { transcript: tx, metrics: m, violations }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_evaluate_and_numbers_match() {
        let v: BTreeMap<String, f64> = [("f".to_string(), 100.0)].into_iter().collect();
        let k = eval_key("f*(0.40-0.05)/(0.90-0.05)", &v).unwrap();
        assert!((k - 41.176).abs() < 0.01);
        assert_eq!(eval_key("compare a/28 with b", &v), None);
        assert_eq!(numeric_match("about 41.2 kg/h", k), Some(true));
        assert_eq!(numeric_match("I got 35 kg/h", k), Some(false));
        assert!(false_praise(Some(false), "Great job! But actually..."));
        assert!(!false_praise(Some(true), "Great job!"));
    }
}
