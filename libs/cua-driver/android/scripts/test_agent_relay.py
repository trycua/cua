#!/usr/bin/env python3
"""Focused stdlib tests; no model calls or Android device required."""
import base64
import http.client
import importlib.util
import json
import pathlib
import signal
import socket
import struct
import tempfile
import threading
import unittest
from unittest import mock
import zlib


spec = importlib.util.spec_from_file_location("agent_relay", pathlib.Path(__file__).with_name("agent-relay.py"))
relay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(relay)


def png(width=2, height=2):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    pixels = b"\x00" * (height * (1 + width * 3))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b"")


def request():
    return {"request_id": "request-1", "task": "Complete the visible task", "allowed_apps": ["ai.cua.fixture"],
            "current_package": "ai.cua.fixture", "width": 2, "height": 2,
            "image_base64": base64.b64encode(png()).decode(), "history": []}


def stream(action=None, init=None):
    action = action or {"type": "tap", "x": 1, "y": 0, "reason": "Visible control"}
    text = json.dumps(action)
    return "\n".join(json.dumps(event) for event in [
        init or {"type": "system", "subtype": "init", "tools": [], "mcp_servers": [], "model": "test-model"},
        {"type": "stream_event", "event": {"type": "content_block_delta"}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}},
        {"type": "result", "subtype": "success", "is_error": False, "result": text, "duration_ms": 42},
    ])


def structured_events(action=None):
    value = {"action": action or {"type": "tap", "x": 1, "y": 0, "reason": "Visible control"}}
    return [
        {"type": "system", "subtype": "init", "tools": ["StructuredOutput"], "mcp_servers": [], "model": "test-model"},
        {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "format-1", "name": "StructuredOutput", "input": value}]}},
        {"type": "result", "subtype": "success", "is_error": False,
         "structured_output": value, "result": json.dumps(value), "duration_ms": 42},
    ]


class ValidationTests(unittest.TestCase):
    def test_request_and_png(self):
        self.assertEqual(relay.validate_request(request()), png())

    def test_invalid_request_fields(self):
        cases = [dict(width=True), dict(width=4097), dict(height=0), dict(request_id="../../escape"),
                 dict(task="x" * 8193), dict(allowed_apps=[]), dict(allowed_apps=["invalid"]),
                 dict(allowed_apps=["ai.cua.fixture"] * 2), dict(current_package="a; command"),
                 dict(image_base64="not base64!"), dict(history=[{}] * 65), dict(history={}),
                 dict(history=[float("inf")]), dict(history=["x" * 8192] * 9),
                 dict(history=[{"data": "x" * 8193}]), dict(history=[{"x": [0] * 65}]),
                 dict(history=[{str(i): 1 for i in range(33)}]), dict(extra="injected prompt")]
        nested = []
        for _ in range(10):
            nested = [nested]
        cases.append(dict(history=nested))
        for changes in cases:
            with self.subTest(changes=list(changes)):
                value = request()
                value.update(changes)
                with self.assertRaises(relay.Invalid):
                    relay.validate_request(value)
        missing = request()
        del missing["task"]
        with self.assertRaises(relay.Invalid):
            relay.validate_request(missing)

    def test_png_integrity_and_dimensions(self):
        values = [b"not PNG", png()[:-5], png() + b"trailing", png(3, 2)]
        corrupt = bytearray(png())
        corrupt[20] ^= 1
        values.append(bytes(corrupt))
        for data in values:
            with self.subTest(size=len(data)), self.assertRaises(relay.Invalid):
                value = request()
                value["image_base64"] = base64.b64encode(data).decode()
                relay.validate_request(value)

    def test_json_duplicate_keys_and_nonfinite(self):
        for raw in ['{"x":1,"x":2}', '{"x":NaN}', '{"x":Infinity}', b"\xff", "[" * 2000]:
            with self.subTest(raw=raw[:20]), self.assertRaises(relay.Invalid):
                relay.decode_json(raw)

    def test_actions(self):
        valid = [{"type": "launch", "package": "ai.cua.fixture", "reason": "Open allowed app"},
                 {"type": "tap", "x": 1, "y": 0, "reason": "Visible control"},
                 {"type": "swipe", "from_x": 0, "from_y": 0, "to_x": 1, "to_y": 1,
                  "duration_ms": 300, "reason": "Scroll"}, {"type": "done", "reason": "Task visibly complete"},
                 {"type": "blocked", "reason": "Cannot progress from current screen"}]
        for action in valid:
            self.assertEqual(relay.validate_action(action, request()), action)
        invalid = [dict(valid[0], package="ai.other.app"), dict(valid[1], x=2), dict(valid[1], y=-1),
                   dict(valid[1], x=True), dict(valid[1], x=1.0), dict(valid[2], duration_ms=0),
                   dict(valid[2], duration_ms=1001), dict(valid[3], reason=""), dict(valid[3], extra="x"),
                   {"type": "shell", "reason": "Run command"}, {"type": [], "reason": "Invalid"}]
        for action in invalid:
            with self.subTest(action=action), self.assertRaises(relay.Invalid):
                relay.validate_action(action, request())

    def test_evidence_must_be_outside_repository(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / ".git").mkdir()
            with self.assertRaises(relay.Invalid):
                relay.prepare_evidence(root / "evidence")


class ModelTests(unittest.TestCase):
    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_subprocess_cancellation_kills_owned_group(self, popen, killpg):
        process = mock.Mock(pid=12345)
        process.poll.return_value = None
        popen.return_value = process
        cancelled = mock.Mock(side_effect=[False, True])
        with self.assertRaises(relay.InferenceCancelled):
            relay.infer_once(request(), "/test/claude", cancelled=cancelled)
        killpg.assert_called_once_with(12345, signal.SIGKILL)
        process.wait.assert_called_once_with(timeout=5)

    @mock.patch.object(relay.subprocess, "Popen")
    def test_cancelled_request_does_not_start_subprocess(self, popen):
        with self.assertRaises(relay.InferenceCancelled):
            relay.infer(request(), "/test/claude", cancelled=lambda: True)
        popen.assert_not_called()

    def parse_events(self, events):
        return relay.parse_model_output("\n".join(map(json.dumps, events)), request())

    def test_structured_result(self):
        action, metadata = self.parse_events(structured_events())
        self.assertEqual(action, {"type": "tap", "x": 1, "y": 0, "reason": "Visible control"})
        self.assertEqual(metadata["tools"], ["StructuredOutput"])
        self.assertEqual(metadata["model"], "test-model")
        events = structured_events()
        del events[-1]["structured_output"]
        self.assertEqual(self.parse_events(events)[0], action)

    def test_structured_output_rejects_host_tools_and_duplicate_formatter(self):
        for tools in (["Bash"], ["StructuredOutput", "Read"], ["StructuredOutput", "StructuredOutput"], []):
            events = structured_events()
            events[0]["tools"] = tools
            with self.subTest(tools=tools), self.assertRaises(relay.InferenceError):
                self.parse_events(events)
        for name in ("Bash", "Read", "mcp__host__execute", None):
            events = structured_events()
            events[1]["message"]["content"][0]["name"] = name
            with self.subTest(name=name), self.assertRaises(relay.InferenceError):
                self.parse_events(events)
        events = structured_events()
        events.insert(2, events[1])
        with self.assertRaises(relay.InferenceError):
            self.parse_events(events)

    def test_structured_output_requires_matching_valid_payloads(self):
        events = structured_events()
        events[-1]["structured_output"] = {"action": {"type": "done", "reason": "Different decision"}}
        with self.assertRaises(relay.Invalid) as caught:
            self.parse_events(events)
        self.assertIn("Different decision", caught.exception.model_response)
        for action in ({"type": "tap", "x": 1, "y": 0, "reason": "Extra field", "duration_ms": 100},
                       {"type": "tap", "x": 2, "y": 0, "reason": "Outside bounds"},
                       {"type": "launch", "package": "ai.other.app", "reason": "Not allowed"}):
            with self.subTest(action=action), self.assertRaises(relay.Invalid) as caught:
                self.parse_events(structured_events(action))
            self.assertIn(action["reason"], caught.exception.model_response)
        for change in ("missing_invocation", "missing_result", "bad_envelope", "unverified_result"):
            events = structured_events()
            if change == "missing_invocation":
                del events[1]
            elif change == "missing_result":
                del events[-1]["structured_output"]
                del events[-1]["result"]
            elif change == "bad_envelope":
                events[-1]["structured_output"] = {"action": events[-1]["structured_output"]["action"], "extra": 1}
            else:
                events[0]["tools"] = []
                del events[1]
            with self.subTest(change=change), self.assertRaises(relay.Invalid):
                self.parse_events(events)

    def test_action_schema_has_exact_variants_and_request_bounds(self):
        schema = relay.action_schema(dict(request(), width=73, height=91, allowed_apps=["ai.cua.other"]))
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(schema["required"], ["action"])
        variants = schema["properties"]["action"]["anyOf"]
        self.assertEqual(len(variants), 5)
        variants = {variant["properties"]["type"]["const"]: variant for variant in variants}
        self.assertEqual(set(variants), {"launch", "tap", "swipe", "done", "blocked"})
        for variant in variants.values():
            self.assertFalse(variant["additionalProperties"])
            self.assertEqual(set(variant["required"]), set(variant["properties"]))
        self.assertEqual(set(variants["tap"]["properties"]), {"type", "reason", "x", "y"})
        self.assertEqual(variants["tap"]["properties"]["x"], {"type": "integer", "minimum": 0, "maximum": 72})
        self.assertEqual(variants["swipe"]["properties"]["to_y"]["maximum"], 90)
        self.assertEqual(variants["swipe"]["properties"]["duration_ms"], {"type": "integer", "minimum": 1, "maximum": 1000})
        self.assertEqual(variants["launch"]["properties"]["package"]["enum"], ["ai.cua.other"])

    def test_completed_stream(self):
        action, metadata = relay.parse_model_output(stream(), request())
        self.assertEqual(action["type"], "tap")
        self.assertEqual(metadata, {"model": "test-model", "tools": [], "duration_ms": 42})

    def test_assistant_fallback(self):
        events = [json.loads(line) for line in stream().splitlines()]
        del events[-1]["result"]
        action, _ = relay.parse_model_output("\n".join(map(json.dumps, events)), request())
        self.assertEqual(action["x"], 1)

    def test_reject_errors_tools_and_bad_results(self):
        invalid = ["", "not json", "[]", '\n'.join(stream().splitlines()[:-1]),
                   '\n'.join(stream().splitlines()[1:]),
                   stream().splitlines()[0] + '\n' + stream(),
                   stream(init={"type": "system", "subtype": "init", "tools": ["Bash"], "model": "model"}),
                   stream(init={"type": "system", "subtype": "init", "tools": [], "mcp_servers": [{}], "model": "model"}),
                   stream() + '\n{"type":"result","subtype":"success","result":"{}"}',
                   stream() + '\n{"type":"error","error":"failure"}',
                   stream().replace('"is_error": false', '"is_error": true'),
                   stream().replace('"subtype": "success"', '"subtype": "error_max_turns"'),
                   stream().replace('"result": "{', '"result": "```{'),
                   stream().replace('"type": "text"', '"type": "tool_use"')]
        for value in invalid:
            with self.subTest(value=value[:80]), self.assertRaises((relay.Invalid, relay.InferenceError)):
                relay.parse_model_output(value, request())

    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_subprocess_input_flags_and_empty_cwd(self, popen, killpg):
        process = mock.Mock(pid=12345, returncode=0)
        process.poll.return_value = 0

        def start(argv, **kwargs):
            self.assertEqual(list(pathlib.Path(kwargs["cwd"]).iterdir()), [])
            self.assertTrue(kwargs["start_new_session"])
            self.assertIn("--print", argv)
            self.assertIn("--safe-mode", argv)
            self.assertIn("--strict-mcp-config", argv)
            self.assertIn("--no-session-persistence", argv)
            self.assertNotIn("--bare", argv)
            for flag, expected in [("--tools", ""), ("--mcp-config", '{"mcpServers":{}}'),
                                   ("--model", "sonnet"), ("--effort", "medium"),
                                   ("--permission-mode", "plan"), ("--input-format", "stream-json"),
                                   ("--output-format", "stream-json")]:
                self.assertEqual(argv[argv.index(flag) + 1], expected)
            message = json.loads(kwargs["stdin"].read())
            self.assertEqual(message["type"], "user")
            self.assertEqual(message["message"]["content"][1]["source"]["data"], request()["image_base64"])
            self.assertNotIn(request()["task"], " ".join(argv))
            self.assertEqual(json.loads(argv[argv.index("--json-schema") + 1]), relay.action_schema(request()))
            kwargs["stdout"].write("\n".join(map(json.dumps, structured_events())).encode())
            return process

        popen.side_effect = start
        action, _ = relay.infer(request(), "/test/claude")
        self.assertEqual(action["type"], "tap")
        killpg.assert_called_once_with(12345, signal.SIGKILL)
        process.wait.assert_called_once_with(timeout=5)

    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_subprocess_timeout_kills_own_group(self, popen, killpg):
        process = mock.Mock(pid=12345)
        process.poll.return_value = None
        popen.return_value = process
        with self.assertRaises(TimeoutError):
            relay.infer_once(request(), "/test/claude", timeout=0)
        killpg.assert_called_once_with(12345, signal.SIGKILL)
        process.wait.assert_called_once_with(timeout=5)

    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_subprocess_failure(self, popen, killpg):
        process = mock.Mock(pid=12345, returncode=1)
        process.poll.return_value = 1
        popen.return_value = process
        with self.assertRaises(relay.InferenceError):
            relay.infer(request(), "/test/claude")

    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_subprocess_output_limit(self, popen, killpg):
        process = mock.Mock(pid=12345, returncode=0)
        process.poll.return_value = None

        def start(argv, **kwargs):
            kwargs["stderr"].write(b"x" * (relay.MAX_OUTPUT + 1))
            kwargs["stderr"].flush()
            return process

        popen.side_effect = start
        with self.assertRaises(relay.InferenceError):
            relay.infer(request(), "/test/claude")
        killpg.assert_called_once_with(12345, signal.SIGKILL)


class RecoveryTests(unittest.TestCase):
    def malformed_events(self):
        events = structured_events()
        events[1]["message"]["content"][0]["input"]["reason"] = "Redundant outer explanation"
        return events

    def malformed_error(self):
        try:
            relay.parse_model_output("\n".join(map(json.dumps, self.malformed_events())), request())
        except relay.MalformedResponse as error:
            return error
        self.fail("Expected recoverable formatting error")

    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_two_subprocesses_preserve_action_evidence_and_cleanup(self, popen, killpg):
        processes = []

        def start(argv, **kwargs):
            index = len(processes)
            process = mock.Mock(pid=12345 + index, returncode=0)
            process.poll.return_value = 0
            if index:
                killpg.assert_called_once_with(12345, signal.SIGKILL)
                processes[0].wait.assert_called_once_with(timeout=5)
                self.assertIn("Formatting correction", argv[argv.index("--system-prompt") + 1])
                content = json.loads(kwargs["stdin"].read())["message"]["content"]
                self.assertIn("Previously rejected action data", content[-1]["text"])
            events = self.malformed_events() if index == 0 else structured_events()
            kwargs["stdout"].write("\n".join(map(json.dumps, events)).encode())
            processes.append(process)
            return process

        popen.side_effect = start
        action, metadata = relay.infer(request(), "/test/claude")
        self.assertEqual(action, structured_events()[-1]["structured_output"]["action"])
        self.assertEqual(popen.call_count, 2)
        self.assertEqual(killpg.call_args_list, [mock.call(12345, signal.SIGKILL), mock.call(12346, signal.SIGKILL)])
        processes[1].wait.assert_called_once_with(timeout=5)
        self.assertIn("Redundant outer explanation", metadata["rejected_attempts"][0]["model_response"])

    @mock.patch.object(relay, "infer_once")
    def test_only_one_retry_and_rejected_evidence_on_failure(self, once):
        once.side_effect = [self.malformed_error(), self.malformed_error()]
        with self.assertRaises(relay.MalformedResponse) as caught:
            relay.infer(request(), "/test/claude")
        self.assertEqual(once.call_count, 2)
        self.assertEqual(len(caught.exception.rejected_attempts), 2)

    @mock.patch.object(relay, "infer_once")
    def test_uncertain_semantic_policy_and_cancellation_failures_never_retry(self, once):
        for error in (relay.Invalid("Invalid x"), relay.Invalid("Unknown action"),
                      relay.InferenceError("Host tool"), relay.InferenceCancelled("Disconnected"),
                      TimeoutError("Timeout")):
            once.reset_mock()
            once.side_effect = error
            with self.subTest(error=error), self.assertRaises(type(error)):
                relay.infer(request(), "/test/claude")
            self.assertEqual(once.call_count, 1)

    def test_late_host_tool_error_overrides_early_formatting_error(self):
        for late in ({"type": "error", "error": "Policy failure"},
                     {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {}}]}}):
            with self.subTest(late=late), self.assertRaises(relay.InferenceError):
                relay.parse_model_output("\n".join(map(json.dumps, self.malformed_events() + [late])), request())
        events = self.malformed_events()
        events[1]["message"]["content"][0]["input"]["action"]["x"] = 900
        with self.assertRaises(relay.Invalid) as caught:
            relay.parse_model_output("\n".join(map(json.dumps, events)), request())
        self.assertNotIsInstance(caught.exception, relay.MalformedResponse)

    @mock.patch.object(relay, "infer_once")
    def test_retry_cannot_change_action(self, once):
        once.side_effect = [self.malformed_error(), ({"type": "done", "reason": "Changed decision"}, {})]
        with self.assertRaises(relay.Invalid) as caught:
            relay.infer(request(), "/test/claude")
        self.assertIn("changed", str(caught.exception))
        self.assertEqual(len(caught.exception.rejected_attempts), 1)

    @mock.patch.object(relay.time, "monotonic")
    @mock.patch.object(relay, "infer_once")
    def test_retry_shares_original_deadline(self, once, monotonic):
        now = [0]
        monotonic.side_effect = lambda: now[0]

        def inference(*args, **kwargs):
            if now[0] == 0:
                self.assertEqual(kwargs["timeout"], 120)
                now[0] = 75
                raise self.malformed_error()
            self.assertEqual(kwargs["timeout"], 45)
            now[0] = 90
            return structured_events()[-1]["structured_output"]["action"], {}

        once.side_effect = inference
        relay.infer(request(), "/test/claude")
        self.assertEqual(once.call_count, 2)

    @mock.patch.object(relay.time, "monotonic")
    @mock.patch.object(relay, "infer_once")
    def test_expired_budget_prevents_retry(self, once, monotonic):
        monotonic.side_effect = [0, 0, 121]
        once.side_effect = self.malformed_error()
        with self.assertRaises(TimeoutError) as caught:
            relay.infer(request(), "/test/claude")
        self.assertEqual(once.call_count, 1)
        self.assertEqual(len(caught.exception.rejected_attempts), 1)

    @mock.patch.object(relay, "infer_once")
    def test_disconnect_between_attempts_prevents_retry(self, once):
        once.side_effect = self.malformed_error()
        cancelled = mock.Mock(side_effect=[False, True])
        with self.assertRaises(relay.InferenceCancelled) as caught:
            relay.infer(request(), "/test/claude", cancelled=cancelled)
        self.assertEqual(once.call_count, 1)
        self.assertEqual(len(caught.exception.rejected_attempts), 1)


class ServerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.evidence = pathlib.Path(self.temp.name)
        self.inference = mock.Mock(return_value=({"type": "done", "reason": "Visible completion"}, {"model": "test"}))
        self.server = relay.RelayServer(0, "t" * 32, self.evidence, "/test/claude", self.inference)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.temp.cleanup()

    def post(self, value=None, token="t" * 32, headers=None, path="/decide"):
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=3)
        body = json.dumps(value if value is not None else request()).encode()
        request_headers = {"Authorization": "Bearer " + token, "Content-Type": "application/json"}
        request_headers.update(headers or {})
        try:
            connection.request("POST", path, body=body, headers=request_headers)
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def test_localhost_and_success_evidence(self):
        self.assertEqual(self.server.server_address[0], "127.0.0.1")
        status, body = self.post()
        self.assertEqual(status, 200)
        self.assertEqual(body["request_id"], "request-1")
        directories = list(self.evidence.iterdir())
        self.assertEqual(len(directories), 1)
        directory = directories[0]
        self.assertRegex(directory.name, r"^[0-9a-f]{32}$")
        metadata = json.loads((directory / "request.json").read_text())
        self.assertNotIn("image_base64", metadata)
        self.assertEqual(metadata["request_id"], "request-1")
        self.assertEqual((directory / "screenshot.png").read_bytes(), png())
        result = json.loads((directory / "result.json").read_text())
        self.assertEqual(result["inference"]["model"], "test")
        self.assertNotIn("t" * 32, json.dumps(result) + json.dumps(metadata))
        self.assertEqual(self.post()[0], 200)
        self.assertEqual(len(list(self.evidence.iterdir())), 2)

    def test_auth_path_body_and_busy(self):
        self.assertEqual(self.post(token="wrong")[0], 401)
        self.assertEqual(self.post(path="/other")[0], 404)
        self.assertEqual(self.post(headers={"Content-Length": str(relay.MAX_BODY + 1)})[0], 413)
        self.assertEqual(self.post(headers={"Transfer-Encoding": "chunked"})[0], 400)
        self.assertEqual(self.post(headers={"Content-Type": "text/plain"})[0], 400)
        self.assertEqual(self.post(value={"task": "missing fields"})[0], 400)
        self.server.inflight.acquire()
        try:
            self.assertEqual(self.post()[0], 429)
        finally:
            self.server.inflight.release()
        self.inference.assert_not_called()
        self.assertEqual(list(self.evidence.iterdir()), [])

    def test_inference_failures_release_lock(self):
        for error, status in [(TimeoutError("private detail"), 504),
                              (relay.InferenceError("private detail"), 502),
                              (relay.Invalid("private detail"), 502)]:
            with self.subTest(error=error):
                self.inference.side_effect = error
                actual, body = self.post()
                self.assertEqual(actual, status)
                self.assertNotIn("private detail", json.dumps(body))
                self.assertFalse(self.server.inflight.locked())
        self.assertEqual(len(list(self.evidence.glob("*/error.json"))), 3)

    def test_invalid_returned_action(self):
        self.inference.return_value = ({"type": "tap", "x": 50, "y": 0, "reason": "Out of bounds"}, {})
        self.assertEqual(self.post()[0], 502)

    def test_rejected_attempt_evidence_is_private_on_success_and_failure(self):
        rejected = [{"attempt": 1, "model_response": "Private rejected formatting"}]
        action = {"type": "done", "reason": "Visible completion"}
        self.inference.return_value = (action, {"model": "test", "rejected_attempts": rejected})
        status, body = self.post()
        self.assertEqual(status, 200)
        self.assertNotIn("Private rejected formatting", json.dumps(body))
        result = json.loads(next(self.evidence.glob("*/result.json")).read_text())
        self.assertEqual(result["inference"]["rejected_attempts"], rejected)
        error = TimeoutError("Timed out after rejected response")
        error.rejected_attempts = rejected
        self.inference.side_effect = error
        status, body = self.post()
        self.assertEqual(status, 504)
        self.assertNotIn("Private rejected formatting", json.dumps(body))
        evidence = json.loads(next(self.evidence.glob("*/error.json")).read_text())
        self.assertEqual(evidence["rejected_attempts"], rejected)

    def test_duplicate_auth_and_lengths(self):
        for duplicate in ("Authorization", "Content-Length"):
            connection = http.client.HTTPConnection(*self.server.server_address, timeout=3)
            body = json.dumps(request()).encode()
            try:
                connection.putrequest("POST", "/decide")
                connection.putheader("Content-Type", "application/json")
                connection.putheader("Authorization", "Bearer " + "t" * 32)
                connection.putheader("Content-Length", str(len(body)))
                connection.putheader(duplicate, "Bearer " + "t" * 32 if duplicate == "Authorization" else str(len(body)))
                connection.endheaders(body)
                response = connection.getresponse()
                self.assertEqual(response.status, 401 if duplicate == "Authorization" else 400)
                response.read()
            finally:
                connection.close()
        self.inference.assert_not_called()

    def test_disconnected_client_keeps_evidence(self):
        entered, release, responded = threading.Event(), threading.Event(), threading.Event()

        def inference(*args, cancelled=None):
            entered.set()
            release.wait(3)
            return {"type": "done", "reason": "Visible completion"}, {"model": "test"}

        original = relay.RelayHandler.respond

        def respond(handler, *args):
            try:
                original(handler, *args)
            finally:
                responded.set()

        self.inference.side_effect = inference
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=3)
        with mock.patch.object(relay.RelayHandler, "respond", respond):
            try:
                connection.request("POST", "/decide", body=json.dumps(request()),
                                   headers={"Authorization": "Bearer " + "t" * 32, "Content-Type": "application/json"})
                self.assertTrue(entered.wait(3))
                connection.close()
                release.set()
                self.assertTrue(responded.wait(3))
            finally:
                connection.close()
                release.set()
        self.assertEqual(len(list(self.evidence.glob("*/result.json"))), 1)

    @mock.patch.object(relay.os, "killpg")
    @mock.patch.object(relay.subprocess, "Popen")
    def test_disconnect_cancels_child_and_accepts_next_request(self, popen, killpg):
        entered, terminated = threading.Event(), threading.Event()
        process = mock.Mock(pid=12345)
        process.poll.return_value = None
        process.wait.side_effect = lambda **kwargs: terminated.set()

        def start(*args, **kwargs):
            entered.set()
            return process

        popen.side_effect = start
        self.inference.side_effect = lambda req, claude, cancelled: relay.infer(req, claude, cancelled=cancelled)
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=3)
        try:
            connection.request("POST", "/decide", body=json.dumps(request()),
                               headers={"Authorization": "Bearer " + "t" * 32, "Content-Type": "application/json"})
            self.assertTrue(entered.wait(3))
            self.assertEqual(self.post()[0], 429)
            connection.close()
            self.assertTrue(terminated.wait(1), "Disconnected inference was not promptly terminated")
            self.assertTrue(self.server.inflight.acquire(timeout=1), "Cancelled request retained the inference lock")
            self.server.inflight.release()
            killpg.assert_called_once_with(12345, signal.SIGKILL)
            process.wait.assert_called_once_with(timeout=5)
            errors = list(self.evidence.glob("*/error.json"))
            self.assertEqual(len(errors), 1)
            self.assertEqual(json.loads(errors[0].read_text())["error"], "Inference cancelled")
            self.assertEqual(list(self.evidence.glob("*/result.json")), [])
            self.inference.side_effect = None
            self.assertEqual(self.post()[0], 200)
        finally:
            connection.close()


class SocketTests(unittest.TestCase):
    def test_socket_probe_preserves_data_and_detects_disconnect(self):
        client, server = socket.socketpair()
        try:
            self.assertFalse(relay.socket_disconnected(server))
            client.sendall(b"x")
            self.assertFalse(relay.socket_disconnected(server))
            self.assertEqual(server.recv(1), b"x")
            client.close()
            self.assertTrue(relay.socket_disconnected(server))
            server.close()
            self.assertTrue(relay.socket_disconnected(server))
        finally:
            client.close()
            server.close()

    def test_request_half_close_is_explicit_cancellation(self):
        client, server = socket.socketpair()
        try:
            client.shutdown(socket.SHUT_WR)
            self.assertTrue(relay.socket_disconnected(server))
        finally:
            client.close()
            server.close()


if __name__ == "__main__":
    unittest.main()
