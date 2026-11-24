"""Deterministic access and teaching decisions for matched synthetic visits.

The two arms receive one identical condition. A model policy can request less
information or choose another eligible move. It cannot enlarge these bounds.
"""
from typing import Literal, TypedDict


Stage = Literal["opening", "feedback", "return"]
PROFILES = (
    "available", "withdrawal", "familiar_form", "stop", "private_answer",
    "example_only", "teacher_queue", "goal_switch", "due_choice",
    "observation_gap", "forget_topic", "assisted_history", "long_gap",
)
EVIDENCE = {
    "writer": "written_artifact", "chemical-engineering": "quantity",
    "violin": "witnessed_performance", "better-person": "written_artifact",
    "worship": "written_artifact",
}
MATERIAL = ("lesson", "worked", "practice", "excerpt")
ACTIONS = ("explain", "practice", "probe", "clarify", "defer", "stop")
MEMORY = ("focus", "move", "goal", "help", "family")


class Condition(TypedDict):
    version: int
    id: str
    domain: str
    setting: str
    days: int
    queue_ahead: int
    teacher_minutes: int
    review_minutes: int
    evidence_mode: str


class Display(TypedDict):
    display: str
    move: str
    focus: str


class Context(TypedDict):
    setting: str
    day: int
    evidence_mode: str
    observed: str


class Rules(TypedDict):
    context: Context
    material: list[str]
    actions: list[str]
    memory: list[str]
    task: bool
    answer: bool
    learner_attempt: bool
    local: Display | None


def condition_for(trial: int, domain: str) -> Condition:
    if type(trial) is not int or trial < 1 or domain not in EVIDENCE:
        raise ValueError("a positive trial and a reviewed domain are required")
    block, slot = divmod(trial - 1, len(PROFILES))
    profile = PROFILES[slot]
    setting = "classroom" if (block + slot) % 2 else "one-to-one"
    days = (7, 30, 180, 730, 1825)[(block + slot) % 5]
    if profile == "long_gap":
        days = (730, 1825)[block % 2]
    queued = profile == "teacher_queue"
    return Condition(
        version=1, id=profile, domain=domain, setting="classroom" if queued else setting,
        days=days, queue_ahead=(3 if block % 2 == 0 else 1) if queued else 0,
        teacher_minutes=10 if queued else 0, review_minutes=3 if queued else 0,
        evidence_mode=EVIDENCE[domain],
    )


def _hold(rules: Rules, message: str, move: str, focus: str) -> None:
    rules.update(material=[], actions=[move], task=False, answer=False, learner_attempt=False,
                 local=Display(display=message, move=move, focus=focus))


def visit_rules(condition: Condition, stage: Stage) -> Rules:
    if stage not in ("opening", "feedback", "return") or condition["id"] not in PROFILES:
        raise ValueError("unknown experiment stage or condition")
    returning = stage == "return"
    context = Context(setting=condition["setting"], day=condition["days"] if returning else 0,
                      evidence_mode=condition["evidence_mode"],
                      observed="The reviewed exercise and source are available. Only the submitted text is observable.")
    rules = Rules(context=context, material=list(MATERIAL), actions=list(ACTIONS), memory=list(MEMORY),
                  task=True, answer=True, learner_attempt=True, local=None)
    profile = condition["id"]
    if profile == "private_answer":
        # A later public exercise cannot authorize access to the earlier private submission.
        rules["memory"] = []
        if not returning:
            context["observed"] = "The learner kept the first response and clarification private."
            _hold(rules, "Your response can stay private. We can return to a shared exercise another time.",
                  "defer", "choice")
        else:
            context["observed"] = "The learner permits this new shared exercise. No prior private answer is available."
    elif profile == "assisted_history":
        if not returning:
            context["observed"] = "A worked demonstration was supplied before the learner submitted this response."
            rules["actions"] = ["explain", "practice", "clarify", "defer", "stop"]
        else:
            context["observed"] = "The earlier response followed supplied help. Offer the fresh task before advancing."
            rules["material"] = ["lesson", "excerpt"]
            rules["actions"] = ["practice", "probe", "clarify", "defer", "stop"]
    elif profile == "forget_topic":
        if returning:
            context["observed"] = "The learner withdrew permission to use earlier teaching metadata."
            rules["memory"] = []
    elif not returning:
        # Future events are hidden from both teachers on the first visit.
        pass
    elif profile == "withdrawal":
        context["observed"] = "Permission for the return exercise's source was withdrawn."
        _hold(rules, "The source for this exercise is on hold. We can leave the review open.",
              "defer", "source")
    elif profile == "familiar_form":
        context["observed"] = "Every released check for this target has already been seen. Guided review remains available."
        rules.update(material=["lesson", "worked", "excerpt"], actions=["explain", "clarify", "defer", "stop"],
                     task=False, learner_attempt=False)
    elif profile == "stop":
        context["observed"] = "The learner explicitly ended today's meeting before any activity or model dispatch."
        rules["memory"] = []
        _hold(rules, "We can stop here.", "stop", "choice")
    elif profile == "example_only":
        context["observed"] = "The learner requested one reviewed example and no further questions today."
        rules.update(material=["worked", "excerpt"], actions=["explain", "defer", "stop"],
                     task=False, learner_attempt=False)
    elif profile == "teacher_queue":
        ahead, minutes, duration = (condition[k] for k in ("queue_ahead", "teacher_minutes", "review_minutes"))
        context["observed"] = f"The learner asked for an individual review. {ahead} requests are ahead, with {minutes} minutes left and {duration} minutes per review."
        if (ahead + 1) * duration <= minutes:
            _hold(rules, "A teacher review fits this meeting. The exercise can wait for that conversation.",
                  "defer", "choice")
        else:
            rules.update(material=["lesson", "worked", "practice"], actions=["explain", "practice", "defer", "stop"],
                         task=False, learner_attempt=False)
    elif profile == "goal_switch":
        context["observed"] = "The learner replaced the previous aim. The old booking has been released; the new aim has no reviewed exercise yet."
        rules.update(memory=[], material=[], actions=["clarify", "defer", "stop"], task=False, learner_attempt=False)
    elif profile == "due_choice":
        context["observed"] = "An earlier review is due. The learner first wants to choose between that review and today's other goal."
        rules.update(material=[], actions=["clarify", "defer", "stop"], task=False, learner_attempt=False)
    elif profile == "observation_gap":
        needed = {"written_artifact": "the actual written artifact", "quantity": "a numerical submission",
                  "witnessed_performance": "a witnessed performance"}[condition["evidence_mode"]]
        context["observed"] = f"The learner described practice. The record lacks {needed}; only self-report is observable."
        rules.update(material=["lesson", "worked", "excerpt"], actions=["explain", "clarify", "defer", "stop"],
                     task=False, learner_attempt=False)
    elif profile == "long_gap":
        context["observed"] = "Intervening study and exposure are unknown. Confirm the current goal before scheduling an independent check."
        rules.update(memory=["goal"], material=["lesson", "excerpt"], actions=["clarify", "explain", "defer", "stop"],
                     task=False, learner_attempt=False)
    return rules


def first_context(condition: Condition) -> Context:
    return visit_rules(condition, "opening")["context"]


def return_context(condition: Condition) -> Context:
    return visit_rules(condition, "return")["context"]


def allowed_material(condition: Condition, stage: Stage) -> list[str]:
    return visit_rules(condition, stage)["material"]


def allowed_actions(condition: Condition, stage: Stage) -> list[str]:
    return visit_rules(condition, stage)["actions"]


def fallback_for(condition: Condition, stage: Stage) -> Display:
    rules = visit_rules(condition, stage)
    if rules["local"] is not None:
        return rules["local"]
    # Defer is legal on every generated route. Never synthesize replacement exercise text.
    return Display(display="We can leave this activity open for another visit.", move="defer", focus="choice")
