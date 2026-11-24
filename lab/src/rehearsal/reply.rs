//! Accept ordinary text at a numeric opening without mistaking every utterance
//! for an assessment submission. Unparsed prose stays in the caller's process.
use super::{assistance, Session, Source};
use crate::{
    protocol::{Assistance, Judgment},
    routing::Intent,
};
use serde_json::{json, Value};

impl Session {
    fn current_form(&self, intent: Intent) -> Result<crate::supply::Form, String> {
        let open = self.opening.as_ref().ok_or("no course exercise is open")?;
        if self.parked || open.epoch != self.epoch || !self.permits(open.form, intent)? {
            return Err("the current opening does not permit this action".into());
        }
        Ok(open.form)
    }

    pub(super) fn reply_event(&self, text: &str, help: Assistance) -> Result<Value, String> {
        if text.len() > crate::workshop::MAX_DRAFT_BYTES {
            return Err("this reply exceeds the local text limit".into());
        }
        let form = self.current_form(Intent::GradeNumeric)?;
        let source = Source::new(self.source_revision);
        // Only the explicitly supported numeric grammar can enter the existing
        // answer history. An inadvertent pasted draft must never enter it.
        if text.len() <= 128
            && matches!(
                form.card.assess(text, &source.live(&self.retired)),
                Some(Judgment::Correct | Judgment::Incorrect)
            )
        {
            return Ok(json!(["answer", text.trim(), assistance(help)]));
        }
        Ok(json!(["reply-options"]))
    }

    pub(super) fn reply_options(&self) -> Result<Value, String> {
        self.current_form(Intent::GradeNumeric)?;
        Ok(json!({
            "contribution": "reply_options",
            "display": "I haven’t recorded an answer. You can enter an amount, ask how to format it, see the worked solution, read the source, or switch to your own writing.",
            "choices": ["reply", "format", "worked", "source", "work"]
        }))
    }

    pub(super) fn course_aid(&mut self, kind: &str) -> Result<Value, String> {
        if kind == "format" {
            self.current_form(Intent::GradeNumeric)?;
            return Ok(json!({
                "contribution": "response_format",
                "display": "Use digits. For a fraction, put a slash between its numerator and denominator. A whole number also works. Your exercise is still open."
            }));
        }
        let form = self.current_form(Intent::Explain)?;
        let source = Source::new(self.source_revision);
        let rule = source.live(&self.retired);
        let text = match kind {
            "worked" => form.card.worked(&rule).map(|(text, _)| text),
            "source" => rule.passage().map(str::to_owned),
            _ => return Err("choose format, worked, or source".into()),
        }
        .ok_or("released teaching material unavailable")?;
        // Reading substantive help changes conditions even if a later answer
        // reports no other assistance. The same exercise stays available.
        let open = self.opening.as_mut().ok_or("no course exercise is open")?;
        open.independent = false;
        self.record_return_contact(form.outcome);
        if form.outcome == super::B {
            self.return_contacts.latest_course_aid =
                Some((self.day, form.revision, kind.to_owned()));
        }
        self.coached_day = Some(self.day);
        Ok(json!({
            "contribution": "course_help",
            "display": format!("{text}\nYou can now try the exercise as guided practice."),
            "aid": kind, "task": form.task
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rehearsal::Action;

    #[test]
    fn unsupported_utterances_are_anonymous_and_stale_aids_leave_state_intact() {
        let mut s = Session::new().unwrap();
        s.apply(Action::Open(30)).unwrap();
        for text in [
            "",
            "one eighth",
            "Can you explain cut off?",
            "Can we edit my story?",
        ] {
            let (event, receipt) = s
                .apply(Action::Reply(text.into(), Assistance::Unknown))
                .unwrap();
            assert_eq!(event, json!(["reply-options"]));
            assert_eq!(receipt["current_opening"]["task"], 1);
            assert!(receipt["credit"].is_null());
        }
        let (event, receipt) = s
            .apply(Action::Reply("3/16".into(), Assistance::NoneReported))
            .unwrap();
        assert_eq!(event, json!(["answer", "3/16", "none"]));
        assert_eq!(receipt["judgment"], "Incorrect");

        for change in [
            Action::WithdrawSource,
            Action::ReissueGrant,
            Action::Work("My own story.".into()),
        ] {
            let mut s = Session::new().unwrap();
            s.apply(Action::Open(30)).unwrap();
            s.apply(change).unwrap();
            let before = s.status().unwrap();
            for kind in ["worked", "source", "format"] {
                assert!(s.apply(Action::CourseAid(kind.into())).is_err());
            }
            assert!(s
                .apply(Action::Reply("1/8".into(), Assistance::NoneReported))
                .is_err());
            assert_eq!(s.status().unwrap(), before);
        }
    }
}
