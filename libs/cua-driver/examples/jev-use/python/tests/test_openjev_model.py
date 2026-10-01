from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))

from choose_action import REQUEST_SCHEMA_V2, validate_request
from decision_models import DecisionRequest, choose
from openjev_model import (
    OpenJevConfig,
    OpenJevDecisionModel,
    OpenJevError,
    decision_request_wire,
    read_openjev_config,
    systemone_url,
    validate_openjev_base_url,
)


def request() -> DecisionRequest:
    raw = validate_request(
        {
            "schema": REQUEST_SCHEMA_V2,
            "goal": "Submit the completed form.",
            "capture_id": "cap-1",
            "snapshot_id": "snap-1",
            "regions": [],
            "elements": [
                {"role_class": "button", "label": "Submit", "state": "enabled"}
            ],
            "progress": [{"step": "fill field", "done": 1, "required": 1}],
            "history": [{"selected_id": "type-value"}],
            "candidates": [
                {"id": "submit", "description": "Submit.", "source": "ax"},
                {"id": "reobserve", "description": "Observe again."},
                {"id": "abstain", "description": "Stop."},
            ],
        }
    )
    return DecisionRequest.from_validated(raw)


def good_response() -> dict:
    return {
        "model": "openjev-fixture",
        "answers": {
            "candidate": {
                "type": "choice",
                "choice": "submit",
                "confidence": 0.9,
                "probabilities": {
                    "submit": 0.9,
                    "reobserve": 0.05,
                    "abstain": 0.05,
                },
            }
        },
    }


class ConfigTest(unittest.TestCase):
    def test_config_is_explicit_and_bounded(self) -> None:
        config = read_openjev_config(
            {
                "OPENJEV_BASE_URL": "https://jev.example/v1/",
                "OPENJEV_MODEL": "custom",
                "OPENJEV_TIMEOUT_MS": "999999",
            }
        )
        self.assertEqual(config.base_url, "https://jev.example/v1/")
        self.assertEqual(config.model, "custom")
        self.assertEqual(config.timeout_ms, 60_000)

    def test_api_key_requires_https_and_url_credentials_are_rejected(self) -> None:
        with self.assertRaisesRegex(OpenJevError, "https"):
            validate_openjev_base_url("http://jev.example", has_api_key=True)
        with self.assertRaisesRegex(OpenJevError, "embed credentials"):
            validate_openjev_base_url(
                "https://user:pass@jev.example",
                has_api_key=False,
            )
        for suffix in ("?tenant=a", "#fragment"):
            with self.subTest(suffix=suffix):
                with self.assertRaisesRegex(OpenJevError, "query or fragment"):
                    validate_openjev_base_url(
                        "https://jev.example" + suffix,
                        has_api_key=False,
                    )

    def test_systemone_endpoint_is_canonical(self) -> None:
        self.assertEqual(
            systemone_url("https://jev.example/v1"),
            "https://jev.example/v1/systemone",
        )
        self.assertEqual(
            systemone_url("https://jev.example"),
            "https://jev.example/v1/systemone",
        )
        self.assertEqual(
            systemone_url("https://jev.example/v1/systemone"),
            "https://jev.example/v1/systemone",
        )


class ModelTest(unittest.TestCase):
    def test_v2_request_is_sent_as_bounded_state(self) -> None:
        seen: dict = {}

        def transport(url, body, headers, timeout):
            seen.update(
                {
                    "url": url,
                    "payload": json.loads(body),
                    "headers": dict(headers),
                    "timeout": timeout,
                }
            )
            return good_response()

        model = OpenJevDecisionModel(
            OpenJevConfig(
                "https://jev.example",
                "secret",
                "openjev-test",
                1_250,
            ),
            transport=transport,
        )
        result = choose(model, request())
        self.assertEqual(result.kind, "selected")
        self.assertEqual(result.selected_id, "submit")
        self.assertEqual(result.model, "openjev-fixture")
        self.assertEqual(seen["url"], "https://jev.example/v1/systemone")
        self.assertEqual(seen["headers"]["Authorization"], "Bearer secret")
        self.assertEqual(seen["timeout"], 1.25)
        sent = seen["payload"]["state"]
        self.assertEqual(sent["schema"], REQUEST_SCHEMA_V2)
        self.assertEqual(sent["snapshot_id"], "snap-1")
        self.assertEqual(sent["elements"][0]["label"], "Submit")
        self.assertEqual(sent["progress"][0]["done"], 1)
        self.assertEqual(
            seen["payload"]["questions"]["candidate"]["criteria"],
            request().criteria,
        )

    def test_central_decision_validation_rejects_unknown_ids(self) -> None:
        bad = good_response()
        bad["answers"]["candidate"]["choice"] = "not-supplied"
        bad["answers"]["candidate"]["probabilities"] = {
            "not-supplied": 0.9,
            "reobserve": 0.05,
            "abstain": 0.05,
        }
        model = OpenJevDecisionModel(
            OpenJevConfig("https://jev.example"),
            transport=lambda *_args: bad,
        )
        result = choose(model, request())
        self.assertEqual(result.kind, "error")
        self.assertEqual(result.reason, "model_error")

    def test_central_decision_validation_rejects_bad_mass_and_argmax(self) -> None:
        bad = good_response()
        bad["answers"]["candidate"]["choice"] = "reobserve"
        bad["answers"]["candidate"]["confidence"] = 0.4
        bad["answers"]["candidate"]["probabilities"] = {
            "submit": 0.5,
            "reobserve": 0.4,
            "abstain": 0.1,
        }
        model = OpenJevDecisionModel(
            OpenJevConfig("https://jev.example"),
            transport=lambda *_args: bad,
        )
        result = choose(model, request())
        self.assertEqual(result.kind, "error")

    def test_missing_answers_is_bounded_error(self) -> None:
        model = OpenJevDecisionModel(
            OpenJevConfig("https://jev.example"),
            transport=lambda *_args: {"model": "x"},
        )
        with self.assertRaises(OpenJevError) as caught:
            model.score(request())
        self.assertEqual(caught.exception.code, "invalid_response")

    def test_cli_openjev_missing_config_fails_before_provider_request(self) -> None:
        env = os.environ.copy()
        for name in (
            "OPENJEV_BASE_URL",
            "OPENJEV_API_KEY",
            "OPENJEV_MODEL",
            "OPENJEV_TIMEOUT_MS",
        ):
            env.pop(name, None)
        result = subprocess.run(
            [
                sys.executable,
                str(BASE / "python/choose_decision.py"),
                "--model",
                "openjev",
            ],
            input=json.dumps(decision_request_wire(request())),
            text=True,
            capture_output=True,
            check=False,
            env=env,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("OpenJev setup failed", result.stderr)

    def test_wire_reconstruction_keeps_candidates_and_progress(self) -> None:
        wire = decision_request_wire(request())
        self.assertEqual(wire["schema"], REQUEST_SCHEMA_V2)
        self.assertEqual([item["id"] for item in wire["candidates"]], [
            "submit",
            "reobserve",
            "abstain",
        ])
        self.assertEqual(wire["progress"][0]["required"], 1)



class _Loopback:
    """Records one POST and replies without following the client anywhere."""

    def __init__(self, mode: str, body: bytes = b"", status: int = 200) -> None:
        self.mode = mode
        self.body = body
        self.status = status
        self.hits: list[str] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args) -> None:
                return

            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length") or 0)
                self.rfile.read(length)
                outer.hits.append(self.path)
                if outer.mode == "redirect":
                    host, port = self.server.server_address[:2]
                    self.send_response(302)
                    self.send_header("Location", f"http://{host}:{port}/stolen")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                payload = outer.body
                self.send_response(outer.status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def base_url(self) -> str:
        host, port = self.server.server_address[:2]
        return f"http://{host}:{port}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


class LoopbackHttpTest(unittest.TestCase):
    def _model(self, server: _Loopback) -> OpenJevDecisionModel:
        return OpenJevDecisionModel(
            OpenJevConfig(server.base_url, timeout_ms=2_000)
        )

    def test_valid_loopback_response_selects_supplied_id(self) -> None:
        server = _Loopback("ok", json.dumps(good_response()).encode())
        try:
            result = choose(self._model(server), request())
        finally:
            server.close()
        self.assertEqual(result.kind, "selected")
        self.assertEqual(result.selected_id, "submit")
        self.assertEqual(server.hits, ["/v1/systemone"])

    def test_unknown_id_is_one_request_and_no_action(self) -> None:
        bad = good_response()
        bad["answers"]["candidate"]["choice"] = "not-supplied"
        bad["answers"]["candidate"]["probabilities"] = {
            "not-supplied": 0.9,
            "reobserve": 0.05,
            "abstain": 0.05,
        }
        server = _Loopback("ok", json.dumps(bad).encode())
        try:
            result = choose(self._model(server), request())
        finally:
            server.close()
        self.assertEqual(result.kind, "error")
        self.assertEqual(result.reason, "model_error")
        self.assertEqual(server.hits, ["/v1/systemone"])

    def test_http_503_is_one_request_and_no_retry(self) -> None:
        server = _Loopback("error", b"unavailable", status=503)
        try:
            with self.assertRaises(OpenJevError) as caught:
                self._model(server).score(request())
        finally:
            server.close()
        self.assertEqual(caught.exception.code, "http_error")
        self.assertEqual(server.hits, ["/v1/systemone"])

    def test_malformed_body_is_invalid_response(self) -> None:
        server = _Loopback("ok", b"not-json")
        try:
            with self.assertRaises(OpenJevError) as caught:
                self._model(server).score(request())
        finally:
            server.close()
        self.assertEqual(caught.exception.code, "invalid_response")
        self.assertEqual(server.hits, ["/v1/systemone"])

    def test_redirect_is_refused_and_not_followed(self) -> None:
        server = _Loopback("redirect")
        try:
            with self.assertRaises(OpenJevError) as caught:
                self._model(server).score(request())
        finally:
            server.close()
        self.assertEqual(caught.exception.code, "http_error")
        self.assertNotIn("/stolen", server.hits)
        self.assertEqual(server.hits, ["/v1/systemone"])

    def test_oversized_response_is_refused(self) -> None:
        server = _Loopback("ok", b"x" * (256 * 1024 + 1))
        try:
            with self.assertRaises(OpenJevError) as caught:
                self._model(server).score(request())
        finally:
            server.close()
        self.assertEqual(caught.exception.code, "response_too_large")
        self.assertEqual(server.hits, ["/v1/systemone"])

if __name__ == "__main__":
    unittest.main()