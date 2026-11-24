//! A finite, reproducible supply of different numerical instances of the
//! existing sixteenths template. This family cannot establish cross-task transfer.
use super::{form, Form, OutcomeId, Source, A, B};

const PARTS: u16 = 16;

// Prefer different missing amounts before returning to the same answer. Within
// each sweep the other operand changes. Existing reviewed forms keep their IDs.
pub(super) fn extend(catalog: &mut Vec<Form>, target: OutcomeId, source: &Source, edition: u32) {
    for sweep in 1..PARTS {
        for key in 1..=PARTS {
            let (first, second) = if target == B {
                (sweep, key)
            } else if target == A && key > sweep {
                (sweep, key - sweep)
            } else {
                continue;
            };
            if first + second > PARTS || (target == A && first > second) {
                continue;
            }
            // Two day-zero encounters and the two later fixed items already
            // have identities. Reordering a sum cannot turn it into new work.
            let reserved = [(1, 2), (2, 2), (3, 2), (3, 5)];
            if reserved.iter().any(|&(a, b)| {
                (first, second) == (a, b) || (target == A && (first, second) == (b, a))
            }) {
                continue;
            }
            let parameters =
                (u64::from(target.0) << 16) | (u64::from(first) << 8) | u64::from(second);
            let task = (1 << 63) | parameters;
            let revision = (1 << 63) | (u64::from(edition) << 24) | parameters;
            let mut next = form(target, source.pin, revision, task, first, second);
            next.family = (1 << 63) | u64::from(target.0);
            catalog.push(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        protocol::Assistance,
        rehearsal::{Action, Session},
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn numerator(prompt: &str, prefix: &str) -> u32 {
        prompt
            .split_once(prefix)
            .unwrap()
            .1
            .split_once('/')
            .unwrap()
            .0
            .parse()
            .unwrap()
    }

    #[test]
    fn five_years_of_matched_returns_and_the_actual_exhaustion_point() {
        for (varied, expected_through_year_five, expected_total) in [(false, 2, 2), (true, 86, 118)]
        {
            let mut session = Session::new().unwrap();
            if varied {
                session.apply(Action::UseVariedInstances).unwrap();
            }
            let mut questions = BTreeSet::new();
            let mut keys = BTreeMap::new();
            let mut first_hold = None;
            for day in (30..=2508).step_by(21) {
                if day == 51 {
                    session.apply(Action::ReplaceSource).unwrap();
                }
                let (_, shown) = session.apply(Action::Open(day)).unwrap();
                if shown["opened"] == true {
                    let prompt = shown["display"].as_str().unwrap();
                    assert!(questions.insert(prompt.to_owned()));
                    let answer = numerator(prompt, "length ") - numerator(prompt, "cut off ");
                    *keys.entry(answer).or_insert(0) += 1;
                    let (_, checked) = session
                        .apply(Action::Answer(
                            format!("{answer}/16"),
                            Assistance::NoneReported,
                        ))
                        .unwrap();
                    assert_eq!(checked["credit"], "independent_return");
                } else {
                    assert_eq!(shown["authority"], "NoFreshForm");
                    first_hold.get_or_insert(day);
                }
                if day == 1815 {
                    assert_eq!(questions.len(), expected_through_year_five);
                    if varied {
                        assert_eq!(keys.values().max(), Some(&8));
                    }
                }
            }
            assert_eq!(questions.len(), expected_total);
            assert_eq!(first_hold, Some(if varied { 2508 } else { 72 }));
        }
    }

    #[test]
    fn source_replacement_does_not_create_new_semantic_tasks() {
        for target in [A, B] {
            let mut old = Vec::new();
            let mut new = Vec::new();
            extend(&mut old, target, &Source::new(1), 1);
            extend(&mut new, target, &Source::new(2), 2);
            assert!(!old.is_empty());
            assert!(old.iter().zip(&new).all(|(a, b)| a.task == b.task
                && a.family == b.family
                && a.revision != b.revision
                && a.source != b.source));
            assert_eq!(
                old.iter().map(|i| i.task).collect::<BTreeSet<_>>().len(),
                old.len()
            );
        }
    }

    #[test]
    fn extra_instances_do_not_supply_permission_or_missing_exposure_history() {
        for (action, authority) in [
            (Action::RevokeGrant, "grant_denied"),
            (Action::WithdrawSource, "NoSupportedForm"),
            (Action::UnknownContact, "NoFreshForm"),
        ] {
            let mut session = Session::new().unwrap();
            session.apply(action).unwrap();
            session.apply(Action::UseVariedInstances).unwrap();
            let (_, receipt) = session.apply(Action::Open(30)).unwrap();
            assert_eq!(receipt["opened"], false);
            assert_eq!(receipt["authority"], authority);
        }
    }
}
