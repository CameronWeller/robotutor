//! An optional forecast for the arithmetic return in this journal. Only the
//! current checkpoint can anchor recall. A delivered prompt still counts as
//! contact when its answer or source no longer qualifies for a checkpoint.
use super::{assistance, Session, B, SEAT};
use crate::{
    curriculum::OutcomeId,
    history::{eligible, Withdrawn},
    protocol::Judgment,
    retention::{read, Cell, Exposure, Moment, Prior},
};
use serde_json::{json, Value};

const CELL: Cell = Cell {
    scope: SEAT.scope,
    outcome: B,
};

#[derive(Clone)]
pub(super) struct ReturnContacts {
    pub(super) covered_from: Moment,
    seen: Vec<Exposure>,
    pub(super) latest_course_aid: Option<(u32, u64, String)>,
}

impl ReturnContacts {
    pub(super) fn from_day(day: u32) -> Self {
        Self {
            covered_from: Moment { day, sequence: 0 },
            seen: Vec::new(),
            latest_course_aid: None,
        }
    }
}

impl Session {
    pub(super) fn event_moment(&self) -> Moment {
        Moment {
            day: self.day,
            sequence: self.revision + 100,
        }
    }

    pub(super) fn record_return_contact(&mut self, target: OutcomeId) {
        if target == B {
            self.return_contacts.seen.push(Exposure {
                cell: CELL,
                at: self.event_moment(),
            });
        }
    }

    /// Project this head to a day, conditional on zero additional encounters.
    /// The fixed model assumptions match the retention investigation's starting
    /// values. This read does not estimate personalized stability from answers.
    pub fn return_forecast(&self, day: u32, target: Option<u32>) -> Result<Value, String> {
        if day < self.day || target.is_some_and(|t| t < day) {
            return Err(
                "forecast day must follow the journal clock and cannot follow its target".into(),
            );
        }
        let progress = self.progress()?;
        let checkpoint = progress.checkpoints.get(&B);
        let mut withdrawn = Withdrawn::new(SEAT.scope);
        withdrawn.sources = self.retired.clone();
        let attempts = self.history.iter().rev().filter(|a| a.outcome == B);
        let latest = attempts.clone().next();
        let anchor = attempts
            .filter(|a| a.judgment == Judgment::Correct && eligible(a, self.day, &withdrawn))
            .find(|a| {
                checkpoint.is_some_and(|c| {
                    c.observed == a.day
                        && self
                            .proposals
                            .iter()
                            .any(|p| p.outcome == B && p.confirmation == a.id && p.due == c.due)
                })
            });
        if checkpoint.is_some() && anchor.is_none() {
            return Err("missing confirmation for reconciled return checkpoint".into());
        }
        let stability_days = 14.0;
        let threshold = 0.85;
        let prior = anchor.map(|a| Prior {
            cell: CELL,
            verified: Moment {
                day: a.day,
                sequence: a.sequence,
            },
            stability_days,
            threshold,
            had_durable_success: false,
        });
        let covered =
            self.complete && prior.is_some_and(|p| p.verified >= self.return_contacts.covered_from);
        let seen = &self.return_contacts.seen;
        let (forecast, _) = read(
            CELL,
            Moment {
                day,
                sequence: u64::MAX,
            },
            target,
            prior,
            covered.then_some(seen.as_slice()),
            None,
            &withdrawn,
        )
        .map_err(|e| format!("{e:?}"))?;
        // Missing encounters could shorten this interval. A recent, positively
        // observed encounter therefore still rules out some spaced checks.
        let last_known = prior.map(|p| seen.last().map_or(p.verified, |e| p.verified.max(e.at)));
        let upper_gap = last_known.map(|at| day - at.day);
        let meets_spacing = forecast.and_then(|f| match f.days_since_exposure {
            Some(gap) => Some(f64::from(gap) >= f.required_gap_days),
            None if upper_gap.is_some_and(|gap| f64::from(gap) < f.required_gap_days) => {
                Some(false)
            }
            None => None,
        });
        Ok(json!({
            "outcome":B.0, "learner":SEAT.scope.learner, "release":SEAT.scope.revision, "class":SEAT.class,
            "journal_day":self.day, "forecast_day":day, "target_day":target,
            "model":{"stability_days":stability_days, "recall_threshold":threshold, "assumes_no_further_contact":true},
            "booked_due":checkpoint.map(|c|c.due), "source_active":self.source_active, "grant":self.grant,
            "supported_performance":anchor.map(|a|json!({"day":a.day,"attempt":a.id,"item":a.item})),
            "last_course_aid":self.return_contacts.latest_course_aid.as_ref().map(|(d,item,kind)|json!({"day":d,"item":item,"kind":kind})),
            "latest_response":latest.map(|a|json!({"day":a.day,"attempt":a.id,"item":a.item,"judgment":format!("{:?}",a.judgment),
                "reported_assistance":assistance(a.assistance),"eligible":eligible(a,self.day,&withdrawn),
                "source_withdrawn":!a.sources.is_disjoint(&withdrawn.sources)})),
            "last_recorded_encounter_day":seen.last().map(|x|x.at.day),
            "rehearsal_contact_coverage":if prior.is_none() {"no_supported_performance"} else if covered {"covered_since_supported_performance"} else {"incomplete_since_supported_performance"},
            "modeled_recall":forecast.map(|f|f.recall), "refresh_due":forecast.map(|f|f.refresh_due),
            "required_gap_days":forecast.map(|f|f.required_gap_days), "days_since_exposure":forecast.and_then(|f|f.days_since_exposure),
            "max_possible_gap_days":upper_gap, "meets_spacing_if_no_further_contact":meets_spacing
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{protocol::Assistance, rehearsal::Action};

    // Every accepted action below is encoded, then restored through the journal
    // transition path before the next command uses it.
    fn step(session: Session, action: Action) -> Session {
        let (event, _) = session.clone().apply(action).unwrap();
        session.restore_next(&event).unwrap()
    }

    fn checked_return() -> Session {
        let session = step(Session::new().unwrap(), Action::Open(30));
        let session = step(
            session,
            Action::Answer("2/16".into(), Assistance::NoneReported),
        );
        step(session, Action::UseVariedInstances)
    }

    #[test]
    fn seeing_the_next_problem_changes_the_spacing_conclusion_even_without_an_answer() {
        let session = checked_return();
        let before = session.return_forecast(54, None).unwrap();
        assert_eq!(before["booked_due"], 51);
        assert_eq!(before["days_since_exposure"], 24);
        assert_eq!(before["meets_spacing_if_no_further_contact"], true);
        assert_eq!(
            session.return_forecast(54, Some(3650)).unwrap()["meets_spacing_if_no_further_contact"],
            false
        );
        let shown = step(session, Action::Open(53));
        let after = shown.return_forecast(54, None).unwrap();
        assert_eq!(after["modeled_recall"], before["modeled_recall"]);
        assert_eq!(after["booked_due"], before["booked_due"]);
        assert_eq!(after["days_since_exposure"], 1);
        assert_eq!(after["meets_spacing_if_no_further_contact"], false);

        let assisted = step(shown.clone(), Action::CourseAid("worked".into()));
        let assisted = step(
            assisted,
            Action::Answer("1/16".into(), Assistance::NoneReported),
        )
        .return_forecast(54, None)
        .unwrap();
        assert_eq!(
            assisted["last_course_aid"]["item"],
            assisted["latest_response"]["item"]
        );
        assert_eq!(assisted["latest_response"]["reported_assistance"], "none");
        assert_eq!(assisted["latest_response"]["eligible"], false);
        assert_eq!(
            assisted["supported_performance"],
            before["supported_performance"]
        );
        assert_eq!(assisted["refresh_due"], true);

        let clean = step(
            shown.clone(),
            Action::Answer("1/16".into(), Assistance::NoneReported),
        )
        .return_forecast(54, None)
        .unwrap();
        assert_eq!(clean["booked_due"], 74);
        assert_eq!(clean["supported_performance"]["day"], 53);
        assert_eq!(clean["refresh_due"], false);

        let failed = step(
            shown,
            Action::Answer("0/16".into(), Assistance::NoneReported),
        )
        .return_forecast(54, None)
        .unwrap();
        assert!(failed["modeled_recall"].is_null());
        assert!(failed["supported_performance"].is_null());
        assert_eq!(failed["latest_response"]["judgment"], "Incorrect");
    }

    #[test]
    fn withdrawals_and_unlocated_help_or_contact_cannot_invent_a_clean_interval() {
        let session = checked_return();
        let withdrawn = step(session.clone(), Action::WithdrawSource)
            .return_forecast(54, None)
            .unwrap();
        assert_eq!(withdrawn["supported_performance"]["day"], 0);
        assert_eq!(withdrawn["latest_response"]["source_withdrawn"], true);
        assert_eq!(withdrawn["last_recorded_encounter_day"], 30);
        assert!(withdrawn["days_since_exposure"].is_null());

        let private = step(session.clone(), Action::Work("My private poem".into()))
            .return_forecast(54, None)
            .unwrap();
        assert!(private["days_since_exposure"].is_null());
        let unknown = step(session, Action::UnknownContact);
        assert!(unknown.return_forecast(54, None).unwrap()["days_since_exposure"].is_null());
        let gap_with_recent_contact = step(
            step(checked_return(), Action::Open(53)),
            Action::UnknownContact,
        );
        let bounded = gap_with_recent_contact.return_forecast(54, None).unwrap();
        assert!(bounded["days_since_exposure"].is_null());
        assert_eq!(bounded["max_possible_gap_days"], 1);
        assert_eq!(bounded["meets_spacing_if_no_further_contact"], false);
        let recovered = step(unknown, Action::RecoverContact);
        assert!(recovered.return_forecast(54, None).unwrap()["days_since_exposure"].is_null());
        let fresh = step(recovered, Action::Open(53));
        let fresh = step(
            fresh,
            Action::Answer("1/16".into(), Assistance::NoneReported),
        );
        assert_eq!(
            fresh.return_forecast(54, None).unwrap()["days_since_exposure"],
            1
        );
    }
}
