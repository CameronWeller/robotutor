//! Opt-in, offline replay of discovery, use-specific support and one learner's return.
use std::collections::{BTreeMap, BTreeSet};
use tutor_lab::curriculum::{
    crawl,
    provenance::{
        Catalog, Claim, Decision as SupportDecision, Lineage, Purpose, Release, Standing, Support,
        Use,
    },
    CrawlBudget, Decision, Discoveries, Document, Outcome, OutcomeId, Progress, ReviewedUnit, Slot,
    Span,
};
use tutor_lab::history::{reconcile, Attempt, CheckpointProposal, Scope, Withdrawn};
use tutor_lab::material::{document_id, FractionCard, FractionTask, RuleSource, UNIT_RULE};
use tutor_lab::protocol::{Assistance, Judgment, Question};

const SUM: OutcomeId = OutcomeId(1);
const REMAINDER: OutcomeId = OutcomeId(2);
const CLASS: u64 = 7;
const HOME: u64 = 8;

struct Source {
    bytes: Vec<u8>,
    pin: Span,
}

impl Source {
    fn new(title: &str, passage: &str) -> Self {
        let header = format!("{title}\n");
        let bytes = format!("{header}{passage}").into_bytes();
        let pin = Span {
            document: document_id(&bytes),
            start: header.len() as u32,
            end: bytes.len() as u32,
        };
        Self { bytes, pin }
    }

    fn view<'a>(&'a self, revoked: &'a BTreeSet<Document>) -> RuleSource<'a> {
        RuleSource {
            bytes: Some(&self.bytes),
            pin: self.pin,
            revoked,
        }
    }
}

struct Index {
    seed: Document,
    handbook: Source,
    guide: Source,
    replacement: Source,
}

impl Index {
    fn fixture() -> Self {
        Self {
            seed: document_id(b"District reading list"),
            handbook: Source::new("Class workbook, edition 1", UNIT_RULE),
            guide: Source::new("District mathematics guide", "With a common unit fraction, the sum counts all unit fractions without changing their size."),
            replacement: Source::new("Class workbook, edition 2", UNIT_RULE),
        }
    }

    fn discover(&self, opt_in: bool, repair: bool, max_edges: usize) -> Discoveries {
        if !opt_in {
            return Discoveries::default();
        }
        let workbook = if repair {
            &self.replacement
        } else {
            &self.handbook
        };
        let adjacent = BTreeSet::from([workbook.pin.document, self.guide.pin.document]);
        let allowed = adjacent.iter().copied().chain([self.seed]).collect();
        crawl(
            self.seed,
            &BTreeMap::from([(self.seed, adjacent)]),
            &allowed,
            CrawlBudget {
                max_nodes: 3,
                max_depth: 1,
                max_edges,
            },
        )
    }

    fn release_support(
        &self,
        found: &Discoveries,
        workbook: &Source,
        day: u32,
        audience: u64,
        purpose: Purpose,
        mirrored: bool,
    ) -> SupportDecision {
        let claim = Claim {
            course: 1,
            outcome: SUM,
            criterion: 1,
            edition: self.seed,
            convention: 1,
        };
        let mut spans = BTreeMap::new();
        let mut candidates = BTreeSet::new();
        for (source, origin) in [(workbook, 1), (&self.guide, if mirrored { 1 } else { 2 })] {
            if !found.documents.contains(&source.pin.document) {
                continue;
            }
            let mut grants = BTreeMap::from([
                ((CLASS, Purpose::Instruction), (0, 30)),
                ((CLASS, Purpose::Assessment), (0, 30)),
            ]);
            if source.pin != self.replacement.pin {
                grants.insert((HOME, Purpose::Instruction), (0, 30));
                grants.insert((HOME, Purpose::Assessment), (0, 30));
            }
            let standing = if source.pin == self.handbook.pin {
                Standing::WithdrawnFrom(8)
            } else {
                Standing::Current
            };
            spans.insert(
                source.pin,
                Support {
                    claim,
                    grants,
                    lineage: Lineage::Original(origin),
                    standing,
                },
            );
            candidates.insert(source.pin);
        }
        if !candidates.contains(&workbook.pin) {
            return SupportDecision::Insufficient;
        }
        let revision = if workbook.pin == self.replacement.pin {
            2
        } else {
            1
        };
        let release = Release {
            revision,
            claim,
            candidates,
            required: [workbook.pin].into(),
            minimum_lineages: 2,
        };
        let request = Use {
            revision: release.revision,
            claim,
            day,
            audience,
            purpose,
        };
        Catalog {
            generation: u64::from(day),
            spans,
        }
        .inspect(&release, request, 32)
        .decision
    }
}

fn inverse_review() -> Source {
    Source::new(
        "Reviewed ribbon unit",
        "Find the missing part by checking which two lengths add to the given total.",
    )
}

fn unit(revision: u64, pin: Span, reviewed_prerequisite: bool) -> ReviewedUnit {
    let prerequisites = reviewed_prerequisite.then_some(SUM).into_iter().collect();
    let inverse = inverse_review();
    let dependency = Source::new(
        "Teacher scope decision",
        "In this version, practice adding equal parts before solving for a missing ribbon length.",
    );
    let mut goal_support = BTreeSet::from([inverse.pin, pin]);
    if reviewed_prerequisite {
        goal_support.insert(dependency.pin);
    }
    ReviewedUnit::build(
        revision,
        vec![
            Outcome {
                id: SUM,
                requires: BTreeSet::new(),
                support: [pin].into(),
            },
            Outcome {
                id: REMAINDER,
                requires: prerequisites,
                support: goal_support,
            },
        ],
    )
    .unwrap()
}

fn upcoming(
    unit: &ReviewedUnit,
    progress: &Progress,
    day: u32,
    revoked: &BTreeSet<Document>,
) -> Decision {
    unit.next(REMAINDER, progress, day, revoked, Slot::Forward)
        .unwrap()
}

fn approved(verdict: SupportDecision) -> bool {
    matches!(verdict, SupportDecision::Approved(_))
}

fn demonstrate() {
    let index = Index::fixture();
    let original = index.discover(true, false, 2);
    let active = BTreeSet::new();
    let scope = Scope {
        learner: 42,
        revision: 1,
    };
    let no_withdrawals = Withdrawn::new(scope);
    let fresh = reconcile(scope, 1, &[], &[], &no_withdrawals).unwrap();
    let direct = unit(1, index.handbook.pin, false);
    let scaffolded = unit(1, index.handbook.pin, true);
    let direct_target = upcoming(&direct, &fresh, 1, &active);
    let scaffold_target = upcoming(&scaffolded, &fresh, 1, &active);
    assert_eq!(
        (direct_target, scaffold_target),
        (Decision::Teach(REMAINDER), Decision::Teach(SUM))
    );
    println!("Request: Teach me to find how much ribbon remains. Direct release: {direct_target:?}. With reviewed prerequisite: {scaffold_target:?}.");

    let use_old = |day, purpose| {
        index.release_support(&original, &index.handbook, day, CLASS, purpose, false)
    };
    assert!(
        approved(use_old(1, Purpose::Instruction)) && approved(use_old(1, Purpose::Assessment))
    );
    let card = FractionCard::propose(FractionTask::Sum, 3, 2, 8).unwrap();
    let rule = index.handbook.view(&active);
    let (question, _) = card.question(&rule).unwrap();
    let wrong = card.assess("5/16", &rule).unwrap();
    let (worked, _) = card.worked(&rule).unwrap();
    let corrected = card.assess("5/8", &rule).unwrap();
    assert_eq!((wrong, corrected), (Judgment::Incorrect, Judgment::Correct));
    println!("Day 1: {question} Scripted answer 5/16: {wrong:?}. Learner requested explanation: {worked} Revised answer 5/8: {corrected:?} after help.");
    let observed = Attempt {
        scope,
        id: 1,
        sequence: 1,
        outcome: SUM,
        item: 10,
        day: 1,
        question: Question::Practice,
        judgment: wrong,
        assistance: Assistance::NoneReported,
        fresh: true,
        sources: [index.handbook.pin.document, index.guide.pin.document].into(),
    };
    let revision = Attempt {
        id: 2,
        sequence: 2,
        judgment: corrected,
        assistance: Assistance::Reported,
        fresh: false,
        ..observed.clone()
    };
    let history = [observed, revision];
    let proposed = [CheckpointProposal {
        scope,
        outcome: SUM,
        first: 1,
        confirmation: 2,
        due: 22,
    }];
    let return_read = reconcile(scope, 7, &history, &proposed, &no_withdrawals).unwrap();
    assert!(return_read.checkpoints.is_empty());
    assert_eq!(
        upcoming(&scaffolded, &return_read, 7, &active),
        Decision::Teach(SUM)
    );
    let next_card = FractionCard::propose(FractionTask::Sum, 2, 4, 9).unwrap();
    assert!(approved(use_old(7, Purpose::Instruction)));
    println!(
        "Day 7: no checkpoint; the teacher offers a different example: {}",
        next_card.question(&rule).unwrap().0
    );

    let revoked = BTreeSet::from([index.handbook.pin.document]);
    let withdrawn = Withdrawn {
        sources: revoked.clone(),
        ..no_withdrawals
    };
    let after = reconcile(scope, 8, &history, &proposed, &withdrawn).unwrap();
    assert_eq!(
        upcoming(&scaffolded, &after, 8, &revoked),
        Decision::RepairSources(SUM)
    );
    assert!(!approved(use_old(8, Purpose::Instruction)));
    assert!(next_card
        .assess("6/9", &index.handbook.view(&revoked))
        .is_none());

    let repair = index.discover(true, true, 2);
    let seats = [CLASS, CLASS, CLASS, HOME];
    let permissions = seats.map(|audience| {
        approved(index.release_support(
            &repair,
            &index.replacement,
            8,
            audience,
            Purpose::Instruction,
            false,
        ))
    });
    assert_eq!(permissions, [true, true, true, false]);
    // The source changed. Rebuild the reviewed unit at a distinct revision.
    // With no approved credit crosswalk, the new release starts without credit.
    let replacement_scope = Scope {
        revision: 2,
        ..scope
    };
    let replacement_progress = reconcile(
        replacement_scope,
        8,
        &[],
        &[],
        &Withdrawn::new(replacement_scope),
    )
    .unwrap();
    let repaired = unit(2, index.replacement.pin, true);
    assert_eq!(
        upcoming(&repaired, &replacement_progress, 8, &revoked),
        Decision::Teach(SUM)
    );
    println!("Day 8: old key withheld even for 6/9. One repaired crawl inspected {} edges. Class, class, class, home instruction releases: {permissions:?}.", repair.edges_inspected);
    if permissions[0] {
        let replacement_card = FractionCard::propose(FractionTask::Sum, 1, 3, 7).unwrap();
        println!(
            "After review, first class seat sees: {}",
            replacement_card
                .question(&index.replacement.view(&revoked))
                .unwrap()
                .0
        );
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--replay") => demonstrate(),
        Some("--compare-routes") => compare_routes(),
        _ => println!(
            "Pass --replay for source repair or --compare-routes for the ribbon-goal comparison."
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LearnerChoice {
    Solve,
    DenominatorError,
    OneRibbonExample,
    Decline,
}

struct RouteTrace {
    questions: Vec<(u32, OutcomeId, String)>,
    explanations: Vec<(u32, OutcomeId, String)>,
    attempts: Vec<Attempt>,
    checkpoints_before_withdrawal: Vec<OutcomeId>,
    repaired_target: Option<OutcomeId>,
}

impl RouteTrace {
    fn new() -> Self {
        Self {
            questions: Vec::new(),
            explanations: Vec::new(),
            attempts: Vec::new(),
            checkpoints_before_withdrawal: Vec::new(),
            repaired_target: None,
        }
    }

    fn progress(&self, scope: Scope, day: u32, withdrawals: &Withdrawn) -> Progress {
        let scoped: Vec<_> = self
            .attempts
            .iter()
            .filter(|a| a.scope == scope)
            .cloned()
            .collect();
        let proposals: Vec<_> = scoped
            .iter()
            .filter(|a| a.day == 1)
            .flat_map(|first| {
                scoped
                    .iter()
                    .filter(move |later| later.day == 7 && later.outcome == first.outcome)
                    .map(move |later| CheckpointProposal {
                        scope,
                        outcome: first.outcome,
                        first: first.id,
                        confirmation: later.id,
                        due: 22,
                    })
            })
            .collect();
        reconcile(scope, day, &scoped, &proposals, withdrawals).unwrap()
    }

    fn question_count_before_ribbon(&self) -> usize {
        self.questions
            .iter()
            .take_while(|(_, target, _)| *target != REMAINDER)
            .count()
    }
}

/// One checked display or assessment using a pinned source on the specified day.
/// The modeled responses deliberately keep the same arithmetic strategy in both arms.
fn encounter(
    trace: &mut RouteTrace,
    index: &Index,
    scope: Scope,
    day: u32,
    target: OutcomeId,
    choice: LearnerChoice,
) {
    if choice == LearnerChoice::Decline {
        return;
    }
    let repair = scope.revision == 2;
    let workbook = if repair {
        &index.replacement
    } else {
        &index.handbook
    };
    let found = index.discover(true, repair, 2);
    let revoked = if repair {
        [index.handbook.pin.document].into()
    } else {
        BTreeSet::new()
    };
    let rule = workbook.view(&revoked);
    let allowed =
        |purpose| approved(index.release_support(&found, workbook, day, CLASS, purpose, false));
    assert!(allowed(Purpose::Instruction));
    let (removed, left, parts) = match day {
        1 => (3, 2, 8),
        7 => (2, 3, 9),
        8 => (1, 3, 7),
        _ => unreachable!(),
    };
    let task = if target == REMAINDER {
        FractionTask::MissingPart
    } else {
        assert_eq!(target, SUM);
        FractionTask::Sum
    };
    let card = FractionCard::propose(task, removed, left, parts).unwrap();
    if choice == LearnerChoice::OneRibbonExample {
        assert_eq!(target, REMAINDER);
        trace
            .explanations
            .push((day, target, card.worked(&rule).unwrap().0));
        return;
    }
    assert!(allowed(Purpose::Assessment));
    let prompt = card.question(&rule).unwrap().0;
    trace.questions.push((day, target, prompt));
    let numerator = if target == SUM { removed + left } else { left };
    let correct_answer = format!("{numerator}/{parts}");
    let answer = if choice == LearnerChoice::DenominatorError {
        format!("{numerator}/{}", 2 * parts)
    } else {
        correct_answer.clone()
    };
    let item = 100 * u64::from(day) + u64::from(target.0);
    let mut sources = BTreeSet::from([workbook.pin.document, index.guide.pin.document]);
    if target == REMAINDER {
        sources.insert(inverse_review().pin.document);
    }
    let id = trace.attempts.len() as u64 + 1;
    let attempt = Attempt {
        scope,
        id,
        sequence: id,
        outcome: target,
        item,
        day,
        question: if trace
            .attempts
            .iter()
            .any(|a| a.scope == scope && a.outcome == target && a.day < day)
        {
            Question::DelayedReview
        } else {
            Question::Practice
        },
        judgment: card.assess(&answer, &rule).unwrap(),
        assistance: Assistance::NoneReported,
        fresh: true,
        sources,
    };
    trace.attempts.push(attempt.clone());
    if attempt.judgment == Judgment::Incorrect {
        // In the error scenario the participant requests a worked explanation.
        trace
            .explanations
            .push((day, target, card.worked(&rule).unwrap().0));
        trace.attempts.push(Attempt {
            id: id + 1,
            sequence: id + 1,
            judgment: card.assess(&correct_answer, &rule).unwrap(),
            assistance: Assistance::Reported,
            fresh: false,
            ..attempt
        });
    }
}

fn run_route(index: &Index, prerequisite_first: bool, choice: LearnerChoice) -> RouteTrace {
    let scope = Scope {
        learner: 42,
        revision: 1,
    };
    let withdrawn = Withdrawn::new(scope);
    let released = unit(1, index.handbook.pin, prerequisite_first);
    let mut trace = RouteTrace::new();
    for day in [1, 7] {
        let progress = trace.progress(scope, day, &withdrawn);
        let Decision::Teach(planned) = upcoming(&released, &progress, day, &withdrawn.sources)
        else {
            unreachable!()
        };
        // On day seven every participant explicitly returns to the original ribbon goal.
        let action = if day == 1 {
            choice
        } else {
            LearnerChoice::Solve
        };
        let target = if action == LearnerChoice::OneRibbonExample {
            REMAINDER
        } else {
            planned
        };
        encounter(&mut trace, index, scope, day, target, action);
        let progress = trace.progress(scope, day, &withdrawn);
        if planned == SUM
            && action == LearnerChoice::Solve
            && upcoming(&released, &progress, day, &withdrawn.sources) == Decision::Teach(REMAINDER)
        {
            encounter(
                &mut trace,
                index,
                scope,
                day,
                REMAINDER,
                LearnerChoice::Solve,
            );
        }
    }
    trace.checkpoints_before_withdrawal = trace
        .progress(scope, 7, &withdrawn)
        .checkpoints
        .keys()
        .copied()
        .collect();
    let revoked = Withdrawn {
        sources: [index.handbook.pin.document].into(),
        ..withdrawn
    };
    let invalidated = trace.progress(scope, 8, &revoked);
    assert!(invalidated.checkpoints.is_empty());
    assert!(matches!(
        upcoming(&released, &invalidated, 8, &revoked.sources),
        Decision::RepairSources(_)
    ));
    assert!(!approved(index.release_support(
        &index.discover(true, false, 2),
        &index.handbook,
        8,
        CLASS,
        Purpose::Assessment,
        false
    )));
    let new_scope = Scope {
        revision: 2,
        ..scope
    };
    let revised = unit(2, index.replacement.pin, prerequisite_first);
    let empty = reconcile(new_scope, 8, &[], &[], &Withdrawn::new(new_scope)).unwrap();
    let Decision::Teach(next) = upcoming(&revised, &empty, 8, &revoked.sources) else {
        unreachable!()
    };
    trace.repaired_target = Some(next);
    encounter(&mut trace, index, new_scope, 8, next, LearnerChoice::Solve);
    trace
}

fn compare_routes() {
    let index = Index::fixture();
    for choice in [
        LearnerChoice::Solve,
        LearnerChoice::DenominatorError,
        LearnerChoice::OneRibbonExample,
        LearnerChoice::Decline,
    ] {
        for prerequisite_first in [false, true] {
            let trace = run_route(&index, prerequisite_first, choice);
            println!("prerequisite first={prerequisite_first}, initial choice={choice:?}");
            for (day, target, prompt) in &trace.questions {
                println!("  day {day}, assess {:?}: {prompt}", target.0);
            }
            for (day, target, text) in &trace.explanations {
                println!("  day {day}, explain {:?}: {text}", target.0);
            }
            let first_ribbon = trace
                .questions
                .iter()
                .find(|(_, target, _)| *target == REMAINDER)
                .map(|r| r.0);
            println!("  first ribbon question day {first_ribbon:?}; preceding arithmetic questions {}; day-7 checkpoints {:?}; after withdrawal {:?}", trace.question_count_before_ribbon(),trace.checkpoints_before_withdrawal,trace.repaired_target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_correct_checks_open_different_work_and_both_checkpoints_are_removed_on_withdrawal() {
        let index = Index::fixture();
        for (pre, expected) in [(false, REMAINDER), (true, SUM)] {
            let trace = run_route(&index, pre, LearnerChoice::Solve);
            assert_eq!(trace.checkpoints_before_withdrawal, [expected]);
            assert_eq!(trace.repaired_target, Some(expected));
            let first_ribbon = trace
                .questions
                .iter()
                .find(|(_, target, _)| *target == REMAINDER)
                .unwrap();
            assert_eq!(first_ribbon.0, if pre { 7 } else { 1 });
            assert_eq!(
                trace.question_count_before_ribbon(),
                if pre { 2 } else { 0 }
            );
            assert!(trace
                .attempts
                .iter()
                .all(|a| a.judgment == Judgment::Correct));
        }
    }

    #[test]
    fn requested_help_and_decline_do_not_turn_into_a_first_day_assessment() {
        let index = Index::fixture();
        for pre in [true, false] {
            for choice in [LearnerChoice::OneRibbonExample, LearnerChoice::Decline] {
                let trace = run_route(&index, pre, choice);
                assert!(trace.questions.iter().all(|r| r.0 > 1));
                assert!(trace.attempts.iter().all(|r| r.day > 1));
                assert!(trace.checkpoints_before_withdrawal.is_empty());
                if choice == LearnerChoice::OneRibbonExample {
                    assert_eq!(trace.explanations.len(), 1);
                    assert!(trace.explanations[0].2.contains("removing 3 leaves 2"));
                } else {
                    assert!(trace.explanations.is_empty());
                }
            }
            let trace = run_route(&index, pre, LearnerChoice::DenominatorError);
            let first = &trace.attempts[0];
            let correction = &trace.attempts[1];
            assert_eq!(first.judgment, Judgment::Incorrect);
            assert_eq!(
                (correction.judgment, correction.assistance, correction.fresh),
                (Judgment::Correct, Assistance::Reported, false)
            );
            assert_eq!(trace.explanations.len(), 1);
            let words = if pre {
                "Their sum is 5/8"
            } else {
                "removing 3 leaves 2"
            };
            assert!(trace.explanations[0].2.contains(words));
            assert!(trace.checkpoints_before_withdrawal.is_empty());
            assert_eq!(
                trace
                    .questions
                    .iter()
                    .any(|(_, target, _)| *target == REMAINDER),
                !pre
            );
        }
    }
    #[test]
    fn replay_closes_the_old_key_and_rechecks_audience_grants() {
        demonstrate();
    }

    #[test]
    fn both_acquisition_and_independent_authority_matter() {
        let index = Index::fixture();
        let outcome = |opt_in, edges, mirrored| {
            let found = index.discover(opt_in, false, edges);
            (
                found.edges_inspected,
                approved(index.release_support(
                    &found,
                    &index.handbook,
                    1,
                    CLASS,
                    Purpose::Instruction,
                    mirrored,
                )),
            )
        };
        assert_eq!(outcome(false, 2, false), (0, false));
        assert_eq!(outcome(true, 1, false), (1, false));
        assert_eq!(outcome(true, 2, true), (2, false));
        assert_eq!(outcome(true, 2, false), (2, true));
    }
}
