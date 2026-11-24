"""Run four isolated model calls on questions exported by the water_author Rust example."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from hashlib import sha256
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


MODEL = "gpt-6.2-sol"
READER = ("Answer the displayed water-budget question as a synthetic learner. Put only your signed "
          "number and unit in display. Briefly explain your own calculation in explanation.")
TUTOR = ("Read this learner's submitted calculation. Give them a brief reply about this one exercise "
         "using the reviewed accounting principle. Address their actual reasoning and offer one thing "
         "they could check for themselves. Write only what the learner should see in display.")


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def call(packet, instruction, fields):
    prompt = ("Use only the synthetic packet below. Use no tools, files or searches. "
              "Return one JSON object.\n" + instruction + "\n" + encode(packet))
    schema = {"type": "object", "properties": {k: {"type": "string"} for k in fields},
              "required": fields, "additionalProperties": False}
    with tempfile.TemporaryDirectory(prefix="water-role-") as directory:
        cwd = Path(directory)
        (cwd / "schema").write_text(encode(schema), encoding="utf-8")
        command = [shutil.which("codex") or "codex", "exec", "--ignore-user-config", "--ephemeral",
                   "--skip-git-repo-check", "--sandbox", "read-only", "--model", MODEL, "--json",
                   "--output-schema", str(cwd / "schema"), "--output-last-message", str(cwd / "out"), "-"]
        start = time.monotonic()
        proc = subprocess.run(command, cwd=cwd, input=prompt.encode(), capture_output=True, timeout=160)
        elapsed = round(time.monotonic() - start, 3)
        if proc.returncode:
            raise RuntimeError(f"model process failed with code {proc.returncode}")
        usage = None
        for line in proc.stdout.splitlines():
            event = json.loads(line)
            if event.get("type") == "turn.completed":
                usage = {k: v for k, v in event["usage"].items() if k.endswith("_tokens") and type(v) is int}
            if event.get("item", {}).get("type") in ("command_execution", "mcp_tool_call", "web_search", "file_change"):
                raise ValueError("reader or tutor used an external tool")
        response = json.loads((cwd / "out").read_text(encoding="utf-8"))
        if set(response) != set(fields) or not all(isinstance(v, str) and v.strip() for v in response.values()):
            raise ValueError("invalid response")
    return {"model_requested": MODEL, "request_sha256": sha256(prompt.encode()).hexdigest(),
            "packet": packet, "response": response, "seconds": elapsed, "usage": usage}


def run(questions, destination):
    if set(questions) != {"source", "questions"} or set(questions["questions"]) != {"T1", "T3", "P1"}:
        raise ValueError("use the Rust --teaching-questions export")
    displays = questions["questions"]
    records = {}
    with destination.open("x", encoding="utf-8") as out:
        out.write("{}\n")

    def record(stage, receipt):
        records[stage] = receipt
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", newline="\n",
                                         dir=destination.parent, delete=False) as partial:
            json.dump(records, partial, ensure_ascii=False, indent=2)
            partial.write("\n")
        Path(partial.name).replace(destination)

    def reader(item):
        return call({"question": displays[item]}, READER, ["display", "explanation"])

    first = reader("T1")
    record("T1", first)
    tutor = call({"question": displays["T1"], "reviewed_principle": questions["source"],
                  "submission": first["response"]["display"], "explanation": first["response"]["explanation"]},
                 TUTOR, ["display"])
    record("feedback", tutor)
    with ThreadPoolExecutor(max_workers=2) as pool:
        pending = {item: pool.submit(reader, item) for item in ("T3", "P1")}
        errors = []
        for item, task in pending.items():
            try:
                record(item, task.result())
            except (RuntimeError, ValueError, subprocess.TimeoutExpired) as exc:
                errors.append(f"{item}: {exc}")
        if errors:
            raise RuntimeError("; ".join(errors))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", required=True)
    parser.add_argument("questions", type=Path)
    parser.add_argument("new_receipt", type=Path)
    args = parser.parse_args()
    run(json.loads(args.questions.read_text(encoding="utf-8")), args.new_receipt)
