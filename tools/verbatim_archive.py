"""Store ordered visible conversation records and their age-encrypted Git backups."""
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess

from append_transcript import append
from local_platform import quiet_process

ARMOR = re.compile(r"-----BEGIN AGE ENCRYPTED FILE-----.*?-----END AGE ENCRYPTED FILE-----", re.S)
DIALOGUE = {"UserMessage": "user", "AgentMessage": "assistant", "Plan": "assistant plan"}
TOOLS = {"McpToolCall", "CommandExecution", "FileChange", "ImageView", "WebSearch", "Extension",
         "SubAgentActivity", "DynamicToolCall", "CollabAgentToolCall"}
CONTROLS = {"request_user_input", "request_user_input_async", "send_message_to_user_async"}


def schema(db):
    db.executescript("""
        CREATE TABLE IF NOT EXISTS journal_offsets(session TEXT PRIMARY KEY, pos INTEGER NOT NULL,
            turn TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS conversation_events(session TEXT, pos INTEGER, data TEXT NOT NULL,
            backed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(session,pos));
        CREATE INDEX IF NOT EXISTS unsaved_events ON conversation_events(backed,session,pos);
        CREATE TABLE IF NOT EXISTS question_calls(session TEXT, call TEXT, PRIMARY KEY(session,call));
        CREATE TABLE IF NOT EXISTS sealed_chunks(session TEXT, first INTEGER, last INTEGER, part INTEGER,
            armor TEXT NOT NULL, PRIMARY KEY(session,first,part));
    """)


def text_parts(parts):
    # Preserve individual text boundaries and whitespace. Do not serialize binary attachments.
    return [p["text"] if isinstance(p.get("text"), str) else {"omitted": "non-text attachment"}
            for p in parts if isinstance(p, dict)]


def question_payload(args):
    if "message" in args:
        return {"message": args["message"]}
    return {"questions": [{key: val for key, val in q.items()
                           if key in ("id", "header", "question", "title", "options")}
                          for q in args.get("questions", [])]}


def extract(db, sid, record):
    source, obj = record.get("type"), record.get("payload", {})
    kind = obj.get("type")
    if source == "event_msg":
        if obj.get("thread_id", sid) != sid:
            return
        if kind == "item_completed":
            item = obj["item"]
            kind = item["type"]
            if kind in DIALOGUE:
                parts = [item["text"]] if kind == "Plan" else text_parts(item.get("content", []))
                return {"role": DIALOGUE[kind], "parts": parts, "source": "visible-item",
                        "phase": item.get("phase"), "id": item.get("id")}
            if kind in TOOLS:
                # The description uses the harness type. Tool arguments and output can be large or private.
                return {"role": "tool", "summary": kind, "status": item.get("status")}
        elif kind in ("user_message", "agent_message") and isinstance(obj.get("message"), str):
            return {"role": "user" if kind == "user_message" else "assistant",
                    "parts": [obj["message"]], "source": "legacy-visible"}
    if source != "response_item":
        return
    if kind == "message" and obj.get("role") in ("assistant", "user"):
        parts = text_parts(obj.get("content", []))
        role = obj["role"]
        if role == "assistant" and (obj.get("channel") in ("analysis", "internal") or
                                    obj.get("phase") not in (None, "commentary", "final_answer")):
            return
        # Normal user text comes from the UI event. The model-input copy can also contain
        # system-provided context. Only the explicit choice-reply wrapper is user dialogue here.
        if role == "user" and not any(isinstance(s, str) and s.lstrip().startswith(
                "<send_user_message_question_reply>") for s in parts):
            return
        return {"role": role, "parts": parts, "source": "message-copy", "phase": obj.get("phase")}
    if kind == "function_call" and obj.get("name", "").split(".")[-1] in CONTROLS:
        args = json.loads(obj["arguments"])
        db.execute("INSERT OR IGNORE INTO question_calls VALUES (?,?)", (sid, obj["call_id"]))
        return {"role": "assistant control", "control": question_payload(args)}
    if kind == "function_call_output" and db.execute(
            "SELECT 1 FROM question_calls WHERE session=? AND call=?", (sid,obj.get("call_id"))).fetchone():
        try:
            answer = json.loads(obj["output"])
        except (ValueError, TypeError):
            return
        if isinstance(answer, dict) and "answers" in answer:
            return {"role": "user control", "answers": answer["answers"]}


def ingest_history(db, session):
    sid = session["id"]
    saved = db.execute("SELECT pos,turn FROM journal_offsets WHERE session=?", (sid,)).fetchone()
    pos, turn = saved if saved else (0, "")
    path = Path(session["source"])
    with path.open("rb") as stream, db:
        if path.stat().st_size < pos:
            pos, turn = 0, ""
        stream.seek(pos)
        while True:
            offset = stream.tell()
            line = stream.readline()
            if not line.endswith(b"\n"):
                break
            rec = json.loads(line)
            obj = rec.get("payload", {})
            if rec.get("type") == "event_msg" and obj.get("type") == "task_started":
                turn = obj.get("turn_id", "")
            event = extract(db, sid, rec)
            if event is None:
                continue
            event.update(offset=offset, turn=obj.get("turn_id") or turn, timestamp=rec.get("timestamp"))
            data = json.dumps(event, ensure_ascii=False, separators=(",", ":"))
            existing = db.execute("SELECT data FROM conversation_events WHERE session=? AND pos=?",
                                  (sid,offset)).fetchone()
            if existing and existing[0] != data:
                raise ValueError("Journal changed at a previously captured source position")
            if not existing:
                db.execute("INSERT INTO conversation_events(session,pos,data) VALUES (?,?,?)", (sid,offset,data))
        db.execute("INSERT OR REPLACE INTO journal_offsets VALUES (?,?,?)", (sid,offset,turn))


def age(*args, data=None):
    return subprocess.run(["age", *args], input=data, capture_output=True,
                          check=True, **quiet_process()).stdout


def identity(folder, db):
    key = folder.parent / "recovery" / "robotutor.agekey"
    key.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    known = db.execute("SELECT value FROM config WHERE key='age_recipient'").fetchone()
    if not key.exists():
        if known:
            raise RuntimeError("Restore the original recovery key before continuing")
        subprocess.run(["age-keygen", "-o", str(key)], check=True, capture_output=True, **quiet_process())
    recipient = subprocess.check_output(["age-keygen", "-y", str(key)], text=True, stderr=subprocess.PIPE,
                                        **quiet_process()).strip()
    if known and known[0] != recipient:
        raise RuntimeError("Recovery key does not match the enrolled recipient")
    with db:
        db.execute("INSERT OR IGNORE INTO config VALUES ('age_recipient',?)", (recipient,))
    return key, recipient


def encrypted_path(session):
    return session["archive"].removesuffix(".md") + "-verbatim.md"


def seal(db, session, recipient):
    sid = session["id"]
    if db.execute("SELECT 1 FROM sealed_chunks WHERE session=?", (sid,)).fetchone():
        return
    rows = db.execute("""SELECT pos,data FROM conversation_events WHERE session=? AND backed=0
                         ORDER BY pos LIMIT 2000""", (sid,)).fetchall()
    if not rows:
        return
    packed = gzip.compress(("\n".join(r[1] for r in rows) + "\n").encode("utf-8"), mtime=0)
    digest = hashlib.sha256(packed).hexdigest()
    size = 240_000  # age armor stays below the append service limit even for incompressible input.
    count = (len(packed) + size - 1) // size
    first, last = rows[0][0], rows[-1][0]
    with db:
        for part in range(count):
            head = json.dumps([1,session["tag"],first,last,part,count,digest]).encode() + b"\n"
            armor = age("-a", "-r", recipient, data=head + packed[part*size:(part+1)*size]).decode("ascii")
            db.execute("INSERT INTO sealed_chunks VALUES (?,?,?,?,?)", (sid,first,last,part,armor))


def publish_encrypted(repo, db, session, recipient):
    sid, tag = session["id"], session["tag"]
    seal(db, session, recipient)
    chunks = db.execute("SELECT first,last,part,armor FROM sealed_chunks WHERE session=? ORDER BY part",
                        (sid,)).fetchall()
    if not chunks:
        return
    path = encrypted_path(session)
    for first,last,part,armor in chunks:
        append(repo, path, "```text\n" + armor.rstrip() + "\n```\n",
               f"sealed-{tag}-{first}-{last}-{part}")
    if not db.execute("SELECT 1 FROM config WHERE key=?", ("linked-"+tag,)).fetchone():
        append(repo, session["archive"], f"[Encrypted verbatim conversation]({Path(path).name})\n",
               "verbatim-link-" + tag)
        append(repo, "logs/coverage.md", f"- [Codex development session {tag}]({session['archive'][5:]})\n",
               "index-" + tag)
    with db:
        db.execute("INSERT OR REPLACE INTO config VALUES (?,?)", ("linked-"+tag, "1"))
        db.execute("UPDATE conversation_events SET backed=1 WHERE session=? AND pos BETWEEN ? AND ?",
                   (sid,first,last))
        db.execute("DELETE FROM sealed_chunks WHERE session=?", (sid,))


def recover(contents, tag, key):
    groups = {}
    for armor in ARMOR.findall(contents):
        head, payload = age("-d", "-i", str(key), data=armor.encode()+b"\n").split(b"\n",1)
        version, ident, first, last, part, count, digest = json.loads(head)
        if version != 1 or ident != tag or not 0 <= part < count:
            raise ValueError("Invalid encrypted session header")
        parts = groups.setdefault((first,last,count,digest), {})
        if part in parts:
            raise ValueError("Duplicate encrypted part")
        parts[part] = payload
    if not groups:
        raise ValueError("No encrypted transcript in the supplied archive")
    events = []
    for (first,last,count,digest),parts in sorted(groups.items()):
        if len(parts) != count:
            raise ValueError("Incomplete encrypted batch")
        packed = b"".join(parts[i] for i in range(count))
        if hashlib.sha256(packed).hexdigest() != digest:
            raise ValueError("Encrypted batch digest mismatch")
        batch = [json.loads(line) for line in gzip.decompress(packed).splitlines()]
        offsets = [e["offset"] for e in batch]
        if not offsets or offsets[0] != first or offsets[-1] != last or offsets != sorted(set(offsets)):
            raise ValueError("Invalid source ordering")
        if events and events[-1]["offset"] >= first:
            raise ValueError("Overlapping encrypted batches")
        events.extend(batch)
    return events


def dialogue(events):
    # The harness can emit both a UI item and a model-input/output copy. Pair equal
    # occurrences within a turn; preserve repetitions and differing source wording.
    def key(e):
        return e.get("turn"), e["role"], json.dumps(e.get("parts"), ensure_ascii=False)
    canonical = Counter(key(e) for e in events if e.get("source") == "visible-item")
    copies = Counter()
    for e in events:
        if e.get("source") in ("message-copy", "legacy-visible"):
            pair = (e["source"],key(e))
            copies[pair] += 1
            if copies[pair] <= canonical[key(e)]:
                continue
        yield e


def export(events, output):
    with Path(output).open("x", encoding="utf-8", newline="") as out:
        for e in dialogue(events):
            role = e["role"]
            if role == "tool":
                text = e["summary"] + (" (" + str(e["status"]) + ")" if e["status"] else "")
            elif "parts" in e:
                text = "\n".join(p if isinstance(p,str) else "[Non-text attachment]" for p in e["parts"])
            else:
                text = json.dumps(e.get("control", e.get("answers")), ensure_ascii=False, indent=2)
            out.write(f"**{role}** ({e['timestamp']})\n\n{text}\n\n")
