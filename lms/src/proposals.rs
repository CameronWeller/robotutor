use super::*;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn metadata(db: &Connection, course: i64) -> R<Option<Value>> {
    Ok(db.query_row("SELECT summary,problem,revision FROM proposals WHERE course=?", [course],
        |r| Ok(json!({"changes":r.get::<_,String>(0)?,"problem":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?}))).optional()?)
}

fn prior_request(db: &Connection, request: &Value) -> R<Option<Value>> {
    let Some(token) = request["token"].as_str() else {
        return Ok(None);
    };
    let saved: Option<(String, i64)> = db
        .query_row(
            "SELECT packet,course FROM proposal_requests WHERE token=?",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match saved {
        Some((packet, course)) if packet == request.to_string() => Ok(Some(json!({"id":course}))),
        Some(_) => Err("Request token already used for another change.".into()),
        None => Ok(None),
    }
}

pub fn generate(db: &mut Connection, request: &Value, worker: &std::path::Path) -> R<Value> {
    if let Some(result) = prior_request(db, request)? {
        return Ok(result);
    }
    let cid = request["id"].as_i64();
    let mut context = if let Some(cid) = cid {
        let (draft, saved, revision): (String,String,i64) = db.query_row(
            "SELECT draft,context,revision FROM courses JOIN proposals ON course=id WHERE id=? AND archived=0",
            [cid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        if Some(revision) != request["revision"].as_i64() {
            return Err("Course changed. Reload before revising.".into());
        }
        let mut c: Value = serde_json::from_str(&saved)?;
        c["previous"] = serde_json::from_str(&draft)?;
        c
    } else {
        json!({"direction":[]})
    };
    if request["edited"].is_object() {
        if cid.is_none() {
            return Err("Direct edits require an existing proposal.".into());
        }
        outline(&request["edited"], true)?;
        context["edited"] = request["edited"].clone();
        let edit = json!({"before":context["previous"], "after":request["edited"]});
        context["direction"].as_array_mut().unwrap().push(edit);
    } else {
        let brief = string(request, "brief")?.trim();
        if brief.is_empty() {
            return Err("Describe your course or requested change.".into());
        }
        context["direction"]
            .as_array_mut()
            .unwrap()
            .push(json!(brief));
        context.as_object_mut().unwrap().remove("edited");
    }
    let mut child = Command::new(std::env::var_os("ROBOTUTOR_PYTHON").unwrap_or_else(|| {
        if cfg!(target_os = "linux") {
            "/usr/bin/python3"
        } else if cfg!(windows) {
            "python"
        } else {
            "python3"
        }
        .into()
    }))
    .arg(worker)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(context.to_string().as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .lines()
            .last()
            .unwrap_or("Course generation failed")
            .to_owned()
            .into());
    }
    let candidate: Value = serde_json::from_slice(&output.stdout)?;
    outline(&candidate["outline"], true)?;
    let draft = candidate["outline"].to_string();
    let problem = string(&candidate, "problem")?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(result) = prior_request(&tx, request)? {
        return Ok(result);
    }
    let course = if let Some(id) = cid {
        if tx.execute("UPDATE courses SET draft=? WHERE id=? AND archived=0 AND (SELECT revision FROM proposals WHERE course=id)=?",
            params![draft,id,request["revision"].as_i64()])? != 1 {return Err("Course changed during generation. Reload and retry.".into());}
        id
    } else {
        tx.execute("INSERT INTO courses(draft) VALUES(?)", [&draft])?;
        tx.last_insert_rowid()
    };
    tx.execute("INSERT INTO proposals VALUES(?,?,?,?,1) ON CONFLICT(course) DO UPDATE SET context=excluded.context,summary=excluded.summary,problem=excluded.problem,revision=revision+1",
        params![course,context.to_string(),string(&candidate,"changes")?,problem])?;
    if problem.is_empty() {
        tx.execute(
            "INSERT INTO versions(course,outline) VALUES(?,?)",
            params![course, draft],
        )?;
    }
    if let Some(token) = request["token"].as_str() {
        tx.execute(
            "INSERT INTO proposal_requests VALUES(?,?,?)",
            params![token, request.to_string(), course],
        )?;
    }
    tx.commit()?;
    Ok(json!({"id":course}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_revision_keeps_release_and_opened_assignment() -> R<()> {
        let mut db = Connection::open_in_memory()?;
        db.execute_batch(include_str!("schema.sql"))?;
        let worker = env::temp_dir().join(format!("robotutor-proposal-{}.py", std::process::id()));
        fs::write(
            &worker,
            r#"import sys,json
c=json.load(sys.stdin)
activity=dict(key="cups",title="Compare cups",instructions="Measure a half cup",objective="Compare portions",criterion="Give equal measures")
course=dict(title="Fractions",destination="Scale a recipe",units=[dict(title="Cooking",activities=[activity])])
problem="Conflicting cup requirement" if c["direction"][-1]=="conflict" else ""
print(json.dumps(dict(outline=course,changes="Proposed measures",problem=problem)))
"#,
        )?;
        let cid = generate(&mut db, &json!({"brief":"fractions"}), &worker)?["id"].clone();
        let open = json!({"action":"open_activity","version":1,"unit":0,"activity":0});
        assert!(action(&mut db, "learner", &open).is_err());
        action(&mut db, "teacher", &json!({"action":"enroll","id":cid}))?;
        let first = action(&mut db, "learner", &open)?;
        action(
            &mut db,
            "learner",
            &json!({"action":"study","id":first["id"]}),
        )?;
        generate(
            &mut db,
            &json!({"id":cid,"revision":1,"brief":"conflict"}),
            &worker,
        )?;
        assert!(generate(
            &mut db,
            &json!({"id":cid,"revision":1,"brief":"stale"}),
            &worker
        )
        .is_err());
        assert_eq!(state(&db, "learner")?["courses"][0]["version"], 1);
        assert_eq!(
            state(&db, "teacher")?["courses"][0]["proposal"]["problem"],
            "Conflicting cup requirement"
        );
        generate(
            &mut db,
            &json!({"id":cid,"revision":2,"brief":"resolved"}),
            &worker,
        )?;
        assert_eq!(
            action(
                &mut db,
                "learner",
                &json!({"version":2,"unit":0,"activity":0,"action":"open_activity"})
            )?,
            first
        );
        assert_eq!(state(&db, "learner")?["assignments"][0]["version"], 1);
        fs::remove_file(worker)?;
        Ok(())
    }
}
