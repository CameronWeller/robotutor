"""Replay a booking revision during a held visit, then resolve only currently granted lesson fields."""
import argparse
from hashlib import sha256
import json
from pathlib import Path
import tempfile

from decision_witness import approved_topic, offered_topics
from learning_conditions import visit_rules
from resumed_work import later_rule, lesson_route
from teaching_consent import revise_booking
from teaching_trial import encode, pack, read, save


def fingerprint(value):
    return sha256(encode(value).encode()).hexdigest()


def on_resume(domain, stored, fields, condition, policy, grants):
    rules = later_rule(condition)
    if rules is None:
        raise ValueError("the case has no later teaching visit")
    received = {k: v for k, v in stored.items() if k in fields and k in rules["memory"]}
    route = lesson_route(policy.get("resume_mode", "booked"), domain, received.get("topic"))
    offers = offered_topics(domain)
    offer = approved_topic(domain, route["slot"], offers)
    release = pack(domain)[route["slot"] + "_pack"]
    proof = [offer, release["source_url"], release["locator"], release["excerpt"]]
    if fingerprint(proof) not in grants:
        return {"origin": "local", "material": {}, "dispatch": None}
    material = {k: release[k] for k in policy["retrieve"] if k in rules["material"]}
    return {"origin": "eligible", "material": material, "source": release["source_url"], "grant": fingerprint(proof),
            "dispatch": {"receipt": received, "lesson_route": route, "title": offers[route["slot"]]["title"]}}


def study(directory):
    names = ("case", "condition", "design", "A-booking", "B-booking", "B-return-memory", "B-return",
             "B-resume-memory", "B-resume", "review-grounded")
    hashes = {name: sha256((directory / (name + ".json")).read_bytes()).hexdigest() for name in names}
    if (hashes["B-resume"] != "4713b2ff2d987c5994d491b3a3313db9696e2fb3807d9b98be39f3ecb507ad66"
            or hashes["case"] != "f201f31cb7555f1f619b623f6550b7711a9f232f379a825df135ee1a304a12ea"):
        raise ValueError("expected the saved 028 case and dispatched B resume")
    records = {name: read(directory / (name + ".json")) for name in names}
    case = records["case"]["response"]
    condition = records["condition"]["condition"]
    domain = condition["domain"]
    holds = visit_rules(condition, "return")
    later = later_rule(condition)
    assert holds["local"] == records["B-return"]["response"] and not holds["material"]
    assert records["B-return"]["origin"] == "local" and not records["B-return"]["packet"]["material"]
    assert case["resume_keep"] and later == records["condition"]["stages"]["resume"]
    policy = records["design"]["packet"]["incumbent"]
    actual = records["B-resume"]
    packet = actual["packet"]
    assert actual["origin"] == "model" and policy["returning"] == packet["policy"]
    assert packet["learner"] == case["resume_request"] and packet["shared_work"] == case["resume_work"]
    course = pack(domain)
    assert packet["new_task"] == course["return_pack"]["practice"]
    offers = offered_topics(domain)
    initial = records["B-booking"]["stored"]
    assert initial == records["A-booking"]["stored"] and initial["topic"] == approved_topic(domain, "first", offers)
    grants = []
    for slot in ("first", "return"):
        release = course[slot + "_pack"]
        grants.append(fingerprint([approved_topic(domain, slot, offers), release["source_url"], release["locator"], release["excerpt"]]))
    cases = [("cancel", initial, case["return_intent"], grants, later["memory"]),
             ("hold_only", initial, "keep", grants, later["memory"]),
             ("cancel_without_booking", {k: v for k, v in initial.items() if k != "topic"}, "cancel_topic", grants, later["memory"]),
             ("clear_all", initial, "forget_all", grants, later["memory"]),
             ("hold_only_no_regrant", initial, "keep", [], later["memory"]),
             ("cancel_no_regrant", initial, "cancel_topic", [], later["memory"]),
             ("keep_narrow_renewal", initial, "keep", grants, ["goal", "family"])]
    rows = {}
    with tempfile.TemporaryDirectory(prefix="booking-hold-") as working:
        for label, opening, intent, current, fields in cases:
            state = revise_booking(opening, intent)
            path = Path(working) / (label + ".json")
            save(path, state)
            routed = on_resume(domain, read(path)["stored"], fields, condition, policy, current)
            rows[label] = {"held_visit": state, "resume": routed}
    cancelled = rows["cancel"]
    assert cancelled["held_visit"] == records["B-return-memory"]
    dispatch = cancelled["resume"]["dispatch"]
    assert dispatch["receipt"] == records["B-resume-memory"]["stored"] == packet["receipt"]
    assert dispatch["lesson_route"] == packet["lesson_route"] and cancelled["resume"]["material"] == packet["material"]
    assert dispatch["lesson_route"]["unit"] == 201 and rows["hold_only"]["resume"]["dispatch"]["lesson_route"]["unit"] == 101
    carried = rows["hold_only"]["resume"]["dispatch"]["receipt"]
    assert {k: v for k, v in carried.items() if k != "topic"} == dispatch["receipt"]
    assert rows["cancel_without_booking"] == cancelled
    assert rows["clear_all"]["resume"]["dispatch"]["receipt"] == {}
    assert rows["keep_narrow_renewal"]["resume"]["dispatch"]["lesson_route"]["unit"] == 201
    assert all(rows[k]["resume"]["dispatch"] is None for k in ("cancel_no_regrant", "hold_only_no_regrant"))
    trace = records["review-grounded"]["packet"]["traces"]["B"]
    assert trace["return_confirmation"] == "No lesson topic is booked now."
    return {"stage_sha256": hashes, "held": {"local_display": holds["local"]["display"],
            "return_confirmation": trace["return_confirmation"], "learner_request": case["return_request"]},
            "reopened": {"day": packet["days"], "request": packet["learner"], "work_sha256": fingerprint(packet["shared_work"]),
                         "shared_practice_sha256": fingerprint(packet["new_task"]),
                         "release_edition": initial["topic"]["edition"], "actual_B_display": actual["response"]["display"]},
            "cases": rows}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    print(json.dumps(study(parser.parse_args().directory), ensure_ascii=True, indent=2))
