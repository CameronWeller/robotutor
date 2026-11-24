"""Compare an adopted tutor display with a single replay under a narrower current field grant."""
import argparse
from hashlib import sha256
from pathlib import Path
import tempfile

from teaching_trial import FIELDS, Model, encode, pack, read, save, tutor_contract
from water_teaching import call


READER = (
    "Review X and Y separately against this writing task and submission. For each display, "
    "does it ask the writer to test what a particular reader would infer or do from a phrase in a concrete "
    "circumstance? Put the shortest sufficient verbatim quote in X_test or Y_test, or the literal absent. "
    "Does it leave selection of actual revised wording to the writer? Put an exact supporting quote in "
    "X_wording or Y_wording, or absent. If it supplies the finished replacement wording, mark wording absent. "
    "Use X_reason and Y_reason for brief explanations. Judge only what is on the page."
)


def digest(raw):
    return sha256(raw).hexdigest()


def run(directory, destination, allowed, expected):
    decision = read(directory / "decision.json")
    if not decision["adopted"] or not set(allowed) <= set(FIELDS):
        raise ValueError("select an adopted policy and bounded field labels")
    original_path = directory / (decision["candidate"] + "-resume.json")
    original = read(original_path)
    source = original["packet"]
    if (digest(encode(source).encode()) != expected or original["origin"] != "model"
            or original["model_requested"] != "gpt-6.2-sol"):
        raise ValueError("archived dispatched packet or model differs from the preregistration")
    selected = source["lesson_route"]["slot"] + "_pack"
    course = pack(decision["domain"])
    released = course[selected]
    if (source["new_task"] != course["return_pack"]["practice"] or source["lesson_route"]["unit"] != released["unit"]
            or any(released[key] != value for key, value in source["material"].items())):
        raise ValueError("archived lesson differs from current pinned teaching release")
    memory = source["receipt"]
    limited = {**source, "receipt": {k: v for k, v in memory.items() if k in allowed}}
    removed = sorted(memory.keys() - limited["receipt"].keys())
    if not removed:
        raise ValueError("this comparison requires an actually removed field")

    if tutor_contract(original["model_requested"], source)[1] != original["request_hash"]:
        raise ValueError("current tutor instructions differ from the archived request")
    prompt, identity, check = tutor_contract(original["model_requested"], limited)
    info = {"trial": decision["trial"], "arm": decision["candidate"],
            "stage_sha256": digest(original_path.read_bytes()), "original_packet_sha256": expected,
            "limited_packet_sha256": digest(encode(limited).encode()), "removed": removed,
            "retained": limited["receipt"], "original_response": original["response"]}
    with destination.open("x", encoding="utf-8") as out:
        out.write(encode(info) + "\n")
    with tempfile.TemporaryDirectory(prefix="return-fields-") as scratch:
        d = Path(scratch)
        model = Model(original["model_requested"], d, protocol=decision["protocol"])
        new = model.request("restricted-resume", prompt, limited, d / "stage.json", identity, check)
        info["restricted"] = read(d / "stage.json")
    save(destination, info)
    labels = ["original", "restricted"]
    if int(identity[-1], 16) % 2:
        labels.reverse()
    mapping = dict(zip(("X", "Y"), labels))
    displays = {"original": original["response"]["display"], "restricted": new["display"]}
    blinded = {label: displays[name] for label, name in mapping.items()}
    blind_packet = {"task": source["new_task"], "submission": source["shared_work"], **blinded}
    verdict = call(blind_packet, READER, [f"{label}_{field}" for label in mapping for field in ("test", "wording", "reason")])
    quote_hits = {}
    for label, name in mapping.items():
        quote_hits[name] = {}
        for criterion in ("test", "wording"):
            quote = verdict["response"][f"{label}_{criterion}"]
            quote_hits[name][criterion] = quote != "absent" and quote in blinded[label]
    info.update(reader=verdict, labels=mapping, exact_quotes=quote_hits)
    save(destination, info)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", required=True)
    parser.add_argument("--expect-packet", required=True)
    parser.add_argument("--fields", nargs="+", choices=FIELDS, required=True)
    parser.add_argument("archive_directory", type=Path)
    parser.add_argument("new_receipt", type=Path)
    args = parser.parse_args()
    run(args.archive_directory, args.new_receipt, args.fields, args.expect_packet)
