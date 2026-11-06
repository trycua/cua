"""Structured action errors outrank incidental stale-token diagnostic text."""

from __future__ import annotations

import argparse
import copy
import io
import json
import sys
import tempfile
import unittest
from contextlib import asynccontextmanager, redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from native_tasks import native_task
from run import DriverToolError
from run_native import is_stale_token_error, run_task

ROOT = Path(__file__).resolve().parents[2]
STALE_MESSAGE = "click failed: element_token is stale; get_window_state again"


class StaleErrorTest(unittest.TestCase):
    def test_structured_code_precedes_diagnostic_text(self) -> None:
        for code in ("permission_denied", "tool_invocation_failed", "future_error", " "):
            with self.subTest(code=code):
                self.assertFalse(is_stale_token_error(DriverToolError(STALE_MESSAGE, code)))

    def test_structured_stale_code_does_not_need_diagnostic_text(self) -> None:
        self.assertTrue(is_stale_token_error(DriverToolError("refused", "stale_element_token")))

    def test_code_less_legacy_messages_remain_supported(self) -> None:
        for error in (
            DriverToolError(STALE_MESSAGE),
            DriverToolError(STALE_MESSAGE, ""),
            RuntimeError(STALE_MESSAGE),
        ):
            with self.subTest(error=error):
                self.assertTrue(is_stale_token_error(error))
        self.assertFalse(is_stale_token_error(RuntimeError("transport failed")))

    def test_wrapped_error_does_not_borrow_its_causes_stale_code(self) -> None:
        error = RuntimeError("transport failed")
        error.__cause__ = DriverToolError("refused", "stale_element_token")
        self.assertFalse(is_stale_token_error(error))


class NativeStaleLoopTest(unittest.IsolatedAsyncioTestCase):
    async def run_case(self, code: str | None) -> tuple[str, list[dict], list[dict], dict]:
        payload = json.loads(
            (ROOT / "fixtures/native/gtk3-window-state-initial-v1.json").read_text(encoding="utf-8")
        )
        actions: list[dict] = []
        reads: list[dict] = []
        with tempfile.TemporaryDirectory() as directory:
            state_path = Path(directory) / "state.json"
            log_path = Path(directory) / "run.jsonl"
            task = native_task("gtk3-counter", state_path, pid=payload["pid"])
            state = {"schema": task.oracle.schema, "pid": payload["pid"], "counter": 0}
            state_path.write_text(json.dumps(state), encoding="utf-8")

            class Session:
                async def __aenter__(self):
                    return self

                async def __aexit__(self, *args):
                    return False

                async def initialize(self):
                    return None

                async def list_tools(self):
                    return SimpleNamespace(tools=[])

                async def call_tool(self, name, arguments):
                    if name == "list_windows":
                        data = {
                            "windows": [
                                {
                                    "window_id": payload["window_id"],
                                    "title": task.scope.window_title,
                                }
                            ]
                        }
                    elif name == "get_window_state":
                        reads.append(dict(arguments))
                        data = copy.deepcopy(payload)
                        snapshot = f"s{len(reads):08d}"
                        data["snapshot_id"] = snapshot
                        for element in data["elements"]:
                            element["element_token"] = f"{snapshot}:{element['element_index']}"
                    elif name == "click":
                        actions.append(dict(arguments))
                        if len(actions) == 1 or code not in (None, "stale_element_token"):
                            return SimpleNamespace(
                                isError=True,
                                structuredContent={} if code is None else {"code": code},
                                content=[{"type": "text", "text": STALE_MESSAGE}],
                            )
                        state["counter"] = 3
                        state_path.write_text(json.dumps(state), encoding="utf-8")
                        data = {"effect": "confirmed"}
                    else:
                        raise AssertionError(f"unexpected tool {name}")
                    return SimpleNamespace(isError=False, structuredContent=data)

            @asynccontextmanager
            async def transport(_):
                yield None, None

            args = argparse.Namespace(
                platform="linux", pid=payload["pid"], provider="mock", log=str(log_path)
            )
            with (
                patch("run_native.stdio_client", transport),
                patch("run_native.ClientSession", return_value=Session()),
                redirect_stdout(io.StringIO()),
            ):
                outcome = await run_task(args, task)
            events = [
                json.loads(line) for line in log_path.read_text(encoding="utf-8").splitlines()
            ]
            return (
                outcome,
                actions,
                reads,
                {"events": events, "state": json.loads(state_path.read_text(encoding="utf-8"))},
            )

    async def test_non_stale_code_stops_without_redispatch(self) -> None:
        for code in ("permission_denied", "tool_invocation_failed", "future_error"):
            with self.subTest(code=code):
                outcome, actions, reads, result = await self.run_case(code)
                self.assertEqual(outcome, "unknown")
                self.assertEqual(len(actions), 1)
                self.assertEqual(len(reads), 1)
                self.assertEqual(result["state"]["counter"], 0)
                self.assertEqual(result["events"][-1]["phase"], "action")
                self.assertFalse(
                    any(
                        event.get("action_error") == "stale_element_token"
                        for event in result["events"]
                    )
                )

    async def test_true_stale_and_code_less_legacy_refusals_reobserve(self) -> None:
        for code in ("stale_element_token", None):
            with self.subTest(code=code):
                outcome, actions, reads, result = await self.run_case(code)
                self.assertEqual(outcome, "verified")
                self.assertEqual(len(actions), 2)
                self.assertEqual(len(reads), 2)
                self.assertNotEqual(actions[0]["element_token"], actions[1]["element_token"])
                self.assertEqual(result["state"]["counter"], 3)
                self.assertEqual(
                    sum(
                        event.get("action_error") == "stale_element_token"
                        for event in result["events"]
                    ),
                    1,
                )


if __name__ == "__main__":
    unittest.main()
