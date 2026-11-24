//! LTLf: linear temporal logic over *finite* traces (De Giacomo & Vardi, 2013).
//!
//! Sessions and learner histories are finite, so we use the finite-trace semantics:
//! `X φ` (strong next) is false at the last position, and `WX φ` (weak next) is true there.
//! Evaluation runs backwards over the trace once per subformula, giving O(|φ|·n) time.
//!
//! Exhaustive state-space checking of the orchestrator uses `stateright` (see tests/model.rs).
//! This module checks the properties against concrete traces produced by simulations and live runs.

use std::rc::Rc;

pub enum Ltl<S> {
    True,
    Atom(Rc<dyn Fn(&S) -> bool>),
    Not(Box<Ltl<S>>),
    And(Box<Ltl<S>>, Box<Ltl<S>>),
    Or(Box<Ltl<S>>, Box<Ltl<S>>),
    Next(Box<Ltl<S>>),
    WeakNext(Box<Ltl<S>>),
    Until(Box<Ltl<S>>, Box<Ltl<S>>),
    Eventually(Box<Ltl<S>>),
    Always(Box<Ltl<S>>),
}

use Ltl::*;

pub fn atom<S>(f: impl Fn(&S) -> bool + 'static) -> Ltl<S> {
    Atom(Rc::new(f))
}
pub fn not<S>(a: Ltl<S>) -> Ltl<S> {
    Not(Box::new(a))
}
pub fn and<S>(a: Ltl<S>, b: Ltl<S>) -> Ltl<S> {
    And(Box::new(a), Box::new(b))
}
pub fn or<S>(a: Ltl<S>, b: Ltl<S>) -> Ltl<S> {
    Or(Box::new(a), Box::new(b))
}
pub fn implies<S>(a: Ltl<S>, b: Ltl<S>) -> Ltl<S> {
    or(not(a), b)
}
pub fn x<S>(a: Ltl<S>) -> Ltl<S> {
    Next(Box::new(a))
}
pub fn wx<S>(a: Ltl<S>) -> Ltl<S> {
    WeakNext(Box::new(a))
}
pub fn until<S>(a: Ltl<S>, b: Ltl<S>) -> Ltl<S> {
    Until(Box::new(a), Box::new(b))
}
/// Weak until: `a W b = (a U b) ∨ G a`.
pub fn weak_until<S>(a: Ltl<S>, b: Ltl<S>) -> Ltl<S>
where
    S: 'static,
{
    let a2 = a.share();
    or(until(a, b), g(a2))
}
pub fn f<S>(a: Ltl<S>) -> Ltl<S> {
    Eventually(Box::new(a))
}
pub fn g<S>(a: Ltl<S>) -> Ltl<S> {
    Always(Box::new(a))
}
/// Bounded eventually: `F≤k a = a ∨ X(a ∨ X(...))`, k steps ahead.
pub fn within<S>(k: usize, a: Ltl<S>) -> Ltl<S>
where
    S: 'static,
{
    let mut acc = a.share();
    for _ in 0..k {
        acc = or(a.share(), x(acc));
    }
    acc
}

impl<S> Ltl<S> {
    /// Structural copy. Atoms are shared through `Rc`, so this is cheap.
    pub fn share(&self) -> Ltl<S> {
        match self {
            True => True,
            Atom(p) => Atom(p.clone()),
            Not(a) => not(a.share()),
            And(a, b) => and(a.share(), b.share()),
            Or(a, b) => or(a.share(), b.share()),
            Next(a) => x(a.share()),
            WeakNext(a) => wx(a.share()),
            Until(a, b) => until(a.share(), b.share()),
            Eventually(a) => f(a.share()),
            Always(a) => g(a.share()),
        }
    }

    /// Truth value at every position of the trace.
    pub fn eval(&self, t: &[S]) -> Vec<bool> {
        let n = t.len();
        match self {
            True => vec![true; n],
            Atom(p) => t.iter().map(|s| p(s)).collect(),
            Not(a) => a.eval(t).into_iter().map(|v| !v).collect(),
            And(a, b) => zip(a.eval(t), b.eval(t), |x, y| x && y),
            Or(a, b) => zip(a.eval(t), b.eval(t), |x, y| x || y),
            Next(a) => shift(a.eval(t), false),
            WeakNext(a) => shift(a.eval(t), true),
            Until(a, b) => {
                let (va, vb) = (a.eval(t), b.eval(t));
                let mut out = vec![false; n];
                for i in (0..n).rev() {
                    let later = i + 1 < n && out[i + 1];
                    out[i] = vb[i] || (va[i] && later);
                }
                out
            }
            Eventually(a) => {
                let va = a.eval(t);
                let mut out = vec![false; n];
                for i in (0..n).rev() {
                    out[i] = va[i] || (i + 1 < n && out[i + 1]);
                }
                out
            }
            Always(a) => {
                let va = a.eval(t);
                let mut out = vec![true; n];
                for i in (0..n).rev() {
                    out[i] = va[i] && (i + 1 >= n || out[i + 1]);
                }
                out
            }
        }
    }

    /// `None` when the formula holds at position 0. Otherwise the most useful position to
    /// inspect: for `G φ`, the first position where φ fails.
    pub fn violation(&self, t: &[S]) -> Option<usize> {
        if t.is_empty() {
            return None; // vacuous: an empty trace makes no promises
        }
        match self {
            Always(a) => a.eval(t).iter().position(|v| !v),
            _ => (!self.eval(t)[0]).then_some(0),
        }
    }
}

fn zip(a: Vec<bool>, b: Vec<bool>, op: impl Fn(bool, bool) -> bool) -> Vec<bool> {
    a.into_iter().zip(b).map(|(x, y)| op(x, y)).collect()
}
fn shift(v: Vec<bool>, at_end: bool) -> Vec<bool> {
    let n = v.len();
    (0..n).map(|i| if i + 1 < n { v[i + 1] } else { at_end }).collect()
}

/// A named property, so reports can say which promise broke.
pub struct Property<S> {
    pub name: &'static str,
    pub formula: Ltl<S>,
}

pub fn check_all<S>(props: &[Property<S>], t: &[S]) -> Vec<(&'static str, usize)> {
    props
        .iter()
        .filter_map(|p| p.formula.violation(t).map(|i| (p.name, i)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(s: &str) -> Vec<char> {
        s.chars().collect()
    }
    fn is(c: char) -> Ltl<char> {
        atom(move |s: &char| *s == c)
    }

    #[test]
    fn finite_trace_semantics() {
        let t = tr("aab");
        assert!(until(is('a'), is('b')).eval(&t)[0]);
        assert!(!until(is('a'), is('c')).eval(&t)[0]);
        assert!(!x(is('a')).eval(&t)[2], "strong next is false at the end");
        assert!(wx(is('a')).eval(&t)[2], "weak next is true at the end");
        assert!(f(is('b')).eval(&t)[0]);
        assert!(!g(is('a')).eval(&t)[0]);
        assert_eq!(g(is('a')).violation(&t), Some(2));
        // response: every a is eventually followed by b
        let resp = g(implies(is('a'), f(is('b'))));
        assert_eq!(resp.violation(&tr("abab")), None);
        assert_eq!(resp.violation(&tr("abaa")), Some(2));
        // bounded response
        assert!(within(1, is('b')).eval(&t)[1]);
        assert!(!within(1, is('b')).eval(&t)[0]);
        // weak until holds when b never comes but a always holds
        assert!(weak_until(is('a'), is('b')).eval(&tr("aaa"))[0]);
        assert!(!until(is('a'), is('b')).eval(&tr("aaa"))[0]);
    }
}
