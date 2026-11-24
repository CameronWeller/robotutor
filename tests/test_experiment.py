import io
import json
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

from experiments import run


def invoke(*args):
    stdout, stderr = io.StringIO(), io.StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        code = run.main(list(args))
    return code, stdout.getvalue(), stderr.getvalue()


def completed_response():
    decision = {
        "action": "clarify",
        "reason": "The response needs clarification.",
        "claim": "There is insufficient evidence of understanding.",
    }
    return {
        "status": "completed",
        "model": "example-model",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": json.dumps(decision)}],
        }],
        "usage": {"input_tokens": 20, "output_tokens": 10},
    }


class ExperimentTests(unittest.TestCase):
    def setUp(self):
        self.api = self.enterContext(patch.object(run, "api_request", autospec=True))
        self.key = self.enterContext(patch.object(run, "get_api_key", return_value="test-key"))

    def test_preview_needs_no_credentials_or_connection(self):
        code, output, errors = invoke("--case", "incorrect", "--model", "example-model")
        self.assertEqual(code, 0, errors)
        request = json.loads(output)
        self.assertEqual(request["model"], "example-model")
        observation = json.loads(request["input"])
        self.assertEqual(observation["supplied_feedback"], "incorrect")
        self.assertNotIn("expected_action", observation)
        self.assertFalse(request["store"])
        self.assertGreater(request["max_output_tokens"], 0)
        self.key.assert_not_called()
        self.api.assert_not_called()

    def test_key_check_only_reads_model_metadata(self):
        def read_model(path, api_key, payload=None):
            self.assertEqual(path, "/v1/models/example-model")
            self.assertEqual(api_key, "test-key")
            self.assertIsNone(payload)
            return {"id": "example-model", "object": "model"}

        self.api.side_effect = read_model
        code, _, errors = invoke("--check-key", "--model", "example-model")
        self.assertEqual(code, 0, errors)
        self.api.assert_called_once()

    def test_live_retains_a_disagreement_after_one_request(self):
        def respond(path, api_key, payload=None):
            self.assertEqual(path, "/v1/responses")
            self.assertEqual(api_key, "test-key")
            self.assertEqual(payload["model"], "example-model")
            return completed_response()

        self.api.side_effect = respond
        code, output, errors = invoke("--live", "--model", "example-model")
        self.assertEqual(code, 0, errors)
        self.assertIs(json.loads(output)["action_matches"], False)
        self.api.assert_called_once()

    def test_only_the_final_decision_is_scored(self):
        response = completed_response()
        response["output"][0]["phase"] = "final_answer"
        response["output"].insert(0, {
            "type": "message",
            "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text", "text": "Checking the observation."}],
        })
        self.api.return_value = response
        code, output, errors = invoke("--live", "--case", "ambiguous")
        self.assertEqual(code, 0, errors)
        report = json.loads(output)
        self.assertIs(report["action_matches"], True)
        self.assertEqual(report["usage"], response["usage"])

    def test_incomplete_and_refused_responses_do_not_become_observations(self):
        incomplete = completed_response()
        incomplete["status"] = "incomplete"
        refused = completed_response()
        refused["output"][0]["content"] = [{"type": "refusal", "refusal": "Declined."}]
        for response in [incomplete, refused]:
            with self.subTest(response=response):
                self.api.reset_mock()
                self.api.return_value = response
                code, output, errors = invoke("--live")
                self.assertNotEqual(code, 0)
                self.assertNotIn("action_matches", output)
                self.assertTrue(errors.strip())
                self.api.assert_called_once()


class CredentialTests(unittest.TestCase):
    def test_api_error_does_not_echo_provider_body_or_key(self):
        with (
            patch.object(run, "get_api_key", return_value="test-key"),
            patch("http.client.HTTPSConnection", autospec=True) as transport,
        ):
            response = transport.return_value.getresponse.return_value
            response.status = 401
            response.read.return_value = b'{"error":"Invalid key: test-key"}'
            code, output, errors = invoke("--check-key")
        self.assertNotEqual(code, 0)
        self.assertIn("401", errors)
        self.assertNotIn("test-key", output + errors)
        transport.return_value.request.assert_called_once()
        transport.return_value.close.assert_called_once()

    def test_missing_key_in_noninteractive_run(self):
        with (
            patch("os.environ", {}),
            patch("sys.stdin", io.StringIO()),
            patch.object(run, "api_request") as api,
        ):
            code, _, errors = invoke("--live")
        self.assertNotEqual(code, 0)
        self.assertIn("OPENAI_API_KEY", errors)
        api.assert_not_called()


if __name__ == "__main__":
    unittest.main()
