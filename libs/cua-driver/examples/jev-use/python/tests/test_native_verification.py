"""Tests for native verification loop, intermediate effect predicates, and poll_oracle."""

from __future__ import annotations

import asyncio
import json
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import Candidate
from native_tasks import (
    AppStateOracle,
    COUNTER_TARGET,
    NativeTask,
    OracleError,
    TARGET_SIZE,
    TaskStep,
    appkit_task,
    check_intermediate_effect,
)
from run_native import PollResult, poll_oracle, run_task


class TestNativeVerification(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.state_path = Path(self.temp_dir.name) / "state.json"
        self.write_state({"counter": 0, "size": "", "agreed": False, "note_saved": None})

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def write_state(self, updates: dict[str, Any], *, pid: int = 42, schema: str = "cua.appkit_task_state_v1") -> None:
        payload = {"schema": schema, "pid": pid, **updates}
        self.state_path.write_text(json.dumps(payload), encoding="utf-8")

    def test_check_intermediate_effect_counter(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        # Expected increment
        self.assertTrue(check_intermediate_effect(task, "ax:button:increment", {"counter": 0}, {"counter": 1}))
        self.assertTrue(check_intermediate_effect(task, "ax:button:increment", {"counter": 1}, {"counter": 2}))
        # Stale counter (not yet incremented)
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 0}, {"counter": 0}))
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 1}, {"counter": 1}))
        # Missing or non-int counter
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", None, {"counter": 1}))
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": "bad"}, {"counter": 1}))
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 0}, {"counter": "bad"}))

    def test_check_intermediate_effect_counter_rejects_bool_and_fractional(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        # In Python isinstance(True, int) is True, but type(True) is bool.
        # Counter predicate must reject bool values!
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": False}, {"counter": True}))
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 0}, {"counter": True}))
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": False}, {"counter": 1}))
        # Rejects fractional floats
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 0.0}, {"counter": 1.0}))
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 0}, {"counter": 1.5}))
        # Rejects non-finite numbers
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", {"counter": 0}, {"counter": float("inf")}))

    def test_check_intermediate_effect_choose_size(self) -> None:
        task = appkit_task("appkit-choose-size", self.state_path, pid=42)
        # Radio large selected
        self.assertTrue(check_intermediate_effect(task, "ax:radio:large", None, {"size": "large"}))
        # Radio large not yet selected
        self.assertFalse(check_intermediate_effect(task, "ax:radio:large", None, {"size": ""}))
        self.assertFalse(check_intermediate_effect(task, "ax:radio:large", None, {"size": "small"}))

    def test_check_intermediate_effect_unsupported_and_none(self) -> None:
        task = appkit_task("appkit-save-note", self.state_path, pid=42)
        # Save note text entry lacks entered_note field in oracle schema -> returns None (conservative full poll)
        self.assertIsNone(check_intermediate_effect(task, "ax:text_input:note:set:note", {}, {}))
        # None candidate
        self.assertIsNone(check_intermediate_effect(task, None, {}, {}))
        # Unknown candidate
        self.assertIsNone(check_intermediate_effect(task, "ax:unknown", {}, {}))

    def test_custom_task_with_familiar_ids_retains_baseline_polling(self) -> None:
        # A custom task using familiar IDs like ax:button:increment must NOT have an effect predicate
        # bound by default, ensuring generic/custom tasks retain baseline whole-task polling.
        base_task = appkit_task("appkit-counter", self.state_path, pid=42)
        custom_task = NativeTask(
            id="custom-counter-task",
            goal="Custom task with familiar candidate IDs",
            scope=base_task.scope,
            allowed_actions=frozenset({"press"}),
            oracle=base_task.oracle,
            check=lambda state: "pending",
            steps=(TaskStep('Press button', "ax:button:increment", 3),),
        )
        self.assertIsNone(custom_task.intermediate_effect)
        # check_intermediate_effect returns None
        self.assertIsNone(check_intermediate_effect(custom_task, "ax:button:increment", {"counter": 0}, {"counter": 1}))

    async def test_custom_task_poll_oracle_retains_full_polling(self) -> None:
        base_task = appkit_task("appkit-counter", self.state_path, pid=42)
        custom_task = NativeTask(
            id="custom-counter-task",
            goal="Custom task with familiar candidate IDs",
            scope=base_task.scope,
            allowed_actions=frozenset({"press"}),
            oracle=base_task.oracle,
            check=lambda state: "pending",
            steps=(TaskStep('Press button', "ax:button:increment", 3),),
        )
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [custom_task.history_entry(1, cand.id)]
        self.write_state({"counter": 1})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                custom_task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            # Custom task must retain baseline 20 attempts of polling
            self.assertEqual(result.outcome, "unknown")
            self.assertEqual(result.status, "continuation")
            self.assertTrue(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 20)

    async def test_poll_oracle_intermediate_immediate(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        self.write_state({"counter": 1})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            self.assertEqual(result.outcome, "unknown")
            self.assertEqual(result.status, "intermediate_witnessed")
            self.assertTrue(result.can_continue)
            # Immediate effect short-circuits without any sleep
            mock_sleep.assert_not_called()

    async def test_poll_oracle_delayed_150ms(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        reads = 0

        def delayed_read() -> dict[str, Any]:
            nonlocal reads
            reads += 1
            if reads < 3:  # 0ms and 100ms read stale 0; 200ms read sees 1
                return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 0}
            return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 1}

        with patch.object(AppStateOracle, "read", side_effect=delayed_read), patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            self.assertEqual(result.status, "intermediate_witnessed")
            self.assertTrue(result.can_continue)
            # Exactly 2 sleeps (100ms each) before effect witnessed at attempt 3
            self.assertEqual(mock_sleep.call_count, 2)

    async def test_poll_oracle_delayed_greater_than_observation_duration(self) -> None:
        # Typical observation is ~350ms. Test delay of 400ms (witnessed on 5th attempt after 4 sleeps).
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        reads = 0

        def delayed_read() -> dict[str, Any]:
            nonlocal reads
            reads += 1
            if reads < 5:
                return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 0}
            return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 1}

        with patch.object(AppStateOracle, "read", side_effect=delayed_read), patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            self.assertEqual(result.status, "intermediate_witnessed")
            self.assertTrue(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 4)

    async def test_poll_oracle_delayed_at_deadline(self) -> None:
        # Effect appears on the 20th attempt (after 19 sleeps)
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        reads = 0

        def delayed_read() -> dict[str, Any]:
            nonlocal reads
            reads += 1
            if reads < 20:
                return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 0}
            return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 1}

        with patch.object(AppStateOracle, "read", side_effect=delayed_read), patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            self.assertEqual(result.status, "intermediate_witnessed")
            self.assertTrue(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 19)

    async def test_poll_oracle_never_applied_timeout(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        self.write_state({"counter": 0})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            self.assertEqual(result.outcome, "unknown")
            self.assertEqual(result.status, "intermediate_timeout")
            # Must NOT continue to dependent mutation!
            self.assertFalse(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 20)

    async def test_terminal_action_with_satisfied_low_level_effect_waits_for_terminal_oracle(self) -> None:
        # On a declared terminal action (3rd increment of 3), the intermediate low-level effect
        # is satisfied (counter=3 == pre 2 + 1), but declared steps remaining is 0!
        # The poller must NOT exit as intermediate_witnessed; it must wait for whole-task terminal success.
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        # History has 3 performed increment steps (all declared steps are done!)
        history = [
            task.history_entry(1, cand.id),
            task.history_entry(2, cand.id),
            task.history_entry(3, cand.id),
        ]

        reads = 0

        def delayed_terminal_read() -> dict[str, Any]:
            nonlocal reads
            reads += 1
            # For the first 2 reads, app state is still pending/transitioning
            if reads < 3:
                return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 2}
            # On 3rd read, terminal state is reached
            return {"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 3}

        with patch.object(AppStateOracle, "read", side_effect=delayed_terminal_read), patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=3, candidate=cand, pre_oracle={"counter": 2}, history=history
            )
            # Must NOT return intermediate_witnessed! Must wait for verified outcome.
            self.assertEqual(result.outcome, "verified")
            self.assertEqual(result.status, "verified")
            self.assertFalse(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 2)

    async def test_budget_exhausted_retains_terminal_polling_despite_witnessed_effect(self) -> None:
        from dataclasses import replace

        task = replace(appkit_task("appkit-counter", self.state_path, pid=42), max_steps=1)
        cand = Candidate("ax:button:increment", "ax", "click", {"pid": 42})
        self.write_state({"counter": 1})
        history = [task.history_entry(1, cand.id)]
        with patch("asyncio.sleep", new_callable=AsyncMock) as sleeper:
            result = await poll_oracle(task, 1, candidate=cand, pre_oracle={"counter": 0}, history=history)
        self.assertEqual(result.outcome, "budget_exhausted")
        self.assertEqual(sleeper.call_count, 20)

    async def test_no_steps_fallback_retains_baseline_polling(self) -> None:
        # If a task declares no steps, has_declared_steps_remaining is False, so poller
        # retains original whole-task polling without intermediate shortcut.
        base_task = appkit_task("appkit-counter", self.state_path, pid=42)
        no_steps_task = NativeTask(
            id="appkit-counter-no-steps",
            goal=base_task.goal,
            scope=base_task.scope,
            allowed_actions=base_task.allowed_actions,
            oracle=base_task.oracle,
            check=base_task.check,
            steps=(),
            intermediate_effect=base_task.intermediate_effect,
        )
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [no_steps_task.history_entry(1, cand.id)]
        self.write_state({"counter": 1})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                no_steps_task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history
            )
            self.assertEqual(result.outcome, "unknown")
            self.assertEqual(result.status, "continuation")
            self.assertTrue(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 20)

    async def test_refuted_state_dominance(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        # Counter is 4 (> COUNTER_TARGET 3) -> Refuted dominates
        self.write_state({"counter": 4})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 3}, history=history
            )
            self.assertEqual(result.outcome, "refuted")
            self.assertEqual(result.status, "refuted")
            self.assertFalse(result.can_continue)
            mock_sleep.assert_not_called()

    async def test_early_terminal_success(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        # Counter reached 3 early on step 1
        self.write_state({"counter": 3})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(
                task, steps=1, candidate=cand, pre_oracle={"counter": 2}, history=history
            )
            self.assertEqual(result.outcome, "verified")
            self.assertEqual(result.status, "verified")
            self.assertFalse(result.can_continue)
            mock_sleep.assert_not_called()

    async def test_oracle_pid_and_schema_failure_propagates(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]

        # Foreign PID failure
        self.write_state({"counter": 1}, pid=999)
        with self.assertRaises(OracleError):
            await poll_oracle(task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history)

        # Schema mismatch failure
        self.write_state({"counter": 1}, schema="cua.foreign_schema_v1")
        with self.assertRaises(OracleError):
            await poll_oracle(task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history)

    async def test_cancellation_propagation(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        self.write_state({"counter": 0})

        async def cancel_sleep(duration: float) -> None:
            raise asyncio.CancelledError()

        with patch("asyncio.sleep", side_effect=cancel_sleep):
            with self.assertRaises(asyncio.CancelledError):
                await poll_oracle(task, steps=1, candidate=cand, pre_oracle={"counter": 0}, history=history)

    async def test_unsupported_save_note_retains_full_polling(self) -> None:
        task = appkit_task("appkit-save-note", self.state_path, pid=42)
        cand = Candidate("ax:text_input:note:set:note", "ax", "set_text", {"pid": 42})
        history = [task.history_entry(1, cand.id)]
        self.write_state({"note_saved": None})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(task, steps=1, candidate=cand, pre_oracle={}, history=history)
            self.assertEqual(result.outcome, "unknown")
            self.assertEqual(result.status, "continuation")
            self.assertTrue(result.can_continue)
            self.assertEqual(mock_sleep.call_count, 20)

    async def test_changed_target_prestate(self) -> None:
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        # 1 prior increment performed, now dispatching 2nd
        history = [task.history_entry(1, cand.id), task.history_entry(2, cand.id)]
        # If pre-action counter was already 1, step expects 2
        self.write_state({"counter": 2})

        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            result = await poll_oracle(task, steps=2, candidate=cand, pre_oracle={"counter": 1}, history=history)
            self.assertEqual(result.status, "intermediate_witnessed")
            self.assertTrue(result.can_continue)
            mock_sleep.assert_not_called()

    def test_discriminator_fails_stale_counter_mutant(self) -> None:
        # Discriminator against mutant that checks `counter > 0` instead of `curr == pre + 1`.
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        pre_oracle = {"counter": 1}
        # App state still has stale counter = 1 (step 2 effect has NOT occurred)
        current_oracle = {"counter": 1}

        # Production predicate rejects stale counter
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", pre_oracle, current_oracle))

        # Mutant implementation accepting any positive counter
        def mutant_check(cand: str, pre: dict, curr: dict) -> bool:
            return cand == "ax:button:increment" and curr.get("counter", 0) > 0

        # Mutant would falsely acknowledge step 2
        self.assertTrue(mutant_check("ax:button:increment", pre_oracle, current_oracle))

    def test_discriminator_fails_dispatch_only_mutant(self) -> None:
        # Discriminator against Slice 1 rejected proposal: gating only on expected_next(history)
        # without verifying app-owned state.
        task = appkit_task("appkit-counter", self.state_path, pid=42)
        pre_oracle = {"counter": 0}
        # Intermediate effect delayed: app-state still 0
        current_oracle = {"counter": 0}

        # Production predicate fails to satisfy because effect is not yet in app state
        self.assertFalse(check_intermediate_effect(task, "ax:button:increment", pre_oracle, current_oracle))

    async def test_run_task_integration_success(self) -> None:
        initial_ws = json.loads((Path(__file__).resolve().parents[2] / "fixtures/native/appkit-window-state-initial-v1.json").read_text())
        pid = initial_ws["pid"]
        self.write_state({"counter": 0}, pid=pid)
        task = appkit_task("appkit-counter", self.state_path, pid=pid)
        from run_native import parse_args
        args = parse_args(["--task", "appkit-counter", "--pid", str(pid), "--state-file", str(self.state_path), "--log", str(Path(self.temp_dir.name) / "run1.log")])

        class MockTool:
            def __init__(self, name: str) -> None:
                self.name = name
                self.inputSchema: dict[str, Any] = {}

        mock_session = AsyncMock()
        mock_session.initialize = AsyncMock()
        mock_session.list_tools = AsyncMock(return_value=MagicMock(tools=[MockTool("click"), MockTool("get_window_state")]))

        current_counter = 0

        async def mock_call(tool: str, arguments: dict[str, Any]) -> dict[str, Any]:
            nonlocal current_counter
            if tool == "get_window_state":
                return initial_ws
            if tool in ("press", "click"):
                current_counter += 1
                self.write_state({"counter": current_counter}, pid=pid)
                return {"success": True}
            return {}

        with patch("run_native.stdio_client") as mock_stdio, \
             patch("run_native.ClientSession") as mock_cs, \
             patch("run_native.find_window", return_value={"window_id": initial_ws["window_id"], "pid": pid}), \
             patch("run_native.Driver.call", side_effect=mock_call), \
             patch("asyncio.sleep", new_callable=AsyncMock):

            mock_stdio.return_value.__aenter__.return_value = (MagicMock(), MagicMock())
            mock_cs.return_value.__aenter__.return_value = mock_session

            res = await run_task(args, task)
            self.assertEqual(res, "verified")
            self.assertEqual(current_counter, 3)

    async def test_run_task_integration_intermediate_timeout_halts(self) -> None:
        initial_ws = json.loads((Path(__file__).resolve().parents[2] / "fixtures/native/appkit-window-state-initial-v1.json").read_text())
        pid = initial_ws["pid"]
        self.write_state({"counter": 0}, pid=pid)
        task = appkit_task("appkit-counter", self.state_path, pid=pid)
        from run_native import parse_args
        args = parse_args(["--task", "appkit-counter", "--pid", str(pid), "--state-file", str(self.state_path), "--log", str(Path(self.temp_dir.name) / "run2.log")])

        class MockTool:
            def __init__(self, name: str) -> None:
                self.name = name
                self.inputSchema: dict[str, Any] = {}

        mock_session = AsyncMock()
        mock_session.initialize = AsyncMock()
        mock_session.list_tools = AsyncMock(return_value=MagicMock(tools=[MockTool("click"), MockTool("get_window_state")]))

        dispatched_actions = 0

        async def mock_call(tool: str, arguments: dict[str, Any]) -> dict[str, Any]:
            nonlocal dispatched_actions
            if tool == "get_window_state":
                return initial_ws
            if tool in ("press", "click"):
                dispatched_actions += 1
                return {"success": True}
            return {}

        with patch("run_native.stdio_client") as mock_stdio, \
             patch("run_native.ClientSession") as mock_cs, \
             patch("run_native.find_window", return_value={"window_id": initial_ws["window_id"], "pid": pid}), \
             patch("run_native.Driver.call", side_effect=mock_call), \
             patch("asyncio.sleep", new_callable=AsyncMock):

            mock_stdio.return_value.__aenter__.return_value = (MagicMock(), MagicMock())
            mock_cs.return_value.__aenter__.return_value = mock_session

            res = await run_task(args, task)
            self.assertEqual(res, "unknown")
            self.assertEqual(dispatched_actions, 1)


if __name__ == "__main__":
    unittest.main()
