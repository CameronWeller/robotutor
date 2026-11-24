"""Run paired, two-visit teaching-policy experiments on the reviewed synthetic packs.

Each trial designs one revision against its predecessor, executes both arms, and
hands blinded transcripts to a separate reviewer. A trial number is an execution
ID. Decisions about system-round credit require inspection of the actual traces.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
from teaching_audit import invoke_auditor
from assessment_choice import CHOICES as ASSESSMENT_CHOICES, assessment_route, local_assessment, required_reference
from learning_conditions import PROFILES, condition_for, fallback_for, visit_rules
from decision_witness import (approved_topic, evaluate_witness, hypothesis_schema, offered_topics,
                              topic_confirmation, validate_design_witness)
from teaching_consent import authorized_memory, memory_options, revise_booking, validate_choices
from teaching_material import DOMAINS, PACKS, pack


MOVES = ("explain", "practice", "probe", "clarify", "defer", "stop")
FOCI = ("condition", "reasoning", "source", "observation", "choice", "unknown")
FIELDS = ("focus", "move", "goal", "help", "family", "topic")
FIELD_NAMES = ("chosen focus", "recorded teaching move", "course goal", "recorded assistance", "exercise family",
               "booked lesson")
MATERIAL = ("lesson", "worked", "practice", "excerpt")
RETURN_CHOICES = ("keep", "cancel_topic", "forget_all", "change_goal")
BASE = {"teaching": "Help with the current task and then offer practice.",
        "returning": "Resume the goal on a different task using the available history.",
        "memory": ["goal"], "retrieve": ["excerpt", "lesson"]}


def encode(obj):
    return json.dumps(obj, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def study_prompt(instruction, packet):
    return ("Work only with this synthetic experiment. Use no tools or external facts. "
            "Return one JSON object without markdown.\n" + instruction + "\n" + encode(packet))


def request_identity(model, prompt):
    return hashlib.sha256((model + "\n" + prompt).encode()).hexdigest()


def save(path, value):
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent,
                                     suffix=".pending", delete=False) as f:
        pending = Path(f.name)
        f.write(encode(value) + "\n")
        f.flush()
        os.fsync(f.fileno())
    pending.replace(path)


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


@contextmanager
def exclusive_study(root):
    root.mkdir(parents=True, exist_ok=True)
    # The OS releases this lock on exit, including an interrupted model call.
    with (root / ".study.lock").open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield


def text(value, limit, field="text"):
    if not isinstance(value, str) or not value.strip() or len(value) > limit:
        raise ValueError(f"{field} must be nonempty text of at most {limit} characters")
    return value


def subset(value, choices):
    if not isinstance(value, list) or any(v not in choices for v in value) or len(value) != len(set(value)):
        raise ValueError("invalid selection")
    return value


def policy(value, protocol=4):
    result = {"teaching": text(value["teaching"], 900, "teaching"), "returning": text(value["returning"], 900, "returning"),
              "memory": subset(value["memory"], FIELDS), "retrieve": subset(value["retrieve"], MATERIAL)}
    if protocol >= 8:
        mode = value["resume_mode"]
        if mode not in ("current", "booked", "compare"):
            raise ValueError("choose a reviewed lesson route")
        result["resume_mode"] = mode
    return result


def object_schema(fields):
    return {"type": "object", "properties": fields, "additionalProperties": False,
            "required": list(fields)}


def shape(stage, protocol=4):
    string = {"type": "string"}
    def strings(names):
        return {name: string for name in names.split()}
    def labels(names):
        return {"type": "string", "enum": list(names)}
    if stage == "design":
        fields = strings("question change prediction falsifier")
        fields["hypothesis"] = hypothesis_schema(protocol=protocol) if protocol >= 8 else hypothesis_schema()
        declared = {**strings("teaching returning"),
                        "memory": {"type": "array", "items": labels(FIELDS)},
                        "retrieve": {"type": "array", "items": labels(MATERIAL)}}
        if protocol >= 8:
            declared["resume_mode"] = labels(("current", "booked", "compare"))
        fields["policy"] = object_schema(declared)
    elif stage == "case":
        fields = {**strings("first_answer surprise return_request"), "topic_selection": labels(("first","return","none")),
                  "days": {"type": "integer"}, "save_topic": {"type": "boolean"}}
        if protocol >= 5:
            fields["return_work"] = {"type": ["string", "null"]}
        if protocol >= 6:
            fields["memory_choices"] = {"type": "array", "items": labels(memory_options())}
            fields["selected_focus"] = {"type": ["string", "null"], "enum": [None, *FOCI]}
        if protocol >= 8:
            fields.update({k: {"type": ["string", "null"]} for k in ("resume_request", "resume_work")})
            fields["resume_keep"] = {"type": "boolean"}
            fields["return_intent"] = labels(RETURN_CHOICES)
        if protocol >= 9:
            fields.update({key: {"type": "array", "items": labels(FIELDS)} for key in ("return_fields", "resume_fields")})
        if protocol >= 10:
            fields["assessment_choice"] = labels(ASSESSMENT_CHOICES)
    elif stage in ("review", "review-grounded"):
        fields = {**strings("early_difference later_difference evidence weakness next_question"),
                  "preferred": labels(("A", "B", "tie", "neither"))}
        if stage == "review-grounded":
            fields["prediction"] = labels(("supported", "refuted", "unobserved"))
    elif stage.endswith("-learner"):
        fields = strings("response")
    else:
        fields = {"display": string, "focus": labels(FOCI), "move": labels(MOVES)}
    return object_schema(fields)


class Model:
    def __init__(self, name, directory, protocol=4):
        self.name = name
        self.directory = directory
        self.protocol = protocol

    def call(self, stage, instruction, packet, check):
        prompt = study_prompt(instruction, packet)
        identity = request_identity(self.name, prompt)
        path = self.directory / (stage + ".json")
        if path.exists():
            receipt = read(path)
            if receipt.get("stage_hash", receipt["request_hash"]) != identity:
                raise ValueError("saved stage belongs to a different request")
            return check(receipt["response"])
        attempts = 3 if self.protocol >= 8 else 2
        for attempt in range(attempts):
            try:
                return self.request(stage, prompt, packet, path, identity, check)
            except (ValueError, KeyError, TypeError) as exc:
                if attempt + 1 == attempts:
                    raise
                prompt += ("\nThe prior candidate was rejected: " + str(exc)[:200] +
                           ". Return a corrected, concise object complying with the declared fields and limits.")

    def request(self, stage, prompt, packet, path, identity, check):
        with tempfile.TemporaryDirectory(prefix="robotutor-trial-") as working:
            output = Path(working) / "answer"
            schema = Path(working) / "schema.json"
            schema.write_text(encode(shape(stage, self.protocol)), encoding="utf-8")
            args = [shutil.which("codex") or "codex", "exec", "--ignore-user-config",
                    "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only",
                    "--model", self.name, "--json", "--output-schema", str(schema),
                    "--output-last-message", str(output), "-"]
            start = time.monotonic()
            try:
                proc = subprocess.run(args, cwd=working, input=prompt.encode(), capture_output=True, timeout=160)
            except subprocess.TimeoutExpired as exc:
                self.transport_failure(stage, identity, time.monotonic() - start, "timeout")
                raise RuntimeError(f"{stage}: live model call timed out") from exc
            elapsed = round(time.monotonic() - start, 3)
            if proc.returncode or not output.exists():
                detail = (proc.stdout + proc.stderr).lower()
                category = "provider or CLI failure"
                if any(s in detail for s in (b"rate limit", b"rate_limit", b"usage limit", b"usage_limit", b"quota")):
                    category = "quota or rate limit"
                elif any(s in detail for s in (b"unauthorized", b"login required", b"authentication")):
                    category = "authentication"
                self.transport_failure(stage, identity, elapsed, category)
                raise RuntimeError(f"{stage}: {category} (exit {proc.returncode})")
            raw = output.read_text(encoding="utf-8").strip()
            if raw.startswith("```json\n") and raw.endswith("```"):
                raw = raw[8:-3].strip()
            usage = None
            for line in proc.stdout.splitlines():
                try:
                    event = json.loads(line)
                except (UnicodeError, ValueError):
                    continue
                if event.get("type") == "turn.completed":
                    usage = {k: v for k, v in (event.get("usage") or {}).items()
                             if k.endswith("_tokens") and isinstance(v, int)}
            actual = request_identity(self.name, prompt)
            try:
                checked = check(json.loads(raw))
            except (ValueError, KeyError, TypeError) as exc:
                with (self.directory / (stage + "-rejections.jsonl")).open("a", encoding="utf-8") as log:
                    log.write(encode({"request_hash": actual, "reason": str(exc)[:200],
                                      "seconds": elapsed, "tokens": usage}) + "\n")
                raise
            save(path, {"request_hash": actual, "stage_hash": identity, "model_requested": self.name, "seconds": elapsed,
                        "tokens": usage, "packet": packet, "response": checked})
            return checked

    def transport_failure(self, stage, identity, elapsed, category):
        with (self.directory / (stage + "-failures.jsonl")).open("a", encoding="utf-8") as log:
            log.write(encode({"request_hash": identity, "reason": category, "seconds": round(elapsed, 3),
                              "usage": "provider acceptance unknown"}) + "\n")


def design(model, domain, course, incumbent, prior):
    instructions = """Design a transferable revision to how a tutor selects instruction,
receives context, records a learner-approved fact, and resumes next time. Base it on the
last comparison's unresolved problem and the learning goal. An independent case author
will generate the inputs AFTER you commit your proposal. Your policy must work without
any case-specific names, answer keys, inferred intentions or invented learner history.
Output keys question, change, prediction, falsifier (each under 400 characters), hypothesis
and policy. The hypothesis preregisters ONE observable difference for this single pair of
visits. Use stage opening/feedback/return, channel material/memory/move, feature (exactly
one allowed field or move), direction candidate_only/incumbent_only. Memory observations
apply at return only. For material/memory the feature must be selected exclusively by
the arm named in direction. A withheld field or unchanged move can falsify the prediction.
Propose a useful difference for this goal; an abstract future population comparison cannot
answer this trial. Also explain what instructional benefit, damage or null result to look
for in the displayed wording. Policy contains teaching and returning (instructions each
<=900 characters), memory (subset of focus, move, goal, help, family, topic), retrieve (subset of
lesson, worked, practice, excerpt). The goal field holds the coarse initial goal; focus is one
of condition/reasoning/source/observation/choice/unknown; move is the displayed move,
family is the initial item family, help is supplied or unknown. Topic is a learner-chosen
reviewed lesson unit bound to its domain and edition. Only the application confirms what
was booked. There is no durable slot for custom
wording or inferred ability. At least one change must alter the data path. Propose an
operational prediction falsifiable on two realistically different tutoring paths with the
same learner input. Distinguish what would require later validation with real learners.
Account for the previous finding. Avoid restating a recent hypothesis with new nouns.
Policies must react to what is actually available at each visit. Neither tutor is told
which future condition will occur. Aim for
different downstream teaching; a restyled acknowledgment is insufficient. Do not include
personal information, exam keys, browsing instructions, or presupposed gains in a learner."""
    if model.protocol >= 5:
        instructions += """
The study plan lists the actual common operating condition, authorized stages, and a
research focus drawn from prior design failures. Your design is blind to learner answers
and selections. Pick a stage in which teaching can actually run. Predict ONE specific
change to what the learner reads at that stage, on this pair, and a contrary observation
that would refute it. Address the last observed finding in the new situation. Do not
require a second future condition, multiple learners, physical observation or an
unreleased task. Where the learner brings work on return, a distinct practice prompt
and new submission are shared with the tutor. The external assessment is held out.
The task and current learner input are common to both arms. A retrieved field or
elected move alone is insufficient to demonstrate an instructional benefit."""
    if model.protocol >= 6:
        instructions += """
The learner separately chooses whether to keep the course goal, assistance record,
activity family or tutor move. A requested focus can hold only the learner's selected
label. Electing a lesson authorizes the topic alone. Study these choices without
assuming what the participant will select. Consider the last actual contrast and the
previous comparison in this domain. Retain useful prior instruction when proposing a
transferable revision. The primary behavioral forecast must be observable in this pair."""
    if model.protocol >= 7:
        instructions += """
The incumbent was selected for this discipline. Preserve the evidence-supported lesson
it provides. Other disciplines' findings may suggest a new comparison here. A benefit
observed in another discipline needs a new comparison before adoption in this one.
Design for other learners in the current domain. State a single behavioral prediction
that the actual paired displays can settle, even if the candidate is later rejected."""
    if model.protocol >= 8:
        instructions = instructions.replace(
            "An independent case author\nwill generate the inputs AFTER you commit your proposal.",
            "An independent case author receives no proposal. Both design and case are frozen before teaching.")
        instructions = instructions.replace(
            "Use stage opening/feedback/return, channel material/memory/move",
            "Use the planned effective stage return/resume, channel material/memory/move/selection")
        instructions = instructions.replace("Memory observations\napply at return only.",
                                              "Memory observations apply at the effective later visit.")
        instructions += """
Use last_finding.effective_stage for the mandatory hypothesis. Trace an unresolved
teaching decision from the first encounter into that later visit. The learner comes
with an actual new attempt at a released practice problem. Specify one difference
in the instruction displayed for that attempt and what would refute it. Do not
relabel an earlier question or test only a first-reply move. Choose resume_mode:
current selects the practice release for today's work, booked selects the approved
lesson if available (default for older policies), compare supplies reviewed material
from the current and first releases together. Retrieval and sharing limits still
apply. The selection channel can test first_unit, return_unit or contrast_material.
Distinguish showing supporting material from using it in instruction. If the original
return could not host this interaction, a separate learner-initiated meeting supplies
it under fresh sharing and release permission. Never turn a stop into a retry."""
    if model.protocol >= 9:
        instructions += """
The participant selects exactly which previously authorized fields may reach each
visit. Saving an earlier focus does not imply that today's tutor may receive it.
Use the currently shared submission when earlier fields are absent."""
    if model.protocol >= 10:
        instructions += """
Submitting a draft for formative feedback does not authorize a separate assessed
response. The participant can submit, practice privately or skip the separate check.
Your instructional prediction must be decidable from what the teacher actually
received and displayed. Review recent_domain_designs. Name the previous mechanism
you are changing in the change field and describe the new operational decision.
Retesting a gate or supplying the same example with new nouns is not a new design.
If following a failure, specify which missing situation this planned visit supplies."""
    def validate(v):
        result = {k: text(v[k], 650, k) for k in ("question", "change", "prediction", "falsifier")}
        result["policy"] = policy(v["policy"], model.protocol)
        witness_args = {"protocol": model.protocol} if model.protocol >= 8 else {}
        result["hypothesis"] = validate_design_witness(v["hypothesis"], result["policy"], incumbent, **witness_args)
        if model.protocol >= 5:
            hypothesis = result["hypothesis"]
            dispatch = prior["planned_visit"]["stages"][hypothesis["stage"]]
            channel = hypothesis["channel"]
            permission = {"move": "actions", "memory": "memory", "material": "material"}.get(channel)
            if not dispatch["model"] or (permission and hypothesis["feature"] not in dispatch[permission]):
                raise ValueError("choose a hypothesis that can be observed under the planned stage permissions")
        if model.protocol >= 8 and hypothesis["stage"] != prior["effective_stage"]:
            raise ValueError("the hypothesis has to target the planned effective later teaching stage")
        if (all(set(result["policy"][k]) == set(incumbent[k]) for k in ("retrieve", "memory"))
                and result["policy"].get("resume_mode", "booked") == incumbent.get("resume_mode", "booked")):
            raise ValueError("revision changed neither retrieval, authorized memory nor lesson selection")
        return result
    return model.call("design", instructions,
                      {"domain": domain, "goal": course["scope"]["goal"], "incumbent": incumbent,
                       "last_finding": prior, "evidence_mode": course["evidence_mode"]}, validate)


def case(model, course, condition):
    offers = offered_topics(condition["domain"])
    practice = course["return_pack"]["practice"] if visit_rules(condition, "return")["learner_attempt"] else None
    later_practice = course["return_pack"]["practice"] if practice is None and model.protocol >= 8 else None
    def validate(v):
        result = {k: text(v[k], 650) for k in ("first_answer", "surprise", "return_request")}
        if type(v["days"]) is not int or v["days"] != condition["days"] or type(v["save_topic"]) is not bool:
            raise ValueError("invalid horizon or sharing choice")
        selection = v["topic_selection"]
        if selection not in offers or v["save_topic"] != (selection != "none"):
            raise ValueError("select first or return for a saved booking; select none to decline")
        if condition["id"] == "private_answer" and selection != "none":
            raise ValueError("the private encounter does not authorize a booking")
        result.update(days=v["days"], save_topic=v["save_topic"], topic_selection=selection)
        if model.protocol >= 5:
            if practice is None and v["return_work"] is not None:
                raise ValueError("this return has no shared practice submission")
            result["return_work"] = text(v["return_work"], 650, "return_work") if practice else None
        if model.protocol >= 6:
            result.update(validate_choices(v, private=condition["id"] == "private_answer"))
        if model.protocol >= 8:
            if (type(v["resume_keep"]) is not bool or
                (later_practice is None and (v["resume_keep"] or v["resume_work"] is not None or v["resume_request"] is not None))):
                raise ValueError("later work and its sharing choice require a separately initiated later visit")
            result.update(resume_keep=v["resume_keep"], **{
                k: text(v[k], 650, k) if later_practice else None for k in ("resume_request", "resume_work")})
            required = {"forget_topic": ["forget_all"], "goal_switch": ["change_goal"], "withdrawal": ["cancel_topic"]}
            if v["return_intent"] not in required.get(condition["id"], ["keep", "cancel_topic"]):
                raise ValueError("choose the explicit booking operation for this planned return")
            result["return_intent"] = v["return_intent"]
        if model.protocol >= 9:
            grants = set(result["memory_choices"])
            if result["selected_focus"] is not None:
                grants.add("focus")
            if result["save_topic"] and result["return_intent"] == "keep":
                grants.add("topic")
            if result["return_intent"] in ("change_goal", "forget_all"):
                grants.clear()
            for field in ("return_fields", "resume_fields"):
                selected = subset(v[field], grants)
                result[field] = [k for k in FIELDS if k in selected]
            if visit_rules(condition, "return")["local"] and result["return_fields"]:
                raise ValueError("a locally held visit uses no earlier personal fields")
            if (not later_practice and result["resume_fields"]) or result["resume_keep"] != bool(result["resume_fields"]):
                raise ValueError("renew only the individually named fields on the separately initiated later visit")
        if model.protocol >= 10:
            choice = v["assessment_choice"]
            if choice not in ASSESSMENT_CHOICES or (practice is None and choice != "skip"):
                raise ValueError("choose an available assessment action; skip an unavailable check")
            result["assessment_choice"] = choice
        return result
    instruction = """Write a realistic fictional participant for the experiment.
Output first_answer to the first task, surprise (a later unexpected learner clarification
that could change what a teacher thinks the answer meant), topic_selection (first/return/none,
the participant's explicit response to the offered released-topic menu, before any tutor
inference), save_topic (true exactly when a menu topic was selected), days (integer 1..1825) and
return_request (what the same learner wants at the next visit). Make the return request
natural; don't recap the clarification for the tutor. Use the exact given days and
operating condition. If the learner stops, changes scope, declines to share, requests
help or has a due review, make their actual words consistent with that condition.
For the private_answer condition decline the booking. At the first visit, the participant
does not yet know future events. A stop on return can follow an earlier consented booking.
Use fictional activities and avoid
personal data or medical advice. Each text field must be under 600 characters. Generate a plausible challenge, including legitimate
alternative responses. Do not score the learner or introduce new curricular material."""
    packet = {"goal": course["scope"]["goal"], "first_task": course["first_pack"]["check"],
              "evidence_mode": course["evidence_mode"], "condition": condition, "offered_topics": offers}
    if model.protocol >= 5:
        packet["return_practice"] = practice
        instruction += """
When return_practice is supplied, write the learner's actual attempt at that exercise
in return_work and explicitly request feedback on this shared work in return_request.
The submission may reveal a defensible alternative or a difficulty. Preserve any
operating condition and consent choice, including forgetting prior work. When
return_practice is null, set return_work to null. Do not invent an observed performance."""
    if model.protocol >= 6:
        packet["memory_options"] = memory_options()
        packet["focus_options"] = list(FOCI)
        instruction += """
Independently select memory_choices from the four displayed options (an empty list
declines all four). The lesson topic and these options are separate. Pick selected_focus
from focus_options only to express the participant's own chosen focus, otherwise use
null. A private_answer visit needs an empty memory_choices list and null selected_focus.
Match the participant's words and chosen disclosure in this case."""
    if model.protocol >= 7:
        packet["study_protocol"] = model.protocol
    if model.protocol >= 8:
        packet.update(later_practice=later_practice, resume_day=condition["days"] + 30 if later_practice else None,
                      return_choices=list(RETURN_CHOICES))
        instruction += """
If later_practice is supplied, a separate synthetic meeting occurs on resume_day,
after the scheduled return has ended. The participant chooses to come back to the
original goal and share a new attempt. Write that request in resume_request and a
specific attempted solution or draft in resume_work. Set resume_keep=true only when
the participant chooses to use still-authorized bounded metadata from earlier visits.
A revoked, never-granted or private field cannot be restored. Preserve the stopped
or held earlier visit on its own terms. If later_practice is null use null for both
resume fields and false for resume_keep. Do not include the scoring exercise.
At the scheduled return the participant takes one explicit booking action. For a
withdrawal choose cancel_topic and request cancellation even though teaching is on
hold. For forget_topic choose forget_all. For goal_switch choose change_goal. For
other study conditions choose keep or cancel_topic and match the return request to
that choice. Cancelling the booking leaves other separately permitted fields alone."""
    if model.protocol >= 9:
        packet["visit_use_options"] = list(FIELDS)
        instruction += """
return_fields and resume_fields are the exact earlier fields you allow the tutor to
receive at each visit. Request only originally authorized fields, omit topic after
cancellation, and use empty lists after forget_all or change_goal. A local stop,
source hold or accepted human review has no tutor to receive return_fields. A
missing resume meeting must use an empty resume_fields. Set resume_keep to whether
resume_fields is nonempty. Match the spoken request to the named fields: requesting
only goal and family does not renew focus. Select an initial focus only if you want
that chosen focus label retained for potential use later."""
    if model.protocol >= 10:
        packet["assessment_choices"] = list(ASSESSMENT_CHOICES)
        instruction += """
Independently choose what to do about the separate assessment after feedback.
If return_practice is null the separate check is unavailable, so select skip. If
it is supplied, select submit, private_practice or skip according to the fictional
participant's own preference, and express the same choice in return_request. The
participant can share a current practice attempt for feedback while keeping their
next personal practice private. Do not include that private response in any field."""
    return model.call("case", instruction, packet, validate)


TEACHING_INSTRUCTION = """Act as the tutor following the policy and the learner's request. Display a
specific useful teaching response in at most 110 words, grounded in available lesson material.
Decide the move actually displayed and the focus a learner could choose to revisit.
Output keys display, move (explain/practice/probe/clarify/defer/stop), focus
(condition/reasoning/source/observation/choice/unknown). You cannot issue a grade or confirm
a saved topic. The application displays its own confirmation from the actual booking."""


def validate_display(v):
    if v["move"] not in MOVES or v["focus"] not in FOCI:
        raise ValueError("unsupported move or focus")
    return {"display": text(v["display"], 1700), "move": v["move"], "focus": v["focus"]}


def tutor_contract(model_name, packet):
    prompt = study_prompt(TEACHING_INSTRUCTION, packet)
    return prompt, request_identity(model_name, prompt), validate_display


def teach(model, stage, packet):
    return model.call(stage, TEACHING_INSTRUCTION, packet, validate_display)


def local_receipt(model, stage, packet, response):
    receipt = {"origin": "local", "model_requested": None, "seconds": 0, "tokens": None,
               "packet": packet, "response": response}
    path = model.directory / (stage + ".json")
    if path.exists() and read(path) != receipt:
        raise ValueError("saved local response belongs to a different input")
    save(path, receipt)
    return response


def display(model, name, stage, packet, condition, rules):
    label = name + "-" + stage
    if rules["local"]:
        return local_receipt(model, label, packet, rules["local"])
    proposed = teach(model, label, packet)
    path = model.directory / (label + ".json")
    receipt = read(path)
    proposed = receipt.get("proposed_response", proposed)
    if proposed["move"] in rules["actions"]:
        receipt["origin"] = "model"
    else:
        receipt["origin"] = "fallback"
        receipt["proposed_response"] = proposed
        receipt["response"] = ({"display": "We can leave this activity open for another visit.",
                                "move": "defer", "focus": "choice"} if stage == "resume" else fallback_for(condition, stage))
    save(path, receipt)
    return receipt["response"]


def shared_case(scenario, stages):
    visible = dict(scenario)
    if not stages["opening"]["answer"]:
        visible.pop("first_answer")
    if not stages["feedback"]["answer"]:
        visible.pop("surprise")
        visible.pop("topic_selection")
        visible.pop("memory_choices", None)
        visible.pop("selected_focus", None)
    return visible


def use_note(memory):
    visible = [label for key, label in zip(FIELDS, FIELD_NAMES) if key in memory]
    if not visible:
        return "No earlier personal context was made available for this activity."
    return "Earlier context made available for this activity includes " + ", ".join(visible) + "."


def arm(model, name, course, scenario, variant, condition, stages):
    first = course["first_pack"]
    common = {"goal": course["scope"]["goal"], "evidence_limit": course["limit"]}
    def context(stage, release, instruction):
        rules = stages[stage]
        return {**common, "policy": instruction, "condition": rules["context"],
                "allowed_moves": rules["actions"],
                "material": {k: release[k] for k in variant["retrieve"] if k in rules["material"]}}

    def initial(stage):
        packet = context(stage, first, variant["teaching"])
        if stages[stage]["task"]:
            packet["task"] = first["check"]
        if stages[stage]["answer"]:
            packet["answer"] = scenario["first_answer"]
        return packet

    opening = display(model, name, "opening", initial("opening"), condition, stages["opening"])
    feedback_packet = {**initial("feedback"), "displayed": opening["display"]}
    offers = offered_topics(condition["domain"])
    booking = approved_topic(condition["domain"], scenario["topic_selection"], offers)
    if stages["feedback"]["answer"]:
        feedback_packet["learner"] = scenario["surprise"]
        feedback_packet["approved_topic"] = booking
    followup = display(model, name, "feedback", feedback_packet, condition, stages["feedback"])
    # The synthetic participant permits the stated topic to travel. Teaching metadata
    # and a bounded focus label can survive. Free text and inferred traits have no slot.
    allowed = {"focus": followup["focus"], "move": followup["move"], "goal": common["goal"],
               "help": "supplied" if condition["id"] == "assisted_history" else "unknown",
               "family": first["unit"]}
    if booking is not None:
        allowed["topic"] = booking
    if model.protocol >= 6:
        stored = authorized_memory(scenario, variant, allowed, stages["feedback"]["memory"])
    else:
        stored = {key: allowed[key] for key in variant["memory"] if key in allowed
                  and key in stages["feedback"]["memory"]} if scenario["save_topic"] else {}
    record = {"format": "paired-booking/1", "stored": stored}
    path = model.directory / (name + "-booking.json")
    if path.exists() and read(path) != record:
        raise ValueError("saved booking differs from the authorized first-visit result")
    save(path, record)
    stored = read(path)["stored"]
    confirmation = topic_confirmation(condition["domain"], stored.get("topic"))
    retained = stored
    if model.protocol >= 5:
        withdraw = condition["id"] in ("forget_topic", "goal_switch")
        state = {"format": "paired-return-memory/1", "stored": {} if withdraw else stored,
                 "reason": condition["id"] if withdraw else "carry"}
        if model.protocol >= 8:
            state = revise_booking(stored, scenario["return_intent"])
            retained = state["stored"]
        return_path = model.directory / (name + "-return-memory.json")
        if return_path.exists() and read(return_path) != state:
            raise ValueError("persisted return state differs from the learner's current authorization")
        save(return_path, state)
        retained = read(return_path)["stored"]
    memory = {key: value for key, value in retained.items() if key in stages["return"]["memory"]}
    if model.protocol >= 9:
        memory = {k: v for k, v in memory.items() if k in scenario["return_fields"]}
    fresh = course["return_pack"]
    def visit_packet(stage, authorized, request, day, effective):
        slot = "return"
        offer = None
        if "topic" in authorized:
            current_offers = offered_topics(condition["domain"])
            matches = [s for s in ("first", "return")
                       if approved_topic(condition["domain"], s, current_offers) == authorized["topic"]]
            if len(matches) != 1:
                raise ValueError("the saved topic no longer resolves to a reviewed lesson")
            slot = matches[0]
            offer = current_offers[slot]
        route = None
        if effective and model.protocol >= 8:
            from resumed_work import lesson_route
            route = lesson_route(variant.get("resume_mode", "booked"), condition["domain"], authorized.get("topic"))
            slot = route["slot"]
        packet = {**context(stage, course[slot + "_pack"], variant["returning"]), "days": day,
                  "learner": request, "receipt": authorized}
        if offer:
            packet["booked_lesson"] = offer
        if route:
            packet["lesson_route"] = route
            packet["comparison_material"] = ({k: first[k] for k in variant["retrieve"] if k in stages[stage]["material"]}
                                             if route["mode"] == "compare" else {})
        return packet
    returned = visit_packet("return", memory, scenario["return_request"], scenario["days"], "resume" not in stages)
    if condition["id"] == "goal_switch":
        returned.pop("goal")
        if model.protocol >= 10:
            from resumed_work import new_goal_packet
            returned = new_goal_packet(returned)
    if stages["return"]["task"]:
        returned["new_task"] = fresh["practice"] if model.protocol >= 5 else fresh["check"]
    if model.protocol >= 5 and stages["return"]["learner_attempt"]:
        returned["shared_work"] = scenario["return_work"]
    next_lesson = display(model, name, "return", returned, condition, stages["return"])
    eligible = stages["return"]["learner_attempt"]
    route = (assessment_route(eligible, scenario["assessment_choice"]) if model.protocol >= 10
             else "submitted" if eligible else "not_offered")
    reference = required_reference(fresh) if model.protocol >= 10 and route == "submitted" else None
    if route == "submitted":
        contents = {"learner_history": shared_case(scenario, stages), "goal": common["goal"],
                    "earlier_instruction": [opening["display"], followup["display"], confirmation],
                    "new_task": fresh["check"], "current_instruction": next_lesson["display"]}
        if reference:
            contents["required_reference"] = reference
        reply = model.call(name + "-learner", """Role-play the fictional learner attempting the
changed task with the instruction that was actually displayed. Preserve the learner's
chosen scope, plausible difficulties and any decision to decline. Respond only with
{"response":"..."} under 500 characters. Do not assume everything taught was learned.
You have no scoring key or physical observation of a performance.""",
                       contents,
                       lambda r: {"response": text(r["response"], 650)})
    else:
        args = local_assessment(eligible, scenario.get("assessment_choice", "skip"), stages["return"]["context"])
        reply = local_receipt(model, name + "-learner", *args)
    trace = {"opening": opening, "feedback": followup, "stored": stored, "confirmation": confirmation,
             "memory": memory, "return": next_lesson, "modeled_attempt": reply["response"]}
    if model.protocol >= 8:
        confirmations = {"carry": "Your earlier sharing choices still apply.",
                         "cancel_topic": "No lesson topic is booked now.",
                         "forget_topic": "No earlier teaching context is carried into this visit.",
                         "goal_switch": "No earlier teaching context is carried into this visit."}
        trace["return_confirmation"] = confirmations[read(return_path)["reason"]]
    if model.protocol >= 9:
        trace["return_use_note"] = use_note(memory)
    if model.protocol >= 10:
        trace["assessment_route"] = route
        trace["assessment_reference"] = reference
    if "resume" in stages:
        rule = stages["resume"]
        authorized = ({k: value for k, value in retained.items() if k in rule["memory"]}
                      if scenario["resume_keep"] else {})
        if model.protocol >= 9:
            authorized = {k: v for k, v in authorized.items() if k in scenario["resume_fields"]}
        if "move" in authorized:
            authorized["move"] = next_lesson["move"]
        state = {"format": "paired-resume-memory/1", "stored": authorized}
        path = model.directory / (name + "-resume-memory.json")
        if path.exists() and read(path) != state:
            raise ValueError("later memory does not match the current sharing choice")
        save(path, state)
        authorized = read(path)["stored"]
        packet = visit_packet("resume", authorized, scenario["resume_request"], rule["context"]["day"], True)
        packet.update(new_task=fresh["practice"], shared_work=scenario["resume_work"])
        trace["resume"] = display(model, name, "resume", packet, condition, rule)
        trace["resume_memory"] = authorized
        if model.protocol >= 9:
            trace["resume_use_note"] = use_note(authorized)
    return trace


def review_grounded(model, snapshot, candidate, incumbent, proposed):
    material = {}
    visits = ["opening", "feedback", "return"]
    if "resume" in snapshot["traces"]["A"]:
        visits.append("resume")
    for name in ("A", "B"):
        material[name] = {visit: read(model.directory / f"{name}-{visit}.json")["packet"]["material"]
                          for visit in visits}
    packet = {**snapshot, "material_actually_retrieved": material}
    if "witness" in snapshot:
        dispatched = {}
        for name in ("A", "B"):
            dispatched[name] = {}
            for stage in visits:
                r = read(model.directory / f"{name}-{stage}.json")
                task = r["packet"].get("new_task", r["packet"].get("task"))
                dispatched[name][stage] = None if r.get("origin") == "local" else task
        packet["task_at_dispatch"] = dispatched
    if model.protocol >= 5:
        # Judge the displayed teaching independently of the mechanical pass/fail result.
        packet.pop("witness")
        packet["shared_work_at_return"] = {
            arm: read(model.directory / f"{arm}-return.json")["packet"].get("shared_work") for arm in ("A", "B")
        }
    if model.protocol >= 8:
        stage = visits[-1]
        packet["shared_work_at_resume"] = {arm: (read(model.directory / f"{arm}-resume.json")["packet"]["shared_work"]
                                                 if "resume" in visits else None) for arm in ("A", "B")}
        packet["effective_lesson_material"] = {arm: {
            k: read(model.directory / f"{arm}-{stage}.json")["packet"][k]
            for k in ("lesson_route", "comparison_material")} for arm in ("A", "B")}

    def validate(v):
        if v["preferred"] not in ("A", "B", "tie", "neither"):
            raise ValueError("invalid arm")
        if v["prediction"] not in ("supported", "refuted", "unobserved"):
            raise ValueError("invalid verdict on the prediction")
        return {"preferred": v["preferred"], "prediction": v["prediction"],
                **{k: text(v[k], 1500) for k in ("early_difference", "later_difference",
                                                  "evidence", "weakness", "next_question")}}
    instruction = """Evaluate the two anonymous tutors against the stated prediction and falsifier.
Inspect their actual displays, approved typed memory, dispatched tasks and EXACT material each arm received
before judging source support. Assess whether the predicted benefit is observed. Return
prediction=supported/refuted/unobserved. A helpful comment on an unrelated dimension cannot
establish the prediction. If the predicted endpoint needs real learners or more comparable
cases, mark it unobserved. The role-played final response passes through a separate assessor
which supplies the new task when its release is eligible even if the tutor did not. Preserve
this distinction. For a claim about where a fact came from, identify its specific task
or material field at dispatch. A true field-presence witness alone cannot settle teaching
quality. Compare learner-visible actions at both visits. Include
preferred=A/B/tie/neither and early_difference, later_difference, evidence, weakness,
next_question, each under 800 characters."""
    if model.protocol >= 8:
        instruction += """
Judge the change on the effective final teaching stage. When resume is present it is
a separate learner-initiated later visit, and the preceding hold or stop stands.
Inspect the lesson_route, public comparison_material, new shared attempt, and actual
display. Trace any relevant first-visit choice or actual teacher decision into this
later instruction. Identify one result an editor could retain, revise or reject, or
the precise missing precondition. A real learning gain cannot be read off this trace."""
    if model.protocol >= 10:
        instruction += """
Read each assessment_route. Private practice, an elected skip and an unavailable
check collect no response. A prepared assessment exercise is not a delivered test
or performance on those paths. Respect the scope of anything the learner elected
to share for formative feedback."""
    verdict = model.call("review-grounded", instruction, packet, validate)
    witnessed = "witness" not in snapshot or snapshot["witness"]["met"]
    adopt = verdict["preferred"] == candidate and verdict["prediction"] == "supported" and witnessed
    return {"review": verdict, "incumbent_after": proposed if adopt else incumbent, "adopted": adopt}


def reassess(directory, model_name):
    decision = read(directory / "decision.json")
    design_receipt = read(directory / "design.json")
    finding = review_grounded(Model(model_name, directory), read(directory / "review.json")["packet"],
                              decision["candidate"], design_receipt["packet"]["incumbent"],
                              design_receipt["response"]["policy"])
    save(directory / "reassessment.json", finding)
    print(decision["trial"], finding["review"]["prediction"], finding["adopted"], flush=True)


def scoped_incumbent(root, trial, domain):
    for i in range(trial - 1, 0, -1):
        path = root / f"{i:03d}" / "decision.json"
        decision = read(path)
        if (decision["domain"] == domain and decision.get("protocol", 1) >= 4
                and decision["adopted"]):
            return decision["incumbent_after"], {
                "scope": domain, "origin": "domain-adoption", "trial": i,
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    if (root / "seed.json").exists():
        seed = read(root / "seed.json")
        if seed.get("domain") == domain or ("domain" not in seed and trial == 1):
            return seed["finding"]["incumbent_after"], {
                "scope": domain, "origin": "external-seed", "trial": None, "sha256": seed["parent_sha256"]}
    return BASE, {"scope": domain, "origin": "baseline", "trial": None, "sha256": None}


def run(root, n, model_name):
    directory = root / f"{n:03d}"
    directory.mkdir(parents=True, exist_ok=True)
    previous = read(root / f"{n-1:03d}" / "decision.json") if n > 1 else None
    if n == 1 and (root / "seed.json").exists():
        previous = read(root / "seed.json")["finding"]
    incumbent = previous["incumbent_after"] if previous else BASE
    if (directory / "parent-correction.json").exists():
        correction = read(directory / "parent-correction.json")
        source_n = correction["source_trial"]
        if not 1 <= source_n < n:
            raise ValueError("correction must cite an earlier completed trial")
        source_dir = root / f"{source_n:03d}"
        old_file, new_file = source_dir / "decision.json", source_dir / "reassessment.json"
        for key, file in (("original_sha256", old_file), ("reassessment_sha256", new_file)):
            if correction[key] != hashlib.sha256(file.read_bytes()).hexdigest():
                raise ValueError("correction cites another version of the decision")
        invalidated = read(old_file)["incumbent_after"]
        if invalidated != incumbent or any(
            read(root / f"{i:03d}" / "decision.json")["incumbent_after"] != invalidated
            for i in range(source_n + 1, n)
        ):
            raise ValueError("the incumbent has already changed; this correction needs a new lineage")
        incumbent = read(new_file)["incumbent_after"]
    domain = DOMAINS[(n - 1) % len(DOMAINS)]
    course = pack(domain)
    # An interrupted study resumes its original prompt and contract.
    versions = set()
    for stage, v5_key, v6_key in (("design", "planned_visit", "previous_contrast"),
                                  ("case", "return_practice", "memory_options")):
        path = directory / (stage + ".json")
        if path.exists():
            submitted = read(path)["packet"]
            if stage == "design":
                submitted = submitted["last_finding"]
            if "assessment_choices" in submitted or submitted.get("study_protocol") == 10:
                versions.add(10)
            elif "visit_use_options" in submitted or submitted.get("study_protocol") == 9:
                versions.add(9)
            elif "effective_stage" in submitted or submitted.get("study_protocol") == 8:
                versions.add(8)
            elif "incumbent_source" in submitted or submitted.get("study_protocol") == 7:
                versions.add(7)
            else:
                versions.add(6 if v6_key in submitted else 5 if v5_key in submitted else 4)
    if len(versions) > 1:
        raise ValueError("cached design and case belong to different protocols")
    protocol = next(iter(versions), 10)
    model = Model(model_name, directory, protocol)
    earlier = [read(root / f"{i:03d}" / "design.json")["response"]["question"]
               for i in range(max(1, n-6), n)]
    prior = {"last_review": previous["review"] if previous else
             "A correct follow-up can still leave the writer's purpose unexplained at return.",
             "recent_questions": earlier, "possible_operating_conditions": list(PROFILES)}
    condition = condition_for(n, domain)
    stages = {s: visit_rules(condition, s) for s in ("opening", "feedback", "return")}
    for stage, rule in stages.items():
        if "focus" in rule["memory"] and not (stage == "return" and condition["id"] == "withdrawal"):
            rule["memory"].append("topic")
    if protocol >= 8:
        from resumed_work import later_rule
        later = later_rule(condition)
        if later:
            stages["resume"] = later
        prior["effective_stage"] = "resume" if later else "return"
    if protocol >= 9:
        prior["visit_use_options"] = list(FIELDS)
    if protocol >= 10:
        prior["assessment_choices"] = list(ASSESSMENT_CHOICES)
        if condition["id"] == "goal_switch":
            stages["return"]["context"]["evidence_mode"] = "unscoped"
        history = []
        for i in range(n - 1, 0, -1):
            path = root / f"{i:03d}"
            decision = read(path / "decision.json")
            if decision["domain"] != domain:
                continue
            old_design = read(path / "design.json")["response"]
            old_policy = old_design["policy"]
            history.append({"trial": i, "change": old_design["change"], "memory": old_policy["memory"],
                            "retrieve": old_policy["retrieve"], "resume_mode": old_policy.get("resume_mode", "booked"),
                            "measured": decision.get("witness", {}).get("met"), "adopted": decision.get("adopted"),
                            "verdict": decision["review"].get("prediction", "unscored")})
            if len(history) == 4:
                break
        prior["recent_domain_designs"] = list(reversed(history))
    if protocol >= 5:
        from research_agenda import focus_for
        prior["planned_visit"] = {
            "condition": condition["id"],
            "shared_work_on_return": stages["return"]["learner_attempt"],
            "stages": {s: {"model": v["local"] is None,
                           **{k: v[k] for k in ("actions", "memory", "material")}}
                       for s, v in stages.items()}}
        prior["research_focus"] = focus_for(n, domain)
    if protocol >= 6:
        prior["previous_contrast"] = previous.get("witness") if previous else None
        prior["domain_predecessor"] = None
        for i in range(n - 1, 0, -1):
            past = read(root / f"{i:03d}" / "decision.json")
            if past["domain"] == domain:
                prior["domain_predecessor"] = {"trial": i, "next_question": past["review"]["next_question"],
                                               "witness": past.get("witness")}
                break
    if protocol >= 7:
        if (directory / "parent-correction.json").exists():
            raise ValueError("a global-incumbent correction cannot be applied to a scoped design")
        incumbent, prior["incumbent_source"] = scoped_incumbent(root, n, domain)
    save(directory / "condition.json", {"condition": condition, "stages": stages})
    # Independent case authoring can overlap design. Both outputs are frozen before teaching.
    with ThreadPoolExecutor(max_workers=1) as pool:
        enrolling = pool.submit(case, model, course, condition)
        proposal = design(model, domain, course, incumbent, prior)
        scenario = enrolling.result()
    # Balance label order across trials. Separate CLI processes have no shared chat.
    candidate = ("A", "B")[n % 2]
    variants = {k: proposal["policy"] if k == candidate else incumbent for k in ("A", "B")}
    with ThreadPoolExecutor(max_workers=2) as pool:
        jobs = {k: pool.submit(arm, model, k, course, scenario, v, condition, stages) for k, v in variants.items()}
        traces = {k: job.result() for k, job in jobs.items()}
    stage = proposal["hypothesis"]["stage"]
    observed = {k: {stage: read(directory / f"{k}-{stage}.json")} for k in ("A", "B")}
    witness_args = {"protocol": protocol} if protocol >= 8 else {}
    witness = evaluate_witness(proposal["hypothesis"], candidate, observed, **witness_args)
    snapshot = {"claims": {k: proposal[k] for k in ("question", "prediction", "falsifier")},
                "condition": condition, "witness": witness, "learner": shared_case(scenario, stages),
                "first_item": {k: course["first_pack"][k] for k in ("excerpt","check","rubric")},
                "return_item": {k: course["return_pack"][k] for k in ("excerpt","check","rubric")},
                "traces": traces}
    decision = {"trial": n, "domain": domain, "candidate": candidate, "protocol": protocol, "witness": witness,
                **review_grounded(model, snapshot, candidate, incumbent, variants[candidate])}
    save(directory / "decision.json", decision)
    print(n, domain, decision["review"]["preferred"], decision["review"]["next_question"], flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="private local study directory")
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--through", type=int, help="last paired trial ID")
    action.add_argument("--reassess", type=int, help="re-review a completed v1 trial on its original displays")
    parser.add_argument("--model", default="gpt-6.2-sol")
    parser.add_argument("--audit-container", help="Linux verifier with the locked Rust auditor built in /build/target")
    parser.add_argument("--seed-from", type=Path,
                        help="begin a new lineage from this completed trial's reassessment or decision")
    args = parser.parse_args()
    with exclusive_study(args.directory):
        if args.reassess is not None:
            if args.seed_from:
                parser.error("seed only a new campaign")
            reassess(args.directory / f"{args.reassess:03d}", args.model)
        else:
            if not 1 <= args.through <= 150:
                parser.error("choose 1..150 paired studies")
            if args.seed_from:
                source = args.seed_from / "reassessment.json"
                if not source.exists():
                    source = args.seed_from / "decision.json"
                seed = {"finding": read(source), "parent_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                        "domain": read(args.seed_from / "decision.json")["domain"]}
                target = args.directory / "seed.json"
                if target.exists():
                    original_seed = read(target)
                    if "domain" not in original_seed:
                        seed.pop("domain")
                    if original_seed != seed:
                        parser.error("choose a new directory for this lineage")
                if not target.exists():
                    if (args.directory / "001").exists():
                        parser.error("the requested directory already has trials")
                    save(target, seed)
            for trial in range(1, args.through + 1):
                completed = args.directory / f"{trial:03d}" / "decision.json"
                if not completed.exists():
                    run(args.directory, trial, args.model)
                if args.audit_container:
                    proof = invoke_auditor(completed.parent, container=args.audit_container)
                    if proof.returncode:
                        raise RuntimeError(proof.stderr.decode(errors="replace")[-1200:])
                    print(proof.stdout.decode(errors="replace").strip(), flush=True)
                else:
                    subprocess.run(["cargo", "run", "--quiet", "--locked", "--manifest-path",
                                    str(PACKS.parents[1] / "Cargo.toml"), "--example", "teaching_trace",
                                    "--", str(completed.parent.resolve())], check=True)
