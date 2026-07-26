from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import run_paired_gpt55 as paired


class PolicyTests(unittest.TestCase):
    def test_fleet_cdp_bridge_targets_the_pinned_guest_port(self) -> None:
        self.assertEqual(
            paired.fleet_pilot.GUEST_CDP_PORT,
            paired.fleet_pilot.CDP_PORT,
        )

    def test_control_and_treatment_share_native_tools(self) -> None:
        control = {tool["name"] for tool in paired.action_tools("screenshot_ax")}
        treatment = {tool["name"] for tool in paired.action_tools("combined")}
        native = {
            "native_click",
            "native_type",
            "native_hotkey",
            "native_scroll",
            "done",
        }
        self.assertEqual(control, native)
        self.assertTrue(native.issubset(treatment))
        self.assertEqual(
            treatment - control,
            {"browser_click", "browser_type", "browser_scroll"},
        )

    def test_model_prompt_projects_cdp_only_into_treatment(self) -> None:
        native = {"tree_markdown": "button"}
        control = paired.model_input_text(
            instruction="task",
            mode="screenshot_ax",
            step=1,
            max_steps=2,
            native=native,
            browser=None,
            history=[],
        )
        treatment = paired.model_input_text(
            instruction="task",
            mode="combined",
            step=1,
            max_steps=2,
            native=native,
            browser=[
                {
                    "tab": 0,
                    "active": True,
                    "title": "mock",
                    "url": "https://mock.invalid",
                    "state": {
                        "outline": "heading",
                        "refs": [{"ref": "p1:1"}],
                        "snapshot": {"complete": True},
                    },
                }
            ],
            history=[],
        )
        self.assertNotIn("semantic_v2 browser state:", control)
        self.assertIn("semantic_v2 browser state:", treatment)
        self.assertIn("p1:1", treatment)

    def test_openai_request_uses_supported_image_detail_and_reasoning_budget(self) -> None:
        captured = {}

        def create(**kwargs):
            captured.update(kwargs)
            return SimpleNamespace(
                status="completed",
                incomplete_details=None,
                output=[
                    SimpleNamespace(
                        type="function_call",
                        name="done",
                        arguments='{"reason":"complete"}',
                    )
                ],
                model="gpt-5.5-2026-04-23",
                usage=None,
                model_dump=lambda **_kwargs: {"status": "completed"},
            )

        client = SimpleNamespace(responses=SimpleNamespace(create=create))
        action, _metadata = paired.choose_action(
            client=client,
            model="gpt-5.5",
            reasoning_effort="xhigh",
            mode="screenshot_ax",
            instruction="task",
            step=1,
            max_steps=1,
            native={"tree_markdown": ""},
            screenshot_b64="AA==",
            browser=None,
            history=[],
        )
        image = captured["input"][0]["content"][1]
        self.assertEqual(image["detail"], "high")
        self.assertEqual(
            captured["max_output_tokens"],
            paired.MAX_OUTPUT_TOKENS,
        )
        self.assertEqual(action["name"], "done")

    def test_incomplete_model_response_is_a_harness_failure(self) -> None:
        response = SimpleNamespace(
            status="incomplete",
            incomplete_details=SimpleNamespace(
                model_dump=lambda **_kwargs: {
                    "reason": "max_output_tokens",
                }
            ),
            output=[],
        )
        client = SimpleNamespace(
            responses=SimpleNamespace(create=lambda **_kwargs: response)
        )
        with self.assertRaisesRegex(
            paired.PairedRunError,
            "model response was 'incomplete'",
        ):
            paired.choose_action(
                client=client,
                model="gpt-5.5",
                reasoning_effort="xhigh",
                mode="screenshot_ax",
                instruction="task",
                step=1,
                max_steps=1,
                native={"tree_markdown": ""},
                screenshot_b64="AA==",
                browser=None,
                history=[],
            )

    def test_driver_refusal_is_returned_to_the_model_loop(self) -> None:
        refusal = {
            "refused": True,
            "tool": "click",
            "detail": "stale element",
            "returncode": 1,
        }
        with (
            patch.object(paired, "driver_call", return_value=refusal),
            patch.object(
                paired,
                "native_snapshot",
                return_value=({"tree_markdown": "unchanged"}, "AA=="),
            ),
        ):
            outcome, _post = paired.execute_action(
                action={
                    "name": "native_click",
                    "arguments": {"element_index": 1, "x": None, "y": None},
                },
                target=paired.NativeTarget(pid=1, window_id=2),
                browser=None,
                session="test",
            )
        self.assertTrue(outcome["refused"])

    def test_failed_episode_cannot_form_a_valid_pair(self) -> None:
        def episode(mode: str, failure=None):
            return {
                "mode": mode,
                "agent_failure": failure,
                "evaluation": {"score": 0.0} if failure is None else None,
                "session_ended": True,
                "resolved_models": ["gpt-5.5-2026-04-23"],
                "reset_evidence": {
                    "guest_chrome_profile": f"/tmp/{mode}",
                    "initial_evaluation": {"score": 0.0},
                },
            }

        errors = paired.validate_pair(
            [
                episode("screenshot_ax"),
                episode("combined", "browser binding failed"),
            ]
        )
        self.assertTrue(errors)
        self.assertTrue(any("combined" in error for error in errors))


class AccountingTests(unittest.TestCase):
    def test_usage_sum_tracks_reasoning_and_cache(self) -> None:
        result = paired.usage_sum(
            [
                {
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 4,
                        "total_tokens": 14,
                        "input_tokens_details": {"cached_tokens": 3},
                        "output_tokens_details": {"reasoning_tokens": 2},
                    }
                },
                {
                    "usage": {
                        "input_tokens": 5,
                        "output_tokens": 1,
                        "total_tokens": 6,
                    }
                },
            ]
        )
        self.assertEqual(result["input_tokens"], 15)
        self.assertEqual(result["output_tokens"], 5)
        self.assertEqual(result["reasoning_tokens"], 2)
        self.assertEqual(result["cached_tokens"], 3)
        self.assertEqual(result["total_tokens"], 20)

    def test_env_file_parser_accepts_export_without_logging_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ".env"
            path.write_text(
                "export OPENAI_API_KEY='test-value'\nIGNORED=nope\n",
                encoding="utf-8",
            )
            parsed = paired.read_env_file(path)
        self.assertEqual(parsed["OPENAI_API_KEY"], "test-value")

    def test_cost_uses_cached_and_long_context_rates(self) -> None:
        normal = paired.estimate_standard_cost(
            [
                {
                    "usage": {
                        "input_tokens": 1_000_000,
                        "output_tokens": 1_000_000,
                        "input_tokens_details": {"cached_tokens": 0},
                    }
                }
            ]
        )
        self.assertEqual(normal["long_context_requests"], 1)
        # Long-context GPT-5.5 is 2x input and 1.5x output.
        self.assertEqual(normal["estimated_usd"], 55.0)

        cached = paired.estimate_standard_cost(
            [
                {
                    "usage": {
                        "input_tokens": 100_000,
                        "output_tokens": 0,
                        "input_tokens_details": {"cached_tokens": 100_000},
                    }
                }
            ]
        )
        self.assertEqual(cached["estimated_usd"], 0.05)


if __name__ == "__main__":
    unittest.main()
