"""Authorize bounded fields for a paired teaching study before writing its booking."""
import argparse
import json
from pathlib import Path

from decision_witness import approved_topic, offered_topics
from learning_conditions import ACTIONS


FOCI = ("condition", "reasoning", "source", "observation", "choice", "unknown")
FIELDS = ("goal", "move", "help", "family", "focus", "topic")


def revise_booking(stored, intent):
    """Apply the participant's return choice even when the teaching source is on hold."""
    reason = {"keep": "carry", "forget_all": "forget_topic",
              "cancel_topic": "cancel_topic", "change_goal": "goal_switch"}[intent]
    if intent == "cancel_topic":
        retained = {k: value for k, value in stored.items() if k != "topic"}
    elif intent in ("forget_all", "change_goal"):
        retained = {}
    else:
        retained = dict(stored)
    return {"format": "paired-return-memory/1", "stored": retained, "reason": reason}


def memory_options():
    """Participant-facing choices. Each field is optional and independent of topic."""
    return {
        "goal": "Keep the broad course goal shown for this visit.",
        "move": "Keep the kind of teaching response I received.",
        "help": "Keep whether this activity recorded supplied help or left it unknown.",
        "family": "Keep the identifier of the kind of exercise I tried.",
    }


def unique_labels(values, menu):
    if (not isinstance(values, (list, tuple, set, frozenset)) or len(values) > len(menu)
            or any(not isinstance(v, str) or v not in menu for v in values)
            or len(values) != len(set(values))):
        raise ValueError("expected distinct labels from the offered fields")
    return set(values)


def validate_choices(scenario, *, private=False):
    """Return normalized case fields, or reject an ambiguous or unauthorized choice."""
    if not isinstance(scenario, dict) or not {"topic_selection", "memory_choices", "selected_focus"} <= scenario.keys():
        raise ValueError("the participant must explicitly choose topic, fields and focus")
    topic = scenario["topic_selection"]
    if topic not in ("first", "return", "none"):
        raise ValueError("unknown lesson topic")
    if "save_topic" in scenario and scenario["save_topic"] is not (topic != "none"):
        raise ValueError("topic receipt disagrees with the selected offer")
    if not isinstance(scenario["memory_choices"], list):
        raise ValueError("memory_choices must be a list")
    menu = memory_options()
    chosen = unique_labels(scenario["memory_choices"], menu)
    focus = scenario["selected_focus"]
    if focus is not None and focus not in FOCI:
        raise ValueError("selected_focus must be an offered focus label or null")
    if private and (chosen or focus is not None or topic != "none"):
        raise ValueError("the private first visit permits no booking fields")
    return {"memory_choices": [name for name in menu if name in chosen], "selected_focus": focus}


def authorized_memory(scenario, variant, available, permitted):
    """Intersect the learner grant, actual teaching inputs, policy request and host boundary.

    The caller supplies edition-checked public course metadata and an offered-topic
    binding. An inferred teacher focus has no authority in this operation.
    """
    grants = validate_choices(scenario)
    requested = unique_labels(variant["memory"], FIELDS)
    permitted = unique_labels(permitted, FIELDS)
    if not isinstance(available, dict) or not set(available).issubset(FIELDS):
        raise ValueError("unknown available memory fields")
    retained = {}
    for name in FIELDS:
        if name not in requested or name not in permitted:
            continue
        if name == "focus":
            if grants["selected_focus"] is not None:
                retained["focus"] = grants["selected_focus"]
            continue
        if name not in available:
            continue
        value = available[name]
        if name == "topic":
            if scenario["topic_selection"] == "none":
                continue
            if not isinstance(value, dict) or not isinstance(value.get("domain"), str):
                raise ValueError("missing released topic binding")
            domain = value["domain"]
            current = approved_topic(domain, scenario["topic_selection"], offered_topics(domain))
            if json.dumps(value, sort_keys=True, allow_nan=False) != json.dumps(current, sort_keys=True):
                raise ValueError("topic differs from the selected release")
            retained[name] = current
        elif name in grants["memory_choices"]:
            valid = {
                "goal": lambda v: isinstance(v, str) and v.isprintable() and bool(v.strip()) and len(v) <= 200,
                "move": lambda v: v in ACTIONS,
                "help": lambda v: v in ("supplied", "unknown"),
                "family": lambda v: type(v) is int and 0 < v <= 0xffffffff,
            }
            if not valid[name](value):
                raise ValueError(f"invalid bounded course field {name}")
            retained[name] = value
    return retained


def replay_trial(directory):
    """Inspect archived facts separately from hypothetical new participant choices."""
    def read(name):
        return json.loads((directory / f"{name}.json").read_bytes())

    scenario = read("case")["response"]
    decision = read("decision")
    candidate = decision["candidate"]
    design = read("design")
    packet = read(f"{candidate}-return")["packet"]
    offers = offered_topics(decision["domain"])
    topic = approved_topic(decision["domain"], scenario["topic_selection"], offers)
    available = {"goal": packet["goal"], "focus": read(f"{candidate}-feedback")["response"]["focus"],
                 "topic": topic}
    policy = design["response"]["policy"]
    permitted = read("condition")["stages"]["feedback"]["memory"]
    print(json.dumps({"observed_trial": decision["trial"], "candidate": candidate,
                      "participant_selected": scenario["topic_selection"], "candidate_requested": policy["memory"],
                      "teacher_focus": available["focus"], "durable": read(f"{candidate}-booking")["stored"],
                      "returned": packet["receipt"], "had_independent_focus_selection": "selected_focus" in scenario}))
    for label, extras in (("topic only", {"selected_focus": None, "memory_choices": []}),
                          ("learner chose choice", {"selected_focus": "choice", "memory_choices": []}),
                          ("goal separately selected", {"selected_focus": None, "memory_choices": ["goal"]})):
        case = {**scenario, **extras}
        print(json.dumps({"hypothetical": label, "same_policy_result": authorized_memory(case, policy, available, permitted),
                          "policy_requests_topic_too": authorized_memory(case, {"memory": list(dict.fromkeys(
                              [*policy["memory"], "topic"]))}, available, permitted)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trial", type=Path, help="extracted protocol-5 trial such as 014")
    replay_trial(parser.parse_args().trial)
