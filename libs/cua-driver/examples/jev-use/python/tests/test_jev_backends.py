from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jev_backends import (
    DEFAULT_LOCAL_JEV_URL,
    DEFAULT_TYPESAFE_BASE_URL,
    JevProtocolError,
    JevTransportError,
    SystemOneHttpClient,
    _NoRedirect,
    choose_with_backend,
    describe_backend,
    read_jev_config,
    systemone_url,
    validate_choice_answer,
    validate_criteria,
    validate_loopback_url,
    validate_remote_url,
)

CRITERIA = {
    "type-verification-value": "Replace the verification field.",
    "reobserve": "Obtain a fresh observation.",
    "abstain": "Stop without acting.",
}


def good_answers(choice="type-verification-value", confidence=0.9):
    rest = (1.0 - confidence) / 2
    return {
        "candidate": {
            "type": "choice",
            "choice": choice,
            "confidence": confidence,
            "probabilities": {
                cid: (confidence if cid == choice else rest) for cid in CRITERIA
            },
        }
    }


def stub_transport(response):
    def transport(url, payload, headers, timeout):
        return response

    return transport


class ConfigTest(unittest.TestCase):
    def test_defaults_to_mock(self) -> None:
        config = read_jev_config({})
        self.assertEqual(config.backend, "mock")
        self.assertEqual(config.model, "jev-latest")
        self.assertEqual(config.timeout_ms, 2500)

    def test_live_alias_and_unknown_backend_handling(self) -> None:
        self.assertEqual(read_jev_config({"JEV_BACKEND": "live"}).backend, "typesafe")
        with self.assertRaisesRegex(ValueError, "JEV_BACKEND"):
            read_jev_config({"JEV_BACKEND": "nope"})

    def test_typesafe_aliases(self) -> None:
        config = read_jev_config(
            {"JEV_BACKEND": "typesafe", "TYPESAFE_API_KEY": "k", "TYPESAFE_MODEL": "m"}
        )
        self.assertEqual(config.api_key, "k")
        self.assertEqual(config.model, "m")
        self.assertEqual(config.base_url, DEFAULT_TYPESAFE_BASE_URL)

    def test_local_defaults_to_loopback_without_key(self) -> None:
        config = read_jev_config({"JEV_BACKEND": "local"})
        self.assertEqual(config.base_url, DEFAULT_LOCAL_JEV_URL)
        self.assertEqual(config.api_key, "")

    def test_bad_timeout_falls_back(self) -> None:
        config = read_jev_config({"JEV_TIMEOUT_MS": "soon"})
        self.assertEqual(config.timeout_ms, 2500)


class UrlTest(unittest.TestCase):
    def test_systemone_url(self) -> None:
        self.assertEqual(
            systemone_url("http://127.0.0.1:8787"), "http://127.0.0.1:8787/v1/systemone"
        )
        self.assertEqual(
            systemone_url("https://api.typesafe.ai/v1/"),
            "https://api.typesafe.ai/v1/systemone",
        )

    def test_loopback_validation(self) -> None:
        self.assertEqual(
            validate_loopback_url("http://127.0.0.1:8787/"), "http://127.0.0.1:8787"
        )
        for bad in (
            "https://127.0.0.1:8787",
            "http://example.com:8787",
            "http://user:pass@127.0.0.1:8787",
        ):
            with self.assertRaises(ValueError):
                validate_loopback_url(bad)

    def test_remote_url_never_sends_credentials_over_plaintext(self) -> None:
        self.assertEqual(
            validate_remote_url("https://jev.example/v1", has_api_key=True),
            "https://jev.example/v1",
        )
        self.assertEqual(
            validate_remote_url("http://jev.example/v1", has_api_key=False),
            "http://jev.example/v1",
        )
        with self.assertRaisesRegex(ValueError, "https"):
            validate_remote_url("http://jev.example/v1", has_api_key=True)
        with self.assertRaisesRegex(ValueError, "embed credentials"):
            validate_remote_url("https://user:pass@jev.example/v1", has_api_key=False)


class ChoiceValidationTest(unittest.TestCase):
    def test_happy_path(self) -> None:
        answer = validate_choice_answer(
            "candidate", good_answers()["candidate"], set(CRITERIA)
        )
        self.assertEqual(answer.choice, "type-verification-value")

    def test_rejects_unknown_choice(self) -> None:
        answers = good_answers()
        answers["candidate"]["choice"] = "nope"
        answers["candidate"]["probabilities"]["nope"] = answers["candidate"][
            "probabilities"
        ].pop("type-verification-value")
        with self.assertRaises(JevProtocolError):
            validate_choice_answer("candidate", answers["candidate"], set(CRITERIA))

    def test_rejects_bad_mass(self) -> None:
        answers = good_answers()
        answers["candidate"]["probabilities"]["abstain"] = 0.9
        with self.assertRaises(JevProtocolError):
            validate_choice_answer("candidate", answers["candidate"], set(CRITERIA))

    def test_rejects_argmax_mismatch(self) -> None:
        answers = good_answers(choice="reobserve", confidence=0.34)
        probs = answers["candidate"]["probabilities"]
        probs["type-verification-value"] = 0.5
        probs["abstain"] = 0.16  # Keep unit mass; only the selected winner is wrong.
        self.assertAlmostEqual(sum(probs.values()), 1.0)
        with self.assertRaisesRegex(JevProtocolError, "argmax"):
            validate_choice_answer("candidate", answers["candidate"], set(CRITERIA))

        # The same distribution is valid once the selected winner is corrected.
        answers["candidate"]["choice"] = "type-verification-value"
        answers["candidate"]["confidence"] = 0.5
        self.assertEqual(
            validate_choice_answer("candidate", answers["candidate"], set(CRITERIA)).choice,
            "type-verification-value",
        )

    def test_rejects_non_finite(self) -> None:
        answers = good_answers()
        answers["candidate"]["confidence"] = float("inf")
        with self.assertRaises(JevProtocolError):
            validate_choice_answer("candidate", answers["candidate"], set(CRITERIA))

    def test_validate_criteria(self) -> None:
        with self.assertRaises(ValueError):
            validate_criteria({})
        with self.assertRaises(ValueError):
            validate_criteria({"a": ""})


class ChooseWithBackendTest(unittest.TestCase):
    def test_mock_is_deterministic(self) -> None:
        config = read_jev_config({})
        outcome = choose_with_backend(
            config, goal="g", observation={}, criteria=CRITERIA
        )
        self.assertTrue(outcome.ok)
        assert outcome.decision is not None
        self.assertEqual(outcome.decision.selected_id, "type-verification-value")
        self.assertAlmostEqual(sum(outcome.decision.probabilities.values()), 1.0)

    def test_typesafe_without_key_skips(self) -> None:
        config = read_jev_config({"JEV_BACKEND": "typesafe"})
        outcome = choose_with_backend(
            config, goal="g", observation={}, criteria=CRITERIA
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "missing_credentials")

    def test_typesafe_key_is_not_sent_to_plaintext_override(self) -> None:
        config = read_jev_config(
            {
                "JEV_BACKEND": "typesafe",
                "JEV_API_KEY": "secret",
                "JEV_BASE_URL": "http://jev.example",
            }
        )

        def must_not_send(*_args):
            raise AssertionError("transport must not run")

        outcome = choose_with_backend(
            config,
            goal="g",
            observation={},
            criteria=CRITERIA,
            transport=must_not_send,
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "validation_error")
        self.assertIn("https", outcome.message or "")

    def test_openjev_without_url_skips(self) -> None:
        config = read_jev_config({"JEV_BACKEND": "openjev"})
        outcome = choose_with_backend(
            config, goal="g", observation={}, criteria=CRITERIA
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "missing_base_url")

    def test_local_rejects_non_loopback(self) -> None:
        config = read_jev_config(
            {"JEV_BACKEND": "local", "JEV_BASE_URL": "http://example.com:8787"}
        )
        outcome = choose_with_backend(
            config, goal="g", observation={}, criteria=CRITERIA
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "validation_error")

    def test_local_happy_path(self) -> None:
        config = read_jev_config({"JEV_BACKEND": "local"})
        response = {"model": "local-jev", "answers": good_answers()}
        outcome = choose_with_backend(
            config,
            goal="g",
            observation={},
            criteria=CRITERIA,
            transport=stub_transport(response),
        )
        self.assertTrue(outcome.ok)
        assert outcome.decision is not None
        self.assertEqual(outcome.decision.selected_id, "type-verification-value")
        self.assertEqual(outcome.decision.model, "local-jev")
        self.assertEqual(outcome.decision.backend, "local")

    def test_timeout_maps_to_skip_reason(self) -> None:
        def transport(url, payload, headers, timeout):
            raise JevTransportError("request timed out: timed out")

        config = read_jev_config({"JEV_BACKEND": "local"})
        outcome = choose_with_backend(
            config, goal="g", observation={}, criteria=CRITERIA, transport=transport
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "timeout")

    def test_http_error_maps_to_skip_reason(self) -> None:
        def transport(url, payload, headers, timeout):
            raise JevTransportError("HTTP 500: boom")

        config = read_jev_config({"JEV_BACKEND": "local"})
        outcome = choose_with_backend(
            config, goal="g", observation={}, criteria=CRITERIA, transport=transport
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "http_error")

    def test_malformed_output_is_invalid_response_not_trusted(self) -> None:
        answers = good_answers()
        answers["candidate"]["probabilities"]["abstain"] = 0.9  # mass 1.81
        config = read_jev_config({"JEV_BACKEND": "local"})
        outcome = choose_with_backend(
            config,
            goal="g",
            observation={},
            criteria=CRITERIA,
            transport=stub_transport({"answers": answers}),
        )
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, "invalid_response")

    def test_describe_backend_redacts_key(self) -> None:
        config = read_jev_config(
            {"JEV_BACKEND": "typesafe", "JEV_API_KEY": "secret-key"}
        )
        described = describe_backend(config)
        self.assertTrue(described["has_api_key"])
        self.assertNotIn("secret-key", str(described))


class HttpClientTest(unittest.TestCase):
    def test_redirect_handler_refuses_redirects(self) -> None:
        handler = _NoRedirect()
        request = __import__("urllib.request", fromlist=["Request"]).Request(
            "https://jev.example/v1/systemone"
        )
        self.assertIsNone(
            handler.redirect_request(
                request,
                None,
                302,
                "Found",
                {},
                "https://other.example/v1/systemone",
            )
        )

    def test_missing_answers_is_protocol_error(self) -> None:
        config = read_jev_config({"JEV_BACKEND": "local"})
        client = SystemOneHttpClient(config, transport=stub_transport({"model": "x"}))
        with self.assertRaises(JevProtocolError):
            client.ask(state={}, questions={})


if __name__ == "__main__":
    unittest.main()
