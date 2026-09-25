"""Exercise the real HTTP client against an owned loopback fixture, not a model."""
from __future__ import annotations

import json
import sys
import threading
import unittest
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jev_backends import choose_with_backend, read_jev_config

CRITERIA = {"type-value": "Enter the value.", "reobserve": "Observe again."}


def answer(choice="type-value"):
    return {
        "model": "loopback-fixture",
        "answers": {
            "candidate": {
                "type": "choice",
                "choice": choice,
                "confidence": 0.8 if choice == "type-value" else 0.2,
                "probabilities": {"type-value": 0.8, "reobserve": 0.2},
            }
        },
    }


@contextmanager
def server_response(payload, *, status=200, stall=False):
    """Retain each actual POST; stall only after receipt, with bounded cleanup."""
    received = []
    entered = threading.Event()
    release = threading.Event()
    body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            self.connection.settimeout(2)
            received.append({
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
                "content_type": self.headers.get("Content-Type"),
                "payload": json.loads(self.rfile.read(int(self.headers["Content-Length"]))),
            })
            entered.set()
            if stall:
                # Never send a response for this case. The client must time out,
                # then the owning test releases the handler in finally.
                release.wait(2)
                self.close_connection = True
                return
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.01))
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", received, entered
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        worker.join(timeout=3)
        if worker.is_alive():
            raise RuntimeError("loopback fixture worker did not exit")


def call(url, *, timeout_ms=1000):
    # Supplying a dictionary avoids inheriting any real provider credentials.
    config = read_jev_config({
        "JEV_BACKEND": "local", "JEV_BASE_URL": url,
        "JEV_MODEL": "fixture-model", "JEV_TIMEOUT_MS": str(timeout_ms),
    })
    return choose_with_backend(
        config, goal="Enter a value.", observation={"label": "café 🙂"}, criteria=CRITERIA
    )


class HttpRoundTripTest(unittest.TestCase):
    def test_success_uses_real_post_and_preserves_unicode_and_candidate_ids(self):
        with server_response(answer()) as (url, received, _):
            outcome = call(url)
            self.assertTrue(outcome.ok)
            self.assertEqual(outcome.decision.selected_id, "type-value")
            self.assertEqual(outcome.decision.model, "loopback-fixture")
            self.assertEqual(len(received), 1)
            self.assertEqual(received[0]["path"], "/v1/systemone")
            self.assertIsNone(received[0]["authorization"])
            self.assertEqual(received[0]["content_type"], "application/json")
            self.assertEqual(received[0]["payload"], {
                "state": {"goal": "Enter a value.", "observation": {"label": "café 🙂"}},
                "model": "fixture-model",
                "questions": {"candidate": {
                    "type": "choice", "instructions": "Select exactly one supplied candidate ID.",
                    "criteria": CRITERIA,
                }},
            })

    def test_http_failure_returns_skip_without_reposting(self):
        with server_response({"error": "fixture unavailable"}, status=503) as (url, received, _):
            outcome = call(url)
            self.assertFalse(outcome.ok)
            self.assertEqual(outcome.reason, "http_error")
            self.assertIsNone(outcome.decision)
            self.assertEqual(len(received), 1)

    def test_non_json_body_returns_invalid_response(self):
        with server_response(b"not JSON") as (url, received, _):
            outcome = call(url)
            self.assertFalse(outcome.ok)
            self.assertEqual(outcome.reason, "invalid_response")
            self.assertIsNone(outcome.decision)
            self.assertEqual(len(received), 1)

    def test_missing_answers_returns_invalid_response(self):
        with server_response({"model": "fixture"}) as (url, received, _):
            outcome = call(url)
            self.assertFalse(outcome.ok)
            self.assertEqual(outcome.reason, "invalid_response")
            self.assertIsNone(outcome.decision)
            self.assertEqual(len(received), 1)

    def test_valid_probability_mass_with_wrong_winner_is_rejected(self):
        with server_response(answer(choice="reobserve")) as (url, received, _):
            outcome = call(url)
            self.assertFalse(outcome.ok)
            self.assertEqual(outcome.reason, "invalid_response")
            self.assertIn("argmax", outcome.message)
            self.assertIsNone(outcome.decision)
            self.assertEqual(len(received), 1)

    def test_response_timeout_returns_skip_without_reposting(self):
        with server_response(answer(), stall=True) as (url, received, entered):
            outcome = call(url, timeout_ms=200)
            self.assertTrue(entered.is_set(), "timeout must follow actual fixture receipt")
            self.assertFalse(outcome.ok)
            self.assertEqual(outcome.reason, "timeout")
            self.assertIsNone(outcome.decision)
            self.assertEqual(len(received), 1)


if __name__ == "__main__":
    unittest.main()
