mod proposals;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{env, fs, io::Read, path::PathBuf};
use tiny_http::{Header, Response, Server, StatusCode};

type R<T> = Result<T, Box<dyn std::error::Error>>;
fn string<'a>(v: &'a Value, field: &str) -> R<&'a str> {
    Ok(v[field].as_str().ok_or(format!("Missing {field}"))?)
}
fn id(v: &Value, field: &str) -> R<i64> {
    Ok(v[field].as_i64().ok_or(format!("Missing {field}"))?)
}
fn outline(v: &Value, publish: bool) -> R<()> {
    for key in ["title", "destination"] {
        if string(v, key)?.trim().is_empty() {
            return Err(format!("Enter {key}").into());
        }
    }
    let units = v["units"].as_array().ok_or("Missing units")?;
    if publish && units.is_empty() {
        return Err("Add a unit before publishing".into());
    }
    for unit in units {
        if string(unit, "title")?.trim().is_empty() {
            return Err("Name the unit".into());
        }
        let activities = unit["activities"].as_array().ok_or("Missing activities")?;
        if publish && activities.is_empty() {
            return Err("Each published unit needs an activity".into());
        }
        for activity in activities {
            for key in ["title", "instructions", "objective", "criterion"] {
                if string(activity, key)?.trim().is_empty() {
                    return Err(format!("Enter activity {key}").into());
                }
            }
        }
    }
    Ok(())
}
fn state(db: &Connection, profile: &str) -> R<Value> {
    let teacher = profile == "teacher";
    let mut courses = Vec::new();
    let mut q=db.prepare("SELECT id,draft,archived,(SELECT max(id) FROM versions WHERE course=c.id),EXISTS(SELECT 1 FROM roster WHERE course=c.id AND learner='learner') FROM courses c")?;
    for r in q.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, bool>(4)?,
        ))
    })? {
        let (cid, draft, archived, version, enrolled) = r?;
        let published: Option<String> = db
            .query_row("SELECT outline FROM versions WHERE id=?", [version], |r| {
                r.get(0)
            })
            .optional()?;
        if teacher || (enrolled && published.is_some()) {
            let published = published
                .map(|p| serde_json::from_str::<Value>(&p))
                .transpose()?;
            let visible = if teacher {
                serde_json::from_str(&draft)?
            } else {
                published.clone().unwrap_or_default()
            };
            courses.push(json!({
                "id": cid, "outline": visible, "published": published, "version": version,
                "proposal": if teacher { proposals::metadata(db,cid)? } else { None }, "archived": archived, "enrolled": enrolled
            }));
        }
    }
    let mut assignments = Vec::new();
    let mut q=db.prepare("SELECT a.id,v.course,a.version,v.outline,unit_idx,activity_idx,due,status FROM assignments a JOIN versions v ON v.id=a.version WHERE ? OR learner=? ORDER BY a.id")?;
    let mut rows = q.query(params![teacher, profile])?;
    while let Some(r) = rows.next()? {
        let aid: i64 = r.get(0)?;
        let o: Value = serde_json::from_str(&r.get::<_, String>(3)?)?;
        let unit = &o["units"][r.get::<_, u32>(4)? as usize];
        let mut subs = Vec::new();
        let mut s=db.prepare("SELECT s.id,work,decision,feedback FROM submissions s LEFT JOIN reviews ON submission=s.id WHERE assignment=? ORDER BY s.id")?;
        let submissions = s.query_map([aid], |r| Ok(json!({
            "id": r.get::<_, i64>(0)?, "work": r.get::<_, String>(1)?,
            "decision": r.get::<_, Option<String>>(2)?, "feedback": r.get::<_, Option<String>>(3)?
        })))?;
        for submission in submissions {
            subs.push(submission?);
        }
        let feedback = subs
            .iter()
            .rev()
            .find_map(|s| s["feedback"].as_str())
            .unwrap_or("");
        assignments.push(json!({
            "id":aid, "course":r.get::<_,i64>(1)?, "version":r.get::<_,i64>(2)?,
            "destination":o["destination"], "unit":unit["title"],
            "activity":unit["activities"][r.get::<_,u32>(5)? as usize],
            "due":r.get::<_,String>(6)?, "status":r.get::<_,String>(7)?,
            "feedback":feedback, "submissions":subs
        }));
    }
    Ok(json!({"courses":courses,"assignments":assignments,"profile":profile}))
}
fn action(db: &mut Connection, profile: &str, v: &Value) -> R<Value> {
    let role: String = db.query_row("SELECT role FROM profiles WHERE id=?", [profile], |r| {
        r.get(0)
    })?;
    let act = string(v, "action")?;
    if act == "state" {
        return state(db, profile);
    }
    if (role == "learner") != matches!(act, "submit" | "study" | "open_activity") {
        return Err("Profile cannot perform this action".into());
    }
    if matches!(act, "save" | "publish")
        && v["id"]
            .as_i64()
            .is_some_and(|id| proposals::metadata(db, id).ok().flatten().is_some())
    {
        return Err("Use a checked course revision.".into());
    }
    let tx = db.transaction()?;
    let mut result = json!({"ok":true});
    match act {
        "save" => {
            let o = &v["outline"];
            outline(o, false)?;
            if let Some(cid) = v["id"].as_i64() {
                if tx.execute(
                    "UPDATE courses SET draft=? WHERE id=? AND archived=0",
                    params![o.to_string(), cid],
                )? != 1
                {
                    return Err("Course unavailable".into());
                }
                result["id"] = cid.into();
            } else {
                tx.execute("INSERT INTO courses(draft) VALUES(?)", [o.to_string()])?;
                result["id"] = tx.last_insert_rowid().into();
            }
        }
        "open_activity" => {
            let version = id(v, "version")?;
            let (unit, activity) = (id(v, "unit")?, id(v, "activity")?);
            let saved:String=tx.query_row("SELECT outline FROM versions v JOIN courses c ON c.id=v.course JOIN roster r ON r.course=c.id WHERE v.id=? AND r.learner=? AND c.archived=0 AND v.id=(SELECT MAX(id) FROM versions WHERE course=c.id)",params![version,profile],|r|r.get(0))?;
            let outline: Value = serde_json::from_str(&saved)?;
            let selected = &outline["units"][unit as usize]["activities"][activity as usize];
            if !selected.is_object() {
                return Err("Unknown activity".into());
            }
            let previous:Option<i64>=tx.query_row(
                "SELECT a.id FROM assignments a JOIN versions old ON a.version=old.id
                 WHERE a.learner=? AND old.course=(SELECT course FROM versions WHERE id=?)
                 AND ((a.version=? AND a.unit_idx=? AND a.activity_idx=?) OR
                 json_extract(old.outline, '$.units['||a.unit_idx||'].activities['||a.activity_idx||'].key')=?)
                 ORDER BY (a.status!='Assigned') DESC, a.id LIMIT 1",
                params![profile,version,version,unit,activity,selected["key"].as_str()],|r|r.get(0)).optional()?;
            let assignment = if let Some(existing) = previous {
                existing
            } else {
                tx.execute("INSERT INTO assignments(version,unit_idx,activity_idx,learner,due) VALUES(?,?,?,?,?)",params![version,unit,activity,profile,selected["due"].as_str().unwrap_or("")])?;
                tx.last_insert_rowid()
            };
            result["id"] = json!(assignment);
        }
        "archive" => {
            if tx.execute(
                "UPDATE courses SET archived=? WHERE id=?",
                params![
                    v["archived"].as_bool().ok_or("Missing archive choice")?,
                    id(v, "id")?
                ],
            )? != 1
            {
                return Err("Unknown course".into());
            }
        }
        "publish" | "enroll" | "remove" | "assign" => {
            let cid = id(v, "id")?;
            let draft: String = tx.query_row(
                "SELECT draft FROM courses WHERE id=? AND archived=0",
                [cid],
                |r| r.get(0),
            )?;
            match act {
                "publish" => {
                    outline(&serde_json::from_str(&draft)?, true)?;
                    tx.execute(
                        "INSERT INTO versions(course,outline) VALUES(?,?)",
                        params![cid, draft],
                    )?;
                }
                "enroll" => {
                    tx.execute("INSERT OR IGNORE INTO roster VALUES(?,'learner')", [cid])?;
                }
                "remove" => {
                    tx.execute(
                        "DELETE FROM roster WHERE course=? AND learner='learner'",
                        [cid],
                    )?;
                }
                _ => {
                    let ver = id(v, "version")?;
                    let saved:String=tx.query_row("SELECT outline FROM versions JOIN roster ON roster.course=versions.course WHERE versions.course=? AND id=? AND learner='learner'",params![cid,ver],|r|r.get(0))?;
                    let o: Value = serde_json::from_str(&saved)?;
                    let u = id(v, "unit")?;
                    let a = id(v, "activity")?;
                    if !o["units"][u as usize]["activities"][a as usize].is_object() {
                        return Err("Unknown published activity".into());
                    }
                    tx.execute("INSERT INTO assignments(version,unit_idx,activity_idx,learner,due) VALUES(?,?,?,'learner',?)",params![ver,u,a,string(v,"due")?])?;
                    result["id"] = tx.last_insert_rowid().into();
                }
            }
        }
        "due" => {
            if tx.execute(
                "UPDATE assignments SET due=? WHERE id=?",
                params![string(v, "due")?, id(v, "id")?],
            )? != 1
            {
                return Err("Unknown assignment".into());
            }
        }
        "study" | "submit" => {
            let aid = id(v, "id")?;
            let (status, active): (String, bool) = tx.query_row(
                "SELECT a.status, c.archived=0 AND EXISTS(
                   SELECT 1 FROM roster WHERE course=c.id AND learner=a.learner
                 ) FROM assignments a JOIN versions v ON v.id=a.version
                   JOIN courses c ON c.id=v.course WHERE a.id=? AND a.learner=?",
                params![aid, profile],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if act == "study" {
                if !active {
                    return Err("Course or enrollment is inactive".into());
                }
                tx.execute(
                    "UPDATE assignments SET status='In progress' WHERE id=? AND status='Assigned'",
                    [aid],
                )?;
            } else {
                let work = string(v, "work")?;
                let key = string(v, "key")?;
                if key.is_empty() || work.trim().is_empty() {
                    return Err("Choose work to submit".into());
                }
                let previous: Option<(i64, String)> = tx
                    .query_row(
                        "SELECT id,work FROM submissions WHERE assignment=? AND request=?",
                        params![aid, key],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                result["revision"] = if let Some((sid, old)) = previous {
                    if old != work {
                        return Err("Request already used for different work".into());
                    }
                    sid
                } else {
                    if !active || status == "Awaiting review" || status == "Accepted" {
                        return Err("Assignment is closed to submissions".into());
                    }
                    tx.execute(
                        "INSERT INTO submissions(assignment,request,work) VALUES(?,?,?)",
                        params![aid, key, work],
                    )?;
                    tx.execute(
                        "UPDATE assignments SET status='Awaiting review' WHERE id=?",
                        [aid],
                    )?;
                    tx.last_insert_rowid()
                }
                .into();
            }
        }
        "review" => {
            let aid = id(v, "id")?;
            let sid = id(v, "revision")?;
            let decision = string(v, "decision")?;
            let feedback = string(v, "feedback")?;
            if !matches!(decision, "Accepted" | "Revision requested")
                || (decision == "Revision requested" && feedback.trim().is_empty())
            {
                return Err("Choose a decision and explain the requested revision".into());
            }
            if tx.execute("UPDATE assignments SET status=? WHERE id=? AND status='Awaiting review' AND ?=(SELECT max(id) FROM submissions WHERE assignment=?)",params![decision,aid,sid,aid])?!=1{return Err("Revision is no longer awaiting review".into())}
            tx.execute(
                "INSERT INTO reviews VALUES(?,?,?)",
                params![sid, decision, feedback],
            )?;
        }
        _ => return Err("Unknown action".into()),
    }
    tx.commit()?;
    Ok(result)
}
fn main() -> R<()> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("robotutor-lms {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let port = env::var("ROBOTUTOR_PORT")
        .unwrap_or("47321".into())
        .parse::<u16>()?;
    let host = format!("127.0.0.1:{port}");
    let mut db = Connection::open(
        env::args()
            .nth(1)
            .ok_or("Expected database path and web directory")?,
    )?;
    db.execute_batch(include_str!("schema.sql"))?;
    let web = PathBuf::from(env::args().nth(2).ok_or("Expected web directory")?);
    let server = Server::http(&host).map_err(|e| e.to_string())?;
    for mut req in server.incoming_requests() {
        let header = |k: &'static str| {
            req.headers()
                .iter()
                .find(|h| h.field.equiv(k))
                .map(|h| h.value.as_str())
        };
        if header("Host") == Some(&host)
            && header("Origin").is_none_or(|o| o == format!("http://{host}"))
            && req.method() == &tiny_http::Method::Get
        {
            let asset = match req.url() {
                "/brand/Nunito.ttf" => Some(("Nunito.ttf", "font/ttf")),
                "/brand/robotutor-logo.svg" => Some(("robotutor-logo.svg", "image/svg+xml")),
                _ => None,
            };
            if let Some((name, mime)) = asset {
                match fs::File::open(web.join("brand").join(name)) {
                    Ok(file) => {
                        let _ = req.respond(
                            Response::from_file(file)
                                .with_header(Header::from_bytes("Content-Type", mime).unwrap()),
                        );
                    }
                    Err(_) => {
                        let _ = req.respond(Response::empty(404));
                    }
                }
                continue;
            }
        }
        let (code, mime, body) = if header("Host") != Some(&host)
            || header("Origin").is_some_and(|o| o != format!("http://{host}"))
        {
            (403, "text/plain", "Local origin required".into())
        } else if req.url() == "/api" && req.method() == &tiny_http::Method::Post {
            let profile = header("X-Profile").unwrap_or("").to_owned();
            let mut body = String::new();
            let packet = req
                .as_reader()
                .take(1_048_577)
                .read_to_string(&mut body)
                .ok()
                .filter(|_| body.len() <= 1_048_576)
                .and_then(|_| serde_json::from_str::<Value>(&body).ok());
            if profile == "teacher" && packet.as_ref().is_some_and(|v| v["action"] == "propose") {
                let database = env::args().nth(1).unwrap();
                let worker = web.with_file_name("desktop").join("course_agent.py");
                std::thread::spawn(move || {
                    let result = (|| -> R<Value> {
                        let mut connection = Connection::open(database)?;
                        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
                        proposals::generate(&mut connection, &packet.unwrap(), &worker)
                    })();
                    let (code, value) = match result {
                        Ok(v) => (200, v),
                        Err(e) => (400, json!({"error":e.to_string()})),
                    };
                    let _ = req.respond(
                        Response::from_string(value.to_string())
                            .with_status_code(StatusCode(code))
                            .with_header(
                                Header::from_bytes("Content-Type", "application/json").unwrap(),
                            ),
                    );
                });
                continue;
            }
            let result = packet
                .ok_or_else(|| "Invalid course request".into())
                .and_then(|v| action(&mut db, &profile, &v));
            match result {
                Ok(v) => (200, "application/json", v.to_string()),
                Err(e) => (
                    400,
                    "application/json",
                    json!({"error":e.to_string()}).to_string(),
                ),
            }
        } else {
            match req.url() {
                "/" => (
                    200,
                    "text/html",
                    fs::read_to_string(web.join("index.html"))?,
                ),
                "/app.js" => (
                    200,
                    "text/javascript",
                    fs::read_to_string(web.join("app.js"))?,
                ),
                "/style.css" => (200, "text/css", fs::read_to_string(web.join("style.css"))?),
                "/health" => (200, "text/plain", "robotutor-lms".into()),
                _ => (404, "text/plain", "Page not found".into()),
            }
        };
        let response = Response::from_string(body)
            .with_status_code(StatusCode(code))
            .with_header(
                Header::from_bytes("Content-Type", format!("{mime}; charset=utf-8")).unwrap(),
            );
        let _ = req.respond(response);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_revision_retry_and_permissions() -> R<()> {
        let mut db = Connection::open_in_memory()?;
        db.execute_batch(include_str!("schema.sql"))?;
        let course = json!({"title":"Music","destination":"Read rhythms","units":[{"title":"Time","activities":[{"title":"Meter","objective":"Count beats","instructions":"Mark accents","criterion":"Four beats"}]}]});
        action(
            &mut db,
            "teacher",
            &json!({"action":"save","outline":course}),
        )?;
        for a in ["publish", "enroll"] {
            action(&mut db, "teacher", &json!({"action":a,"id":1}))?;
        }
        action(
            &mut db,
            "teacher",
            &json!({"action":"assign","id":1,"version":1,"unit":0,"activity":0,"due":""}),
        )?;
        let mut revised = course.clone();
        revised["units"][0]["activities"][0]["criterion"] = "Six beats".into();
        action(
            &mut db,
            "teacher",
            &json!({"action":"save","id":1,"outline":revised}),
        )?;
        action(&mut db, "teacher", &json!({"action":"publish","id":1}))?;
        let student = state(&db, "learner")?;
        assert_eq!(
            student["assignments"][0]["activity"]["criterion"],
            "Four beats"
        );
        assert_eq!(
            student["courses"][0]["outline"]["units"][0]["activities"][0]["criterion"],
            "Six beats"
        );
        let submit = json!({"action":"submit","id":1,"work":"One two three four","key":"first"});
        let first = action(&mut db, "learner", &submit)?;
        assert_eq!(first, action(&mut db, "learner", &submit)?);
        let review = json!({"action":"review","id":1,"revision":first["revision"],"feedback":"Mark beat one","decision":"Revision requested"});
        assert!(action(&mut db, "learner", &review).is_err());
        assert!(action(&mut db, "teacher", &submit).is_err());
        action(&mut db, "teacher", &review)?;
        assert_eq!(first, action(&mut db, "learner", &submit)?);
        let second = action(
            &mut db,
            "learner",
            &json!({"action":"submit","id":1,"work":"ONE two three four","key":"second"}),
        )?;
        action(
            &mut db,
            "teacher",
            &json!({"action":"review","id":1,"revision":second["revision"],"decision":"Accepted","feedback":""}),
        )?;
        action(
            &mut db,
            "teacher",
            &json!({"action":"archive","id":1,"archived":true}),
        )?;
        let a = &state(&db, "teacher")?["assignments"][0];
        assert_eq!(a["submissions"].as_array().unwrap().len(), 2);
        assert_eq!(a["status"], "Accepted");
        assert_eq!(*a, state(&db, "learner")?["assignments"][0]);
        Ok(())
    }
}
