"""Replay an already-confirmed alternative to an overdue review through two isolated visits."""
import argparse
from hashlib import sha256
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from decision_witness import approved_topic, offered_topics
from learning_conditions import visit_rules
from teaching_trial import encode, exclusive_study, pack, read, save


def key(value):
    return sha256(encode(value).encode()).hexdigest()


def source_id(release):
    return key([release[k] for k in ("source_url", "locator", "excerpt")])


def serve(path, request):
    with exclusive_study(path.parent):
        state = read(path)
        due = state["due"]
        domain = due["domain"]
        offers = offered_topics(domain)
        slot = request["slot"]
        selected = approved_topic(domain, slot, offers)
        if selected is None:
            return {"result": "no_choice", "display": "The review is still due. You can choose another activity."}
        if selected != request["selected"]:
            return {"result": "selection_changed", "display": "The exercise selection has changed. Please choose a current exercise."}
        course = pack(domain)
        release = course[slot + "_pack"]
        if selected not in request["grants"] or source_id(release) not in request["sources"]:
            return {"result": "unavailable", "display": "That exercise is unavailable. The holding-tank review is still due."}
        if slot == "first" and selected != due:
            raise ValueError("selected due review no longer matches the outstanding obligation")
        role = "check" if selected == due else "practice"
        event = {"day": request["day"], "item": selected, "role": role, "display_digest": key(release[role])}
        previous = state["contacts"].get(request["visit_id"])
        if previous is not None and event != previous:
            raise ValueError("a delivered visit cannot be reinterpreted as another choice")
        state["contacts"][request["visit_id"]] = event
        save(path, state)
        return {"result": role, "unit": selected["unit"], "display": release[role], "due": due["unit"]}


def fresh(path, request):
    result = subprocess.run([sys.executable, "-B", __file__, "--serve", str(path)],
                            input=encode(request).encode(), capture_output=True, check=True)
    return json.loads(result.stdout)


def study(archive):
    stamps = {"case": "e3e097eb2e911e201e6058cdc280029ae5c7023a56650d04ad475c7ffa9d582f",
              "condition": "dd7bccf23f63367062bf0c693622e9b6725eb46a9a8c5ec7c1e3f3eeda121337",
              "A-return": "99a1a61aabe1a3b98343907dc56c8a297d30b9be142d3ca5611ff86bc99c5e06",
              "B-return": "1d86006f52f651280a17f82e009631739361eb6e379afbb8c71d900e05c52c73"}
    for name, expected in stamps.items():
        if sha256((archive / (name + ".json")).read_bytes()).hexdigest() != expected:
            raise ValueError(f"archived {name} differs")
    condition = read(archive / "condition.json")["condition"]
    original = [read(archive / (arm + "-return.json")) for arm in ("A", "B")]
    old_rule = visit_rules(condition, "return")
    assert condition["id"] == "due_choice" and not old_rule["task"] and not old_rule["material"]
    assert all("new_task" not in item["packet"] and not item["packet"]["material"] for item in original)
    domain = condition["domain"]
    offers = offered_topics(domain)
    due = approved_topic(domain, "first", offers)
    other = approved_topic(domain, "return", offers)
    assert all(item["packet"]["receipt"]["topic"] == due for item in original)
    course = pack(domain)
    grants = [due, other]
    sources = [source_id(course[slot + "_pack"]) for slot in ("first", "return")]
    request = {"visit_id": "current-meeting", "day": condition["days"], "slot": "return",
               "selected": other, "grants": grants, "sources": sources}
    starting = {"due": due, "contacts": {}, "submissions": []}
    outcomes = {}
    with tempfile.TemporaryDirectory(prefix="due-choice-") as scratch:
        path = Path(scratch) / "agenda.json"
        for label, changes in [
            ("chosen", {}), ("due_chosen", {"slot": "first", "selected": due}),
            ("wrong_unit_grant", {"grants": [due]}), ("second_source_withdrawn", {"sources": sources[:1]}),
            ("stale_edition", {"selected": {**other, "edition": "0" * 64}}),
            ("wrong_selection", {"selected": due}), ("undecided", {"slot": "none", "selected": None}),
        ]:
            save(path, starting)
            now = fresh(path, {**request, **changes})
            after = read(path)
            if label == "chosen":
                assert fresh(path, request) == now and read(path) == after
            later = fresh(path, {**request, "visit_id": "next-meeting", "day": condition["days"] + 1,
                                 "slot": "first", "selected": due})
            final = read(path)
            assert after["due"] == due == final["due"] and not final["submissions"]
            assert later["display"] == course["first_pack"]["check"]
            assert len(final["contacts"]) == len(after["contacts"]) + 1
            outcomes[label] = {"now": now, "contacts_now": len(after["contacts"]),
                               "next": later, "contacts_next": len(final["contacts"]), "submissions": len(final["submissions"])}
    assert outcomes["chosen"]["now"]["display"] == course["return_pack"]["practice"]
    assert outcomes["due_chosen"]["now"]["display"] == course["first_pack"]["check"]
    assert all(outcomes[k]["contacts_now"] == 0 for k in outcomes if k not in ("chosen", "due_chosen"))
    return {"source_stage_sha256": stamps, "request": read(archive / "case.json")["response"]["return_request"],
            "baseline_displays": [item["response"]["display"] for item in original], "outcomes": outcomes}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--study", type=Path)
    action.add_argument("--serve", type=Path)
    args = parser.parse_args()
    print(json.dumps(study(args.study) if args.study else serve(args.serve, json.load(sys.stdin)), indent=2))
