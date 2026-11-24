"""Compare an archived goal-switch reply with one neutral intake and a blind reading."""
import argparse
from pathlib import Path

from matched_tutor import compare, digest
from resumed_work import new_goal_packet
from teaching_trial import encode, read
from water_teaching import MODEL


def run(directory, output, expected_return, expected_resume, expected_packet, old_rule):
    decision = read(directory / "decision.json")
    arm = decision["candidate"]
    bound = {}
    records = {}
    for stage, expected in (("return", expected_return), ("resume", expected_resume)):
        path = directory / f"{arm}-{stage}.json"
        bound[stage] = digest(path.read_bytes())
        if bound[stage] != expected:
            raise ValueError("archived tutor stage differs from preregistration")
        records[stage] = read(path)
    case = read(directory / "case.json")["response"]
    original = records["return"]
    resume = records["resume"]
    if (not decision["adopted"] or case["return_intent"] != "change_goal"
            or original["model_requested"] != MODEL or original.get("origin") != "model"
            or original["packet"]["learner"] != case["return_request"]
            or resume["packet"]["learner"] != case["resume_request"]
            or resume["packet"].get("shared_work") != case["resume_work"] or not case["resume_work"]
            or original["packet"]["policy"] != resume["packet"]["policy"]
            or resume["packet"]["policy"] != decision["incumbent_after"]["returning"]):
        raise ValueError("requires a released adopted policy and a confirmed switch and resumption")
    corrected = new_goal_packet(original["packet"])
    if digest(encode(corrected).encode()) != expected_packet:
        raise ValueError("projected intake packet differs from preregistration")
    evidence = {"trial": decision["trial"], "arm": arm, "original_stage_sha256": bound,
                "old_rule_under_test": old_rule, "original_return": original, "archived_resume": resume}
    packet = {"current_request": corrected["learner"], "exercise_to_check": old_rule}
    instruction = ("Read X and Y separately. For each, identify a question or concrete invitation to share "
                   "an example to establish what the learner wants to work on in the new goal. Quote the "
                   "shortest sufficient exact text under intake, or use absent. Does the display prescribe "
                   "the exercise named under exercise_to_check before seeing the learner's work? Quote "
                   "the shortest sufficient exact text under exercise, or use absent. Judge the wording alone.")
    compare(output, decision["protocol"], evidence, corrected, packet, instruction, ("intake", "exercise"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("new_receipt", type=Path)
    parser.add_argument("--live", action="store_true", required=True)
    parser.add_argument("--return-sha", required=True)
    parser.add_argument("--resume-sha", required=True)
    parser.add_argument("--neutral-packet-sha", required=True)
    parser.add_argument("--old-rule", required=True)
    args = parser.parse_args()
    run(args.directory, args.new_receipt, args.return_sha, args.resume_sha, args.neutral_packet_sha, args.old_rule)
