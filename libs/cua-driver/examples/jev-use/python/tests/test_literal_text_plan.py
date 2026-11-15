"""Literal text plans bind fresh, verify independently and never retry input."""

from __future__ import annotations

import asyncio
import copy
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from literal_text_plan import (
    LiteralPlanHandoff,
    LiteralTextStep,
    execute_literal_text_plan,
    validate_literal_text_plan,
)

APPKIT = Path(__file__).resolve().parents[2] / "fixtures/native/appkit-window-state-initial-v1.json"


def window_state(token="s1:1", *, value="", pid=7, duplicate=False, truncated=False):
    target = {
        "element_index": 1,
        "role": "AXTextField",
        "label": "Email",
        "value": value,
        "enabled": True,
        "parent_index": 0,
        "element_token": token,
        "frame": {"x": 10, "y": 10, "w": 80, "h": 20},
    }
    elements = [
        {"element_index": 0, "role": "AXWindow", "label": "Main", "frame": {"x": 0, "y": 0, "w": 500, "h": 400}},
        target,
    ]
    if duplicate:
        elements.append({**target, "element_index": 2, "element_token": "s1:2"})
    return {
        "pid": pid,
        "window_id": 9,
        "snapshot_id": token.split(":")[0],
        "capture_id": "cap1",
        "elements_complete": True,
        "truncated": truncated,
        "window_bounds": {"x": 0, "y": 0, "width": 500, "height": 400},
        "elements": elements,
    }


class Driver:
    def __init__(self, payloads):
        self.payloads = list(payloads)
        self.inputs = []
        self.observations = []

    async def call(self, name, args):
        if name == "get_window_state":
            self.observations.append(args)
            return copy.deepcopy(self.payloads.pop(0))
        self.inputs.append((name, args))
        return {"ok": True}


class ExecuteTests(unittest.IsolatedAsyncioTestCase):
    async def run_plan(self, driver, *, step_ok=True, final_ok=True, context_ok=True, choose=None, steps=None, pid=7, window_id=9):
        async def verify_step(step):
            return step_ok

        async def verify_final():
            return final_ok

        return await execute_literal_text_plan(
            driver,
            pid=pid,
            window_id=window_id,
            platform="macos",
            steps=steps or [LiteralTextStep("Email", "caller literal")],
            verify_step=verify_step,
            verify_final=verify_final,
            verify_context=lambda observation: context_ok,
            choose=choose,
        )

    async def test_sets_exact_literal_through_the_fresh_token(self):
        driver = Driver([window_state(), window_state("s2:1")])
        result = await self.run_plan(driver)
        self.assertEqual(result, {"status": "verified_complete", "steps": [{"label": "Email", "verified": True}]})
        self.assertEqual(
            driver.inputs,
            [("set_value", {"pid": 7, "window_id": 9, "element_token": "s2:1", "value": "caller literal"})],
        )

    async def test_checked_in_appkit_window(self):
        payload = json.loads(APPKIT.read_text())
        driver = Driver([payload, payload])
        await self.run_plan(driver, pid=payload["pid"], window_id=payload["window_id"], steps=[LiteralTextStep("Note", "hello")])
        self.assertEqual(
            driver.inputs,
            [("set_value", {"pid": payload["pid"], "window_id": payload["window_id"], "element_token": "s00000086:21", "value": "hello"})],
        )
        self.assertTrue(all(args.get("full_output") is True for args in driver.observations))

    async def test_two_steps_need_no_chooser(self):
        state = window_state()
        name = {**state["elements"][1], "element_index": 2, "label": "Name", "element_token": "s1:2"}
        state["elements"].append(name)
        driver = Driver([state] * 4)
        verified = []

        async def verify_step(step):
            verified.append(step.label)
            return True

        async def verify_final():
            return True

        await execute_literal_text_plan(
            driver,
            pid=7,
            window_id=9,
            platform="macos",
            steps=[LiteralTextStep("Name", "Ada"), LiteralTextStep("Email", "ada@example.test")],
            verify_step=verify_step,
            verify_final=verify_final,
            verify_context=lambda observation: True,
        )
        self.assertEqual([(args["element_token"], args["value"]) for _, args in driver.inputs], [("s1:2", "Ada"), ("s1:1", "ada@example.test")])
        self.assertEqual(verified, ["Name", "Email"])

    async def test_changed_field_before_input_hands_off(self):
        driver = Driver([window_state(), window_state("s2:1", value="other")])
        with self.assertRaises(LiteralPlanHandoff):
            await self.run_plan(driver)
        self.assertEqual(driver.inputs, [])

    async def test_duplicate_label_is_ambiguous(self):
        driver = Driver([window_state(duplicate=True)])
        with self.assertRaises(LiteralPlanHandoff):
            await self.run_plan(driver)
        self.assertEqual(driver.inputs, [])

    async def test_risky_label_is_refused(self):
        state = window_state()
        state["elements"][1]["label"] = "Delete account"
        driver = Driver([state])
        with self.assertRaises(LiteralPlanHandoff):
            await self.run_plan(driver, steps=[LiteralTextStep("Delete account", "x")])
        self.assertEqual(driver.inputs, [])

    async def test_wrong_owner_or_context_hands_off(self):
        for payload, context_ok in [(window_state(pid=8), True), (window_state(), False)]:
            driver = Driver([payload])
            with self.subTest(context_ok=context_ok), self.assertRaises(LiteralPlanHandoff):
                await self.run_plan(driver, context_ok=context_ok)
            self.assertEqual(driver.inputs, [])

    async def test_acknowledged_input_is_not_completion(self):
        for step_ok, final_ok in [(False, True), (True, False)]:
            driver = Driver([window_state(), window_state("s2:1")])
            with self.subTest(step_ok=step_ok), self.assertRaises(LiteralPlanHandoff):
                await self.run_plan(driver, step_ok=step_ok, final_ok=final_ok)
            self.assertEqual(len(driver.inputs), 1)

    async def test_truncated_tree_reobserves_once_then_hands_off(self):
        driver = Driver([window_state(truncated=True), window_state(truncated=True)])
        with self.assertRaises(LiteralPlanHandoff):
            await self.run_plan(driver)
        self.assertEqual([args.get("timeout_ms") for args in driver.observations], [None, 5000])
        self.assertEqual(driver.inputs, [])

    async def test_chooser_cannot_pick_anything_else(self):
        async def choose(candidate, observation):
            return "invented"

        driver = Driver([window_state()])
        with self.assertRaises(LiteralPlanHandoff):
            await self.run_plan(driver, choose=choose)
        self.assertEqual(driver.inputs, [])

    async def test_driver_refusal_stops_before_next_input(self):
        class RefusingDriver(Driver):
            async def call(self, name, args):
                result = await super().call(name, args)
                if name == "set_value":
                    raise RuntimeError("structured refusal")
                return result

        driver = RefusingDriver([window_state(), window_state("s2:1")])
        with self.assertRaisesRegex(RuntimeError, "structured refusal"):
            await self.run_plan(driver, steps=[LiteralTextStep("Email", "x"), LiteralTextStep("Name", "y")])
        self.assertEqual(len(driver.inputs), 1)

    async def test_timeout_after_input_neither_retries_nor_completes(self):
        class SlowDriver(Driver):
            async def call(self, name, args):
                result = await super().call(name, args)
                if name == "set_value":
                    await asyncio.sleep(1)
                return result

        async def verify(*_):
            raise AssertionError("a timed-out input cannot reach verification")

        driver = SlowDriver([window_state(), window_state("s2:1")])
        with self.assertRaises(asyncio.TimeoutError):
            await execute_literal_text_plan(
                driver,
                pid=7,
                window_id=9,
                platform="macos",
                steps=[LiteralTextStep("Email", "x")],
                verify_step=verify,
                verify_final=verify,
                verify_context=lambda observation: True,
                timeout_s=0.05,
            )
        self.assertEqual(len(driver.inputs), 1)

    async def test_too_many_or_duplicate_steps_refused_before_observing(self):
        for steps in [
            [LiteralTextStep(str(i), "x") for i in range(4)],
            [LiteralTextStep("Email", "x"), LiteralTextStep("Email", "y")],
        ]:
            driver = Driver([])
            with self.subTest(steps=steps), self.assertRaises(LiteralPlanHandoff):
                await self.run_plan(driver, steps=steps)
            self.assertEqual(driver.observations, [])


class ValidateTests(unittest.TestCase):
    BASE = {"owner": {"pid": 7, "window_id": 9}, "steps": [{"label": "Email", "value": "x"}]}

    def test_exact_transcription_is_accepted(self):
        expected = [LiteralTextStep("Email", "x")]
        self.assertEqual(validate_literal_text_plan(self.BASE, pid=7, window_id=9, expected=expected), tuple(expected))

    def test_reordered_steps_are_refused(self):
        expected = [LiteralTextStep("Name", "person"), LiteralTextStep("Email", "x")]
        payload = {"owner": {"pid": 7, "window_id": 9}, "steps": [{"label": "Email", "value": "x"}, {"label": "Name", "value": "person"}]}
        with self.assertRaises(LiteralPlanHandoff):
            validate_literal_text_plan(payload, pid=7, window_id=9, expected=expected)

    def test_changed_or_widened_authority_is_refused(self):
        variants = []
        for where, key, value in [
            ("owner", "pid", 8),
            ("owner", "pid", True),
            ("owner", "extra", 1),
            ("step", "value", " x"),
            ("step", "label", "email"),
            ("step", "action", "click"),
        ]:
            item = copy.deepcopy(self.BASE)
            (item["owner"] if where == "owner" else item["steps"][0])[key] = value
            variants.append(item)
        variants += [
            {**self.BASE, "extra": True},
            {**self.BASE, "steps": []},
            {**self.BASE, "steps": self.BASE["steps"] * 2},
        ]
        for item in variants:
            with self.subTest(item=item), self.assertRaises(LiteralPlanHandoff):
                validate_literal_text_plan(item, pid=7, window_id=9, expected=[LiteralTextStep("Email", "x")])


if __name__ == "__main__":
    unittest.main()
