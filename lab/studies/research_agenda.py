"""Offer one research question to the designer and inspect completed paired trials offline."""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import re

from teaching_report import read_object, safe_bytes


# Questions rotate independently of domain and of the 13 environment conditions.
# Each identifies the teaching decision to study and an effect visible in a pair.
AGENDA = (
    ("first_use", "Can the first tutor reply teach one useful thing before asking the learner for another demonstration?",
     "Compare an immediate small demonstration with a diagnostic opening.",
     "Inspect the first learner-facing move and the first task attempted without that demonstration."),
    ("authority", "What should the teacher do when a task, its source and the proposed feedback no longer agree?",
     "Compare explaining from admitted evidence with withholding the affected judgment and seeking repair.",
     "Trace the supported material into the displayed response and the next eligible teaching action."),
    ("ambiguity", "When two explanations fit the same answer, which question would actually distinguish them?",
     "Compare asking for the missing observation with reteaching the entire procedure.",
     "Check whether a clarification changes the instructional move on the next turn."),
    ("selected_memory", "What should another tutor remember after the learner approves an instructional topic?",
     "Compare carrying the exact released choice with reopening on a general goal alone.",
     "Compare the transmitted return fields with what the tutor says it remembered."),
    ("assistance", "What instruction follows a correct response that came immediately after a worked example?",
     "Compare another demonstration with a changed attempt that preserves the outstanding target.",
     "Identify the actual support shown before the response and what the return teacher asks for."),
    ("teacher_time", "How can the learner make progress while an individual teacher review is pending?",
     "Compare an available local practice move with a handoff that leaves the review obligation open.",
     "Inspect what can be taught within the admitted resources and whether either tutor invents a completed review."),
    ("access", "Which help changes the presentation and which help performs the target skill for the learner?",
     "Compare restating the response format with supplying part of the answer.",
     "Check which material the tutor exposes, the move it takes and what evidence that leaves available."),
    ("veto", "How does the tutor honor an explicit request to skip or change course while preserving useful work?",
     "Compare stopping or reselecting with another probe on the former target.",
     "Look for an unsolicited question, a saved commitment or an unwanted task in the next display."),
    ("demonstration", "What makes a demonstration transferable when the next task changes representation?",
     "Compare narrating the invariant with showing how to use it on a contrasting case.",
     "Inspect the learner-visible difference in demonstration and the later task; record any untested transfer claim."),
)

DOMAIN_PROBE = {
    "chemical-engineering": "A numerical answer can fit the wrong system boundary. Which inventory and sign would the learner explain on a changed diagram?",
    "writer": "A correct prediction about a supplied notice can coexist with a misleading rewrite. What action would the writer's own sentence invite?",
    "violin": "Counted rests in a text exchange can support rehearsal. Which question needs an actual witnessed re-entry before making a performance claim?",
    "better-person": "A written repair can reopen someone else's unfinished contribution. What remains unknown about their reaction or the writer's conduct?",
    "worship": "Distinguish choosing a detail in a fictional line from a private devotional practice. What can the learner try without disclosing belief or experience?",
}


def focus_for(trial, domain):
    """Bounded design context; no realized condition, surprise, arm label or model output."""
    if type(trial) is not int or trial < 1 or domain not in DOMAIN_PROBE:
        raise ValueError("a positive trial and a reviewed teaching domain are required")
    name, question, comparison, inspection = AGENDA[(trial - 1) % len(AGENDA)]
    return {"id": name, "research_question": question, "compare": comparison,
            "inspect": inspection, "domain_probe": DOMAIN_PROBE[domain]}


def _read(directory, name):
    return read_object(safe_bytes(directory / name), name)


def _delta(candidate, incumbent):
    return {"candidate_only": sorted(set(candidate) - set(incumbent)),
            "incumbent_only": sorted(set(incumbent) - set(candidate))}


def _fields(receipt, key):
    if receipt.get("origin") == "local":
        return ()
    fields = receipt["packet"][key]
    if not isinstance(fields, dict):
        raise ValueError("invalid dispatched field map")
    return fields.keys()


def _signatures(directory, decision):
    candidate = decision["candidate"]
    if candidate not in ("A", "B") or decision["trial"] != int(directory.name):
        raise ValueError("invalid trial or arm assignment")
    incumbent = "B" if candidate == "A" else "A"
    design = _read(directory, "design.json")
    current, proposed = design["packet"]["incumbent"], design["response"]["policy"]
    declared = {k: _delta(proposed[k], current[k]) for k in ("retrieve", "memory")}
    executed = {}
    for stage in ("opening", "feedback", "return"):
        a, b = (_read(directory, f"{arm}-{stage}.json") for arm in (candidate, incumbent))
        moves = [a["response"]["move"], b["response"]["move"]]
        projection = {"material": _delta(_fields(a, "material"), _fields(b, "material")),
                      "moves": moves if moves[0] != moves[1] else "same"}
        if stage == "return":
            projection["memory"] = _delta(_fields(a, "receipt"), _fields(b, "receipt"))
        executed[stage] = projection
    # Case folding, punctuation and whitespace normalization; no synonym expansion.
    question = design["response"]["question"]
    if not isinstance(question, str):
        raise ValueError("research question must be text")
    words = re.findall(r"\w+", question.casefold())
    if not words:
        raise ValueError("missing research question")
    signatures = {"declared_field_delta": declared, "executed_contrast": executed,
                  "question_word_sequence": " ".join(words)}
    hypothesis = design["response"].get("hypothesis")
    if hypothesis is not None:
        signatures["witness_question"] = {key: hypothesis[key] for key in
                                          ("stage", "channel", "feature", "direction")}
    return signatures


def audit_directory(directory):
    """Only read completed numbered records. Do not reopen or modify a model session."""
    groups = defaultdict(lambda: defaultdict(list))
    completed, pending, issues, missing_witness = [], [], [], []
    for path in sorted(Path(directory).iterdir()):
        if (not re.fullmatch(r"[0-9]{3}", path.name) or path.is_symlink()
                or (hasattr(path, "is_junction") and path.is_junction()) or not path.is_dir()):
            continue
        if not (path / "decision.json").exists():
            pending.append(int(path.name))
            continue
        try:
            signatures = _signatures(path, _read(path, "decision.json"))
        except (ValueError, KeyError, OSError, TypeError) as error:
            issues.append({"trial": int(path.name), "error": type(error).__name__})
            continue
        trial = int(path.name)
        completed.append(trial)
        if "witness_question" not in signatures:
            missing_witness.append(trial)
        for name, signature in signatures.items():
            key = json.dumps(signature, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
            groups[name][key].append(trial)
    repeats = {name: [{"trials": ids, "signature": json.loads(key)}
                      for key, ids in sorted(values.items(), key=lambda item: (-len(item[1]), item[1]))
                      if len(ids) > 1] for name, values in groups.items()}
    return {"readable_completed_ids": completed, "pending_ids": pending, "unreadable": issues,
            "no_preregistered_witness": missing_witness, "repeated": repeats}


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    modes = cli.add_mutually_exclusive_group(required=True)
    modes.add_argument("--audit", type=Path, help="directory containing completed NNN trial directories")
    modes.add_argument("--focus", type=int, help="trial ID to preview in the specified domain")
    cli.add_argument("--domain", choices=DOMAIN_PROBE, default="writer")
    opts = cli.parse_args()
    result = audit_directory(opts.audit) if opts.audit else focus_for(opts.focus, opts.domain)
    print(json.dumps(result, indent=2, ensure_ascii=False))
