"""Rehearse a source-attribution correction on trial 010 with an inspected feedback gate."""
import argparse
from hashlib import sha256
from pathlib import Path
import tempfile

from learning_conditions import condition_for, fallback_for
from teaching_trial import FOCI, MOVES, PACKS, Model, encode, pack, read, save, study_prompt
from water_teaching import call


MODEL = "gpt-6.2-sol"
INSTRUCTION = """Act as the tutor following the policy and the learner's request. Display a
specific useful teaching response in at most 110 words, grounded in available lesson material.
Decide the move actually displayed and the focus a learner could choose to revisit.
Output keys display, move (explain/practice/probe/clarify/defer/stop), focus
(condition/reasoning/source/observation/choice/unknown). You cannot issue a grade."""
RULE = ("For attribution, compare the offered words to the exact reviewed passage. Identify the "
        "words that support your account, distinguish a contribution from the fictional situation "
        "or writer, and correct an earlier tutor attribution if it was wrong. Leave the image and "
        "wording to the writer. Practice may stay private. The writer decides what is meaningful.")
OPERATION = "Compare a feature borrowed from the current public passage with a scene detail the writer chooses."
READER = ("Read the two teaching sequences X and Y independently. Use the supplied public passages and facts. "
          "For each feedback, put an exact excerpt in X_correction or Y_correction showing whether the tutor "
          "corrects the earlier claim that the psalm provides the lamp. Write absent if it does not. "
          "For each return, put an exact excerpt in X_action or Y_action if it invites two versions for "
          "comparing a feature from the public passage with a detail or wording owned by the writer. Write "
          "absent otherwise. Explain separately whether each return permits privacy and whether either "
          "display claims spiritual or spoken performance. Do not infer any effect on learning.")


def digest(data):
    return sha256(data).hexdigest()


def span(slot):
    domain = "worship"
    release = pack(domain)[slot]
    raw = (PACKS / (domain + ".json")).read_bytes().replace(b"\r\n", b"\n")
    text = release["excerpt"].encode()
    field = b'"excerpt": "' + text + b'"'
    if raw.count(field) != 1:
        raise ValueError("the public excerpt must have a unique literal in this edition")
    start = raw.index(field) + len(b'"excerpt": "')
    return {"role": "reviewed_public_passage", "url": release["source_url"], "locator": release["locator"],
            "fixture_edition": digest(raw), "fixture_utf8_bytes": [start, start + len(text)],
            "quote": raw[start:start + len(text)].decode()}


def tutors_prompt(packet):
    return study_prompt(INSTRUCTION, packet)


def historical(directory, stage):
    path = directory / ("A-" + stage + ".json")
    record = read(path)
    actual = digest((MODEL + "\n" + tutors_prompt(record["packet"])).encode())
    expected = {"feedback": "26f933d5032be669aed439a186497fe61a66976bd0487ea439097770342bb747",
                "return": "b405f888bb13f4a3be355cd079ba2956c8327e68c81d75b2291a5e307b709fc4"}[stage]
    if digest(path.read_bytes()) != expected or actual != record["request_hash"] or record["origin"] != "model":
        raise ValueError("original trial 010 stage or historical tutor prompt differs")
    return record


def ask(stage, packet):
    def check(response):
        if (response["move"] not in MOVES or response["focus"] not in FOCI
                or not response["display"].strip() or len(response["display"]) > 1700):
            raise ValueError("tutor display is unavailable under these stage rules")
        return response
    prompt = tutors_prompt(packet)
    identity = digest((MODEL + "\n" + prompt).encode())
    with tempfile.TemporaryDirectory(prefix="attribution-teaching-") as working:
        model = Model(MODEL, Path(working), protocol=3)
        path = Path(working) / "stage.json"
        model.request(stage, prompt, packet, path, identity, check)
        record = read(path)
        if record["response"]["move"] in packet["allowed_moves"]:
            record["origin"] = "model"
        else:
            record["origin"] = "fallback"
            record["proposed_response"] = record["response"]
            record["response"] = fallback_for(condition_for(10, "worship"), stage.split("-")[-1])
        return record


def feedback(directory, output):
    old = historical(directory, "feedback")
    new = {**old["packet"], "attribution_rule": RULE, "source": span("first_pack")}
    new["submitted_language"] = {"role": "learner_submission_and_claim", "text": new.pop("answer")}
    with output.open("x", encoding="utf-8") as file:
        file.write("{}\n")
    save(output, {"historical_feedback_request": old["request_hash"], "feedback": ask("A-feedback", new)})


def finish(directory, output, quote):
    old = historical(directory, "return")
    data = read(output)
    response = data["feedback"]["response"]["display"]
    if set(data) != {"historical_feedback_request", "feedback"} or not quote or quote not in response:
        raise ValueError("an editor must locate the attribution distinction in the actual feedback first")
    data["inspected_feedback_quote"] = quote
    public_note = {"operation": OPERATION, "basis": span("first_pack")["fixture_edition"]}
    data["public_teaching_note"] = public_note
    new = {**old["packet"], "attribution_rule": RULE, "source": span("return_pack"), "teaching_note": public_note}
    data["historical_return_request"] = old["request_hash"]
    data["return"] = ask("A-return", new)
    save(output, data)
    original = {"feedback": historical(directory, "feedback")["response"]["display"], "return": old["response"]["display"]}
    changed = {s: data[s]["response"]["display"] for s in ("feedback", "return")}
    labels = {"X": "original", "Y": "revised"} if int(data["return"]["request_hash"][-1], 16) % 2 else {"X": "revised", "Y": "original"}
    sequences = {"original": original, "revised": changed}
    packet = {k: sequences[v] for k, v in labels.items()}
    packet.update(first_public_quote=span("first_pack")["quote"], current_public_quote=span("return_pack")["quote"],
                  fictional_task="The worker finds a lamp has been left on for their walk home.",
                  learner_intent="They meant the lamp as gratitude. On return they wanted to try two versions aloud.")
    data["reader"] = call(packet, READER, [f"{label}_{key}" for label in labels for key in ("correction", "action", "reason")])
    data["blind_order"] = labels
    data["quote_checks"] = {}
    for label, name in labels.items():
        found = {}
        for field, stage in (("correction", "feedback"), ("action", "return")):
            text = data["reader"]["response"][f"{label}_{field}"]
            found[field] = text != "absent" and text in sequences[name][stage]
        data["quote_checks"][name] = found
    save(output, data)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--feedback", action="store_true")
    action.add_argument("--finish-with-quote", metavar="VERBATIM")
    parser.add_argument("directory", type=Path)
    parser.add_argument("receipt", type=Path)
    args = parser.parse_args()
    if args.feedback:
        feedback(args.directory, args.receipt)
    else:
        finish(args.directory, args.receipt, args.finish_with_quote)
