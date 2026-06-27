"""Regression tests for the UITARS coordinate parser code-execution fix.

The UITARS agent loop turns a model-emitted ``Action:`` string into computer
actions. Coordinate boxes (``start_box``/``end_box``) are data only -- a numeric
tuple/list such as ``"[0.1, 0.2, 0.1, 0.2]"``. Previously they were passed to a
bare ``eval()`` in ``convert_to_computer_actions``; when float normalization in
``parse_uitars_response`` failed (a ``ValueError``) the *raw* model string was
kept and later evaluated, so a hijacked / malicious UI-TARS model could run
arbitrary Python in the agent host process.

These tests exercise the real ``parse_uitars_response`` +
``convert_to_computer_actions`` chain with non-numeric coordinate strings and
assert the expression is treated as data (rejected), never executed. They FAIL
on the unpatched code (the expression is eval'd) and PASS once box parsing uses
a literal/numeric-only parser.
"""

import importlib

import pytest

uitars = importlib.import_module("cua_agent.loops.uitars")


# A module-level flag flipped only if a payload is *executed* as Python.
_EXECUTED = {"hit": False}


def _mark_executed():
    _EXECUTED["hit"] = True
    return (0.1, 0.1, 0.1, 0.1)


@pytest.fixture(autouse=True)
def _reset_executed():
    _EXECUTED["hit"] = False
    yield
    _EXECUTED["hit"] = False


def _action(text: str):
    """Run the full model-text -> computer-action chain for one Action string."""
    parsed = uitars.parse_uitars_response(text, image_width=1000, image_height=1000)
    return uitars.convert_to_computer_actions(parsed, image_width=1000, image_height=1000)


@pytest.mark.parametrize(
    "action_type,template",
    [
        ("click", 'Action: click(start_box="{box}")'),
        ("double_click", 'Action: double_click(start_box="{box}")'),
        ("right_click", 'Action: right_click(start_box="{box}")'),
    ],
)
def test_single_box_expression_is_not_executed(action_type, template, monkeypatch):
    """A non-numeric coordinate expression must not be evaluated as Python.

    On the unpatched code ``eval("(1+1, ...)")`` runs the arithmetic and yields a
    valid click; with the fix the box is rejected as non-numeric data and no
    code runs.
    """
    # Expose a callable in the parser's namespace; if the box string were eval'd
    # this call would fire and flip the sentinel.
    monkeypatch.setattr(uitars, "_payload", _mark_executed, raising=False)
    box = "_payload()"
    actions = _action(template.format(box=box))
    assert _EXECUTED["hit"] is False, (
        f"{action_type}: model-supplied box string was executed as Python (eval)"
    )
    # And the malicious box must not produce a coordinate action.
    assert actions == [], f"{action_type}: non-numeric box should yield no action"


def test_scroll_box_expression_is_not_executed(monkeypatch):
    """Scroll evaluates start_box too; the expression must not run.

    Unlike click, scroll degrades to a centered scroll when the box is invalid,
    so the security assertion is purely that no code was executed and the
    coordinates are the image center -- not values derived from the expression.
    """
    monkeypatch.setattr(uitars, "_payload", _mark_executed, raising=False)
    actions = _action('Action: scroll(start_box="_payload()", direction="down")')
    assert _EXECUTED["hit"] is False, "scroll: model-supplied box string was executed (eval)"
    assert len(actions) == 1 and actions[0]["type"] == "computer_call"
    # Centered fallback for a 1000x1000 image, never an eval'd coordinate.
    assert actions[0]["action"]["x"] == 500
    assert actions[0]["action"]["y"] == 500


def test_drag_box_expressions_are_not_executed(monkeypatch):
    """The drag path evaluates both start_box and end_box -- neither may run."""
    monkeypatch.setattr(uitars, "_payload", _mark_executed, raising=False)
    text = 'Action: drag(start_box="_payload()", end_box="_payload()")'
    actions = _action(text)
    assert _EXECUTED["hit"] is False, "drag: model-supplied box string was executed (eval)"
    assert actions == [], "drag: non-numeric boxes should yield no drag action"


def test_arithmetic_box_is_not_computed_into_coordinates():
    """Arithmetic in a box must not be silently computed (the PoC payload)."""
    actions = _action('Action: click(start_box="(1+1, 1+1, 1+1, 1+1)")')
    # Unpatched: eval computes (2,2,2,2) -> a click at (2,2). Fixed: rejected.
    assert actions == [], "arithmetic coordinate expression was evaluated"


def test_parse_box_coordinates_rejects_expressions():
    """The shared helper accepts numeric literals and rejects everything else."""
    assert uitars.parse_box_coordinates("[0.1, 0.2, 0.3, 0.4]") == [0.1, 0.2, 0.3, 0.4]
    assert uitars.parse_box_coordinates("(10, 20)") == [10.0, 20.0]
    for bad in ("1+1", "(1+1, 2)", "__import__('os').getcwd()", "_payload()", "[a, b]", "True"):
        assert uitars.parse_box_coordinates(bad) is None, f"expected reject: {bad!r}"


def test_valid_numeric_box_still_clicks():
    """Backward-compat: a well-formed numeric box still produces a click action."""
    actions = _action('Action: click(start_box="(500, 500)")')
    assert len(actions) == 1
    assert actions[0]["type"] == "computer_call"
    # 500/1000 normalized -> center at 500px on a 1000px image.
    assert actions[0]["action"]["x"] == 500
    assert actions[0]["action"]["y"] == 500
