"""Bind a participant's topic selection to a release, and test one frozen paired prediction."""
from hashlib import sha256
import json
from pathlib import Path

from learning_conditions import ACTIONS, EVIDENCE, MATERIAL, MEMORY


PACKS = Path(__file__).resolve().parents[1] / "fixtures" / "goals"
FEATURES = {"material": MATERIAL, "memory": (*MEMORY, "topic"), "move": ACTIONS}
STAGES = ("opening", "feedback", "return")
RESUMED_STAGES = (*STAGES, "resume")
RESUMED_FEATURES = {**FEATURES, "selection": ("first_unit", "return_unit", "contrast_material")}
DIRECTIONS = ("candidate_only", "incumbent_only")


def offered_topics(domain):
    """Derive participant-facing choices from the current independently pinned fixture."""
    if domain not in EVIDENCE:
        raise ValueError("unknown reviewed domain")
    raw = (PACKS / (domain + ".json")).read_bytes().replace(b"\r\n", b"\n")
    edition = sha256(raw).hexdigest()
    if json.loads((PACKS / "reviews.json").read_bytes()).get(domain) != edition:
        raise ValueError("topic menu requires the reviewed edition")
    course = json.loads(raw)
    choices = {}
    for slot in ("first", "return"):
        unit = course[slot + "_pack"]["unit"]
        titles = [u["title"] for u in course["proposal"]["units"] if u["id"] == unit]
        if type(unit) is not int or not 0 < unit <= 0xffffffff or len(titles) != 1:
            raise ValueError("released topic must resolve to one reviewed unit")
        title = titles[0]
        if not isinstance(title, str) or not 0 < len(title.strip()) <= 200:
            raise ValueError("released topic needs a bounded title")
        choices[slot] = {"domain": domain, "unit": unit, "edition": edition, "title": title}
    if choices["first"]["unit"] == choices["return"]["unit"]:
        raise ValueError("first and return topics must be distinct choices")
    return {**choices, "none": None}


def approved_topic(domain, selection, offers):
    """Resolve only an explicit choice from the exact earlier offer against the current release."""
    current = offered_topics(domain)
    if json.dumps(offers, sort_keys=True, allow_nan=False) != json.dumps(current, sort_keys=True):
        raise ValueError("offered topics no longer match this release")
    if not isinstance(selection, str) or selection not in current:
        raise ValueError("expected an explicit first, return or none selection")
    if selection == "none":
        return None
    return {k: current[selection][k] for k in ("domain", "unit", "edition")}


def topic_confirmation(domain, stored):
    """Call after persistence, with the topic actually stored under this participant's grant."""
    if stored is None:
        return "No lesson topic was saved."
    for key, offer in offered_topics(domain).items():
        if key == "none":
            continue
        if (isinstance(stored, dict) and type(stored.get("unit")) is int
                and stored == {k: offer[k] for k in ("domain", "unit", "edition")}):
            return "Saved lesson topic: " + offer["title"]
    raise ValueError("stored topic has no matching released title")


def hypothesis_schema(protocol=4):
    def labels(values):
        return {"type": "string", "enum": list(values)}
    features = RESUMED_FEATURES if protocol >= 8 else FEATURES
    fields = {"stage": labels(RESUMED_STAGES if protocol >= 8 else STAGES), "channel": labels(features),
              "feature": labels(sorted(set().union(*features.values()))),
              "direction": labels(DIRECTIONS)}
    return {"type": "object", "properties": fields, "required": list(fields),
            "additionalProperties": False}


def validate_hypothesis(raw, protocol=4):
    if not isinstance(raw, dict) or set(raw) != {"stage", "channel", "feature", "direction"}:
        raise ValueError("the prediction needs exactly stage, channel, feature and direction")
    if any(not isinstance(value, str) for value in raw.values()):
        raise ValueError("prediction fields must be labels")
    stages = RESUMED_STAGES if protocol >= 8 else STAGES
    features = RESUMED_FEATURES if protocol >= 8 else FEATURES
    if raw["stage"] not in stages or raw["direction"] not in DIRECTIONS:
        raise ValueError("unknown prediction stage or direction")
    if raw["feature"] not in features.get(raw["channel"], ()):
        raise ValueError("feature is unavailable for this prediction channel")
    if raw["channel"] == "memory" and raw["stage"] not in ("return", "resume"):
        raise ValueError("the first visit has no incoming return memory")
    if raw["channel"] == "selection" and raw["stage"] not in ("return", "resume"):
        raise ValueError("a lesson selection requires a return or resumed visit")
    return dict(raw)


def expected_pair(hypothesis):
    candidate = hypothesis["direction"] == "candidate_only"
    return {"candidate": candidate, "incumbent": not candidate}


def validate_design_witness(hypothesis, candidate_policy, incumbent_policy, protocol=4):
    """Exclude impossible static contrasts at design time, before seeing either learner or result."""
    hypothesis = validate_hypothesis(hypothesis, protocol)
    field = {"material": "retrieve", "memory": "memory"}.get(hypothesis["channel"])
    if field is not None:
        selection = {"candidate": hypothesis["feature"] in candidate_policy[field],
                     "incumbent": hypothesis["feature"] in incumbent_policy[field]}
        if selection != expected_pair(hypothesis):
            raise ValueError("the named policy field must differ in the predicted direction")
    if hypothesis["channel"] == "selection" and hypothesis["feature"] == "contrast_material":
        comparison = {"candidate": candidate_policy.get("resume_mode", "booked") == "compare",
                      "incumbent": incumbent_policy.get("resume_mode", "booked") == "compare"}
        if comparison != expected_pair(hypothesis):
            raise ValueError("the predicted policy alone must request comparison material")
    return hypothesis


def _present(hypothesis, receipt):
    channel, feature = hypothesis["channel"], hypothesis["feature"]
    if channel == "move":
        response = receipt["response"]
        if response["move"] not in ACTIONS or not isinstance(response["display"], str) or not response["display"].strip():
            raise ValueError("missing executed teaching move")
        return response["move"] == feature
    # A host-side hold has no dispatched teacher packet, even if the local log records a plan.
    if receipt.get("origin") == "local":
        return False
    if channel == "selection":
        if receipt.get("origin") not in ("model", "fallback"):
            raise ValueError("lesson selection requires a dispatched teacher packet")
        packet = receipt["packet"]
        if feature == "contrast_material":
            material = packet.get("comparison_material", {})
            if not isinstance(material, dict):
                raise ValueError("comparison material must be a field map")
            return bool(material)
        route = packet.get("lesson_route")
        if route is None:
            return False
        if not isinstance(route, dict) or route.get("slot") not in ("first", "return"):
            raise ValueError("dispatched lesson route needs a released slot")
        return route["slot"] == ("first" if feature == "first_unit" else "return")
    slot = "receipt" if channel == "memory" else "material"
    transmitted = receipt["packet"][slot]
    if not isinstance(transmitted, dict):
        raise ValueError("missing actual dispatched teaching fields")
    if feature not in transmitted:
        return False
    if transmitted[feature] is None or transmitted[feature] == "":
        raise ValueError("selected field did not carry a value")
    return True


def evaluate_witness(hypothesis, candidate, arms, protocol=4):
    """Use executed, labeled stage receipts. Neither prose preferences nor a planned policy are evidence."""
    hypothesis = validate_hypothesis(hypothesis, protocol)
    if candidate not in ("A", "B") or set(arms) != {"A", "B"}:
        raise ValueError("the paired arms and candidate assignment must be identified")
    stage = hypothesis["stage"]
    incumbent = "B" if candidate == "A" else "A"
    observed = {"candidate": _present(hypothesis, arms[candidate][stage]),
                "incumbent": _present(hypothesis, arms[incumbent][stage])}
    expected = expected_pair(hypothesis)
    return {"hypothesis": hypothesis, "expected": expected, "observed": observed, "met": observed == expected}


def _checks():
    import unittest

    class Controls(unittest.TestCase):
        def test_prospective_topic_acceptance_and_decline(self):
            for domain in EVIDENCE:
                menu = offered_topics(domain)
                for selection in ("first", "return"):
                    stored = approved_topic(domain, selection, menu)
                    self.assertEqual(set(stored), {"domain", "edition", "unit"})
                    self.assertEqual(stored["edition"], menu[selection]["edition"])
                    self.assertEqual(topic_confirmation(domain, stored), "Saved lesson topic: " + menu[selection]["title"])
                self.assertIsNone(approved_topic(domain, "none", menu))
                with self.assertRaises(ValueError):
                    approved_topic(domain, True, menu)
                old = {**menu, "first": {**menu["first"], "edition": "0" * 64}}
                with self.assertRaises(ValueError):
                    approved_topic(domain, "first", old)
                with self.assertRaises(ValueError):
                    topic_confirmation(domain, {**stored, "answer": "unapproved text"})

        def test_preregistration_and_masked_contrast(self):
            h = {"stage": "return", "channel": "memory", "feature": "topic", "direction": "candidate_only"}
            self.assertEqual(validate_design_witness(h, {"memory": ["topic"]}, {"memory": []}), h)
            with self.assertRaises(ValueError):
                validate_design_witness(h, {"memory": ["topic"]}, {"memory": ["topic"]})
            with self.assertRaises(ValueError):
                validate_hypothesis({**h, "stage": "feedback"})
            menu = offered_topics("chemical-engineering")
            topic = approved_topic("chemical-engineering", "first", menu)
            candidate = {"packet": {"receipt": {"topic": topic}}}
            incumbent = {"packet": {"receipt": {}}}
            arms = {"B": {"return": candidate}, "A": {"return": incumbent}}
            self.assertTrue(evaluate_witness(h, "B", arms)["met"])
            self.assertFalse(evaluate_witness(h, "A", arms)["met"])
            self.assertTrue(evaluate_witness({**h, "direction": "incumbent_only"}, "A", arms)["met"])
            candidate["origin"] = "local"
            outcome = evaluate_witness(h, "B", arms)
            self.assertEqual(outcome["observed"], {"candidate": False, "incumbent": False})
            self.assertFalse(outcome["met"])

        def test_fallback_uses_dispatched_context_and_executed_move(self):
            h = {"stage": "opening", "channel": "move", "feature": "probe", "direction": "candidate_only"}
            fallback = {"origin": "fallback", "packet": {"material": {"worked": "A reviewed example."}},
                        "proposed_response": {"move": "probe"}, "response": {"move": "defer", "display": "Let's pause."}}
            held = {"origin": "local", "packet": {"material": {}}, "response": {"move": "stop", "display": "Stopped."}}
            arms = {"A": {"opening": fallback}, "B": {"opening": held}}
            self.assertFalse(evaluate_witness(h, "A", arms)["met"])
            self.assertTrue(evaluate_witness({**h, "feature": "defer"}, "A", arms)["met"])
            self.assertTrue(evaluate_witness({**h, "channel": "material", "feature": "worked"}, "A", arms)["met"])

        def test_later_grant_and_booked_release(self):
            from learning_conditions import PROFILES, condition_for, visit_rules
            from resumed_work import later_rule, lesson_route

            for index in range(1, 2 * len(PROFILES) + 1):
                condition = condition_for(index, "writer")
                later = later_rule(condition)
                self.assertEqual(later is None, visit_rules(condition, "return")["learner_attempt"])
                if later:
                    self.assertEqual(later["context"]["day"], condition["days"] + 30)
                    self.assertEqual(later["context"]["setting"], condition["setting"])
                    self.assertEqual(later["context"]["evidence_mode"], condition["evidence_mode"])
                    self.assertTrue(later["task"] and later["answer"])
                    self.assertFalse(later["learner_attempt"])
                    self.assertEqual(later["material"], list(MATERIAL))
                    self.assertEqual(later["actions"], list(ACTIONS))
                    self.assertIsNone(later["local"])
                    expected_memory = [] if condition["id"] in ("forget_topic", "goal_switch") else [*MEMORY, "topic"]
                    self.assertEqual(later["memory"], expected_memory)

            for domain in EVIDENCE:
                menu = offered_topics(domain)
                first = approved_topic(domain, "first", menu)
                returned = menu["return"]["unit"]
                self.assertEqual(lesson_route("booked", domain, first),
                                 {"mode": "booked", "slot": "first", "unit": first["unit"], "comparison_unit": None})
                for unavailable in (None, {**first, "edition": "old"}, {**first, "domain": "other"},
                                    {**first, "title": "unapproved metadata"}, {**first, "unit": True}):
                    self.assertEqual(lesson_route("booked", domain, unavailable)["unit"], returned)
                self.assertEqual(lesson_route("current", domain, first)["unit"], returned)
                self.assertEqual(lesson_route("compare", domain, first),
                                 {"mode": "compare", "slot": "return", "unit": returned,
                                  "comparison_unit": first["unit"]})
                with self.assertRaises(ValueError):
                    lesson_route("other", domain, first)

        def test_selection_witness_counts_only_dispatched_material(self):
            for stage in ("resume", "return"):
                h = {"stage": stage, "channel": "selection", "feature": "contrast_material", "direction": "candidate_only"}
                for old_protocol in (4, 5, 6, 7):
                    self.assertEqual(hypothesis_schema(old_protocol), hypothesis_schema())
                    with self.assertRaises(ValueError):
                        validate_hypothesis(h, old_protocol)
                with self.assertRaises(ValueError):
                    validate_hypothesis({**h, "stage": "opening"}, 8)
                with self.assertRaises(ValueError):
                    validate_design_witness(h, {}, {"resume_mode": "compare"}, 8)
                validate_design_witness(h, {"resume_mode": "compare"}, {}, 8)
                packet = {"lesson_route": {"slot": "return"}, "comparison_material": {"worked": "Different example"}}
                arms = {"A": {stage: {"origin": "fallback", "packet": packet}},
                        "B": {stage: {"origin": "model", "packet": {"lesson_route": {"slot": "first"}}}}}
                self.assertTrue(evaluate_witness(h, "A", arms, 8)["met"])
                self.assertTrue(evaluate_witness({**h, "feature": "return_unit"}, "A", arms, 8)["met"])
                self.assertTrue(evaluate_witness({**h, "feature": "first_unit"}, "B", arms, 8)["met"])
                packet["comparison_material"] = {}
                self.assertFalse(evaluate_witness(h, "A", arms, 8)["met"])
                arms["B"][stage]["origin"] = "local"
                self.assertFalse(evaluate_witness({**h, "feature": "first_unit"}, "B", arms, 8)["met"])
                del arms["B"][stage]["origin"]
                with self.assertRaises(ValueError):
                    evaluate_witness(h, "A", arms, 8)
                with self.assertRaises(KeyError):
                    evaluate_witness(h, "A", {"A": {}, "B": {}}, 8)

    result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(Controls))
    return result.wasSuccessful()


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", required=True, help="run synthetic local controls")
    parser.parse_args()
    raise SystemExit(0 if _checks() else 1)
