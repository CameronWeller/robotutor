"""Report a local synthetic teaching campaign and pack independently audited evidence."""
import argparse
from collections import Counter
import hashlib
import html
import json
from math import ceil
from pathlib import Path
import re
import stat
from statistics import median
import subprocess
import tempfile
import zipfile

from teaching_audit import invoke_auditor
from teaching_consent import validate_choices
from teaching_trial import BASE, DOMAINS

ARMS = ("A", "B")
VISITS = ("opening", "feedback", "return")
REQUIRED_ARM_STAGES = tuple(f"{arm}-{stage}" for arm in ARMS for stage in (*VISITS, "learner"))
RESUME_STAGES = tuple(f"{arm}-resume" for arm in ARMS)
ARM_STAGES = (*REQUIRED_ARM_STAGES, *RESUME_STAGES)
STAGES = ("design", "case", *ARM_STAGES, "review", "review-grounded")
RECEIPT_KEYS = {"request_hash", "stage_hash", "model_requested", "seconds", "tokens",
                "packet", "response", "origin", "proposed_response"}
MAX_FILE = 2_000_000


def safe_bytes(path):
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_size > MAX_FILE
            or getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT):
        raise ValueError(f"unsupported file {path.name}")
    return path.read_bytes()


def read_object(data, name):
    value = json.loads(data)
    if not isinstance(value, dict):
        raise ValueError(f"{name} is not an object")
    return value


def auditor_executable():
    manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
    command = ["cargo", "build", "--offline", "--locked", "--manifest-path", str(manifest),
               "--example", "teaching_trace", "--message-format=json"]
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message.get("target", {}).get("name") == "teaching_trace" and message.get("executable"):
            return Path(message["executable"])
    raise ValueError("Cargo did not report the teaching_trace executable")


def validate_receipt(receipt, stage):
    if not {"seconds", "tokens", "model_requested", "packet", "response"} <= receipt.keys() <= RECEIPT_KEYS:
        raise ValueError(f"{stage} has unexpected receipt fields")
    origin = receipt.get("origin", "model")
    if origin not in ("model", "fallback", "local"):
        raise ValueError(f"{stage} has an unknown origin")
    if not isinstance(receipt["packet"], dict) or not isinstance(receipt["response"], dict):
        raise ValueError(f"{stage} has no packet or response")
    if stage == "design":
        keys = {"question", "change", "prediction", "falsifier", "policy"}
        if "hypothesis" in receipt["response"]:
            keys.add("hypothesis")
    elif stage == "case":
        keys = {"first_answer", "surprise", "return_request", "days", "save_topic"}
        if "topic_selection" in receipt["response"]:
            keys.add("topic_selection")
        if "return_work" in receipt["response"]:
            keys.add("return_work")
        keys.update(receipt["response"].keys() & {"memory_choices", "selected_focus", "resume_request",
                                                  "resume_work", "resume_keep", "return_intent",
                                                  "return_fields", "resume_fields", "assessment_choice"})
    elif stage.startswith("review"):
        keys = {"preferred", "early_difference", "later_difference", "evidence", "weakness", "next_question"}
        if stage == "review-grounded":
            keys.add("prediction")
    elif stage.endswith("-learner"):
        keys = {"response"}
    else:
        keys = {"display", "focus", "move"}
    if set(receipt["response"]) != keys:
        raise ValueError(f"{stage} has unexpected response fields")
    if stage == "design":
        policy = receipt["response"]["policy"]
        expected = {"teaching", "returning", "memory", "retrieve"}
        if set(policy) not in (expected, expected | {"resume_mode"}):
            raise ValueError("unexpected policy fields")
        if "resume_mode" in policy and policy["resume_mode"] not in ("current", "booked", "compare"):
            raise ValueError("unknown resumption policy")
    if type(receipt["seconds"]) not in (int, float) or not 0 <= receipt["seconds"] <= 86400:
        raise ValueError(f"{stage} has invalid elapsed time")
    tokens = receipt["tokens"]
    if tokens is not None and (not isinstance(tokens, dict) or any(
            not key.endswith("_tokens") or type(n) is not int or n < 0 for key, n in tokens.items())):
        raise ValueError(f"{stage} has invalid usage")
    if origin != "local" and (not isinstance(receipt["model_requested"], str) or
                               not re.fullmatch("[0-9a-f]{64}", receipt.get("request_hash", ""))):
        raise ValueError(f"{stage} lacks a live request receipt")


def costs(stages):
    totals = Counter()
    for name, receipt in stages.items():
        origin = receipt.get("origin", "model")
        if name in ARM_STAGES and not name.endswith("-learner"):
            totals[origin] += 1
        if origin != "local":
            totals["calls"] += 1
            totals["seconds"] += receipt["seconds"]
            usage = receipt["tokens"] or {}
            totals.update(usage)
            for field in ("input_tokens", "output_tokens"):
                if field not in usage:
                    totals[f"missing_{field}"] += 1
    return totals


def rejection_counts(directory):
    counts = Counter()
    for name in STAGES:
        transport = directory / f"{name}-failures.jsonl"
        if transport.exists():
            try:
                for line in safe_bytes(transport).splitlines():
                    try:
                        record = read_object(line, name)
                        seconds = record["seconds"]
                        if (set(record) != {"request_hash", "reason", "seconds", "usage"}
                                or record["usage"] != "provider acceptance unknown"
                                or type(seconds) not in (int, float) or not 0 <= seconds <= 86400):
                            raise ValueError("malformed failed attempt")
                        counts["transport_failures"] += 1
                        counts["transport_seconds"] += seconds
                    except (KeyError, ValueError, UnicodeError):
                        counts["unreadable_rows"] += 1
            except (OSError, ValueError):
                counts["unreadable_ledgers"] += 1
        path = directory / f"{name}-rejections.jsonl"
        if not path.exists():
            continue
        try:
            lines = safe_bytes(path).splitlines()
        except (ValueError, OSError):
            counts["unreadable_ledgers"] += 1
            continue
        for line in lines:
            try:
                row = read_object(line, name)
                if not {"request_hash", "reason"} <= row.keys():
                    raise ValueError("incomplete rejection")
                counts["rejections"] += 1
                seconds = row.get("seconds")
                if type(seconds) in (int, float) and 0 <= seconds <= 86400:
                    counts["rejected_seconds"] += seconds
                    counts["measured_rejected_seconds"] += 1
                usage = row.get("tokens")
                if isinstance(usage, dict):
                    for key, value in usage.items():
                        if key.endswith("_tokens") and type(value) is int and value >= 0:
                            counts[f"rejected_{key}"] += value
                            counts[f"measured_rejected_{key}"] += 1
            except (ValueError, UnicodeError):
                counts["unreadable_rows"] += 1
    return counts


def permitted_names(decision, directory, has_resume=False):
    version = decision.get("protocol", 1)
    if type(version) is not int or version not in range(1, 11):
        raise ValueError("unknown protocol")
    names = ["design", "case", *REQUIRED_ARM_STAGES, "decision", "review" if version == 1 else "review-grounded"]
    if version >= 3:
        names.append("condition")
    if version >= 4:
        names.extend(("A-booking", "B-booking"))
        if (directory / "parent-correction.json").exists():
            names.append("parent-correction")
    if version >= 5:
        names.extend(("A-return-memory", "B-return-memory"))
    if has_resume:
        if version < 8:
            raise ValueError("extra teaching visit differs from declared protocol")
        names.extend((*RESUME_STAGES, "A-resume-memory", "B-resume-memory"))
    if version == 1 and any((directory / f"{n}.json").exists() for n in ("review-grounded", "reassessment")):
        names.extend(("review-grounded", "reassessment"))
    return names


def first_finding(root):
    seed_file = root / "seed.json"
    if not seed_file.exists():
        return {"incumbent_after": BASE, "review":
                "A correct follow-up can still leave the writer's purpose unexplained at return."}, None, None
    record = read_object(safe_bytes(seed_file), "seed")
    if (set(record) not in ({"finding", "parent_sha256"}, {"finding", "parent_sha256", "domain"})
            or not isinstance(record["finding"], dict)):
        raise ValueError("malformed external campaign seed")
    finding = record["finding"]
    if "domain" in record and (record["domain"] not in DOMAINS
                               or finding.get("domain", record["domain"]) != record["domain"]):
        raise ValueError("external seed has an invalid or mismatched source domain")
    if not isinstance(finding.get("incumbent_after"), dict) or not isinstance(finding.get("review"), dict):
        raise ValueError("external seed lacks a policy or review")
    serialized = json.dumps(finding, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    if record["parent_sha256"] not in [hashlib.sha256(serialized + ending).hexdigest()
                                      for ending in (b"\n", b"\r\n")]:
        raise ValueError("external seed does not match its canonical source hash")
    return finding, record["parent_sha256"], record.get("domain")


def bind_design(number, decision, design, previous, correction, first, seed_hash=None, seed_domain=None):
    protocol = decision.get("protocol", 1)
    if number == 1:
        incumbent, review, witness = first["incumbent_after"], first["review"], first.get("witness")
    else:
        prior = previous.get(number - 1)
        if prior is None:
            raise ValueError("preceding trial has no audited decision")
        incumbent, review, witness = prior["original_policy"], prior["review"], prior["witness"]
    packet = design["packet"]
    finding = packet.get("last_finding")
    if protocol >= 7:
        if correction:
            raise ValueError("global correction cannot authorize a subject-scoped incumbent")
        domain = decision["domain"]
        eligible = next((i for i in reversed(previous) if previous[i]["domain"] == domain and
                         previous[i]["protocol"] >= 4 and previous[i]["adopted"]), None)
        source = {"scope": domain, "origin": "baseline", "trial": None, "sha256": None}
        incumbent = BASE
        if eligible is not None:
            old = previous[eligible]
            source.update(origin="domain-adoption", trial=eligible, sha256=old["original"])
            incumbent = old["original_policy"]
        elif seed_hash and (seed_domain == domain or (seed_domain is None and number == 1)):
            source.update(origin="external-seed", sha256=seed_hash)
            incumbent = first["incumbent_after"]
        if (not isinstance(finding, dict) or
                json.dumps(finding.get("incumbent_source"), sort_keys=True) != json.dumps(source, sort_keys=True)):
            raise ValueError("design incumbent source differs from the audited subject history")
    elif isinstance(finding, dict) and "incumbent_source" in finding:
        raise ValueError("subject-scoped incumbent source differs from declared protocol")
    elif correction:
        incumbent = previous[correction["source_trial"]]["amended_policy"]
    if packet.get("incumbent") != incumbent:
        raise ValueError("design incumbent differs from the audited predecessor")
    if protocol >= 3:
        if not isinstance(finding, dict):
            raise ValueError("design lacks a structured previous finding")
        indices = range(max(1, number - 6), number)
        if any(i not in previous for i in indices):
            raise ValueError("recent design questions include an unaudited predecessor")
        if finding.get("recent_questions") != [previous[i]["question"] for i in indices]:
            raise ValueError("design recent questions differ from the audited sequence")
    if (finding.get("last_review") if protocol >= 3 else finding) != review:
        raise ValueError("design received a different preceding review")
    if protocol >= 6:
        domain = decision["domain"]
        domain_prior = next((i for i in reversed(previous) if previous[i]["domain"] == domain), None)
        expected = None
        if domain_prior is not None:
            old = previous[domain_prior]
            expected = {"trial": domain_prior, "next_question": old["review"]["next_question"],
                        "witness": old["witness"]}
        # JSON equality preserves distinctions (e.g. false versus zero) that Python == elides.
        for field, value in (("previous_contrast", witness), ("domain_predecessor", expected)):
            if field not in finding or json.dumps(finding[field], sort_keys=True) != json.dumps(value, sort_keys=True):
                raise ValueError(f"design {field} differs from its audited predecessor")
        if protocol >= 10:
            same_domain = [i for i in previous if previous[i]["domain"] == domain][-4:]
            expected = []
            for i in same_domain:
                old = previous[i]
                policy = old["candidate_policy"]
                expected.append({"trial": i, "change": old["change"], "memory": policy["memory"],
                                 "retrieve": policy["retrieve"], "resume_mode": policy.get("resume_mode", "booked"),
                                 "measured": old["witness"]["met"] if old["witness"] else None,
                                 "adopted": old["adopted"], "verdict": old["review"].get("prediction", "unscored")})
            if json.dumps(finding.get("recent_domain_designs"), sort_keys=True) != json.dumps(expected, sort_keys=True):
                raise ValueError("design subject history differs from previous audited designs")


def snapshot(directory, number, scratch, auditor, previous, seed, container=None):
    """Only validated stage/decision bytes seen by this audit may enter its archive."""
    stages = {}
    objects = {}
    files = {}
    problems = []
    for name in (*STAGES, "decision", "condition", "reassessment", "A-booking", "B-booking",
                 "A-return-memory", "B-return-memory", "A-resume-memory", "B-resume-memory", "parent-correction"):
        path = directory / f"{name}.json"
        if not path.exists():
            continue
        try:
            data = safe_bytes(path)
            value = read_object(data, name)
            if name in STAGES:
                validate_receipt(value, name)
                stages[name] = value
            elif name == "decision":
                if set(value) - {"trial", "protocol", "domain", "candidate", "review", "adopted", "incumbent_after", "witness"}:
                    raise ValueError("unexpected decision fields")
            elif name == "reassessment" and set(value) != {"review", "adopted", "incumbent_after"}:
                raise ValueError("unexpected reassessment fields")
            elif name.endswith("-booking") and (set(value) != {"format", "stored"} or
                    value["format"] != "paired-booking/1" or not isinstance(value["stored"], dict)):
                raise ValueError("invalid paired booking")
            elif name.endswith("-return-memory") and (
                    set(value) != {"format", "stored", "reason"} or value["format"] != "paired-return-memory/1"
                    or not isinstance(value["stored"], dict) or value["reason"] not in (
                        "carry", "forget_topic", "goal_switch", "cancel_topic")):
                raise ValueError("invalid return memory")
            elif name.endswith("-resume-memory") and (
                    set(value) != {"format", "stored"} or value["format"] != "paired-resume-memory/1"
                    or not isinstance(value["stored"], dict)):
                raise ValueError("invalid resumption memory")
            elif name == "parent-correction":
                if set(value) != {"source_trial", "original_sha256", "reassessment_sha256"}:
                    raise ValueError("unexpected correction fields")
                source = value["source_trial"]
                if type(source) is not int or source not in previous or not 1 <= source < number:
                    raise ValueError("missing audited correction source")
                parent = previous[source]
                if (not parent["amended"] or value["original_sha256"] != parent["original"]
                        or value["reassessment_sha256"] != parent["amended"] or any(
                            i not in previous or previous[i]["original_policy"] != parent["original_policy"]
                            for i in range(source + 1, number))):
                    raise ValueError("correction does not match audited lineage")
            objects[name] = value
            files[name] = data
        except (ValueError, TypeError, OSError, UnicodeError):
            problems.append(f"invalid {name}.json")
    metrics = costs(stages)
    metrics.update(rejection_counts(directory))
    decision = objects.get("decision")
    if not decision:
        problems.append("no complete decision")
    else:
        try:
            has_resume = "resume" in objects.get("condition", {}).get("stages", {})
            names = permitted_names(decision, directory, has_resume)
            has_witness = decision.get("protocol", 1) >= 4
            for record, label in (("design", "hypothesis"), ("case", "topic_selection")):
                if record in objects and (label in objects[record]["response"]) != has_witness:
                    problems.append(f"{record} fields differ from declared protocol")
            if ("witness" in decision) != has_witness:
                problems.append("decision witness differs from declared protocol")
            protocol = decision.get("protocol", 1)
            if "case" in objects:
                scenario = objects["case"]["response"]
                case_packet = objects["case"]["packet"]
                if ((protocol >= 7 and (type(case_packet.get("study_protocol")) is not int
                                        or case_packet["study_protocol"] != protocol))
                        or (protocol < 7 and "study_protocol" in case_packet)):
                    problems.append("case study marker differs from declared protocol")
                if ("return_work" in scenario) != (protocol >= 5):
                    problems.append("shared-work field differs from declared protocol")
                if any((field in scenario) != (protocol >= 6) for field in ("memory_choices", "selected_focus")):
                    problems.append("learner memory fields differ from declared protocol")
                if any((field in scenario) != (protocol >= 8)
                       for field in ("resume_request", "resume_work", "resume_keep", "return_intent")):
                    problems.append("resumption case fields differ from declared protocol")
                if any((field in scenario) != (protocol >= 9) for field in ("return_fields", "resume_fields")):
                    problems.append("per-visit field choices differ from declared protocol")
                if (("assessment_choice" in scenario) != (protocol >= 10)
                        or ("assessment_choices" in case_packet) != (protocol >= 10)):
                    problems.append("assessment choices differ from declared protocol")
                if protocol >= 10 and scenario.get("assessment_choice") not in ("submit", "private_practice", "skip"):
                    problems.append("unknown assessment choice")
                if protocol >= 8 and scenario.get("return_intent") not in (
                        "keep", "cancel_topic", "forget_all", "change_goal"):
                    problems.append("unknown learner return intent")
                if protocol >= 6:
                    is_private = objects.get("condition", {}).get("condition", {}).get("id") == "private_answer"
                    validate_choices(scenario, private=is_private)
            if "design" in objects:
                design = objects["design"]["response"]
                if ("resume_mode" in design["policy"]) != (protocol >= 8):
                    problems.append("resumption policy differs from declared protocol")
                if protocol >= 8:
                    hypothesis = design["hypothesis"]
                    effective = "resume" if has_resume else "return"
                    if (hypothesis["stage"] != effective or
                            objects["design"]["packet"]["last_finding"].get("effective_stage") != effective):
                        problems.append("primary contrast targets a different teaching visit")
                    if hypothesis["channel"] == "selection" and hypothesis["feature"] not in (
                            "first_unit", "return_unit", "contrast_material"):
                        problems.append("unknown selection witness")
                bind_design(number, decision, objects["design"], previous, objects.get("parent-correction"), *seed)
            if protocol < 8 and any(objects.get(f"{arm}-return-memory", {}).get("reason") == "cancel_topic"
                                    for arm in ARMS):
                problems.append("topic cancellation differs from declared protocol")
            if protocol >= 10 and "review-grounded" in objects:
                for arm in ARMS:
                    trace = objects["review-grounded"]["packet"]["traces"][arm]
                    route = trace["assessment_route"]
                    if route not in ("submitted", "private_practice", "skipped", "not_offered"):
                        problems.append("unknown assessment route")
                    receipt = objects.get(f"{arm}-learner")
                    if receipt:
                        submitted = route == "submitted"
                        if ((receipt.get("origin", "model") != "local") != submitted
                                or bool(receipt["packet"].get("new_task")) != submitted):
                            problems.append("assessment route differs from actual task dispatch")
                        reference = receipt["packet"].get("required_reference")
                        if ("assessment_reference" not in trace or trace["assessment_reference"] != reference
                                or ("required_reference" in receipt["packet"] and
                                    (not submitted or not isinstance(reference, dict)
                                     or set(reference) != {"unit", "locator", "excerpt"}))):
                            problems.append("assessment reference differs from delivered source")
            if type(decision.get("trial")) is not int or decision["trial"] != number:
                problems.append("directory and trial ID disagree")
            if missing := sorted(set(names) - set(files)):
                problems.append("missing " + ", ".join(missing))
            if extra := sorted(set(files) - set(names)):
                problems.append("unexpected protocol records " + ", ".join(extra))
        except (AttributeError, KeyError, TypeError, ValueError) as exc:
            problems.append(str(exc))
    if not problems:
        scratch.mkdir()
        for name in names:
            (scratch / f"{name}.json").write_bytes(files[name])
        checked = invoke_auditor(scratch, executable=auditor, container=container)
        if checked.returncode:
            problems.append("Rust packet/temporal audit rejected the snapshot")
    return objects, files, metrics, problems


def wording(value, limit=280):
    single = " ".join(value.split())
    if len(single) > limit:
        single = single[:limit].rsplit(" ", 1)[0] + " …"
    # Quoted model prose stays inert in Markdown.
    return re.sub(r"([\\`*_[\]{}|])", r"\\\1", html.escape(single, quote=False))


def cost_text(metrics):
    inputs, outputs = (metrics[k] for k in ("input_tokens", "output_tokens"))
    result = (f"{metrics['calls']} accepted live calls ({inputs:,} input / {outputs:,} output tokens, "
              f"{metrics['seconds']:.1f} summed call seconds), {metrics['rejections']} recorded rejections")
    if metrics["missing_input_tokens"] or metrics["missing_output_tokens"]:
        result += (f" (usage incomplete on {metrics['missing_input_tokens']} input and "
                   f"{metrics['missing_output_tokens']} output receipts)")
    if metrics["rejections"]:
        result += (f" (known rejected input {metrics['rejected_input_tokens']:,} on "
                   f"{metrics['measured_rejected_input_tokens']}/{metrics['rejections']}, output "
                   f"{metrics['rejected_output_tokens']:,} on {metrics['measured_rejected_output_tokens']}/{metrics['rejections']}, "
                   f"{metrics['rejected_seconds']:.1f} seconds on {metrics['measured_rejected_seconds']}/{metrics['rejections']})")
    if metrics["transport_failures"]:
        result += (f", {metrics['transport_failures']} transport failures ({metrics['transport_seconds']:.1f} "
                   "seconds, provider acceptance and token usage unknown)")
    if metrics["unreadable_rows"] or metrics["unreadable_ledgers"]:
        result += f" ({metrics['unreadable_rows']} unreadable attempt rows, {metrics['unreadable_ledgers']} unreadable ledgers)"
    return result


def return_evidence(obj, arm):
    tutor = obj[f"{arm}-return"]
    participant = obj[f"{arm}-learner"]
    sent = tutor.get("origin", "model") != "local"
    packet = tutor["packet"]
    review_stage = obj.get("review-grounded", obj.get("review"))
    source_task = " ".join(review_stage["packet"]["return_item"]["check"].split())
    display = " ".join(tutor["response"]["display"].split())
    return {"dispatched": sent, "local": not sent,
            "task": sent and bool(packet.get("new_task")), "shared": sent and bool(packet.get("shared_work")),
            "verbatim": bool(source_task) and source_task.casefold() in display.casefold(),
            "assessor_task": participant.get("origin", "model") != "local" and bool(participant["packet"].get("new_task")),
            "modeled": participant.get("origin", "model") != "local"}


def trial_text(number, obj, metrics):
    decision = obj["decision"]
    proposal = obj["design"]["response"]
    original = decision["review"]
    effective = obj.get("reassessment", decision)
    review = effective["review"]
    disposition = "adopted" if effective.get("adopted", original["preferred"] == decision["candidate"]) else "held"
    verdict = review.get("prediction", "unscored in v1")
    condition = obj.get("condition", {}).get("condition", {}).get("id", "unspecified in v1/v2")
    lines = [f"### {number:03d} · {decision['domain']} · {condition}", "",
             f"**Question.** {wording(proposal['question'], 650)}",
             f"**Design change.** {wording(proposal['change'], 650)}",
             f"**Prediction.** {wording(proposal['prediction'], 650)}",
             f"**Falsifier.** {wording(proposal['falsifier'], 650)}",
             f"**Reviewer’s first-visit comparison.** {wording(review['early_difference'], 1500)}", ""]
    if decision.get("protocol", 1) >= 6:
        case = obj["case"]["response"]
        fields = ", ".join(case["memory_choices"]) or "none"
        lines.extend([f"The simulated learner selected topic offer **{case['topic_selection']}**, "
                      f"optional fields **{fields}** and a separate focus of **{case['selected_focus'] or 'none'}**.", ""])
    if decision.get("protocol", 1) >= 7:
        source = obj["design"]["packet"]["last_finding"]["incumbent_source"]
        parent = f", trial {source['trial']:03d}" if source["trial"] is not None else ""
        lines.extend([f"The **{source['scope']}** incumbent came from **{source['origin']}**{parent}.", ""])
    if decision.get("protocol", 1) >= 8:
        mode = proposal["policy"]["resume_mode"]
        lines.append(f"The candidate selected **{mode}** for subsequent teaching. The fictional learner "
                     f"requested **{case['return_intent']}** at the second visit.")
        if "A-resume" in obj:
            lines.append(f"The fictional learner asked on the additional visit, “{wording(case['resume_request'], 650)}”")
        lines.append("")
    if decision.get("protocol", 1) >= 10:
        lines.extend([f"The fictional learner chose **{case['assessment_choice']}** for assessment.", ""])
    evidence = Counter()
    for arm in ARMS:
        receipt = obj[f"{arm}-return"]
        reply = receipt["response"]
        arm_evidence = return_evidence(obj, arm)
        evidence.update(arm_evidence)
        label = "candidate" if arm == decision["candidate"] else "incumbent"
        origin = receipt.get("origin", "model")
        opening = obj[f"{arm}-opening"]["response"]["move"]
        feedback = obj[f"{arm}-feedback"]["response"]
        lines.append(f"- {arm} ({label}) opened with **{opening}**, then used **{feedback['move']}** at feedback. "
                     f"“{wording(feedback['display'], 140)}” On return ({origin}), it displayed "
                     f"**{reply['move']}** with focus **{reply['focus']}**. “{wording(reply['display'], 170)}”")
        if decision.get("protocol", 1) >= 5:
            memory = obj[f"{arm}-return-memory"]
            lines.append(f"  Return memory: {memory['reason']}. "
                         f"Fresh learner work reached tutor dispatch: {'yes' if arm_evidence['shared'] else 'no'}.")
            if decision.get("protocol", 1) >= 8:
                trace = obj["review-grounded"]["packet"]["traces"][arm]
                lines.append(f"  The host displayed “{wording(trace['return_confirmation'], 450)}”")
                if decision.get("protocol", 1) >= 9:
                    lines.append(f"  The second-visit use notice read “{wording(trace['return_use_note'], 650)}”")
        resumed = obj.get(f"{arm}-resume")
        if resumed is not None:
            resume_origin = resumed.get("origin", "model")
            sent = resume_origin != "local"
            shared = sent and bool(resumed["packet"].get("shared_work"))
            evidence.update(resumptions=1, resume_dispatched=sent, resume_local=not sent, resume_shared=shared)
            resume_display = resumed["response"]
            elapsed = f"{resumed['seconds']:.2f} s" if sent and resumed["seconds"] > 0 else (
                "unmeasured call time" if sent else "local display")
            memory = obj[f"{arm}-resume-memory"]["stored"]
            lines.append(f"  The additional visit carried these saved fields: {', '.join(sorted(memory)) or 'none'}. "
                         f"Fresh work reached tutor dispatch: {'yes' if shared else 'no'}.")
            lines.append(f"  On the additional visit ({resume_origin}, {elapsed}) the tutor used "
                         f"**{resume_display['move']}**. “{wording(resume_display['display'], 1600)}”")
            if decision.get("protocol", 1) >= 9:
                lines.append(f"  The additional-visit use notice read “{wording(trace['resume_use_note'], 650)}”")
        if decision.get("protocol", 1) >= 8:
            packet = (resumed if resumed is not None else receipt)["packet"]
            route = packet["lesson_route"]
            aids = ", ".join(sorted(packet["comparison_material"])) or "none"
            lines.append(f"  The final teaching packet selected unit **{route['unit']}** using "
                         f"**{route['mode']}**. Comparison materials in this packet: {aids}.")
        if decision.get("protocol", 1) >= 10:
            route = trace["assessment_route"]
            evidence["assessment_" + route] += 1
            reference = obj[f"{arm}-learner"]["packet"].get("required_reference")
            evidence["assessment_reference"] += int(reference is not None)
            deliver = (f"The assessor received the required excerpt from unit {reference['unit']}."
                       if reference is not None else "No reference excerpt was delivered.")
            lines.append(f"  The assessment route was **{route}**. "
                         f"A separate assessor task was dispatched: {'yes' if arm_evidence['assessor_task'] else 'no'}. "
                         + deliver)
    later = "across later visits" if "A-resume" in obj else "on return"
    lines.extend(["", f"**Reviewed difference {later}.** {wording(review['later_difference'], 350)}",
                  f"**Verdict.** {verdict}. Preferred {review['preferred']}. Candidate {disposition}."])
    if "witness" in decision:
        result = decision["witness"]
        hyp = result["hypothesis"]
        values = lambda pair: " / ".join("present" if pair[arm] else "absent" for arm in ("candidate", "incumbent"))
        lines.append(f"**Preregistered contrast.** {hyp['stage']} · {hyp['channel']} · {hyp['feature']}. "
                     f"Candidate / incumbent expected {values(result['expected'])}, observed {values(result['observed'])}. "
                     f"Execution witness {'met' if result['met'] else 'missed'}. "
                     "The prediction verdict and preference above are the reviewer's separate quality judgment.")
    if "reassessment" in obj:
        first = "adopted" if original["preferred"] == decision["candidate"] else "held"
        lines.append(f"The original v1 decision preferred {original['preferred']} and {first} the candidate. "
                     "The grounded reassessment is shown above. The original lineage stays recorded.")
    if "parent-correction" in obj:
        lines.append(f"The compared incumbent incorporates the audited reassessment of trial "
                     f"{obj['parent-correction']['source_trial']:03d}.")
    if "A-resume" in obj:
        lines.append(f"The additional visit generated {evidence['resume_dispatched']}/2 tutor dispatches. "
                     "It had no separate learner assessment.")
    lines.extend([f"**Lesson or limitation.** {wording(review['weakness'])}",
                  f"**Next unresolved question.** {wording(review['next_question'], 1500)}",
                  f"**Measured.** {cost_text(metrics)}. The dispatch packet carried the changed task in {evidence['task']}/2 arms. "
                  f"The full task appeared verbatim in {evidence['verbatim']}/2 tutor displays. The separate assessor "
                  f"supplied it in {evidence['assessor_task']}/2 arms and recorded {evidence['modeled']}/2 modeled replies.", ""])
    return lines, condition, verdict, disposition, evidence


def packed(zf, name, data):
    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    zf.writestr(info, data)


def accumulate_resources(obj, settings, roles):
    setting = obj.get("condition", {}).get("condition", {}).get("setting", "unspecified")
    arms = settings.setdefault(setting, {"count": 0, "input": [], "seconds": []})
    for arm in ARMS:
        arms["count"] += 1
        visits = [obj[f"{arm}-{stage}"] for stage in (*VISITS, "resume") if f"{arm}-{stage}" in obj]
        dispatched = [receipt for receipt in visits if receipt.get("origin", "model") != "local"]
        if all("input_tokens" in (receipt["tokens"] or {}) for receipt in dispatched):
            arms["input"].append(sum(receipt["tokens"]["input_tokens"] for receipt in dispatched))
        if all(receipt["seconds"] > 0 for receipt in dispatched):
            arms["seconds"].append(sum(receipt["seconds"] for receipt in dispatched))
    for stage in STAGES:
        receipt = obj.get(stage)
        if receipt is None or receipt.get("origin", "model") == "local":
            continue
        if stage in ARM_STAGES:
            role = "Simulated learner" if stage.endswith("-learner") else "Instructor displays"
        else:
            role = {"design": "Study design", "case": "Synthetic enrollment",
                    "review": "Reviewer", "review-grounded": "Reviewer"}[stage]
        usage = receipt["tokens"] or {}
        counts = roles.setdefault(role, Counter())
        counts["calls"] += 1
        for field in ("input_tokens", "output_tokens", "cached_input_tokens"):
            if field in usage:
                counts[field] += usage[field]
                counts[field + "_known"] += 1


def resource_tables(settings, roles):
    lines = ["", "## Instructor use and study overhead", "",
             "Each arm includes the opening, feedback and return tutor displays, plus any additional resumption "
             "display required by its condition. Local displays contribute zero model "
             "tokens and zero model-call time. Each distribution uses arms with complete telemetry for its "
             "measure. The 90th percentile is the nearest rank. Times include CLI startup and network time. "
             "Elapsed sums across visits do not measure concurrent classroom throughput or invoice cost.", "",
             "| Setting | Completed arms | Input measured | Median / p90 input tokens | Time measured | "
             "Median / p90 call seconds |", "| --- | ---: | ---: | ---: | ---: | ---: |"]
    def distribution(values, precision):
        if not values:
            return "—"
        p90 = sorted(values)[ceil(0.9 * len(values)) - 1]
        return f"{median(values):,.{precision}f} / {p90:,.{precision}f}"
    for setting, data in sorted(settings.items()):
        lines.append(f"| {setting} | {data['count']} | {len(data['input'])} | {distribution(data['input'], 1)} "
                     f"| {len(data['seconds'])} | {distribution(data['seconds'], 2)} |")
    lines += ["", "The following totals count accepted nonlocal stage receipts. Each token total gives the "
              "number of calls that supplied it in parentheses. Cached input is reported separately as a component "
              "of input. Rejected calls and receipts outside the audited trials are reported separately.", "",
              "| Role | Calls | Input tokens (calls) | Cached input tokens (calls) | Output tokens (calls) |",
              "| --- | ---: | ---: | ---: | ---: |"]
    for role, counts in sorted(roles.items()):
        cells = [f"{counts[field]:,} ({counts[field + '_known']})" if counts[field + "_known"] else "unknown (0)"
                 for field in ("input_tokens", "cached_input_tokens", "output_tokens")]
        lines.append(f"| {role} | {counts['calls']} | " + " | ".join(cells) + " |")
    return lines


def report(root, through, output, archive, auditor=None, container=None):
    checked = Counter()
    partial = Counter()
    groups = {}
    verdicts = Counter()
    witness_counts = Counter()
    questions, policies, paths = set(), set(), set()
    details, gaps, manifest = [], [], []
    totals = Counter()
    previous = {}
    stage_seconds = {}
    settings, roles = {}, {}
    seed = first_finding(root)
    seed_hash = seed[1]
    output.parent.mkdir(parents=True, exist_ok=True)
    archive.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=archive.parent) as working:
        scratch = Path(working)
        bundle = scratch / "stages.zip"
        with zipfile.ZipFile(bundle, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            for number in range(1, through + 1):
                folder = root / f"{number:03d}"
                if not folder.is_dir() or getattr(folder.lstat(), "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT or folder.is_symlink():
                    gaps.append((number, "no regular trial directory", None))
                    continue
                obj, files, metrics, problems = snapshot(folder, number, scratch / folder.name, auditor,
                                                         previous, seed, container)
                if problems:
                    gaps.append((number, ", ".join(problems), metrics))
                    partial.update(metrics)
                    continue
                lines, condition, verdict, disposition, evidence = trial_text(number, obj, metrics)
                details.extend(lines)
                checked.update(metrics)
                decision = obj["decision"]
                previous[number] = {"original": hashlib.sha256(files["decision"]).hexdigest(),
                                    "original_policy": decision["incumbent_after"],
                                    "review": decision["review"], "question": obj["design"]["response"]["question"],
                                    "domain": decision["domain"], "witness": decision.get("witness"),
                                    "adopted": decision.get("adopted"), "protocol": decision.get("protocol", 1),
                                    "change": obj["design"]["response"]["change"],
                                    "candidate_policy": obj["design"]["response"]["policy"],
                                    "amended": hashlib.sha256(files["reassessment"]).hexdigest() if "reassessment" in obj else None,
                                    "amended_policy": obj.get("reassessment", {}).get("incumbent_after")}
                totals.update(trials=1)
                totals.update(evidence)
                if decision.get("protocol", 1) >= 5:
                    totals["shared_work_arms"] += len(ARMS)
                accumulate_resources(obj, settings, roles)
                for name in STAGES:
                    receipt = obj.get(name)
                    if (receipt and receipt.get("origin", "model") != "local" and receipt["seconds"] > 0):
                        kind = name.split("-", 1)[1] if name in ARM_STAGES else name
                        stage_seconds.setdefault(kind, []).append(receipt["seconds"])
                verdicts[verdict] += 1
                if "witness" in decision:
                    witness_counts["met" if decision["witness"]["met"] else "missed"] += 1
                for kind, key in (("Domain", decision["domain"]), ("Condition", condition)):
                    group = groups.setdefault((kind, key), Counter())
                    group.update(trials=1)
                    group.update(evidence)
                    group[disposition] += 1
                    group["witness_met"] += int(decision.get("witness", {}).get("met", False))
                    group["witness_n"] += int("witness" in decision)
                    group["blind_supported"] += int(verdict == "supported")
                    group["unobserved"] += int(verdict == "unobserved")
                    group["unscored"] += int(verdict == "unscored in v1")
                    for arm in ARMS:
                        group["display_" + obj[f"{arm}-return"].get("origin", "model")] += 1
                proposal = obj["design"]["response"]
                questions.add(" ".join(proposal["question"].casefold().split()))
                cfg = proposal["policy"]
                policies.add(json.dumps(cfg, sort_keys=True))
                paths.add((tuple(sorted(cfg["memory"])), tuple(sorted(cfg["retrieve"]))))
                for name, data in sorted(files.items()):
                    path = f"{number:03d}/{name}.json"
                    packed(zf, path, data)
                    manifest.append(f"{hashlib.sha256(data).hexdigest()}  {path}")
            packed(zf, "SHA256SUMS", ("\n".join(manifest) + "\n").encode())
        archive_hash = hashlib.sha256(bundle.read_bytes()).hexdigest()
        complete = totals["trials"]
        report_lines = ["# Paired teaching-policy evidence", "",
            f"Audited **{complete} of {through}** requested trial IDs. The archive contains {len(manifest)} "
            f"allowlisted stage, condition and decision records. Archive SHA-256 `{archive_hash}`.", "",
            (f"The first trial used the separately supplied seed with source SHA-256 `{seed_hash}`. "
             "Full lineage replay also requires that seed. Its source campaign was outside this audit." if seed_hash else
             "The first trial used the unseeded starting policy and finding.") +
            " Each later design was checked against the original preceding review and the applicable incumbent. "
            "Through V6, the incumbent came from the preceding decision or a documented correction. V3 onward "
            "required the recorded recent questions. V6 onward required the previous execution contrast and "
            "latest same-domain study finding. V7 onward required a same-domain adopted policy, an eligible external "
            "seed or the baseline, with the source recorded.", "",
            f"**Complete trials.** {cost_text(checked)}. Across opening, feedback, return and any additional resumption displays, "
            f"{checked['model']} came directly from the model, {checked['local']} were local and "
            f"{checked['fallback']} used a fixed fallback after a model proposal.", "",
            f"**Coverage.** {len(questions)} distinct question texts, {len(policies)} distinct full candidate policies "
            f"and {len(paths)} distinct memory/retrieval field combinations. Tutor packets included the changed "
            f"task at dispatch in {totals['task']}/{2*complete} return arms. Of {totals['shared_work_arms']} V5+ return arms, "
            f"{totals['shared']} dispatched fresh shared learner work. The full task appeared verbatim in {totals['verbatim']}/{2*complete} "
            f"tutor displays. The separate assessor supplied its task in {totals['assessor_task']}/{2*complete} arms "
            f"and recorded {totals['modeled']}/{2*complete} modeled replies. Live human outcomes observed: 0.", "",
            f"Additional visits produced {totals['resume_dispatched']} tutor dispatches and {totals['resume_local']} "
            f"local displays across {totals['resumptions']} resumed arms. "
            f"{totals['resume_shared']} of those tutor dispatches received fresh shared work. The separate assessor only ran on "
            "the earlier return visit.", "",
            f"V10 assessment outcomes by arm: {totals['assessment_not_offered']} unavailable before learner choice; "
            f"among eligible arms, {totals['assessment_submitted']} submitted, {totals['assessment_private_practice']} "
            f"chose private practice, and {totals['assessment_skipped']} skipped. The assessor received a "
            f"required reference excerpt with {totals['assessment_reference']} of the submitted tasks. "
            "Earlier protocols did not record these routes.", "",
            "Reviewer prediction verdicts: " + (", ".join(f"{k} {v}" for k,v in sorted(verdicts.items())) or "none") + ".", "",
            f"V4+ preregistered execution contrasts met {witness_counts['met']} and missed {witness_counts['missed']}. "
            f"{complete-sum(witness_counts.values())} earlier trials did not register this measure. A contrast demonstrates "
            "the presence or absence of the named input or move; the reviewer separately judges the instructional claim.", "",
            "Call time sums concurrent stages. The rejection ledger reports measured usage separately. Rejected and "
            "interrupted requests with incomplete telemetry may consume additional quota. A return-task packet can reach a tutor without the tutor showing "
            "the task. Verbatim display counts ignore whitespace and case. Paraphrased or partial exercises need review. "
            "A modeled reply can follow a prompt supplied by the assessor. Trial IDs provide no automatic "
            "system-round credit or evidence of student learning.", "",
            "| Group | Trials | Adopted / held | Prediction unobserved / unscored | Return display model / local / fallback | Tasks in packets | Modeled replies |",
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
        for (kind,key), group in sorted(groups.items()):
            if kind != "Condition":
                continue
            size = 2*group["trials"]
            report_lines.append(f"| {kind} · {key} | {group['trials']} | {group['adopted']} / {group['held']} | "
                                f"{group['unobserved']} / {group['unscored']} | {group['display_model']} / {group['display_local']} / {group['display_fallback']} | "
                                f"{group['task']}/{size} | {group['modeled']}/{size} |")
        report_lines += ["", "| Domain | Trials | Tutor return dispatch / local hold | Execution witness met / measured | "
                         "Blind review supported / scored | Adopted |",
                         "| --- | ---: | ---: | ---: | ---: | ---: |"]
        for (kind,key), group in sorted(groups.items()):
            if kind == "Domain":
                report_lines.append(f"| {key} | {group['trials']} | {group['dispatched']} / {group['local']} | "
                                    f"{group['witness_met']} / {group['witness_n']} | {group['blind_supported']} / "
                                    f"{group['trials'] - group['unscored']} | {group['adopted']} |")
        report_lines.extend(resource_tables(settings, roles))
        if stage_seconds:
            report_lines += ["", "Elapsed times include CLI startup and network time. Each row uses accepted nonlocal "
                             "call receipts with positive measured duration, including a model proposal followed by a fallback.", "",
                             "| Call stage | Calls timed | Median seconds | Range in seconds |",
                             "| --- | ---: | ---: | ---: |"]
            for stage, seconds in sorted(stage_seconds.items()):
                report_lines.append(f"| {stage} | {len(seconds)} | {median(seconds):.2f} | {min(seconds):.2f}–{max(seconds):.2f} |")
        report_lines += ["", "## Coverage gaps", ""]
        if gaps:
            missing = [f"{n:03d}" for n,reason,m in gaps if m is None]
            if missing:
                report_lines.append("No trial directory for " + ", ".join(missing) + ".")
            for n, reason, m in gaps:
                if m is not None:
                    report_lines.append(f"- {n:03d} · {reason}. {cost_text(m)} in readable receipts.")
            report_lines += ["", f"Receipts outside the audited archive account for {cost_text(partial)}."]
        else:
            report_lines.append("Every requested trial ID passed the Rust audit.")
        report_lines += ["", "## Individual trials", "", *details,
                         "Extract the archive, verify its SHA256SUMS entries, and run the repository's "
                         "`teaching_trace` executable against each numbered directory. The archive omits raw CLI logs.", ""]
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", newline="\n", dir=output.parent, delete=False) as f:
            pending = Path(f.name)
            f.write("\n".join(report_lines))
        try:
            bundle.replace(archive)
            pending.replace(output)
        finally:
            pending.unlink(missing_ok=True)
    print(f"{complete}/{through} audited, {len(gaps)} gaps, archive sha256 {archive_hash}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="one local campaign's numbered trial directories")
    parser.add_argument("--through", type=int, required=True, choices=range(1,151), metavar="1..150")
    parser.add_argument("--report", required=True, type=Path, help="output Markdown file")
    parser.add_argument("--archive", required=True, type=Path, help="output ZIP file")
    verifier = parser.add_mutually_exclusive_group()
    verifier.add_argument("--auditor", type=Path, help="existing teaching_trace executable; default builds it offline")
    verifier.add_argument("--audit-container", help="existing Linux verifier container with the Rust executable")
    args = parser.parse_args()
    if not args.directory.is_dir() or args.report.suffix.lower() != ".md" or args.archive.suffix.lower() != ".zip":
        parser.error("supply an existing study directory, an .md report and a .zip archive")
    report(args.directory.resolve(), args.through, args.report, args.archive,
           args.auditor.resolve() if args.auditor else None if args.audit_container else auditor_executable(),
           args.audit_container)


if __name__ == "__main__":
    main()
