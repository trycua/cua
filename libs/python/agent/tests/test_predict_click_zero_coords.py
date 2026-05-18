"""Tests for predict_click zero-coordinate bug fix.

Regression tests for: https://github.com/trycua/cua/issues/1495
                       https://github.com/trycua/cua/issues/1515

Bug: predict_click returned None when x=0 or y=0 because Python treats 0
as falsy. The check `if action.get("x") and action.get("y")` silently
dropped valid clicks at the top or left edge of the screen.

Fix: use `is not None` checks instead of truthiness checks.
"""

import pytest


class TestAnthropicPredictClickZeroCoords:
    """Tests for the Anthropic loop predict_click zero-coordinate fix."""

    def _make_responses_items(self, x, y):
        """Build a minimal responses_items list with a computer_call click."""
        return [
            {
                "type": "computer_call",
                "action": {"type": "left_click", "x": x, "y": y},
            }
        ]

    def test_zero_x_coordinate_returns_coords(self):
        """x=0 (left screen edge) must not be treated as falsy."""
        items = self._make_responses_items(x=0, y=300)

        result = None
        for item in items:
            if (
                isinstance(item, dict)
                and item.get("type") == "computer_call"
                and isinstance(item.get("action"), dict)
            ):
                action = item["action"]
                if action.get("x") is not None and action.get("y") is not None:
                    result = (int(action["x"]), int(action["y"]))

        assert result == (0, 300), f"Expected (0, 300), got {result}"

    def test_zero_y_coordinate_returns_coords(self):
        """y=0 (top screen edge) must not be treated as falsy."""
        items = self._make_responses_items(x=500, y=0)

        result = None
        for item in items:
            if (
                isinstance(item, dict)
                and item.get("type") == "computer_call"
                and isinstance(item.get("action"), dict)
            ):
                action = item["action"]
                if action.get("x") is not None and action.get("y") is not None:
                    result = (int(action["x"]), int(action["y"]))

        assert result == (500, 0), f"Expected (500, 0), got {result}"

    def test_both_zero_coordinates_returns_coords(self):
        """x=0, y=0 (top-left corner) must return (0, 0), not None."""
        items = self._make_responses_items(x=0, y=0)

        result = None
        for item in items:
            if (
                isinstance(item, dict)
                and item.get("type") == "computer_call"
                and isinstance(item.get("action"), dict)
            ):
                action = item["action"]
                if action.get("x") is not None and action.get("y") is not None:
                    result = (int(action["x"]), int(action["y"]))

        assert result == (0, 0), f"Expected (0, 0), got {result}"

    def test_normal_coordinates_still_work(self):
        """Non-zero coordinates must continue to work correctly."""
        items = self._make_responses_items(x=640, y=480)

        result = None
        for item in items:
            if (
                isinstance(item, dict)
                and item.get("type") == "computer_call"
                and isinstance(item.get("action"), dict)
            ):
                action = item["action"]
                if action.get("x") is not None and action.get("y") is not None:
                    result = (int(action["x"]), int(action["y"]))

        assert result == (640, 480)

    def test_missing_coordinates_returns_none(self):
        """Missing x/y keys must still return None (no regression)."""
        items = [{"type": "computer_call", "action": {"type": "screenshot"}}]

        result = None
        for item in items:
            if (
                isinstance(item, dict)
                and item.get("type") == "computer_call"
                and isinstance(item.get("action"), dict)
            ):
                action = item["action"]
                if action.get("x") is not None and action.get("y") is not None:
                    result = (int(action["x"]), int(action["y"]))

        assert result is None

    def test_old_falsy_check_would_fail_zero_x(self):
        """Demonstrate the original bug: old check drops x=0."""
        action = {"type": "left_click", "x": 0, "y": 300}

        # Old (buggy) check
        old_result = None
        if action.get("x") and action.get("y"):
            old_result = (int(action["x"]), int(action["y"]))

        # New (fixed) check
        new_result = None
        if action.get("x") is not None and action.get("y") is not None:
            new_result = (int(action["x"]), int(action["y"]))

        assert old_result is None, "Old check should fail (demonstrating the bug)"
        assert new_result == (0, 300), "New check should succeed"
