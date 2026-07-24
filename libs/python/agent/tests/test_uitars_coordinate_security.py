"""Security regression tests for model-supplied UITARS coordinates."""

import importlib

import pytest

uitars = importlib.import_module("cua_agent.loops.uitars")

_EXECUTED = {"value": False}


def _payload():
    _EXECUTED["value"] = True
    return (0.1, 0.1, 0.1, 0.1)


@pytest.fixture(autouse=True)
def reset_execution_sentinel():
    _EXECUTED["value"] = False
    yield
    _EXECUTED["value"] = False


def convert_model_action(action: str):
    parsed = uitars.parse_uitars_response(f"Action: {action}", image_width=1000, image_height=1000)
    return uitars.convert_to_computer_actions(parsed, image_width=1000, image_height=1000)


@pytest.mark.parametrize(
    "action",
    [
        'click(start_box="_payload()")',
        'left_double(start_box="_payload()")',
        'right_single(start_box="_payload()")',
        'scroll(start_box="_payload()", direction="down")',
        'drag(start_box="_payload()", end_box="[0.1, 0.1, 0.1, 0.1]")',
        'drag(start_box="[0.1, 0.1, 0.1, 0.1]", end_box="_payload()")',
    ],
)
def test_model_coordinate_expression_is_rejected(action, monkeypatch):
    monkeypatch.setattr(uitars, "_payload", _payload, raising=False)

    assert convert_model_action(action) == []
    assert _EXECUTED["value"] is False


@pytest.mark.parametrize(
    "box",
    [
        "1 + 1",
        "[1 + 1, 2, 3, 4]",
        "__import__('os').getcwd()",
        "_payload()",
        "[True, 0.2, 0.3, 0.4]",
        "[0.1, 0.2]",
        "[0.1, 0.2, 0.3, 0.4, 0.5]",
        "[float('nan'), 0.2, 0.3, 0.4]",
        "[1e1000, 0.2, 0.3, 0.4]",
    ],
)
def test_coordinate_parser_rejects_non_finite_or_non_numeric_boxes(box):
    assert uitars.parse_box_coordinates(box) is None


@pytest.mark.parametrize(
    ("box", "expected"),
    [
        ("[0.1, 0.2, 0.3, 0.4]", (0.1, 0.2, 0.3, 0.4)),
        ("(0, 1, 2, 3)", (0.0, 1.0, 2.0, 3.0)),
    ],
)
def test_coordinate_parser_accepts_finite_four_number_sequences(box, expected):
    assert uitars.parse_box_coordinates(box) == expected


def test_valid_model_coordinate_still_produces_click():
    actions = convert_model_action('click(start_box="(500, 500)")')

    assert len(actions) == 1
    assert actions[0]["type"] == "computer_call"
    assert actions[0]["action"]["x"] == 500
    assert actions[0]["action"]["y"] == 500
