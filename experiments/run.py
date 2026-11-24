"""Preview a Robotutor experiment, check model access, or submit one synthetic case."""

import argparse
import getpass
import http.client
import json
import os
import sys
import warnings
from datetime import datetime, timezone
from urllib.parse import quote

from .response_evidence import CASES, build_request


def get_api_key():
    key = os.environ.get("OPENAI_API_KEY", "").strip()
    if not key:
        if not sys.stdin.isatty():
            raise ValueError("Set OPENAI_API_KEY or run in a terminal for hidden key entry.")
        with warnings.catch_warnings():
            warnings.simplefilter("error", getpass.GetPassWarning)
            try:
                key = getpass.getpass("OpenAI project key (hidden): ").strip()
            except (getpass.GetPassWarning, EOFError):
                raise ValueError("Hidden key entry is unavailable. Set OPENAI_API_KEY in your shell.") from None
    if not key or not key.isascii() or not key.isprintable() or any(c.isspace() for c in key):
        raise ValueError("Enter a nonempty API key without spaces or control characters.")
    return key


def api_request(path, api_key, payload=None):
    headers = {"Authorization": "Bearer " + api_key}
    body = None
    if payload is not None:
        body = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"

    connection = http.client.HTTPSConnection("api.openai.com", timeout=60)
    try:
        connection.request("GET" if body is None else "POST", path, body=body, headers=headers)
        response = connection.getresponse()
        if response.status != 200:
            hints = {
                400: "Check that the model supports the request shown in the preview.",
                401: "Check your OpenAI project key.",
                403: "Check the project and the key's endpoint permissions.",
                404: "Check the model ID and your project's access.",
                429: "Check project billing, available quota, and rate limits.",
            }
            hint = hints.get(response.status, "Check the API service status.")
            raise ValueError(f"OpenAI returned HTTP {response.status}. {hint}")
        data = json.load(response)
    except (OSError, http.client.HTTPException):
        raise ValueError("The connection to OpenAI failed. A submitted request may have reached the API.") from None
    except (json.JSONDecodeError, UnicodeDecodeError):
        raise ValueError("OpenAI returned an unreadable response.") from None
    finally:
        connection.close()
    if not isinstance(data, dict):
        raise ValueError("OpenAI returned an unexpected response.")
    return data


def extract_decision(response):
    if response.get("status") != "completed":
        raise ValueError("The model response did not complete. No decision was scored.")
    try:
        parts = [
            part
            for item in response["output"]
            if item["type"] == "message" and item["role"] == "assistant"
            and item.get("phase") in (None, "final_answer")
            for part in item["content"]
        ]
        if len(parts) != 1 or parts[0]["type"] != "output_text":
            raise ValueError
        decision = json.loads(parts[0]["text"])
        if set(decision) != {"action", "reason", "claim"}:
            raise ValueError
        if not all(isinstance(value, str) and value.strip() for value in decision.values()):
            raise ValueError
        if decision["action"] not in {case["expected_action"] for case in CASES.values()}:
            raise ValueError
    except (AttributeError, KeyError, TypeError, ValueError):
        raise ValueError("The response did not contain a valid decision. No decision was scored.") from None
    return decision


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--check-key", action="store_true", help="check model access without generating a response")
    modes.add_argument("--live", action="store_true", help="send one billable Responses API request")
    parser.add_argument("--case", choices=CASES, default="correct-unknown")
    parser.add_argument("--model", default="gpt-6.2-astra")
    args = parser.parse_args(argv)

    try:
        if args.check_key:
            model = api_request("/v1/models/" + quote(args.model, safe=""), get_api_key())
            if model.get("id") != args.model:
                raise ValueError("Model lookup returned an unexpected model ID.")
            print(f"Model lookup succeeded for {args.model}.")
            return 0

        request = build_request(args.case, args.model)
        if not args.live:
            print(json.dumps(request, indent=2))
            return 0

        response = api_request("/v1/responses", get_api_key(), request)
        decision = extract_decision(response)
        expected = CASES[args.case]["expected_action"]
        report = {
            "experiment": "response-evidence",
            "completed_at_utc": datetime.now(timezone.utc).isoformat(),
            "case": args.case,
            "input": json.loads(request["input"]),
            "requested_model": args.model,
            "returned_model": response.get("model"),
            "decision": decision,
            "expected_action": expected,
            "action_matches": decision["action"] == expected,
            "usage": response.get("usage"),
        }
        print(json.dumps(report, indent=2))
        return 0
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Interrupted.", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
