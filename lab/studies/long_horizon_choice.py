"""Compare return policies on identical copies of one synthetic learner's Rust journal."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from time import perf_counter


def build():
    manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
    result = subprocess.run(["cargo", "build", "--offline", "--locked", "--manifest-path", str(manifest),
                             "--example", "longitudinal_rehearsal", "--message-format=json"],
                            check=True, capture_output=True, text=True)
    return next(Path(m["executable"]) for m in map(json.loads, result.stdout.splitlines())
                if m.get("target", {}).get("name") == "longitudinal_rehearsal" and m.get("executable"))


class History:
    def __init__(self, exe, path):
        self.exe, self.path = exe, path
        self.head = None

    def call(self, verb, *args, content=""):
        result = subprocess.run([self.exe, verb, self.path, *map(str, args)], input=content.encode(),
                                capture_output=True, timeout=60)
        if result.returncode:
            raise ValueError(result.stderr.decode(errors="replace").strip())
        receipt = json.loads(result.stdout)
        self.head = receipt["head"]
        return receipt

    def step(self, *args, content=""):
        return self.call("apply", self.head, *args, content=content)

    def fork(self, suffix):
        copy = History(self.exe, self.path.with_name(suffix + ".jsonl"))
        shutil.copyfile(self.path, copy.path)
        copy.call("show", self.head)
        return copy


def return_packet(state, forecast, day, choice):
    """Typed information available to the scheduling comparison, without private prose."""
    goal = state.get("goal_choice", {})
    return {"day": day, "choice": choice, "booked": forecast["booked_due"],
            "course_parked": state["parked"], "coverage": state["exposure_complete"],
            "course_source": [state["source_revision"], state["source_active"]],
            "course_granted": state["grant"], "goal": goal.get("requested_unit"),
            "parish_granted": goal.get("parish_grant"), "pending_goal_turn": goal.get("awaiting")}


def actions(history, lane, day, requested_unit):
    if lane == "ribbon":
        return history.step("resume", day)
    if lane == "private":
        return history.step("practice-visit", day)
    if lane == "saved-course":
        return history.step("goal", "visit")
    if lane in ("sample-101", "sample-103"):
        unit = int(lane.split("-")[1])
        if requested_unit == unit:
            return history.step("goal", "study", unit, "repeat")
        return history.step("goal", "try", requested_unit, unit)
    raise ValueError("unknown return lane")


def display(receipt):
    content = receipt.get("display")
    if isinstance(content, dict):
        for key in ("worked", "practice", "case", "invitation", "message", "experiment"):
            if key in content:
                return content[key]
    return content or receipt.get("reason") or receipt.get("authority") or receipt["contribution"]


def compare(history, day, latest, choice, request):
    start = perf_counter()
    state = history.call("show", history.head)
    forecast = history.call("forecast", history.head, day)
    seconds = perf_counter() - start
    packet = return_packet(state, forecast, day, choice)
    prior_bytes = history.path.stat().st_size
    baseline = history.fork(f"baseline-{day}")
    baseline_result = actions(baseline, latest, day, packet["goal"])
    chosen = actions(history, choice, day, packet["goal"])
    assert chosen["obligation"] == baseline_result["obligation"] == state["obligation"]
    if choice == "ribbon":
        why = f"You asked for the ribbon review booked for day {packet['booked']}. "
        why += ("The current source is available." if packet["course_source"][1] else
                "The source for that exercise is on hold.")
    elif choice == "saved-course":
        why = "You asked for the parish-service lesson. The one-visit thanksgiving sample left that choice in place."
    else:
        why = "You asked for your saved private exercise. Your ribbon review can wait."
    print(json.dumps({"day": day, "request": request, "latest_lane": latest, "selected_lane": choice,
                      "context_bytes": len(json.dumps(packet, separators=(",", ":")).encode()),
                      "journal_bytes_before": prior_bytes, "context": packet,
                      "two_read_processes_ms": round(1000*seconds, 1),
                      "baseline": display(baseline_result), "chosen": display(chosen), "why": why,
                      "baseline_piece": baseline_result["contribution"], "chosen_piece": chosen["contribution"],
                      "checkpoint_after": chosen["checkpoint"], "credit": chosen["credit"]},
                     ensure_ascii=False))
    return chosen


def study(exe, directory):
    directory.mkdir(exist_ok=False)
    history = History(exe, directory / "learner.jsonl")
    try:
        initial = history.call("init")
    except ValueError as exc:
        # The Windows adapter may sync the new header then fail to open its parent
        # for directory fsync. Recover only a new, exactly identified genesis.
        if os.name != "nt" or "os error 5" not in str(exc):
            raise
        header = b'{"format":"longitudinal-rehearsal/1"}'
        if history.path.read_bytes() != header + b"\n":
            raise
        initial = history.call("show", hashlib.sha256(header).hexdigest())
        print("Recovered the synced Windows genesis after directory sync failed.")
    assert initial["learner"] == 42 and initial["checkpoint"]["due"] == 21
    opener = history.step("open", 30)
    aid = history.step("course-aid", "worked")
    draft = "A paper heron watches the ticket desk."
    local = history.step("work", content=draft)
    history.step("practice-case", "warm")
    history.step("practice-next", "invitation")
    history.step("withdraw")
    print(json.dumps({"day":30,"opening":display(opener),"aid":display(aid),
                      "private_local_quote":local["display"]["quote"],"private_credit":local["credit"]}))

    blocked = compare(history, 180, "private", "ribbon", "Can we return to my unfinished ribbon exercise?")
    assert not blocked["opened"] and not blocked["source_active"]
    history.step("replace")
    history.step("unknown")
    missing = history.step("resume", 180)
    assert not missing["opened"]
    history.step("recover")
    reopened = history.step("resume", 180)
    assert reopened["opened"] and reopened["task"] != opener["task"]
    history.step("goal", "choose", 103)
    held_goal = history.step("goal", "visit")
    assert held_goal["contribution"] == "goal_held"
    sample = history.step("goal", "try", 103, 101)
    assert sample["shown_unit"] == 101 and sample["goal_choice"]["requested_unit"] == 103
    print(json.dumps({"day":180,"after_replacement_unknown_contacts":display(missing),
                      "recovered_course":display(reopened),"new_goal":display(held_goal),
                      "accepted_one_visit_sample":display(sample)}))

    history.step("goal", "source", "granted")
    own_goal = compare(history, 730, "sample-101", "saved-course",
                       "Can we study the part of a parish service that invites everyone to respond?")
    assert own_goal["shown_unit"] == 103 and own_goal["credit"] is None
    later = compare(history, 1460, "sample-103", "private",
                    "I would like the invitation exercise I saved for myself.")
    assert "inviting a choice" in display(later) and later["checkpoint"]["due"] == 21
    history.step("practice-next", "clear")
    history.step("varied-instances")
    final = compare(history, 1825, "private", "ribbon",
                    "Let's work on ribbon lengths again. I'd like a worked example before I answer.")
    assert final["opened"]
    worked = history.step("course-aid", "worked")
    answer = history.step("reply", "none", content="1/16")
    assert answer["credit"] is None and answer["checkpoint"]["due"] == 21
    status = history.call("show", history.head)
    assert status["learner"] == 42
    assert all(draft.encode() not in path.read_bytes() for path in directory.glob("*.jsonl"))
    print(json.dumps({"day":1825,"worked":display(worked),"response":"1/16",
                      "judgment":answer["judgment"],"independent":answer["independent"],
                      "due_after_guided_work":answer["checkpoint"]["due"],
                      "written_draft_in_any_journal":False,"journal_bytes_final":history.path.stat().st_size}))
    same = compare(history, 1860, "ribbon", "ribbon", "I'd like to keep working on ribbon lengths.")
    assert same["checkpoint"]["due"] == 21


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep", type=Path, help="new, absent directory for the synthetic journals")
    parser.add_argument("--rehearsal", type=Path, help="existing longitudinal_rehearsal executable")
    args = parser.parse_args()
    binary = args.rehearsal.resolve() if args.rehearsal else build()
    if args.keep:
        study(binary, args.keep)
    else:
        with tempfile.TemporaryDirectory(prefix="robotutor-horizon-") as tmp:
            study(binary, Path(tmp)/"study")
