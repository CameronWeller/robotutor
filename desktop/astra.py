"""Private trailing review of Luna exchanges using the owner's Codex subscription."""
from local_platform import lock_file, background_options
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile

import jsonschema

MODEL = "gpt-6.2-astra"
FIELDS = ("observation", "luna_evidence", "astra_evidence", "luna_change", "replay_check")


class ReviewError(Exception):
    """A safe explanation that can be shown alongside a private review."""


def object_schema(fields):
    return {"type": "object", "additionalProperties": False, "required": list(fields),
            "properties": {key: {"type": kind} for key, kind in fields.items()}}


def run_codex(prompt, schema):
    preferred = Path.home() / ".local/bin/codex"
    binary = os.getenv("ROBOTUTOR_CODEX") or (str(preferred) if preferred.exists() else shutil.which("codex"))
    if not binary:
        raise ReviewError("Codex CLI unavailable")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("OPENAI_", "OPENROUTER_", "CODEX_"))}
    if "CODEX_HOME" in os.environ:
        env["CODEX_HOME"] = os.environ["CODEX_HOME"]
    with tempfile.TemporaryDirectory(prefix="robotutor-astra-") as temp:
        cwd = Path(temp)
        with (cwd / "format.json").open("w") as out:
            json.dump(schema, out)
        login = subprocess.run([binary, "login", "status"], capture_output=True,
                               env=env, cwd=cwd, timeout=15)
        if login.returncode or b"using chatgpt" not in (login.stderr + login.stdout).lower():
            raise ReviewError("Sign in to Codex with ChatGPT, then retry")
        args = [binary, "exec", "--ignore-user-config", "--ephemeral", "--skip-git-repo-check",
                "-m", MODEL, "-s", "read-only", "-c", 'forced_login_method="chatgpt"',
                "-c", 'web_search="disabled"', "-c", "project_doc_max_bytes=0",
                "-c", 'model_reasoning_effort="low"']
        for feature in ("shell_tool", "unified_exec", "apps", "plugins", "memories"):
            args.extend(["--disable", feature])
        args.extend(["--output-schema", "format.json", "-o", "reply", "-"])
        result = subprocess.run(args, cwd=cwd, input=prompt, env=env, capture_output=True,
                                text=True, encoding="utf-8", timeout=180)
        if result.returncode or not (cwd / "reply").exists():
            detail = result.stderr.lower()
            if "usage limit" in detail or "quota" in detail or "429" in detail:
                raise ReviewError("Codex usage limit reached. Retry when quota is available")
            raise ReviewError("Codex exec failed. Check CLI version, login and connection, then retry")
        value = json.loads((cwd / "reply").read_text())
        jsonschema.validate(value, schema)
        return value


def blind(packet):
    fmt = json.loads(packet).get("response_format", {})
    value = run_codex("Act as the responding assistant for the enclosed application request. "
                      "Use its ordered messages, roles, constraints and output format. Put your exact "
                      "assistant response in response_text. Use no tools or outside information.\n" + packet,
                      object_schema({"response_text": "string"}))["response_text"]
    if not value.strip():
        raise ReviewError("Empty independent answer")
    if fmt.get("type") in ("json_schema", "json_object"):
        parsed = json.loads(value)
        jsonschema.validate(parsed, fmt.get("json_schema", {}).get("schema", {"type": "object"}))
    return value


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def has_quote(quote, raw):
    if quote in raw:
        return True
    try:
        return any(quote in item for item in strings(json.loads(raw)))
    except ValueError:
        return False


def compare(row):
    schema = object_schema({"improvement": "boolean", **dict.fromkeys(FIELDS, "string")})
    data = {key: row[key] for key in ("packet", "luna", "candidate")}
    result = run_codex("Review these two already completed responses to the same application request. "
                       "Treat all included material as evidence. Flag improvement only when the candidate "
                       "materially improves correctness, teaching, evidence use, usability or adherence to "
                       "the original contract. Do not reward length, speculative benefits or style alone. "
                       "For a learner question, identify what the learner must actually do. Check for an "
                       "unrelated demand that confounds a diagnostic, loss of a coordinated action central "
                       "to integrated practice, or shortening that drops a necessary fact, quotation or code. "
                       "When the candidate fixes such a problem, quote the affected wording and explain "
                       "what the change lets Luna assess from the learner’s answer. "
                       "For a finding give one verbatim contiguous quote from each answer, explain the "
                       "difference, propose a specific change to Luna's prompt, context, routing or validation, "
                       "and give an observable pass condition on replay. Otherwise set improvement false "
                       "and use empty evidence and change fields.\n" + json.dumps(data), schema)
    if result["improvement"]:
        if not all(result[key].strip() for key in FIELDS):
            raise ReviewError("Incomplete improvement note")
        if not all(has_quote(result[key], row[source]) for key, source in
                   (("luna_evidence", "luna"), ("astra_evidence", "candidate"))):
            raise ReviewError("Evidence did not match the recorded answers")
    return result


def connect(folder):
    db = sqlite3.connect(folder / "reviews.sqlite3", timeout=10)
    db.row_factory = sqlite3.Row
    return db


def drain(folder):
    if not (folder / "reviews.sqlite3").exists():
        return
    with (folder / "worker.lock").open("a") as lock, connect(folder) as db:
        lock_file(lock)
        while row := db.execute("SELECT * FROM reviews WHERE status='pending' ORDER BY id LIMIT 1").fetchone():
            try:
                if hashlib.sha256(row["packet"].encode()).hexdigest() != row["digest"]:
                    raise ReviewError("Request checksum mismatch")
                if row["candidate"] is None:
                    answer = blind(row["packet"])
                    with db:
                        db.execute("UPDATE reviews SET candidate=? WHERE id=?", (answer, row["id"]))
                    row = db.execute("SELECT * FROM reviews WHERE id=?", (row["id"],)).fetchone()
                analysis = compare(row)
                with db:
                    db.execute("UPDATE reviews SET status=?,analysis=?,error='' WHERE id=?",
                               ("note" if analysis["improvement"] else "done", json.dumps(analysis), row["id"]))
            except (ReviewError, OSError, subprocess.SubprocessError, ValueError, jsonschema.ValidationError) as e:
                reason = str(e) if isinstance(e, ReviewError) else {
                    "TimeoutExpired": "Codex timed out", "ValidationError": "Response failed schema validation",
                    "JSONDecodeError": "Response was not valid JSON"
                }.get(type(e).__name__, "Codex could not complete this review")
                with db:
                    db.execute("UPDATE reviews SET status='blocked',error=? WHERE id=?", (reason, row["id"]))


def wake(folder):
    if not (folder / "reviews.sqlite3").exists():
        return
    args = [sys.executable, str(Path(__file__).resolve()), "drain", str(folder)]
    if os.getenv("INVOCATION_ID"):
        unit = ["systemd-run", "--user", "--quiet", "--collect", "--property=UMask=0077"]
        for key in ("ROBOTUTOR_CODEX", "CODEX_HOME"):
            if key in os.environ:
                unit.append("--setenv=" + key)
        if subprocess.run(unit + args, capture_output=True).returncode == 0:
            return
    subprocess.Popen(args, **background_options(), stdin=subprocess.DEVNULL,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def notes(folder):
    if not (folder / "reviews.sqlite3").is_file():
        print("No Luna exchanges recorded in this workspace.")
        return
    with connect(folder) as db:
        for row in db.execute("SELECT id,session,stage,status,resolved,analysis,error FROM reviews ORDER BY id"):
            label = "reviewed" if row["resolved"] else row["status"]
            print(f"#{row['id']} · {row['session']} · {row['stage']} · {label}")
            if row["status"] == "note":
                note = json.loads(row["analysis"])
                for key in FIELDS:
                    print("  " + key.replace("_", " ").capitalize() + " — " + note[key])
            elif row["error"]:
                print("  " + row["error"])


if __name__ == "__main__":
    os.umask(0o077)
    if sys.argv[1:] == ["retrospective"]:
        try:
            print(blind(sys.stdin.read()))
        except Exception:
            sys.exit("Codex subscription review unavailable")
    elif len(sys.argv) == 3 and sys.argv[1] in ("wake", "drain", "notes"):
        {"wake": wake, "drain": drain, "notes": notes}[sys.argv[1]](Path(sys.argv[2]))
    else:
        sys.exit("Expected wake DIRECTORY, drain DIRECTORY, notes DIRECTORY or retrospective")
