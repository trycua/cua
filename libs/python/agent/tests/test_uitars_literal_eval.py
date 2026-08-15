"""Tests for safe coordinate parsing in convert_to_computer_actions.

Verifies that eval() has been replaced with ast.literal_eval() across all
action handlers in uitars.py — click, double_click, right_click, scroll, drag.
"""

import ast
import pathlib
import re

import pytest

IMAGE_W, IMAGE_H = 1000, 800


def _coords_to_xy(box_str: str, width: int = IMAGE_W, height: int = IMAGE_H):
    """Replicate the midpoint-to-pixel conversion used in uitars.py."""
    coords = ast.literal_eval(box_str)
    x = int((coords[0] + coords[2]) / 2 * width)
    y = int((coords[1] + coords[3]) / 2 * height)
    return x, y


def test_click_midpoint_calculation():
    x, y = _coords_to_xy("[0.1, 0.2, 0.3, 0.4]")
    assert x == int(0.2 * IMAGE_W)
    assert y == int(0.3 * IMAGE_H)


def test_full_screen_box_gives_center():
    x, y = _coords_to_xy("[0.0, 0.0, 1.0, 1.0]")
    assert x == int(0.5 * IMAGE_W)
    assert y == int(0.5 * IMAGE_H)


def test_literal_eval_rejects_arbitrary_code():
    """The old eval() would execute this; literal_eval must raise."""
    with pytest.raises((ValueError, SyntaxError)):
        ast.literal_eval("__import__('os').system('id')")


def test_literal_eval_rejects_function_calls():
    with pytest.raises((ValueError, SyntaxError)):
        ast.literal_eval("[print('pwned'), 0, 1, 1]")


def test_literal_eval_accepts_list_of_floats():
    result = ast.literal_eval("[0.25, 0.1, 0.75, 0.9]")
    assert result == [0.25, 0.1, 0.75, 0.9]


def test_drag_start_and_end_boxes():
    start_x, start_y = _coords_to_xy("[0.0, 0.0, 0.2, 0.2]")
    end_x, end_y = _coords_to_xy("[0.8, 0.8, 1.0, 1.0]")
    assert start_x < end_x
    assert start_y < end_y


_UITARS = (
    pathlib.Path(__file__).parent.parent.parent.parent
    / "libs/python/agent/cua_agent/loops/uitars.py"
)


@pytest.mark.skipif(not _UITARS.exists(), reason="uitars.py not on path")
def test_source_uses_literal_eval_not_eval():
    src = _UITARS.read_text()
    assert "ast.literal_eval" in src, "ast.literal_eval not found in uitars.py"
    bare_eval_calls = re.findall(r"(?<!\w)eval\(", src)
    assert bare_eval_calls == [], f"bare eval() still present: {bare_eval_calls}"


@pytest.mark.skipif(not _UITARS.exists(), reason="uitars.py not on path")
def test_source_imports_ast():
    src = _UITARS.read_text()
    assert "import ast" in src, "ast module not imported in uitars.py"
