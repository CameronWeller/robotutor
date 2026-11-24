//! A closed-domain authoring contract. A teacher, template or model may propose
//! parameters. Only the reviewed claim and the arithmetic kernel can supply facts.

use crate::curriculum::{Document, Span};
use crate::protocol::Judgment;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// An original source excerpt for this experimental pack. Matching it certifies
/// the identity of the rule used by this template, not arbitrary entailment.
pub const UNIT_RULE: &str =
    "For equal parts of the same whole, add the part counts and keep the part size.";

pub fn document_id(bytes: &[u8]) -> Document {
    Sha256::digest(bytes).into()
}

/// The broker supplies the authorized bytes for this request. The caller must
/// commit delivery against the same release and rights revision. Each operation
/// checks the supplied view, including submissions for previously delivered items.
pub struct RuleSource<'a> {
    pub bytes: Option<&'a [u8]>,
    pub pin: Span,
    pub revoked: &'a BTreeSet<Document>,
}

impl RuleSource<'_> {
    /// The exact passage an authorized card uses at this instant.
    pub fn passage(&self) -> Option<&str> {
        let pin = self.active_span()?;
        let bytes = self.bytes?.get(pin.start as usize..pin.end as usize)?;
        std::str::from_utf8(bytes).ok()
    }

    fn active_span(&self) -> Option<Span> {
        let bytes = self.bytes?;
        let pin = self.pin;
        if self.revoked.contains(&pin.document) || document_id(bytes) != pin.document {
            return None;
        }
        let selected = bytes.get(pin.start as usize..pin.end as usize)?;
        (selected == UNIT_RULE.as_bytes()).then_some(pin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FractionTask {
    Sum,
    MissingPart,
}

/// All three counts refer to equal pieces of the same whole. Both operations
/// fit within one whole. Broader fraction problems require a different template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FractionCard {
    task: FractionTask,
    first: u16,
    second: u16,
    parts: u16,
}

impl FractionCard {
    pub fn propose(task: FractionTask, first: u16, second: u16, parts: u16) -> Option<Self> {
        let total = first.checked_add(second)?;
        (first > 0 && second > 0 && total <= parts).then_some(Self {
            task,
            first,
            second,
            parts,
        })
    }

    pub fn question(&self, source: &RuleSource<'_>) -> Option<(String, Span)> {
        let pin = source.active_span()?;
        let Self {
            first,
            second,
            parts,
            ..
        } = self;
        let prompt = match self.task {
            FractionTask::Sum => format!("What is {first}/{parts} + {second}/{parts}?"),
            FractionTask::MissingPart => {
                let total = first + second;
                format!("A ribbon has length {total}/{parts} metre. You cut off {first}/{parts} metre. What fraction of a metre remains?")
            }
        };
        Some((prompt, pin))
    }

    /// This is an instructional asset. Displaying it for the assessed item
    /// consumes the unassisted opportunity for that attempt.
    pub fn worked(&self, source: &RuleSource<'_>) -> Option<(String, Span)> {
        let pin = source.active_span()?;
        let Self {
            first,
            second,
            parts,
            ..
        } = self;
        let total = first + second;
        let explanation = match self.task {
            FractionTask::Sum => {
                let noun = if *first == 1 { "part" } else { "parts" };
                format!("{UNIT_RULE} Here the part size is 1/{parts}. Count {first} {noun} and {second} more. Their sum is {total}/{parts}.")
            }
            FractionTask::MissingPart => format!("Measure the ribbon in parts of 1/{parts} metre each. Starting with {total} parts and removing {first} leaves {second}. Check by adding the lengths: {first}/{parts} + {second}/{parts} = {total}/{parts} metre."),
        };
        Some((explanation, pin))
    }

    /// Evaluate only the numeric result. Natural language, unsupported notation
    /// and invalid denominators go to clarification. No judgment of reasoning.
    pub fn assess(&self, answer: &str, source: &RuleSource<'_>) -> Option<Judgment> {
        source.active_span()?;
        Some(self.numeric_judgment(answer))
    }

    fn numeric_judgment(&self, answer: &str) -> Judgment {
        let answer = answer.trim();
        if answer.is_empty() {
            return Judgment::Missing;
        }
        let (n, d) = answer.split_once('/').unwrap_or((answer, "1"));
        let count = |s: &str| {
            let s = s.trim();
            s.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| s.parse::<u32>().ok())
                .flatten()
        };
        let (Some(n), Some(d)) = (count(n), count(d)) else {
            return Judgment::Ambiguous;
        };
        if d == 0 {
            return Judgment::Ambiguous;
        }
        let expected = match self.task {
            FractionTask::Sum => self.first + self.second,
            FractionTask::MissingPart => self.second,
        };
        if u64::from(n) * u64::from(self.parts) == u64::from(d) * u64::from(expected) {
            Judgment::Correct
        } else {
            Judgment::Incorrect
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Vec<u8>, Span) {
        let bytes = format!("Unit 1\n{UNIT_RULE}").into_bytes();
        let pin = Span {
            document: document_id(&bytes),
            start: 7,
            end: bytes.len() as u32,
        };
        (bytes, pin)
    }

    #[test]
    fn requires_exact_authorized_bytes_and_active_span() {
        let (bytes, pin) = fixture();
        let revoked = BTreeSet::from([pin.document]);
        let active = BTreeSet::new();
        let card = FractionCard::propose(FractionTask::Sum, 1, 2, 3).unwrap();
        let view = RuleSource {
            bytes: Some(&bytes),
            pin,
            revoked: &active,
        };
        assert!(card.question(&view).is_some());
        for invalid in [
            RuleSource {
                bytes: None,
                ..view
            },
            RuleSource {
                revoked: &revoked,
                ..view
            },
            RuleSource {
                bytes: Some(UNIT_RULE.as_bytes()),
                ..view
            },
            RuleSource {
                pin: Span { start: 8, ..pin },
                ..view
            },
            RuleSource {
                pin: Span { end: 2, ..pin },
                ..view
            },
            RuleSource {
                pin: Span {
                    end: u32::MAX,
                    ..pin
                },
                ..view
            },
        ] {
            assert!(card.question(&invalid).is_none());
            assert!(card.worked(&invalid).is_none());
            assert_eq!(card.assess("1", &invalid), None);
        }
    }

    #[test]
    fn two_question_families_agree_with_reduced_fraction_oracle() {
        fn reduced(n: u16, d: u16) -> (u16, u16) {
            let (mut a, mut b) = (n, d);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            (n / a, d / a)
        }

        let mut comparisons = 0;
        for d in 2..=12 {
            for first in 1..d {
                for second in 1..=d - first {
                    for (task, target) in [
                        (FractionTask::Sum, first + second),
                        (FractionTask::MissingPart, second),
                    ] {
                        let card = FractionCard::propose(task, first, second, d).unwrap();
                        for answer_d in 1..=12 {
                            for answer_n in 0..=answer_d + 1 {
                                let response = format!("{answer_n}/{answer_d}");
                                let equivalent = reduced(answer_n, answer_d) == reduced(target, d);
                                let expected = if equivalent {
                                    Judgment::Correct
                                } else {
                                    Judgment::Incorrect
                                };
                                assert_eq!(card.numeric_judgment(&response), expected);
                                comparisons += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(comparisons, 58_344);
    }

    #[test]
    fn grading_is_exact_and_unsupported_responses_remain_open() {
        let (bytes, pin) = fixture();
        let active = BTreeSet::new();
        let rule = RuleSource {
            bytes: Some(&bytes),
            pin,
            revoked: &active,
        };
        let card = FractionCard::propose(FractionTask::Sum, 3, 1, 4).unwrap();
        for correct in ["1", " 4 / 4 ", "4294967295/4294967295"] {
            assert_eq!(card.assess(correct, &rule), Some(Judgment::Correct));
        }
        for ambiguous in ["4/0", "1/1/1", "one whole", "1.0", "4294967296"] {
            assert_eq!(card.assess(ambiguous, &rule), Some(Judgment::Ambiguous));
        }
        assert_eq!(card.assess(" ", &rule), Some(Judgment::Missing));
        assert_eq!(card.assess("4/8", &rule), Some(Judgment::Incorrect));
        assert!(FractionCard::propose(FractionTask::Sum, u16::MAX, 1, 4).is_none());
        assert!(FractionCard::propose(FractionTask::Sum, 0, 0, 0).is_none());
        assert!(FractionCard::propose(FractionTask::Sum, 3, 2, 4).is_none());
        let changed = FractionCard::propose(FractionTask::MissingPart, 2, 3, 8).unwrap();
        assert_eq!(changed.assess("6/16", &rule), Some(Judgment::Correct));
        assert_ne!(
            changed.question(&rule).unwrap().0,
            changed.worked(&rule).unwrap().0
        );
    }
}
