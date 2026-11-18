"""Loopback S1 decision-service provider tests (RFC #4268 Phase 3)."""

from __future__ import annotations

import json
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run_native import parse_args
from s1_service import S1ServiceError, choose_s1_service, s1_service_url, validate_decision

REQUEST = {
    "schema": "cua.jev_choice_request_v2",
    "goal": "Set the counter to 3.",
    "capture_id": "cap-1",
    "regions": [],
    "history": [],
    "candidates": [
        {"id": "ax:button:increment", "description": "Click Increment", "source": "ax"},
        {"id": "reobserve", "description": "Observe again"},
        {"id": "abstain", "description": "Stop"},
    ],
}


def decision(**overrides):
    value = {
        "schema": "cua.decision_choice_v1",
        "kind": "selected",
        "capture_id": "cap-1",
        "selected_id": "ax:button:increment",
        "model": "cua-s1-4b-0.2",
        "confidence": 0.9,
        "probabilities": {"ax:button:increment": 0.9, "reobserve": 0.06, "abstain": 0.04},
        "reason": None,
    }
    value.update(overrides)
    return value


class Service:
    """A one-endpoint loopback server that records requests and replays a response."""

    def __init__(self, status: int, body: bytes):
        self.received: list[dict] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                outer.received.append(json.loads(self.rfile.read(length)))
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/decide"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class S1ServiceUrlTest(unittest.TestCase):
    def test_accepts_loopback_http(self):
        for url in ("http://127.0.0.1:8791/decide", "http://localhost:8791/decide", "http://[::1]:8791/decide"):
            self.assertEqual(s1_service_url(url), url)

    def test_rejects_remote_tls_credentials_and_missing(self):
        for url in (
            "http://10.0.0.5:8791/decide",
            "http://example.com/decide",
            "https://127.0.0.1:8791/decide",
            "http://user:secret@127.0.0.1:8791/decide",
            "",
        ):
            with self.assertRaises(S1ServiceError):
                s1_service_url(url)


class ValidateDecisionTest(unittest.TestCase):
    def test_accepts_selected_reobserve_and_abstain(self):
        self.assertEqual(validate_decision(decision(), REQUEST)["selected_id"], "ax:button:increment")
        for choice in ("reobserve", "abstain"):
            probabilities = {"ax:button:increment": 0.1, "reobserve": 0.1, "abstain": 0.1, choice: 0.8}
            value = decision(kind=choice, selected_id=choice, confidence=0.8, probabilities=probabilities)
            self.assertEqual(validate_decision(value, REQUEST)["kind"], choice)

    def test_rejects_malformed_decisions(self):
        bad = [
            decision(schema="cua.decision_choice_v0"),
            decision(kind="act"),
            decision(capture_id="cap-2"),
            decision(kind="error", selected_id=None, reason="model_error"),
            decision(selected_id="ax:button:reset"),
            decision(kind="reobserve"),
            decision(probabilities={"ax:button:increment": 1.0}),
            decision(probabilities={"ax:button:increment": 1.5, "reobserve": 0, "abstain": 0}),
            decision(confidence=float("nan")),
            decision(confidence=True),
            [],
        ]
        for value in bad:
            with self.assertRaises(S1ServiceError, msg=json.dumps(value, default=str)):
                validate_decision(value, REQUEST)


class ChooseS1ServiceTest(unittest.TestCase):
    def test_posts_the_request_and_returns_the_choice(self):
        service = Service(200, json.dumps(decision()).encode())
        try:
            choice, confidence, probabilities = choose_s1_service(REQUEST, url=service.url)
        finally:
            service.close()
        self.assertEqual(service.received, [REQUEST])
        self.assertEqual(choice, "ax:button:increment")
        self.assertEqual(confidence, 0.9)
        self.assertEqual(set(probabilities), {"ax:button:increment", "reobserve", "abstain"})

    def test_http_errors_and_invalid_json_raise(self):
        for status, body in ((400, b'{"error":"invalid_request"}'), (200, b"not json")):
            service = Service(status, body)
            try:
                with self.assertRaises(S1ServiceError):
                    choose_s1_service(REQUEST, url=service.url)
            finally:
                service.close()

    def test_unreachable_service_raises(self):
        service = Service(200, b"{}")
        url = service.url
        service.close()
        with self.assertRaises(S1ServiceError):
            choose_s1_service(REQUEST, url=url, timeout=2)


class RunnerArgumentsTest(unittest.TestCase):
    def test_runner_accepts_the_s1_provider(self):
        args = parse_args(["--task", "appkit-counter", "--provider", "s1", "--pid", "1", "--state-file", "s"])
        self.assertEqual(args.provider, "s1")


if __name__ == "__main__":
    unittest.main()
