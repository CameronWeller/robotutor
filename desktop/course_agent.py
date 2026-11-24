"""Draft and independently check an outline from course direction."""
from local_platform import lock_file
import hashlib
import json
import os
from pathlib import Path
import tempfile
import sys

from astra import ReviewError, run_codex


def record(**fields):
    return {"type": "object", "additionalProperties": False,
            "properties": fields, "required": list(fields)}


def sequence(item):
    return {"type": "array", "items": item, "minItems": 1}


text = {"type": "string", "minLength": 1}
activity = record(key=text, due={"type":["string", "null"]}, title=text, instructions=text, objective=text, criterion=text)
unit = record(title=text, activities=sequence(activity))
outline = record(title=text, destination=text, units=sequence(unit))


def fields(course):
    values = {("Course", field): course[field] for field in ("title", "destination")}
    for group in course["units"]:
        for a in group["activities"]:
            for name in ("title", "instructions", "objective", "criterion", "due"):
                values[(a.get("key", a["title"]), name)] = a.get(name)
    return values


def teacher_edits(direction):
    protected = {}
    for item in direction:
        if isinstance(item, dict) and "before" in item and "after" in item:
            before, after = fields(item["before"]), fields(item["after"])
            for key in before.keys() - after.keys():
                protected.pop(key, None)
            protected.update({key: val for key, val in after.items() if before.get(key) != val})
    return protected


def propose(context, generate=run_codex):
    candidate = context.get("edited")
    protected = teacher_edits(context.get("direction", []))
    reason = "Teacher revised the unit or activity sequence."
    if candidate and context.get("previous"):
        previous = fields(context["previous"])
        changed = [f"{key[0]} · {key[1]}" for key, val in fields(candidate).items() if previous.get(key) != val]
        if changed:
            reason = "Teacher updated " + ", ".join(changed) + "."

    problem = ""
    for attempt in range(2):
        if candidate is None or attempt:
            proposal = generate(
                "Start with a small exchange that a learner can answer in a few words. For sustained assignments, specify the learning purpose, an intended audience and use, a deliverable, a first checkpoint and a chance to revise after feedback. Include evidence of usefulness in the agreed success check. "
                "Propose a teachable course. The destination is a concrete final performance. Address activity instructions to the learner. Give observable success criteria that avoid revealing the answer. "
                "Give each activity a unique persistent key and keep existing keys when revising or moving it. Use null due dates unless the teacher supplied a date, then use YYYY-MM-DD. Honor the original brief and accumulated direction. Preserve direct edits verbatim "
                "unless expressly changed. For conflicting requirements preserve the earlier edit and "
                "explain the conflict for review. Describe teaching changes and their purpose in plain language for the reader. Omit storage keys, schema fields and empty deadlines from that explanation. "
                + json.dumps({"context": context, "prior_proposal": candidate, "repair": problem}),
                record(outline=outline, changes=text))
            candidate, reason = proposal["outline"], proposal["changes"]
        keys = [a.get("key") for u in candidate["units"] for a in u["activities"]]
        if None in keys or len(keys) != len(set(keys)):
            problem = "Give each activity its own persistent key."
            continue
        proposed = fields(candidate)
        conflict = next((key for key, value in protected.items() if proposed.get(key) != value), None)
        if conflict:
            problem = f"Preserve the teacher’s {conflict[1]} edit for {conflict[0]}. Edit that field directly to replace it."
            continue
        review = generate(
            "Independently check the course against every requirement, prior direct edit, audience, "
            "and requested change. Check teachability, sequencing, an actionable final destination and learner-facing instructions. For audience projects check how the learner will find out if their work helped. Evaluate observable work and reasoning. Exclude judgments of character or religious agreement. Reject criteria that disclose an answer the learner is meant to derive. Reject "
            "unresolved conflicting instructions and identify the precise conflict for the teacher. "
            "Give concrete evidence or the unmet requirement in explanation. "
            + json.dumps({"context": context, "candidate": candidate}),
            record(accepted={"type": "boolean"}, explanation=text))
        if review["accepted"]:
            return dict(outline=candidate, changes=reason, problem="")
        problem = review["explanation"]
    return dict(outline=candidate, changes=reason, problem=problem)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()[:24]


def atomic(path, text, create=False):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False, mode="w") as file:
        try:
            file.write(text)
            file.flush()
            os.fsync(file.fileno())
            file.close()
            if create:
                try:
                    os.link(file.name, path)
                except FileExistsError:
                    pass
            else:
                os.replace(file.name, path)
        finally:
            Path(file.name).unlink(missing_ok=True)


def save_personal(path, seed, request):
    with path.with_suffix(".lock").open("a") as lock:
        lock_file(lock)
        if request is not None or not path.exists():
            state = json.loads(path.read_text()) if path.exists() else dict(seed=seed, direction=[], outline=None)
            if request is not None:
                state["seed"] = seed
            direction = state["direction"] + ([request] if request else [])
            try:
                result = propose(dict(seed=state["seed"], direction=direction, previous=state["outline"]))
                state.update(problem=result["problem"], changes=result["changes"])
                if not result["problem"]:
                    state.update(outline=result["outline"], direction=direction)
            except Exception as error:
                state["problem"] = str(error)[:300]
            atomic(path, json.dumps(state))


if __name__ == "__main__":
    try:
        packet = json.load(sys.stdin)
        if sys.argv[1:2] in (["--personal"], ["--revise-personal"]):
            os.umask(0o077)
            directory = Path(sys.argv[2]) / ".courses"
            directory.mkdir(mode=0o700, exist_ok=True)
            seed, request = (packet["seed"], packet["request"]) if sys.argv[1] == "--revise-personal" else (packet, None)
            save_personal(directory / (digest(seed["goal"]) + ".json"), seed, request)
        else:
            print(json.dumps(propose(packet)))
    except ReviewError as error:
        sys.exit(str(error))
