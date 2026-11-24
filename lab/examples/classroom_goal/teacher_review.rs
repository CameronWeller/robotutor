//! Replay a capacity prediction separately from a learner's accepted review and its delivery.
use super::*;
use sha2::{Digest, Sha256};
type Check<T> = Result<T, String>;
const SAMPLE: &str = "My shift ends at six, but I always take one more call.";
const COMMENT:&str="The shift ending at six sounds concrete. Read the last clause aloud and decide whether the worker would use it.";

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Status {
    Requested,
    Accepted,
    Interrupted,
    Declined,
    Completed,
}
impl Status {
    fn owed(self) -> bool {
        matches!(self, Self::Accepted | Self::Interrupted)
    }
}
struct Ticket {
    seat: u64,
    state: Status,
    events: Vec<Value>,
}
impl Ticket {
    fn new(seat: u64) -> Self {
        Self {
            seat,
            state: Status::Requested,
            events: vec![],
        }
    }
    fn record(&mut self, kind: &str, slot: u64, proof: Option<&str>) -> Check<()> {
        let state = match (self.state, kind) {
            (Status::Requested | Status::Interrupted, "accept") => Status::Accepted,
            (Status::Accepted, "interrupt") => Status::Interrupted,
            (Status::Accepted | Status::Interrupted | Status::Requested, "decline") => {
                Status::Declined
            }
            (Status::Accepted, "complete")
                if proof
                    .is_some_and(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit())) =>
            {
                Status::Completed
            }
            _ => return Err("invalid or duplicate review transition".into()),
        };
        if kind != "complete" && proof.is_some() {
            return Err("unexpected comment reference".into());
        }
        self.events.push(json!([self.seat, kind, slot, proof]));
        self.state = state;
        Ok(())
    }
    fn reopen(&self, dir: &Path) -> Check<Self> {
        let file = dir.join("events.json");
        fs::write(&file, serde_json::to_vec(&self.events).unwrap()).map_err(|e| e.to_string())?;
        let rows: Vec<Value> = serde_json::from_slice(&fs::read(&file).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mut out = Self::new(self.seat);
        for row in rows {
            if row[0] != self.seat || row.as_array().is_none_or(|a| a.len() != 4) {
                return Err("invalid subject or event".into());
            }
            out.record(
                row[1].as_str().ok_or("invalid event")?,
                row[2].as_u64().ok_or("invalid meeting")?,
                row[3].as_str(),
            )?;
            if out.events.last() != Some(&row) {
                return Err("invalid saved receipt".into());
            }
        }
        let queue = if out.state.owed() {
            vec![Request {
                seat: self.seat,
                goal: 20,
                meeting: 1,
            }]
        } else {
            vec![]
        };
        let (saved, _) = reopen(&dir.join("backlog.json"), &queue, false);
        if saved != queue {
            return Err("reopened queue disagrees with obligation".into());
        }
        Ok(out)
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_json(path: &Path) -> Check<Value> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
struct Forecast {
    fits: bool,
    ahead: u64,
    minutes: u64,
    cost: u64,
    display: String,
}
fn forecast(root: &Path, trial: &str, arm: &str, pin: &str) -> Check<Forecast> {
    let bytes =
        fs::read(root.join(trial).join(format!("{arm}-return.json"))).map_err(|e| e.to_string())?;
    if digest(&bytes) != pin {
        return Err("wrong archived return receipt".into());
    }
    let r: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let assignment = read_json(&root.join(trial).join("condition.json"))?;
    let c = &assignment["condition"];
    let number = |field: &str| c[field].as_u64().ok_or("invalid capacity");
    let (ahead, minutes, cost) = (
        number("queue_ahead")?,
        number("teacher_minutes")?,
        number("review_minutes")?,
    );
    if r["packet"]["condition"]["observed"]!=format!("The learner asked for an individual review. {ahead} requests are ahead, with {minutes} minutes left and {cost} minutes per review.") {
        return Err("staffing figures differ from the observed visit".into());
    }
    Ok(Forecast {
        fits: (ahead + 1) * cost <= minutes,
        ahead,
        minutes,
        cost,
        display: r["response"]["display"]
            .as_str()
            .ok_or("no learner display")?
            .into(),
    })
}
fn book(t: &mut Ticket, slot: u64, fits: bool, accept: bool) -> Check<()> {
    if !accept {
        return t.record("decline", slot, None);
    }
    if !fits {
        return Err("no review capacity this meeting".into());
    }
    t.record("accept", slot, None)
}
fn finish(
    t: &mut Ticket,
    slot: u64,
    reviewer_for: u64,
    submitted: Option<&str>,
    received: &str,
    dir: &Path,
) -> Check<()> {
    if t.state != Status::Accepted
        || !t.events.last().is_some_and(|r| r[2] == slot)
        || reviewer_for != t.seat
        || submitted.is_none_or(|s| s.trim().is_empty())
        || received.trim().is_empty()
    {
        return Err(
            "review needs a pending slot, assigned reviewer, current sharing and actual comment"
                .into(),
        );
    }
    let content =
        serde_json::to_vec(&json!({"seat":t.seat,"meeting":slot,"comment":received})).unwrap();
    let pin = digest(&content);
    fs::write(dir.join(format!("private-comment-{pin}.json")), &content)
        .map_err(|e| e.to_string())?;
    t.record("complete", slot, Some(&pin))
}
fn learner_view(t: &Ticket, viewer: u64, share: bool, dir: &Path) -> Check<(String, bool)> {
    if viewer != t.seat {
        return Err("review is scoped to another learner".into());
    }
    let message=match t.state {
        Status::Requested=>"There was no reserved review. You can request a three-minute slot next meeting.",
        Status::Accepted=>"Your three-minute review is booked. A teacher comment has not arrived.",
        Status::Interrupted=>"Your booked review did not take place. You can take the next three-minute slot or decline.",
        Status::Declined=>"You declined this review. No teacher comment was recorded.",
        Status::Completed if !share=>"Your review took place. You can choose whether to reopen the teacher's comment.",
        Status::Completed=>{
            let row=t.events.last().ok_or("no completion receipt")?;
            let raw=fs::read(dir.join(format!("private-comment-{}.json",row[3].as_str().ok_or("missing private reference")?))).map_err(|e|e.to_string())?;
            let body:Value=serde_json::from_slice(&raw).map_err(|e|e.to_string())?;
            if row[3]!=digest(&raw) || body["seat"]!=viewer || body["meeting"]!=row[2] {return Err("comment differs from assigned service receipt".into());}
            return Ok((body["comment"].as_str().ok_or("missing comment")?.into(),true));
        }
    };
    Ok((message.into(), false))
}
struct Seen {
    interrupted: bool,
    owed: bool,
    completed: bool,
    share: bool,
    disclosed: bool,
}
fn monitor_view() -> Monitor<Seen> {
    let mut p = Predicates::builder();
    let honest = p.new_predicate(ClosurePredicate::new(
        |s: &Seen| (!s.interrupted || s.owed) && (!s.disclosed || s.completed && s.share),
        "interrupted service stays owed; only completed authorized service reveals a comment",
    ));
    let p = p.build();
    let m = Property::always(Property::atomic(honest)).to_monitoring_automaton(&p);
    Monitor::new(p, m)
}
fn observe(t: &Ticket, share: bool, dir: &Path, m: &mut Monitor<Seen>) -> Check<Value> {
    let reopened = t.reopen(dir)?;
    let (display, disclosed) = learner_view(&reopened, t.seat, share, dir)?;
    let state = reopened.state;
    if !m.next_state(&Seen {
        interrupted: state == Status::Interrupted,
        owed: state.owed(),
        completed: state == Status::Completed,
        share,
        disclosed,
    }) {
        return Err("review disclosure or obligation invariant failed".into());
    }
    Ok(
        json!({"status":format!("{state:?}"),"owed":state.owed(),"display":display,"comment_shown":disclosed}),
    )
}
fn paths(seat: u64, f: &Forecast) -> Check<Value> {
    let dir = scratch();
    let exercise = || -> Check<Value> {
        let mut m = monitor_view();
        let mut t = Ticket::new(seat);
        let old = observe(&t, false, &dir, &mut m)?;
        let order = book(&mut t, 1, f.fits, true);
        if !f.fits {
            if order.is_ok() {
                return Err("overbooked".into());
            }
            let current = observe(&t, false, &dir, &mut m)?;
            book(&mut t, 2, true, true)?;
            return Ok(
                json!({"forecast_only_display":f.display,"forecast_only_next_visit":old,"overflow":current,"accepted_next_window":observe(&t,false,&dir,&mut m)?}),
            );
        }
        order?;
        let pending = observe(&t, false, &dir, &mut m)?;
        let mut on_time = t.reopen(&dir)?;
        finish(&mut on_time, 1, seat, Some(SAMPLE), COMMENT, &dir)?;
        let on_time = observe(&on_time, true, &dir, &mut m)?;
        let mut interrupted = t.reopen(&dir)?;
        interrupted.record("interrupt", 1, None)?;
        let missed = observe(&interrupted, false, &dir, &mut m)?;
        let mut declined = interrupted.reopen(&dir)?;
        book(&mut declined, 2, true, false)?;
        let decline = observe(&declined, false, &dir, &mut m)?;
        let mut again = interrupted.reopen(&dir)?;
        book(&mut again, 2, true, true)?;
        let booked = observe(&again, false, &dir, &mut m)?;
        if finish(&mut again, 2, seat, None, COMMENT, &dir).is_ok()
            || finish(&mut again, 2, seat + 1, Some(SAMPLE), COMMENT, &dir).is_ok()
            || finish(&mut again, 1, seat, Some(SAMPLE), COMMENT, &dir).is_ok()
        {
            return Err("unauthorized review".into());
        }
        finish(&mut again, 2, seat, Some(SAMPLE), COMMENT, &dir)?;
        let completed = observe(&again, true, &dir, &mut m)?;
        let withheld = observe(&again, false, &dir, &mut m)?;
        if finish(&mut again, 2, seat, Some(SAMPLE), COMMENT, &dir).is_ok()
            || learner_view(&again, seat + 1, true, &dir).is_ok()
        {
            return Err("duplicate completion or cross-seat access".into());
        }
        let journal = fs::read_to_string(dir.join("events.json")).map_err(|e| e.to_string())?;
        if journal.contains(SAMPLE) || journal.contains(COMMENT) {
            return Err("private prose entered shared queue journal".into());
        }
        Ok(
            json!({"forecast_only_display":f.display,"forecast_only_next_visit":old,"accepted":pending,"completed_on_time":on_time,
            "interrupted":missed,"declined":decline,"rescheduled":booked,"completed":completed,"completed_without_return_grant":withheld}),
        )
    }();
    for file in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let path = file.map_err(|e| e.to_string())?.path();
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    fs::remove_dir(dir).map_err(|e| e.to_string())?;
    exercise
}
pub(super) fn study(root: &Path) -> Check<Value> {
    let mut report = json!({});
    for (trial, seat, arm, hash) in [
        (
            "007",
            7,
            "B",
            "dfc1b3ac43cebe6f9075ce4676c2ffa062b22737e931ea932131c7ecbd144157",
        ),
        (
            "020",
            20,
            "A",
            "7292d370787e2e20621d36884b707aaafc29408a453012a6949a836d1b85ddb7",
        ),
    ] {
        let f = forecast(root, trial, arm, hash)?;
        report[trial] = json!({"minutes":f.minutes,"ahead":f.ahead,"cost":f.cost,"fits":f.fits,"replay":paths(seat,&f)?});
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_preserves_debt_and_rechecks_authority() {
        let f = Forecast {
            fits: true,
            ahead: 1,
            minutes: 10,
            cost: 3,
            display: "A teacher review fits.".into(),
        };
        let r = paths(20, &f).unwrap();
        assert_eq!(r["forecast_only_next_visit"]["owed"], false);
        assert_eq!(r["interrupted"]["owed"], true);
        assert_eq!(r["rescheduled"]["owed"], true);
        assert_eq!(r["completed"]["display"], COMMENT);
        assert_eq!(r["completed_without_return_grant"]["comment_shown"], false);
        assert_eq!(r["declined"]["owed"], false);
        assert!(!monitor_view().next_state(&Seen {
            interrupted: true,
            owed: false,
            completed: false,
            share: true,
            disclosed: false
        }));
        assert!(!monitor_view().next_state(&Seen {
            interrupted: false,
            owed: false,
            completed: true,
            share: false,
            disclosed: true
        }));
    }
}
