"""Project one completed booking transaction into the tutor's current teaching packet."""
import argparse
from pathlib import Path

from matched_tutor import compare, digest
from teaching_trial import encode, read


CANCELED = "No lesson topic is booked now."
CRITERIA = ("completed", "teaching", "unresolved", "repeat", "heard")
READER = ("Read X and Y separately against the learner's present request and the application's executed outcome. "
          "For each criterion return the shortest sufficient exact quote, or the literal absent. "
          "completed: tells the learner that the booking is already canceled or no topic is booked. "
          "teaching: gives concrete help with the submitted written count or tap. "
          "unresolved: implies the cancellation still needs doing or disclaims the ability to fulfill it. "
          "repeat: instructs the learner to repeat the booking operation, supply further permission, or start another "
          "visit to complete the cancellation or obtain this feedback. "
          "heard: claims to have heard an actual performance. Judge only the displayed words.")


def cancellation_receipt(transaction, confirmation):
    if (not isinstance(transaction, dict) or set(transaction) != {"format", "reason", "stored"}
            or transaction["format"] != "paired-return-memory/1" or transaction["reason"] != "cancel_topic"
            or not isinstance(transaction["stored"], dict) or "topic" in transaction["stored"]
            or confirmation != CANCELED):
        raise ValueError("requires completed cancellation and its canonical current outcome")
    return {"operation": "cancel_topic", "status": "completed", "topic_booked": False, "display": confirmation}


def run(directory, output, arm, expected_stage, expected_transaction, expected_learner, expected_packet):
    names = {"return": expected_stage, "return-memory": expected_transaction, "learner": expected_learner}
    records = {}
    for name, expected in names.items():
        path = directory / f"{arm}-{name}.json"
        if digest(path.read_bytes()) != expected:
            raise ValueError(f"{name} differs from preregistration")
        records[name] = read(path)
    source = records["return"]["packet"]
    transaction = records["return-memory"]
    case = read(directory / "case.json")["response"]
    decision = read(directory / "decision.json")
    trace = read(directory / "review-grounded.json")["packet"]["traces"][arm]
    previous = read(directory / f"{arm}-booking.json")["stored"]
    receipt = cancellation_receipt(transaction, trace["return_confirmation"])
    if ("topic" not in previous or {k: v for k, v in previous.items() if k != "topic"} != transaction["stored"]
            or case["return_intent"] != "cancel_topic" or source["learner"] != case["return_request"]
            or not case["return_work"] or source["shared_work"] != case["return_work"]
            or "application_receipt" in source):
        raise ValueError("requires an actual canceled booking and a newly shared teaching attempt")
    corrected = {**source, "application_receipt": receipt}
    if digest(encode(corrected).encode()) != expected_packet:
        raise ValueError("projected packet differs from preregistration")
    evidence = {"trial": decision["trial"], "arm": arm, "original_stage_sha256": names,
                "original_return": records["return"], "original_learner_response": records["learner"]["response"],
                "original_learner_received_current_confirmation": receipt["display"] in
                encode(records["learner"]["packet"]), "application_receipt": receipt}
    reader_context = {"current_request": source["learner"], "current_host_outcome": receipt["display"]}
    compare(output, decision["protocol"], evidence, corrected, reader_context, READER, CRITERIA)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("directory", type=Path)
    p.add_argument("new_receipt", type=Path)
    p.add_argument("--live", action="store_true", required=True)
    p.add_argument("--arm", choices=("A", "B"), required=True)
    for flag in ("return-sha", "transaction-sha", "learner-sha", "packet-sha"):
        p.add_argument("--" + flag, required=True)
    a = p.parse_args()
    run(a.directory, a.new_receipt, a.arm, a.return_sha, a.transaction_sha, a.learner_sha, a.packet_sha)
