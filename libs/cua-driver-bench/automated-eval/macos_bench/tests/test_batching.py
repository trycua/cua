# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import claude_events as ce  # noqa: E402


def use(name, **inp):
    return {"name": f"mcp__cua__{name}", "input": inp}


class BatchingTests(unittest.TestCase):
    def test_counts_single_and_batch_calls(self):
        uses = {
            "1": use("get_window_state"),
            "2": use("click", element_token="a:1"),
            "3": use("run_steps", steps=[{"tool": "click"}, {"tool": "type_text"}, {"tool": "press_key"}]),
            "4": use("run_script", script='const a = cua.getApp("X"); await a.click({name:"ok"}); await a.typeText("hi");'),
            "5": {"name": "ToolSearch", "input": {}},
        }
        b = ce.batching_summary(uses)
        self.assertEqual(b["mcp_calls"], 4)
        self.assertEqual(b["single_calls"], 2)
        self.assertEqual(b["batch_calls"], 2)
        self.assertEqual(b["run_steps_calls"], 1)
        self.assertEqual(b["steps_in_run_steps"], 3)
        self.assertEqual(b["script_inner_calls_est"], 3)
        self.assertEqual(b["actions_total"], 8)
        self.assertEqual(b["batched_action_share"], round(6 / 8, 4))

    def test_empty(self):
        b = ce.batching_summary({})
        self.assertEqual(b["mcp_calls"], 0)
        self.assertIsNone(b["batched_action_share"])


if __name__ == "__main__":
    unittest.main()
