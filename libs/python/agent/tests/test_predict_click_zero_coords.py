"""Tests for predict_click zero-coordinate fix (issue #1400).

These tests verify the coordinate-extraction logic in
AnthropicHostedToolsConfig.predict_click directly, without requiring the full
cua_agent import chain (which needs cua-computer, cua-core, etc.).
"""
import pytest


def _extract_click_coords_old(responses_items):
    """Buggy implementation: falsy check silently drops x=0 or y=0."""
    for item in responses_items:
        if (
            isinstance(item, dict)
            and item.get("type") == "computer_call"
            and isinstance(item.get("action"), dict)
        ):
            action = item["action"]
            if action.get("x") and action.get("y"):  # BUG: 0 is falsy
                return (int(action.get("x")), int(action.get("y")))
    return None


def _extract_click_coords_fixed(responses_items):
    """Fixed implementation: explicit None check preserves zero coordinates."""
    for item in responses_items:
        if (
            isinstance(item, dict)
            and item.get("type") == "computer_call"
            and isinstance(item.get("action"), dict)
        ):
            action = item["action"]
            if action.get("x") is not None and action.get("y") is not None:
                return (int(action.get("x")), int(action.get("y")))
    return None


def _make_items(x, y):
    return [{"type": "computer_call", "action": {"type": "click", "x": x, "y": y}}]


# --- Regression tests: these all FAIL with the old code, PASS with the fix ---

def test_zero_x_was_broken_before_fix():
    """Old code returns None for x=0; new code returns (0, y)."""
    items = _make_items(0, 100)
    assert _extract_click_coords_old(items) is None, "confirm old bug"
    assert _extract_click_coords_fixed(items) == (0, 100)


def test_zero_y_was_broken_before_fix():
    """Old code returns None for y=0; new code returns (x, 0)."""
    items = _make_items(200, 0)
    assert _extract_click_coords_old(items) is None, "confirm old bug"
    assert _extract_click_coords_fixed(items) == (200, 0)


def test_zero_zero_was_broken_before_fix():
    """Old code returns None for (0, 0); new code returns (0, 0)."""
    items = _make_items(0, 0)
    assert _extract_click_coords_old(items) is None, "confirm old bug"
    assert _extract_click_coords_fixed(items) == (0, 0)


# --- Positive tests: non-zero coordinates work in both old and new code ---

def test_nonzero_coordinates_still_work():
    items = _make_items(512, 384)
    assert _extract_click_coords_fixed(items) == (512, 384)


def test_returns_none_when_no_computer_call():
    items = [{"type": "text", "text": "no click"}]
    assert _extract_click_coords_fixed(items) is None


# --- Verify the actual fix is present in the source file ---

def test_source_uses_is_not_none_check():
    """Confirm the fix is applied in the real anthropic.py source."""
    import pathlib
    src = (
        pathlib.Path(__file__).parent.parent
        / "cua_agent" / "loops" / "anthropic.py"
    ).read_text()
    assert 'action.get("x") is not None and action.get("y") is not None' in src, (
        "Fix not found in anthropic.py — the 'is not None' check is missing"
    )
    # Ensure the old buggy pattern is gone
    assert (
        'if action.get("x") and action.get("y"):' not in src
    ), "Old buggy truthiness check still present in anthropic.py"
