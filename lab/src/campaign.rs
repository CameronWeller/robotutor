//! Deterministic, budgeted challenger search on synthetic teaching-service workloads.
//! Retain the incumbent between design passes. Reserve final workloads until selection ends.

mod fixture;
pub mod forward;

use fixture::{simulate, Audit, Workload, DESIGN, REGRESSION, RESERVED};
use std::collections::BTreeSet;
use std::fmt::{self, Write};

const CACHE_SIZES: [u8; 5] = [0, 1, 3, 6, 12];
const AXES: [&str; 4] = [
    "review cadence",
    "attempt cap",
    "cache capacity",
    "follow-up allowance",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Policy {
    pub period: u8,
    pub attempts: u8,
    pub cache: u8,
    pub extra: u8,
}

impl Policy {
    const BASELINE: Self = Self {
        period: 4,
        attempts: 1,
        cache: 0,
        extra: 0,
    };

    fn values(self) -> [u8; 4] {
        [self.period, self.attempts, self.cache, self.extra]
    }

    fn tie_order(self) -> (u8, u8, u8, u8) {
        (self.extra, self.attempts, self.cache, self.period)
    }

    fn candidates() -> BTreeSet<Self> {
        let mut set = BTreeSet::new();
        for period in 1..=4 {
            for attempts in 1..=4 {
                for cache in CACHE_SIZES {
                    for extra in 0..=2 {
                        set.insert(Self {
                            period,
                            attempts,
                            cache,
                            extra,
                        });
                    }
                }
            }
        }
        set
    }

    fn change(self, candidate: Self) -> String {
        let differences: Vec<_> = self
            .values()
            .into_iter()
            .zip(candidate.values())
            .enumerate()
            .filter(|(_, (before, after))| before != after)
            .map(|(axis, (before, after))| format!("{} {before}->{after}", AXES[axis]))
            .collect();
        differences.join(" + ")
    }
}

impl fmt::Display for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}/{}",
            self.period, self.attempts, self.cache, self.extra
        )
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct Metrics {
    pub visits: u32,
    pub first: u32,
    pub goals: u32,
    pub zero: u32,
    pub reviews: u32,
    pub late: u32,
    pub unserved: u32,
    pub calls: u32,
    pub hits: u32,
    pub extra: u32,
    pub audits: u32,
    pub retracted: u32,
    pub rebuilt: u32,
}

impl Metrics {
    fn utility(self) -> i64 {
        // Illustrative and inspectable service tradeoffs, frozen before workload evaluation.
        20 * i64::from(self.first) + 30 * i64::from(self.goals) + 4 * i64::from(self.reviews)
            - 15 * i64::from(self.zero)
            - 4 * i64::from(self.unserved)
            - i64::from(self.late)
            - i64::from(self.calls)
            - i64::from(self.extra)
    }
}

struct Evaluation {
    solo: Metrics,
    class: Metrics,
    score: i64,
}

fn evaluate(
    workloads: &[Workload; 2],
    policy: Policy,
    audit: &mut Audit,
) -> Result<Evaluation, String> {
    let solo = simulate(&workloads[0], policy, audit)?;
    let class = simulate(&workloads[1], policy, audit)?;
    let score = solo.utility() * 1000 / i64::from(workloads[0].seats)
        + class.utility() * 1000 / i64::from(workloads[1].seats);
    Ok(Evaluation { solo, class, score })
}

// Allocate more investigations to dimensions whose observed service pressure remains high.
// Dividing by prior investigations also returns search effort to neglected dimensions.
fn focus(evaluation: &Evaluation, tested: [u32; 4]) -> usize {
    let mut pressure = [0; 4];
    for metrics in [evaluation.solo, evaluation.class] {
        for (axis, count) in [
            metrics.late,
            metrics.unserved,
            metrics.calls,
            metrics
                .visits
                .saturating_sub(metrics.first + metrics.reviews),
        ]
        .into_iter()
        .enumerate()
        {
            pressure[axis] += 100 * count / metrics.visits;
        }
    }
    (0..4)
        .max_by_key(|&axis| {
            (
                (1 + pressure[axis]) * 1000 / (1 + tested[axis]),
                std::cmp::Reverse(axis),
            )
        })
        .unwrap()
}

fn challenger(remaining: &BTreeSet<Policy>, incumbent: Policy, focus: usize) -> Policy {
    let origin = incumbent.values();
    // Search the most local change first. Prefer the current pressure within an equally local shell.
    remaining
        .iter()
        .copied()
        .min_by_key(|candidate| {
            let values = candidate.values();
            let count = (0..4).filter(|&i| values[i] != origin[i]).count();
            let rank = |value| CACHE_SIZES.iter().position(|&x| x == value).unwrap();
            let distance = usize::from(
                origin[0].abs_diff(values[0])
                    + origin[1].abs_diff(values[1])
                    + origin[3].abs_diff(values[3]),
            ) + rank(origin[2]).abs_diff(rank(values[2]));
            (
                count,
                values[focus] == origin[focus],
                distance,
                candidate.tie_order(),
            )
        })
        .expect("nonempty search space")
}

fn row_metrics(out: &mut String, metrics: Metrics) {
    for value in [
        metrics.visits,
        metrics.first,
        metrics.goals,
        metrics.zero,
        metrics.reviews,
        metrics.late,
        metrics.unserved,
        metrics.calls,
        metrics.hits,
        metrics.extra,
        metrics.audits,
        metrics.retracted,
        metrics.rebuilt,
    ] {
        write!(out, "\t{value}").unwrap();
    }
}

fn evaluation_row(output: &mut String, role: &str, policy: Policy, evaluation: &Evaluation) {
    write!(output, "{role}\t{policy}\t{}", evaluation.score).unwrap();
    row_metrics(output, evaluation.solo);
    row_metrics(output, evaluation.class);
    output.push('\n');
}

fn metrics_header() -> String {
    let mut names = String::new();
    for cohort in ["solo", "class"] {
        for key in [
            "visits",
            "first",
            "goal_seats",
            "zero_seats",
            "returns",
            "due_visits",
            "unserved",
            "calls",
            "hits",
            "extra",
            "ltl_lanes",
            "retracted",
            "rebuilt",
        ] {
            write!(names, "\t{cohort}_{key}").unwrap();
        }
    }
    names
}

pub struct Report {
    pub ledger: String,
    pub summary: String,
    selected: Policy,
    pub accepted: usize,
}

impl Report {
    /// Inspect the selected policy on training fixtures with isolated changes to expiry and budget.
    pub fn diagnostics(&self) -> Result<String, String> {
        fixture::diagnose(self.selected)
    }
}

/// Each pass tests a previously unseen candidate against the policy retained by earlier passes.
/// The TSV is stable, credential-free evidence. Re-running with a different budget is supported.
pub fn run(passes: usize) -> Result<Report, String> {
    let mut candidates = Policy::candidates();
    let mut incumbent = Policy::BASELINE;
    candidates.remove(&incumbent);
    if passes == 0 || passes > candidates.len() {
        return Err(format!("choose 1..={} design passes", candidates.len()));
    }
    let mut audit = Audit::default();
    let mut best = evaluate(&DESIGN, incumbent, &mut audit)?;
    let mut summary = format!("role\tpolicy\tscore{}\n", metrics_header());
    evaluation_row(&mut summary, "design-baseline", incumbent, &best);
    let mut ledger = format!("pass\tparent_pass\tincumbent\tplanning_pressure\tdesign_change\tcandidate\tscore\tdelta\tdecision\tnext_pressure\tobserved_change{}\n",metrics_header());
    let mut tested = [0; 4];
    let mut axis = focus(&best, tested);
    let mut parent_pass = 0;
    let mut accepted = 0;
    for pass in 1..=passes {
        let candidate = challenger(&candidates, incumbent, axis);
        candidates.remove(&candidate);
        for (i, value) in candidate.values().into_iter().enumerate() {
            if incumbent.values()[i] != value {
                tested[i] += 1;
            }
        }
        let result = evaluate(&DESIGN, candidate, &mut audit)?;
        let delta = result.score - best.score;
        let keep_tie = delta == 0 && candidate.tie_order() < incumbent.tie_order();
        let keep = delta > 0 || keep_tie;
        let decision = match (delta.cmp(&0), keep_tie) {
            (std::cmp::Ordering::Greater, _) => "keep_higher_utility",
            (std::cmp::Ordering::Less, _) => "reject_lost_utility",
            (_, true) => "keep_preferred_tie",
            (_, false) => "reject_tie",
        };
        let next_axis = focus(&result, tested);
        write!(
            &mut ledger,
            "{pass}\t{parent_pass}\t{incumbent}\t{}\t{}\t{candidate}\t{}\t{delta}\t{decision}\t{}",
            AXES[axis],
            incumbent.change(candidate),
            result.score,
            AXES[next_axis]
        )
        .unwrap();
        for (label, after, before) in [
            ("solo", result.solo, best.solo),
            ("class", result.class, best.class),
        ] {
            let delta = |a, b| i64::from(a) - i64::from(b);
            write!(
                &mut ledger,
                "{}{label} unserved {:+}, due {:+}, first {:+}",
                if label == "solo" { "\t" } else { " / " },
                delta(after.unserved, before.unserved),
                delta(after.late, before.late),
                delta(after.first, before.first)
            )
            .unwrap();
        }
        row_metrics(&mut ledger, result.solo);
        row_metrics(&mut ledger, result.class);
        ledger.push('\n');
        if keep {
            best = result;
            incumbent = candidate;
            parent_pass = pass;
            accepted += 1;
        }
        axis = next_axis;
    }
    evaluation_row(&mut summary, "design-selected", incumbent, &best);
    // Freeze selected policy before the previously inspected regression and fresh reserved pair.
    for (suite, workloads) in [("regression", &REGRESSION), ("reserved", &RESERVED)] {
        for (role, policy) in [("baseline", Policy::BASELINE), ("selected", incumbent)] {
            let result = evaluate(workloads, policy, &mut audit)?;
            evaluation_row(&mut summary, &format!("{suite}-{role}"), policy, &result);
        }
    }
    writeln!(
        &mut summary,
        "# passes={passes} accepted={accepted} unique_ltl_traces={} design={}/{} reserved={}/{}",
        audit.checked_traces(),
        DESIGN[0].name,
        DESIGN[1].name,
        RESERVED[0].name,
        RESERVED[1].name
    )
    .unwrap();
    Ok(Report {
        ledger,
        summary,
        selected: incumbent,
        accepted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_candidate_is_new_and_parent_is_the_last_retained_design() {
        let result = run(12).unwrap();
        let mut seen = BTreeSet::new();
        let mut parent = 0;
        for line in result.ledger.lines().skip(1) {
            let fields: Vec<_> = line.split('\t').collect();
            let pass: usize = fields[0].parse().unwrap();
            assert_eq!(fields[1], parent.to_string());
            assert!(seen.insert(fields[5]));
            if fields[8].starts_with("keep_") {
                parent = pass;
            }
        }
        assert_eq!(seen.len(), 12);
        assert!(run(240).is_err());
    }
}
