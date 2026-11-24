"""Run one matched tutor replay and one blind, quote-grounded comparison."""
from hashlib import sha256
from pathlib import Path
import tempfile

from teaching_trial import Model, encode, read, save, tutor_contract
from water_teaching import MODEL, call


def digest(data):
    return sha256(data).hexdigest()


def compare(output, protocol, evidence, corrected, reader_context, instruction, criteria):
    original = evidence["original_return"]
    if (original.get("origin") != "model" or original["model_requested"] != MODEL or
            tutor_contract(MODEL, original["packet"])[1] != original["request_hash"]):
        raise ValueError("tutor model or instructions differ from the original call")
    prompt, identity, validate = tutor_contract(MODEL, corrected)
    with output.open("x", encoding="utf-8") as stream:
        stream.write(encode(evidence) + "\n")
    with tempfile.TemporaryDirectory(prefix="matched-tutor-") as scratch:
        path = Path(scratch) / "corrected.json"
        model = Model(MODEL, path.parent, protocol=protocol)
        # One request: a rejected draw is an outcome, not authorization to draw again.
        model.request("corrected", prompt, corrected, path, identity, validate)
        evidence["corrected"] = read(path)
    save(output, evidence)
    names = ["original", "corrected"]
    if int(identity[-1], 16) % 2:
        names.reverse()
    order = dict(zip(("X", "Y"), names))
    displays = {"original": original["response"]["display"], "corrected": evidence["corrected"]["response"]["display"]}
    packet = {**reader_context, **{label: displays[k] for label, k in order.items()}}
    result = call(packet, instruction, [f"{label}_{key}" for label in order for key in criteria])
    judged = {}
    for label, name in order.items():
        judged[name] = {}
        for field in criteria:
            quote = result["response"][label + "_" + field]
            judged[name][field] = ("absent" if quote == "absent" else
                                   "quoted" if quote in displays[name] else "unmatched")
    evidence.update(reader=result, order=order, quote_status=judged)
    save(output, evidence)
