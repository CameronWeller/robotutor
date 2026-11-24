//! `cargo run --release --bin lab -- sim [learners]` : long-horizon comparison vs baselines
//! `cargo run --release --bin lab -- live <goal> <persona> <sessions> <seed>` : real model calls
//! `cargo run --release --bin lab -- calibrate <episodes>` : live grader calibration summary

use robotutor_core::atlas::{fixtures, Regime};
use robotutor_core::policy::Params;
use robotutor_core::sim::{personas, run_core, run_fixed, run_reactive, Noise, Report};
use std::collections::BTreeMap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sim") => sim(args.get(1).and_then(|n| n.parse().ok()).unwrap_or(40), &Params::default(), &Noise::default()),
        Some("live") => live(&args[1..]),
        Some("diag") => {
            let g = &fixtures()[0];
            let p = &personas(g)[0];
            let r = run_core(g, p, &Noise::default(), &Params::default(), 16, 7);
            println!("{}", serde_json::to_string(&r).unwrap());
        }
        _ => eprintln!("usage: lab sim [n] | lab live <goal> <persona> <sessions> <seed>"),
    }
}

fn mean(v: &[f64]) -> f64 {
    let v: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
}

pub fn sim(n: u64, params: &Params, noise: &Noise) {
    let sessions = 16;
    for goal in fixtures().iter().filter(|g| g.regime != Regime::Formative) {
        println!("\n## {} ({} sessions, {n} learners per persona)\n", goal.name, sessions);
        println!("| policy | persona | start | d+28 unaided | gain/hr | minutes | items | reviews | chall | shown | false-shown | belief err | audit Brier | $/learner | LTL viol |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        let mut all: BTreeMap<String, Vec<Report>> = BTreeMap::new();
        for p in personas(goal) {
            let mut rows: BTreeMap<&str, Vec<Report>> = BTreeMap::new();
            for s in 0..n {
                let seed = 1000 * s + 7;
                let core = run_core(goal, &p, noise, params, sessions, seed);
                let per = (core.items / sessions).max(4);
                rows.entry("core").or_default().push(core);
                rows.entry("reactive-chat").or_default().push(run_reactive(goal, &p, noise, sessions, per, seed));
                rows.entry("fixed+review").or_default().push(run_fixed(goal, &p, noise, sessions, per, seed));
            }
            for (pol, rs) in rows {
                row(pol, p.name, &rs);
                all.entry(pol.to_string()).or_default().extend(rs);
            }
        }
        for (pol, rs) in &all {
            row(pol, "ALL", rs);
        }
        let viol: Vec<&String> = all.values().flatten().flat_map(|r| r.violations.iter()).collect();
        if !viol.is_empty() {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for v in &viol {
                *counts.entry(v.split(" (").next().unwrap_or(v)).or_default() += 1;
            }
            println!("\nLTL violations: {counts:?}");
        }
    }
}

fn row(pol: &str, persona: &str, rs: &[Report]) {
    let f = |g: fn(&Report) -> f64| mean(&rs.iter().map(g).collect::<Vec<_>>());
    let gain_hr = mean(&rs.iter().map(|r| (r.outcome_d28 - r.start) / (r.minutes / 60.0).max(0.1)).collect::<Vec<_>>());
    println!(
        "| {pol} | {persona} | {:.2} | {:.3} | {:.3} | {:.0} | {:.0} | {:.1} | {:.1} | {:.2} | {:.2} | {:.3} | {:.3} | {:.4} | {} |",
        f(|r| r.start),
        f(|r| r.outcome_d28),
        gain_hr,
        f(|r| r.minutes),
        f(|r| r.items as f64),
        f(|r| r.reviews as f64),
        f(|r| r.challenges as f64),
        f(|r| r.shown as f64),
        f(|r| r.false_shown as f64),
        f(|r| r.belief_error),
        f(|r| r.audit_brier),
        f(|r| r.cost_usd),
        rs.iter().map(|r| r.violations.len()).sum::<usize>(),
    );
}

fn live(a: &[String]) {
    let goal_name = a.first().map(String::as_str).unwrap_or("chemical-engineering");
    let persona_name = a.get(1).map(String::as_str).unwrap_or("novice-steady");
    let sessions: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let seed: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let goals = fixtures();
    let goal = goals.iter().find(|g| g.name == goal_name).expect("goal");
    let persona = personas(goal).into_iter().find(|p| p.name == persona_name).expect("persona");
    let ep = robotutor_core::live::run_episode(goal, &persona, &Params::default(), sessions, seed);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runs");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("{goal_name}-{persona_name}-s{sessions}-seed{seed}.md"));
    let mut doc = ep.transcript.clone();
    doc.push_str(&format!("\n## Metrics\n\n```json\n{}\n```\n\nLTL violations: {:?}\n", serde_json::to_string_pretty(&ep.metrics).unwrap(), ep.violations));
    std::fs::write(&path, doc).unwrap();
    println!("{}", serde_json::to_string(&ep.metrics).unwrap());
    println!("violations: {:?}", ep.violations);
    println!("spent ${:.4} (this process), transcript {}", robotutor_core::llm::spent_usd(), path.display());
}
