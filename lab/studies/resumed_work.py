"""Reopen a restricted teaching visit under a new grant and select reviewed lesson material."""
from decision_witness import offered_topics
from learning_conditions import ACTIONS, MATERIAL, MEMORY, Condition, Context, Rules, visit_rules


RESUME_MODES = ("current", "booked", "compare")
RESUMED_OBSERVATION = (
    "The learner returned to the original goal on their own initiative. "
    "A new grant permits both reviewed practice releases and the learner's newly shared submission for this visit."
)


def later_rule(condition: Condition) -> Rules | None:
    if visit_rules(condition, "return")["learner_attempt"]:
        return None
    context = Context(setting=condition["setting"], day=condition["days"] + 30,
                      evidence_mode=condition["evidence_mode"], observed=RESUMED_OBSERVATION)
    memory = [] if condition["id"] in ("goal_switch", "forget_topic") else [*MEMORY, "topic"]
    return Rules(context=context, material=list(MATERIAL), actions=list(ACTIONS), memory=memory,
                 task=True, answer=True, learner_attempt=False, local=None)


def lesson_route(mode, domain, stored_topic):
    """Only an exact approved release reference can change the booked primary unit."""
    if mode not in RESUME_MODES:
        raise ValueError("unknown resumed lesson mode")
    offers = offered_topics(domain)
    slot = "return"
    if mode == "booked" and isinstance(stored_topic, dict) and type(stored_topic.get("unit")) is int:
        for released_slot in ("first", "return"):
            offer = offers[released_slot]
            if stored_topic == {k: offer[k] for k in ("domain", "unit", "edition")}:
                slot = released_slot
                break
    return {"mode": mode, "slot": slot, "unit": offers[slot]["unit"],
            "comparison_unit": offers["first"]["unit"] if mode == "compare" else None}
NEW_GOAL_POLICY = ("Help the learner scope the newly chosen goal. Use their current request to ask one useful "
                   "intake question, offer to receive a sample, or stop. Let the learner decide the objective "
                   "before prescribing an exercise.")
NEW_GOAL_LIMIT = "No exercise or assessment criterion for this new goal has been released for this visit."


def new_goal_packet(packet):
    """Discard the previous subject's teaching instructions as well as its personal context."""
    if packet.get("receipt") or packet.get("material") or packet.get("new_task"):
        raise ValueError("new-goal intake requires an empty prior projection and no released exercise")
    condition = packet["condition"]
    return {"days": packet["days"], "learner": packet["learner"], "allowed_moves": packet["allowed_moves"],
            "receipt": {}, "material": {}, "policy": NEW_GOAL_POLICY, "evidence_limit": NEW_GOAL_LIMIT,
            "condition": {"setting": condition["setting"], "day": condition["day"],
                          "observed": condition["observed"], "evidence_mode": "unscoped"}}
