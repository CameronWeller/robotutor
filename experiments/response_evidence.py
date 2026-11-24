"""Synthetic inputs and Responses API request for the response-evidence probe."""

import json


CLARIFY = "clarify"
ASK_REASONING = "ask for reasoning"
INDEPENDENT_CHECK = "offer an independently attempted check"
CHANGED_CHECK = "offer a changed-example check"


def _case(response, assistance, feedback, expected_action):
    return {
        "task": "What is 3/4 + 1/4?",
        "response": response,
        "assistance": assistance,
        "supplied_feedback": feedback,
        "expected_action": expected_action,
    }


CASES = {
    "correct-unknown": _case("1", "unknown", "correct", INDEPENDENT_CHECK),
    "correct-reported": _case("1", "reported", "correct", INDEPENDENT_CHECK),
    "correct-none": _case("1", "none reported", "correct", CHANGED_CHECK),
    "incorrect": _case("4/8", "unknown", "incorrect", ASK_REASONING),
    "ambiguous": _case("1/[illegible]", "unknown", "ambiguous", CLARIFY),
}

INSTRUCTIONS = """Apply this toy tutor policy to the supplied observation.
Use the supplied feedback as the judgment of the response. If the feedback is
ambiguous, ask for clarification. If incorrect, ask for the learner's reasoning.
If correct with unknown or reported assistance, offer an independently attempted
check. If correct with no assistance reported, offer a changed-example check.
Choose the action, give a short reason for it, and state what the evidence
supports. One response cannot establish mastery, an error does not diagnose its
cause, and assistance disclosures do not verify independent performance."""


def build_request(case_id, model):
    case = CASES[case_id]
    observation = {
        field: case[field]
        for field in ("task", "response", "assistance", "supplied_feedback")
    }
    return {
        "model": model,
        "instructions": INSTRUCTIONS,
        "input": json.dumps(observation),
        "store": False,
        "max_output_tokens": 1024,
        "reasoning": {"effort": "low"},
        "text": {
            "format": {
                "type": "json_schema",
                "name": "response_evidence",
                "strict": True,
                "schema": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": [CLARIFY, ASK_REASONING, INDEPENDENT_CHECK, CHANGED_CHECK],
                        },
                        "reason": {"type": "string"},
                        "claim": {"type": "string"},
                    },
                    "required": ["action", "reason", "claim"],
                    "additionalProperties": False,
                },
            }
        },
    }
