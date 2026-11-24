"""Capture visible Robotutor dialogue and publish timelines and reviewed Markdown transcripts."""
import argparse
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import json
import os
import re
from pathlib import Path
import sqlite3
import subprocess
import sys
import threading
import time
from uuid import UUID

from append_transcript import append
from read_conversation import request
from local_platform import quiet_process
import verbatim_archive as verbatim

ARCHIVE = "https://github.com/CameronWeller/robotutor-transcripts/blob/main/"
ROLES = {"UserMessage": "User", "AgentMessage": "Assistant", "Plan": "Assistant plan"}
PHASES = {"commentary": "Assistant progress", "final_answer": "Assistant reply"}


def tag(thread):
    return hashlib.sha256(("robotutor/codex/" + str(UUID(thread))).encode()).hexdigest()[:24]


def connect(folder):
    folder.mkdir(mode=0o700, parents=True, exist_ok=True)
    db = sqlite3.connect(folder / "capture.sqlite3", timeout=15)
    db.row_factory = sqlite3.Row
    db.executescript("""
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS pins (id TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY, tag TEXT UNIQUE, source TEXT NOT NULL, archive TEXT UNIQUE,
            cursor INTEGER DEFAULT 0, indexed INTEGER DEFAULT 0, pending INTEGER DEFAULT 0);
        CREATE TABLE IF NOT EXISTS messages (
            seq INTEGER PRIMARY KEY, session TEXT NOT NULL, item TEXT NOT NULL,
            stamp TEXT NOT NULL, role TEXT NOT NULL, body TEXT NOT NULL, sent INTEGER DEFAULT 0,
            UNIQUE(session,item));
        CREATE INDEX IF NOT EXISTS pending_messages ON messages(session,sent,seq);
    """)
    verbatim.schema(db)
    with db:
        db.execute("INSERT OR IGNORE INTO config VALUES ('since', ?)", (str(time.time()),))
    return db


def worktrees(repo):
    raw = subprocess.check_output(["git", "-C", repo, "worktree", "list", "--porcelain", "-z"], **quiet_process())
    return {Path(os.fsdecode(field[9:])).resolve() for field in raw.split(b"\0")
            if field.startswith(b"worktree ")}


def discover(repo, db):
    roots = worktrees(repo)
    pins = {r[0] for r in db.execute("SELECT id FROM pins")}
    home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex")).resolve()
    proc = subprocess.Popen(["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, encoding="utf-8", **quiet_process())
    timer = threading.Timer(25, proc.kill)
    timer.start()
    try:
        request(proc, 1, "initialize", {"clientInfo": {"name": "robotutor_archive", "version": "1"},
                "capabilities": {"experimentalApi": True}})
        number = 1
        for archived in (False, True):
            cursor = None
            while True:
                number += 1
                result = request(proc, number, "thread/list", {"archived": archived, "useStateDbOnly": True,
                    "currentAccountOnly": True, "sortKey": "updated_at", "limit": 100,
                    "cursor": cursor})
                for t in result["data"]:
                    remote = (t.get("gitInfo") or {}).get("originUrl") or ""
                    project = remote.removesuffix(".git").lower() in (
                        "https://github.com/cameronweller/robotutor", "git@github.com:cameronweller/robotutor")
                    if t["id"] not in pins and Path(t["cwd"]).resolve() not in roots and not project:
                        continue
                    if not t.get("path"):
                        continue
                    source = Path(t["path"]).resolve()
                    if not any(source.is_relative_to(home / p) for p in ("sessions", "archived_sessions")):
                        continue
                    ident = tag(t["id"])
                    if not source.name.endswith(t["id"] + ".jsonl"):
                        continue
                    day = datetime.fromtimestamp(t["createdAt"], timezone.utc).strftime("%Y/%m/%d")
                    path = f"logs/{day}/codex-session-{ident}.md"
                    with db:
                        db.execute("""INSERT INTO sessions(id,tag,source,archive) VALUES (?,?,?,?)
                            ON CONFLICT(id) DO UPDATE SET source=excluded.source""",
                            (t["id"], ident, str(source), path))
                cursor = result.get("nextCursor")
                if not cursor:
                    break
    finally:
        timer.cancel()
        proc.kill()
        proc.wait()


def visible(record, thread, since):
    if record.get("type") != "event_msg":
        return
    event = record.get("payload", {})
    if event.get("type") != "item_completed" or event.get("thread_id") != thread:
        return
    item = event.get("item", {})
    kind = item.get("type")
    if kind not in ROLES or not item.get("id"):
        return
    timestamp = datetime.fromisoformat(record["timestamp"].replace("Z", "+00:00"))
    if timestamp.tzinfo is None or timestamp.timestamp() < since:
        return
    role = ROLES[kind]
    if kind == "Plan":
        body = item.get("text", "")
    else:
        if kind == "AgentMessage":
            if item.get("phase") not in (None, *PHASES):
                return
            role = PHASES.get(item.get("phase"), role)
        body = "\n".join(c["text"] if c.get("type") in ("Text", "text") and isinstance(c.get("text"), str)
                         else "[Non-text content omitted]" for c in item.get("content", []))
    if body:
        return str(item["id"]), timestamp.astimezone(timezone.utc).isoformat(timespec="milliseconds"), role, body


def ingest(db, session, since):
    path = Path(session["source"])
    if not path.is_file():
        return
    with path.open("rb") as stream, db:
        cursor = session["cursor"]
        stream.seek(cursor if path.stat().st_size >= cursor else 0)
        while True:
            start = stream.tell()
            line = stream.readline()
            if not line.endswith(b"\n"):
                break
            event = visible(json.loads(line), session["id"], since)
            if event:
                db.execute("INSERT OR IGNORE INTO messages(session,item,stamp,role,body) VALUES (?,?,?,?,?)",
                           (session["id"], *event))
        db.execute("UPDATE sessions SET cursor=? WHERE id=?", (start, session["id"]))


def publish_batch(repo, db, session):
    sid = session["id"]
    with db:
        end = session["pending"] or db.execute(
            "SELECT max(seq) FROM (SELECT seq FROM messages WHERE session=? AND sent=0 ORDER BY seq LIMIT 100)",
            (sid,)).fetchone()[0]
        if not end:
            return False
        db.execute("UPDATE sessions SET pending=? WHERE id=?", (end, sid))
    rows = db.execute("SELECT seq,stamp,role FROM messages WHERE session=? AND sent=0 AND seq<=? ORDER BY seq",
                      (sid, end)).fetchall()
    path, ident = session["archive"], session["tag"]
    if not session["indexed"]:
        append(repo, path, f"# Codex development session {ident}\n\n"
               "Automatic timeline of visible harness events since enrollment. "
               "Message wording is available for local review.\n", "session-" + ident)
        append(repo, "logs/coverage.md", f"- [Codex development session {ident}]({path[5:]})\n",
               "index-" + ident)
    timeline = "\n".join(f"- {row['stamp']} — {row['role']}" for row in rows)
    append(repo, path, timeline + "\n", f"events-{ident}-{rows[0]['seq']}-{end}")
    with db:
        db.execute("UPDATE messages SET sent=1 WHERE session=? AND seq<=?", (sid, end))
        db.execute("UPDATE sessions SET indexed=1,pending=0 WHERE id=?", (sid,))
    return True


def health(db, key, value):
    with db:
        db.execute("INSERT OR REPLACE INTO config VALUES (?,?)", (key, value))


def publisher(repo, folder):
    with closing(connect(folder)) as db:
        while True:
            sessions = db.execute("""SELECT * FROM sessions WHERE id IN
                (SELECT session FROM messages WHERE sent=0) ORDER BY rowid""").fetchall()
            errors = []
            for session in sessions:
                try:
                    publish_batch(repo / "logs", db, session)
                except Exception as exc:
                    errors.append(type(exc).__name__)
            health(db, "publish_status", ", ".join(errors) or "ok")
            time.sleep(20 if errors else 5)


def watch(repo, folder):
    from local_platform import lock_file
    with closing(connect(folder)) as db, (folder / "watch.lock").open("a") as lock:
        lock_file(lock)
        # Background authentication must fail into the durable queue instead of opening UI.
        os.environ.update(GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="0", GCM_GUI_PROMPT="0")
        os.environ.pop("GIT_ASKPASS", None)
        os.environ.pop("SSH_ASKPASS", None)
        threading.Thread(target=publisher, args=(repo, folder), daemon=True).start()
        since = float(db.execute("SELECT value FROM config WHERE key='since'").fetchone()[0])
        next_scan = 0
        while True:
            if (folder / "stop").exists():
                health(db, "heartbeat", "stopped")
                return
            if time.monotonic() >= next_scan:
                try:
                    discover(repo, db)
                    health(db, "discovery_status", "ok")
                except Exception as exc:
                    health(db, "discovery_status", type(exc).__name__)
                next_scan = time.monotonic() + 25
            errors = []
            for session in db.execute("SELECT * FROM sessions").fetchall():
                try:
                    ingest(db, session, since)
                    verbatim.ingest_history(db, session)
                except (OSError, ValueError, KeyError) as exc:
                    errors.append(type(exc).__name__)
            health(db, "capture_status", ", ".join(errors) or "ok")
            health(db, "heartbeat", datetime.now(timezone.utc).isoformat(timespec="seconds"))
            time.sleep(2)


def trailer(folder, message, source):
    thread = os.environ.get("CODEX_THREAD_ID")
    if not thread or source in ("commit", "merge", "squash"):
        return
    ident = tag(thread)
    with closing(connect(folder)) as db:
        row = db.execute("SELECT archive FROM sessions WHERE tag=?", (ident,)).fetchone()
    # A commit can precede discovery on a new thread. The tag remains searchable in the archive.
    key, value = ("Session-Log", ARCHIVE + row[0].removeprefix("logs/")) if row else ("Robotutor-Session", ident)
    subprocess.run(["git", "-c", "trailer.ifExists=doNothing", "interpret-trailers", "--in-place",
                    "--trailer", f"{key}: {value}", str(message)], check=True, **quiet_process())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--state", type=Path)
    cmds = parser.add_subparsers(dest="action", required=True)
    cmds.add_parser("watch")
    cmds.add_parser("start")
    cmds.add_parser("stop")
    cmds.add_parser("status")
    restore = cmds.add_parser("restore")
    restore.add_argument("session")
    restore.add_argument("file", type=Path)
    restore.add_argument("--identity", type=Path, required=True)
    restore.add_argument("--jsonl", action="store_true", help="Write exact text parts and event boundaries")
    pin = cmds.add_parser("include-thread")
    pin.add_argument("thread", type=UUID)
    for name in ("export", "publish-reviewed"):
        p = cmds.add_parser(name)
        p.add_argument("session", help="Public session tag from status")
        p.add_argument("file", type=Path)
        if name == "publish-reviewed":
            p.add_argument("--entry", required=True)
    hook = cmds.add_parser("trailer")
    hook.add_argument("message", type=Path)
    hook.add_argument("source", nargs="?", default="")
    args = parser.parse_args()
    if args.action == "restore":
        if not re.fullmatch(r"[a-f0-9]{24}", args.session):
            parser.error("Invalid session tag")
        ref = "refs/remotes/origin/conversation-logs"
        paths = subprocess.check_output(["git", "-C", args.repo, "ls-tree", "-r", "--name-only", ref,
                                        "logs"], text=True, **quiet_process()).splitlines()
        matches = [p for p in paths if p.endswith(f"/codex-session-{args.session}-verbatim.md")]
        if len(matches) != 1:
            parser.error("Fetch conversation-logs and supply a published session tag")
        saved = subprocess.check_output(["git", "-C", args.repo, "show", ref + ":" + matches[0]],
                                        encoding="utf-8", **quiet_process())
        events = verbatim.recover(saved, args.session, args.identity)
        if args.jsonl:
            with args.file.open("x", encoding="utf-8", newline="\n") as out:
                for event in events:
                    out.write(json.dumps(event, ensure_ascii=False) + "\n")
        else:
            verbatim.export(events, args.file)
        return
    if args.state is None:
        parser.error("--state is required for local recorder commands")
    for name, value in (("GIT_AUTHOR_NAME", "Robotutor Session Archive"), ("GIT_AUTHOR_EMAIL", "archive@localhost"),
                        ("GIT_COMMITTER_NAME", "Robotutor Session Archive"), ("GIT_COMMITTER_EMAIL", "archive@localhost")):
        os.environ.setdefault(name, value)
    if args.action == "watch":
        watch(args.repo, args.state)
        return
    if args.action == "stop":
        (args.state / "stop").touch()
        return
    if args.action == "start":
        if (args.state / "stop").exists():
            return
        # pythonw is only the login launcher. Give the worker a hidden console so
        # grandchildren such as Git's credential helper inherit a noninteractive console.
        interpreter = Path(sys.executable).with_name("python.exe") if os.name == "nt" else sys.executable
        subprocess.Popen([interpreter, str(Path(__file__).resolve()), "--repo", str(args.repo),
                          "--state", str(args.state), "watch"], stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **quiet_process())
        return
    if args.action == "trailer":
        trailer(args.state, args.message, args.source)
        return
    with closing(connect(args.state)) as db:
        if args.action == "include-thread":
            with db:
                db.execute("INSERT OR IGNORE INTO pins VALUES (?)", (str(args.thread),))
        elif args.action == "status":
            for key, value in db.execute("SELECT key,value FROM config ORDER BY key"):
                print(key, value)
            for row in db.execute("""SELECT tag,count(seq),sum(CASE WHEN sent=0 THEN 1 ELSE 0 END)
                FROM sessions JOIN messages ON session=id GROUP BY tag"""):
                print(row[0], "captured", row[1], "awaiting receipt", row[2])
            for row in db.execute("""SELECT tag,count(pos),sum(1-backed) FROM sessions JOIN conversation_events
                    ON session=id GROUP BY tag"""):
                print(row[0], "verbatim events", row[1], "stored locally for review", row[2])
        else:
            row = db.execute("SELECT id,archive FROM sessions WHERE tag=?", (args.session,)).fetchone()
            if not row:
                parser.error("Unknown session tag")
            if args.action == "publish-reviewed":
                print(append(args.repo / "logs", row["archive"], args.file.read_text(encoding="utf-8"), args.entry))
            else:
                events = [json.loads(r[0]) for r in db.execute(
                    "SELECT data FROM conversation_events WHERE session=? ORDER BY pos", (row["id"],))]
                if events:
                    verbatim.export(events, args.file)
                else:
                    # Preserve access to records made by older installations when their source no longer exists.
                    with args.file.open("x", encoding="utf-8", newline="") as out:
                        for stamp, role, body in db.execute("SELECT stamp,role,body FROM messages WHERE session=? ORDER BY seq",
                                                           (row["id"],)):
                            out.write(f"**{role}** ({stamp})\n\n{body}\n\n")


if __name__ == "__main__":
    main()
