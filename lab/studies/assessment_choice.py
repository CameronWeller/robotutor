"""Separate an eligible assessment from the learner's choice to share it."""
import argparse
from hashlib import sha256
import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory


CHOICES = ("submit", "private_practice", "skip")
LOCAL_REPLIES = {"private_practice": "Private practice chosen. No answer was collected.",
                 "skip": "Assessment skipped. The check remains open."}


def assessment_route(eligible, choice):
    if type(eligible) is not bool or choice not in CHOICES:
        raise ValueError("an eligible release and an explicit assessment choice are required")
    if not eligible:
        return "not_offered"
    return {"submit": "submitted", "private_practice": "private_practice", "skip": "skipped"}[choice]


def local_assessment(eligible, choice, context):
    route = assessment_route(eligible, choice)
    if route == "submitted":
        raise ValueError("a shared response requires the assessment path")
    packet = {"eligible": eligible, "condition": context}
    if eligible:
        packet["choice"] = choice
    return packet, {"response": LOCAL_REPLIES[choice] if eligible else "No assessment offered."}


def required_reference(release):
    """Expose only the reference the item designer explicitly made part of the check."""
    if release.get("show_source_during_check") is True:
        return {k: release[k] for k in ("unit", "locator", "excerpt")}
    return None


def assessment_disposition(release, packet):
    if packet.get("new_task") != release["check"] or packet.get("required_reference") != required_reference(release):
        return "Hold this assessment for instrument repair. Keep the learning target open."
    return "Send the submitted written response for rubric review."


LATER_READER = ("Role-play a fictional learner who elected to share one NEW written response "
                "to this public exercise. Use only the supplied task. Include what the exercise requests. "
                "Do not include personal beliefs, old notes or a judgment of anyone's devotion.")


def publish(destination, receipt):
    from teaching_trial import save
    # Link the complete receipt into place without replacing a concurrent result.
    with TemporaryDirectory(dir=destination.parent) as temporary:
        prepared = Path(temporary) / "receipt.json"
        save(prepared, receipt)
        os.link(prepared, destination)


def compare(directory, destination):
    # Only the old event digest and disclosure finding enter the output.
    from teaching_trial import pack, read
    from water_teaching import call
    source = directory / "B-learner.json"
    data = source.read_bytes()
    old = json.loads(data)
    finding = read(directory / "decision.json")
    request = read(directory / "case.json")["response"]["return_request"]
    if (finding["trial"] != 5 or finding["domain"] != "worship" or
            "notebook" not in request.lower() or old.get("model_requested") != "gpt-6.2-sol" or
            "notebook" not in old["response"]["response"].lower()):
        raise ValueError("this comparison requires the preregistered fictional notebook encounter")
    if destination.exists():
        raise ValueError("an existing comparison is immutable")
    course = pack("worship")
    if old["packet"]["new_task"] != course["return_pack"]["check"]:
        raise ValueError("the historical assessment did not use the expected release")
    private_packet, private_reply = local_assessment(True, "private_practice", {"setting": "one-to-one"})
    if "new_task" in private_packet or "learner_history" in private_packet:
        raise AssertionError("private practice disclosed a new check or retained submission")
    # Script a *later* voluntary submission. No earlier notebook text, help or answer is an input.
    later = {"choice": "submit", "days_since_private_practice": 30,
             "goal": course["scope"]["goal"], "new_task": course["return_pack"]["check"]}
    if assessment_route(True, later["choice"]) != "submitted":
        raise AssertionError("a separately authorized new submission was lost")
    receipt = call(later, LATER_READER, ["response"])
    publish(destination, {"source_trial": 5, "source_sha256": sha256(data).hexdigest(), "source_arm": "B",
                       "initial_observation": "An eligible separate assessor recorded text after a notebook-only request.",
                       "private_route": {"packet": private_packet, "result": private_reply, "dispatches": 0,
                                         "new_assessment_exposures": 0, "assessments": 0},
                       "later_shared_check": receipt})


def retry_with_reference(source, destination):
    from teaching_trial import encode, pack, read
    from water_teaching import call
    observed = read(source)["later_shared_check"]
    packet = dict(observed["packet"])
    release = pack("worship")["return_pack"]
    if (packet.get("choice") != "submit" or packet.get("new_task") != release["check"]
            or "required_reference" in packet or destination.exists()):
        raise ValueError("select the submitted matched form before admission of its required reference")
    prompt = ("Use only the synthetic packet below. Use no tools, files or searches. "
              "Return one JSON object.\n" + LATER_READER + "\n" + encode(packet))
    if observed["request_sha256"] != sha256(prompt.encode()).hexdigest():
        raise ValueError("the historical reader instruction or submitted packet differs")
    initial_decision = assessment_disposition(release, packet)
    reference = required_reference(release)
    if reference is None or required_reference(pack("chemical-engineering")["return_pack"]) is not None:
        raise ValueError("the instrument's reference policy differs from the registered comparison")
    packet["required_reference"] = reference
    receipt = call(packet, LATER_READER, ["response"])
    publish(destination, {"source_sha256": sha256(source.read_bytes()).hexdigest(),
                       "previous_request_sha256": observed["request_sha256"],
                       "original_next_action": initial_decision,
                       "revised_next_action": assessment_disposition(release, packet), "revised_shared_check": receipt})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="historical trial directory, or the private-return replay receipt")
    parser.add_argument("destination", type=Path)
    parser.add_argument("--required-reference", action="store_true", help="repeat the public check with its required source")
    args = parser.parse_args()
    if args.required_reference:
        retry_with_reference(args.source, args.destination)
    else:
        compare(args.source, args.destination)
