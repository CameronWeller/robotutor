#!/usr/bin/env python3
"""Archive Claude Code dialogue in the private logs submodule with every Robotutor commit.

Each visible message becomes one entry, keyed by its Claude Code message ID, in
logs/YYYY/MM/DD/claude-session-TAG.md. Entries are appended through the transcript appender, so a
published entry is never rewritten. Before each commit the logs submodule is fast-forwarded to the
archive commit that holds the session so far, and the gitlink is staged with the code change.
"""
import argparse
from datetime import datetime, timezone
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from append_transcript import publish_many, validate

MARKER = "# robotutor-claude-transcripts"
ARCHIVE_FORMAT = "robotutor-development-transcripts-v1"
SESSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{7,127}")
ENTRY_LIMIT = 400_000  # Characters. Longer messages are split below the appender's 512 KB limit.
BATCH_LIMIT = 4_000_000

# The archive keeps credentials out (logs/README.md). Replace them before anything is written.
SECRETS = [
    ("private-key", re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----", re.S)),
    ("age-secret-key", re.compile(r"AGE-SECRET-KEY-1[0-9A-Za-z]{20,}")),
    ("github-token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{20,})")),
    ("anthropic-key", re.compile(r"\bsk-ant-[A-Za-z0-9_-]{16,}")),
    ("openai-key", re.compile(r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{32,}")),
    ("aws-access-key", re.compile(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b")),
    ("slack-token", re.compile(r"\bxox[abposr]-[A-Za-z0-9-]{10,}")),
    ("google-api-key", re.compile(r"\bAIza[0-9A-Za-z_-]{35}")),
    ("google-oauth", re.compile(r"\b(?:ya29\.[0-9A-Za-z_-]{20,}|GOCSPX-[0-9A-Za-z_-]{20,})")),
    ("stripe-key", re.compile(r"\b[rs]k_(?:live|test)_[0-9A-Za-z]{16,}")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")),
]
KEEP_PREFIX = [
    ("authorization", re.compile(r"(?i)(\bauthorization\s*[:=]\s*[\"']?(?:bearer|basic|token)\s+)[A-Za-z0-9._~+/=-]{12,}")),
    ("url-credentials", re.compile(r"(\b[a-z][a-z0-9+.-]*://)[^/\s:@\"']+:[^/\s@\"']+(?=@)")),
    ("assigned-secret", re.compile(
        r"(?i)(\b[A-Z0-9_.-]*(?:secret|passw(?:or)?d|token|api[_-]?key|private[_-]?key|credential)[A-Z0-9_.-]*"
        r"[\"']?\s*[:=]\s*[\"']?)(?=[A-Za-z0-9+/=_.-]*[0-9])(?=[A-Za-z0-9+/=_.-]*[A-Za-z])[A-Za-z0-9+/=_.-]{16,}")),
]


def redact(text):
    for kind, pattern in SECRETS:
        text = pattern.sub(f"[REDACTED:{kind}]", text)
    for kind, pattern in KEEP_PREFIX:
        text = pattern.sub(lambda m: m.group(1) + f"[REDACTED:{kind}]", text)
    return text.replace("<!-- transcript-entry ", "<!-- transcript-entry​ ")


def tag(session):
    return hashlib.sha256(("robotutor/claude/" + session).encode()).hexdigest()[:24]


def summary(block):
    """One line for a tool call. Arguments and results stay out, as in the Codex recorder."""
    name = block.get("name", "tool")
    args = block.get("input") or {}
    if name.endswith("post_message") and isinstance(args.get("text"), str):
        return None  # Rendered as a visible reply instead.
    detail = args.get("description") if isinstance(args.get("description"), str) else ""
    return f"Tool call: {name}" + (f" ({detail})" if detail else "")


def visible(record):
    """Return (role, body) for a record a person saw or wrote, or None."""
    if record.get("isSidechain") or record.get("isMeta") or record.get("isCompactSummary"):
        return None
    kind = record.get("type")
    content = (record.get("message") or {}).get("content")
    if kind == "user":
        if isinstance(content, str):
            return "User", content
        texts = [b.get("text", "") for b in content or [] if isinstance(b, dict) and b.get("type") == "text"]
        return ("User", "\n".join(texts)) if texts else None
    if kind == "assistant" and isinstance(content, list):
        parts = []
        for b in content:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "text":
                parts.append(b.get("text", ""))
            elif b.get("type") == "tool_use":
                args = b.get("input") or {}
                if b.get("name", "").endswith("post_message") and isinstance(args.get("text"), str):
                    parts.append("Reply to the user:\n\n" + args["text"])
                else:
                    parts.append(summary(b))
        parts = [p for p in parts if p]
        return ("Assistant", "\n\n".join(parts)) if parts else None
    return None


def read_messages(path, start=0):
    """Yield (end offset, message ID, timestamp, role, body) for complete lines after start."""
    with Path(path).open("rb") as stream:
        stream.seek(start)
        offset = start
        for line in stream:
            if not line.endswith(b"\n"):
                return  # Claude Code is still writing this line.
            offset += len(line)
            try:
                record = json.loads(line)
            except ValueError:
                continue
            found = visible(record)
            ident = record.get("uuid")
            if found and found[1].strip() and ident and re.fullmatch(r"[A-Za-z0-9-]{8,80}", ident):
                yield offset, ident, record.get("timestamp", ""), found[0], found[1]


def entries_for(ident, stamp, role, body):
    text = redact(body.rstrip())
    pieces = [text[i:i + ENTRY_LIMIT] for i in range(0, len(text), ENTRY_LIMIT)] or [""]
    for number, piece in enumerate(pieces):
        label = role if len(pieces) == 1 else f"{role}, part {number + 1} of {len(pieces)}"
        yield f"**{label}** ({stamp})\n\n{piece}\n", f"m-{ident}" + (f"-{number}" if number else "")


RAW_PART_LIMIT = 40_000_000  # Bytes per raw record file, below GitHub's 50 MB warning.
RAW_ENTRY = re.compile(r"<!-- transcript-entry (r-[0-9a-f]{8}-[0-9]+(?:-[0-9]+)?) [0-9a-f]{64} -->\n"
                       r"(\S+) @ ([0-9]+)(?: part [0-9]+ of [0-9]+)?\n\n(`{3,})jsonl\n(.*?)\n\4\n", re.S)


def redact_value(value):
    if isinstance(value, str):
        return redact(value)
    if isinstance(value, list):
        return [redact_value(v) for v in value]
    if isinstance(value, dict):
        return {redact(k): redact_value(v) for k, v in value.items()}
    return value


def redact_raw(line):
    """Redact one JSONL record. Lines without a match keep their exact bytes."""
    text = line.decode("utf-8", errors="replace").rstrip("\r\n")
    try:
        record = json.loads(text)
    except ValueError:
        return redact(text)
    cleaned = redact_value(record)
    return text if cleaned == record else json.dumps(cleaned, ensure_ascii=False, separators=(",", ":"))


def raw_sources(main):
    """The main transcript and the subagent transcripts Claude Code keeps beside it."""
    found = [("main", main)]
    folder = main.with_suffix("")
    if folder.is_dir():
        found += [(p.relative_to(folder).as_posix(), p) for p in sorted(folder.rglob("*.jsonl"))]
    return found


def raw_entries(name, path, start):
    """Yield (end offset, [(content, entry)]) for each complete source line after start."""
    key = hashlib.sha256(name.encode()).hexdigest()[:8]
    label = re.sub(r"\s", "_", name)
    with Path(path).open("rb") as stream:
        stream.seek(start)
        offset = start
        for line in stream:
            if not line.endswith(b"\n"):
                return
            text = redact_raw(line)
            fence = "`" * max(3, max((len(m) for m in re.findall(r"`+", text)), default=0) + 1)
            pieces = [text[i:i + ENTRY_LIMIT] for i in range(0, len(text), ENTRY_LIMIT)] or [""]
            out = []
            for number, piece in enumerate(pieces):
                part = f" part {number + 1} of {len(pieces)}" if len(pieces) > 1 else ""
                out.append((f"{label} @ {offset}{part}\n\n{fence}jsonl\n{piece}\n{fence}\n",
                            f"r-{key}-{offset}" + (f"-{number}" if number else "")))
            offset += len(line)
            yield offset, out


def restore_raw(records, output):
    """Rebuild one JSONL file per source from raw record files, in source order."""
    lines = {}
    for record in records:
        for _, name, offset, _, piece in RAW_ENTRY.findall(Path(record).read_text(encoding="utf-8")):
            lines.setdefault(name, {}).setdefault(int(offset), []).append(piece)
    output.mkdir(parents=True, exist_ok=True)
    written = []
    for name, rows in sorted(lines.items()):
        target = output / (name.replace("/", "__").removesuffix(".jsonl") + ".jsonl")
        with target.open("w", encoding="utf-8", newline="\n") as out:
            for offset in sorted(rows):
                out.write("".join(rows[offset]) + "\n")
        written.append(target)
    return written


PARENT = {}  # The index a running git commit uses, which may be a temporary one for path commits.


def isolate_from_hook():
    """Git exports GIT_DIR and GIT_INDEX_FILE to hooks. Keep them for the code repo, not the submodule."""
    if os.environ.get("GIT_INDEX_FILE"):
        PARENT["GIT_INDEX_FILE"] = os.path.abspath(os.environ["GIT_INDEX_FILE"])
    names = subprocess.run(["git", "rev-parse", "--local-env-vars"], capture_output=True, text=True).stdout
    for name in names.split():
        os.environ.pop(name, None)


def git(repo, *args, check=True):
    env = dict(os.environ, **PARENT) if PARENT and Path(repo).name != "logs" else None
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True,
                          check=check, env=env).stdout.strip()


def state_file(repo):
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    folder = common / "claude-transcripts"
    folder.mkdir(mode=0o700, exist_ok=True)
    return folder / "sessions.json"


def load_state(repo):
    path = state_file(repo)
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}


def save_state(repo, state):
    path = state_file(repo)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, indent=1, sort_keys=True), encoding="utf-8")
    os.replace(tmp, path)


def find_transcript(session):
    home = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude")
    matches = glob.glob(str(home / "projects" / "*" / (session + ".jsonl")))
    return matches[0] if len(matches) == 1 else None


def register(repo, session, transcript=None):
    """Remember a session so that a later commit from an ordinary terminal still captures it."""
    if not session or not SESSION.fullmatch(session):
        return
    transcript = transcript or find_transcript(session)
    if not transcript:
        return
    state = load_state(repo)
    row = state.setdefault(session, {"offset": 0, "indexed": False})
    if row.get("transcript") != str(transcript):
        row["transcript"] = str(transcript)
        save_state(repo, state)


def archive_ready(repo):
    archive = repo / "logs"
    return (archive / ".archive-format").is_file() and \
        (archive / ".archive-format").read_text(encoding="utf-8").strip() == ARCHIVE_FORMAT


def pending(repo):
    """Sessions with dialogue not yet published."""
    found = {}
    for session, row in load_state(repo).items():
        path = Path(row.get("transcript", ""))
        if path.is_file():
            offset = row["offset"] if path.stat().st_size >= row["offset"] else 0
            raw = row.get("raw", {})
            if any(True for _ in read_messages(path, offset)) or any(
                    any(True for _ in raw_entries(n, p, raw.get(n, 0) if p.stat().st_size >= raw.get(n, 0) else 0))
                    for n, p in raw_sources(path)):
                found[session] = row
    return found


def publish_session(repo, session, row):
    path = Path(row["transcript"])
    offset = row["offset"] if path.stat().st_size >= row["offset"] else 0
    messages = list(read_messages(path, offset))
    if "record" not in row:
        first = messages[0][2][:10] if messages else ""
        day = (first or datetime.now(timezone.utc).strftime("%Y-%m-%d")).replace("-", "/")
        row["record"] = f"logs/{day}/claude-session-{tag(session)}.md"
    record = row["record"]
    header = (f"# Claude Code development session {tag(session)}\n\n"
              "Automatic record of the visible dialogue, captured with each commit. Tool calls appear "
              "as one-line summaries here and in full in the linked raw transcript. Credentials matching "
              "known formats are redacted.\n",
              "session-" + tag(session))
    batch, size = [header], 0
    for end, ident, stamp, role, body in messages:
        for content, entry in entries_for(ident, stamp, role, body):
            validate(record, content, entry)
            batch.append((content, entry))
            size += len(content)
        if size >= BATCH_LIMIT:
            publish_many(repo / "logs", record, batch, keep_existing=True, message="Append Claude Code session record")
            row["offset"], batch, size = end, [], 0
    publish_many(repo / "logs", record, batch, keep_existing=True, message="Append Claude Code session record")
    if messages:
        row["offset"] = messages[-1][0]
    if not row.get("indexed"):
        publish_many(repo / "logs", "logs/coverage.md",
                     [(f"- [Claude Code development session {tag(session)}]({record[5:]})\n", "index-claude-" + tag(session))],
                     keep_existing=True, message="Index Claude Code session record")
        row["indexed"] = True
    publish_raw(repo, session, row, path)
    return True


def archive_head(repo):
    archive = repo / "logs"
    git(archive, "fetch", "--no-tags", "origin", "main")
    return git(archive, "rev-parse", "FETCH_HEAD")


def publish_raw(repo, session, row, main):
    """Append every new raw source line, splitting the record into numbered parts as it grows."""
    archive, base = repo / "logs", row["record"].removesuffix(".md") + "-raw"
    raw = row.setdefault("raw", {})
    head = archive_head(repo)
    names = git(archive, "ls-tree", "--name-only", head, "--", str(Path(base[5:]).parent) + "/").split("\n")
    parts = sorted(n for n in names if re.fullmatch(re.escape(Path(base).name) + r"-[0-9]{2}\.md", Path(n).name))
    done = set()
    if not raw:
        # Local state was lost. Entries already published are skipped instead of repeated.
        for name in parts:
            done |= set(re.findall(r"<!-- transcript-entry (r-[0-9a-f-]+) ", git(archive, "show", f"{head}:{name}")))
    number = len(parts) or 1
    size = int(git(archive, "cat-file", "-s", f"{head}:{parts[-1]}")) if parts else 0

    def flush(batch):
        nonlocal number, size
        if not batch:
            return
        if size and size + sum(len(c) for c, _ in batch) > RAW_PART_LIMIT:
            number, size = number + 1, 0
        path = f"{base}-{number:02d}.md"
        header = (f"# Claude Code raw transcript {tag(session)}, part {number}\n\n"
                  "Every line Claude Code wrote for the session and its subagents, including tool input and "
                  "output, with credentials in known formats redacted. Each entry is one source line. "
                  "Rebuild the JSONL files with `sh tools/claude-transcripts restore-raw`.\n",
                  f"raw-header-{number:02d}")
        publish_many(archive, path, [header, *batch], keep_existing=True, message="Append Claude Code raw transcript")
        if size == 0:
            publish_many(archive, row["record"], [(f"[Raw transcript, part {number}]({Path(path).name})\n",
                                                   f"raw-link-{number:02d}")],
                         keep_existing=True, message="Link Claude Code raw transcript")
        size += sum(len(c.encode()) for c, _ in batch) + 2000

    for name, path in raw_sources(main):
        start = raw.get(name, 0) if path.stat().st_size >= raw.get(name, 0) else 0
        batch, total, reached = [], 0, start
        for end, entries in raw_entries(name, path, start):
            for content, entry in entries:
                if entry not in done:
                    validate(f"{base}-01.md", content, entry)
                    batch.append((content, entry))
                    total += len(content)
            reached = end
            if total >= BATCH_LIMIT:
                flush(batch)
                raw[name], batch, total = end, [], 0
        flush(batch)
        raw[name] = reached  # Only after the lines are on the archive.


def checkpoint(repo):
    """Fast-forward the submodule to the archive's main branch and stage the gitlink."""
    archive = repo / "logs"
    if git(archive, "status", "--porcelain"):
        raise RuntimeError("The logs submodule has local changes. Archive records are only appended by the tools.")
    before = git(archive, "rev-parse", "HEAD")
    git(archive, "fetch", "--no-tags", "origin", "main")
    git(archive, "merge", "--ff-only", "FETCH_HEAD")
    git(repo, "add", "logs")
    return before != git(archive, "rev-parse", "HEAD")


def capture(repo, session=None, transcript=None):
    register(repo, session or os.environ.get("CLAUDE_CODE_SESSION_ID"), transcript)
    todo = pending(repo)
    if not todo:
        return []
    if not archive_ready(repo):
        raise RuntimeError("Run `git submodule update --init logs` so this session's transcript can be archived.")
    for name, value in (("GIT_AUTHOR_NAME", "Robotutor Session Archive"), ("GIT_AUTHOR_EMAIL", "archive@localhost"),
                        ("GIT_COMMITTER_NAME", "Robotutor Session Archive"), ("GIT_COMMITTER_EMAIL", "archive@localhost")):
        os.environ.setdefault(name, value)
    state = load_state(repo)
    done = []
    try:
        for name in sorted(todo):
            if publish_session(repo, name, state[name]):
                done.append(state[name]["record"])
    finally:
        save_state(repo, state)
    checkpoint(repo)
    return done


def stage_published(repo):
    """Include an archive checkpoint made earlier, for example by the Claude Code hook before a path commit."""
    if not archive_ready(repo) or not any(row.get("record") for row in load_state(repo).values()):
        return
    staged = git(repo, "ls-files", "-s", "--", "logs").split()
    if not staged or staged[1] != git(repo / "logs", "rev-parse", "HEAD"):
        git(repo, "add", "logs")


def verify_gitlink(repo, old_rev, new_rev=None):
    """The staged or committed logs gitlink may only move forward along the archive history."""
    def link(rev):
        if rev is None:
            out = git(repo, "ls-files", "-s", "--", "logs").split()
            return out[1] if out else None
        out = git(repo, "ls-tree", rev, "--", "logs", check=False).split()
        return out[2] if out else None
    old, new = link(old_rev), link(new_rev)
    if old and not new:
        return ["The logs submodule must not be removed."]
    if not old or old == new or not archive_ready(repo):
        return []
    if subprocess.run(["git", "-C", str(repo / "logs"), "merge-base", "--is-ancestor", old, new],
                      capture_output=True).returncode:
        return [f"The logs gitlink moved from {old[:12]} to {new[:12]}, which is not a fast-forward."]
    return []


GIT_HOOKS = {"pre-commit": "pre-commit", "pre-push": "pre-push \"$@\""}


def install_git_hooks(repo):
    hooks = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-path", "hooks"))
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    if not hooks.resolve().is_relative_to(common.resolve()):
        return [f"Git hooks live outside this checkout ({hooks}). Chain tools/claude-transcripts into them."]
    hooks.mkdir(exist_ok=True)
    notes = []
    for name, action in GIT_HOOKS.items():
        hook = hooks / name
        body = ("#!/bin/sh\n" + MARKER + "\n"
                "tool=\"$(git rev-parse --show-toplevel)/tools/claude-transcripts\"\n"
                "[ -f \"$tool\" ] || exit 0\n"
                f"exec sh \"$tool\" {action}\n")
        if hook.exists() and MARKER not in hook.read_text(encoding="utf-8", errors="replace"):
            notes.append(f"{hook} belongs to another tool. Add: sh tools/claude-transcripts {action}")
            continue
        if not hook.exists() or hook.read_text(encoding="utf-8") != body:
            with hook.open("w", encoding="utf-8", newline="\n") as handle:
                handle.write(body)
            hook.chmod(0o755)
    return notes


COMMIT = re.compile(r"(?:^|[;&|(]\s*|\s)git(?:\s+-[cC]\s+\S+)*\s+commit\b")
BYPASS = re.compile(r"(?:^|\s)(?:--no-verify|-[a-zA-Z]*n[a-zA-Z]*)(?=\s|$)")
FORCE = re.compile(r"(?:^|\s)(?:--force(?:-with-lease|-if-includes)?(?:=\S*)?|-[a-zA-Z]*f[a-zA-Z]*|\+\S+)(?=\s|$)")
HISTORY = re.compile(r"\bgit\s+(?:filter-branch|filter-repo|replace)\b|\bbfg\b")


def guard(command):
    """Refuse agent commands that would skip capture or drop archived history."""
    for part in re.split(r"&&|\|\||;|\n|\|", command):
        try:
            words = shlex.split(part)
        except ValueError:
            words = part.split()
        if "git" not in words:
            continue
        rest = words[words.index("git") + 1:]
        while rest[:1] and rest[0] in ("-c", "-C"):
            rest = rest[2:]
        verb, flags = (rest[0], " " + " ".join(rest[1:])) if rest else ("", "")
        if verb == "commit" and BYPASS.search(flags):
            return "Commit without --no-verify. The pre-commit hook archives the Claude transcript."
        if verb == "push" and FORCE.search(flags):
            return "Do not force-push. Commits and archived transcripts are never rewritten or dropped."
    if HISTORY.search(command):
        return "History rewriting tools are not allowed in Robotutor checkouts."
    return None


def touches_archive(repo, path):
    try:
        return Path(path).resolve().is_relative_to((repo / "logs").resolve())
    except (OSError, ValueError):
        return False


def hook(repo, event):
    """Claude Code hook entry point. Exit status 2 blocks the tool call and tells the agent why."""
    name = event.get("hook_event_name")
    register(repo, event.get("session_id"), event.get("transcript_path"))
    if name == "SessionStart":
        notes = install_git_hooks(repo)
        if not archive_ready(repo):
            init = subprocess.run(["git", "-C", str(repo), "submodule", "update", "--init", "logs"],
                                  capture_output=True, text=True)
            if init.returncode:
                notes.append("The private logs submodule is unavailable, so commits from this session will "
                             "be stopped until it is initialized.")
        for note in notes:
            print(note, file=sys.stderr)
        return 0
    if name != "PreToolUse":
        return 0
    tool, args = event.get("tool_name"), event.get("tool_input") or {}
    if tool in ("Edit", "Write", "MultiEdit", "NotebookEdit") and touches_archive(
            repo, args.get("file_path") or args.get("notebook_path") or ""):
        print("logs/ is the append-only transcript archive. Use tools/append_transcript.py.", file=sys.stderr)
        return 2
    if tool == "Bash":
        command = args.get("command", "")
        reason = guard(command)
        if reason:
            print(reason, file=sys.stderr)
            return 2
        if COMMIT.search(command):
            try:
                capture(repo, event.get("session_id"), event.get("transcript_path"))
            except Exception as exc:  # An unarchived commit must not proceed.
                print(f"Transcript capture failed, so the commit was blocked: {exc}", file=sys.stderr)
                return 2
    return 0


def pre_push(repo, lines):
    problems = []
    for line in lines:
        local_ref, local, remote_ref, old = line.split()
        if set(old) == {"0"} or set(local) == {"0"}:
            continue
        known = subprocess.run(["git", "-C", str(repo), "cat-file", "-e", old + "^{commit}"],
                               capture_output=True).returncode == 0
        if not known or subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor", old, local],
                                       capture_output=True).returncode:
            problems.append(f"{remote_ref}: the push would replace commits. Fetch and merge instead of rewriting.")
            continue
        problems += [f"{remote_ref}: {p}" for p in verify_gitlink(repo, old, local)]
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path)
    cmds = parser.add_subparsers(dest="action", required=True)
    cmds.add_parser("hook", help="Claude Code hook entry point (event JSON on stdin)")
    cmds.add_parser("pre-commit", help="Archive registered sessions and stage the logs gitlink")
    push = cmds.add_parser("pre-push", help="Refuse pushes that replace commits")
    push.add_argument("remote", nargs="?")
    push.add_argument("url", nargs="?")
    cap = cmds.add_parser("capture", help="Archive a session now")
    cap.add_argument("--session")
    cap.add_argument("--transcript")
    cmds.add_parser("install", help="Install the git hooks in this checkout")
    back = cmds.add_parser("restore-raw", help="Rebuild JSONL transcripts from raw record files")
    back.add_argument("records", nargs="+", type=Path)
    back.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.action == "restore-raw":
        for path in restore_raw(args.records, args.output):
            print(path)
        return 0
    isolate_from_hook()
    repo = Path(git(args.repo or os.environ.get("CLAUDE_PROJECT_DIR") or Path.cwd(), "rev-parse", "--show-toplevel"))
    if args.action == "hook":
        return hook(repo, json.load(sys.stdin))
    if args.action == "install":
        for note in install_git_hooks(repo):
            print(note, file=sys.stderr)
        return 0
    if args.action == "pre-push":
        problems = pre_push(repo, sys.stdin.read().splitlines())
    else:
        try:
            for record in capture(repo, getattr(args, "session", None), getattr(args, "transcript", None)):
                print(f"Archived Claude Code dialogue in {record}", file=sys.stderr)
        except Exception as exc:
            print(f"Transcript capture failed, so the commit was stopped: {exc}", file=sys.stderr)
            return 1
        if args.action == "pre-commit":
            stage_published(repo)
        head = git(repo, "rev-parse", "--verify", "-q", "HEAD", check=False)
        problems = verify_gitlink(repo, head or None, None) if head and args.action == "pre-commit" else []
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
