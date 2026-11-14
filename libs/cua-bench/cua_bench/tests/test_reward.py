"""Tests for structured reward extraction (cua_bench.reward)."""

import pytest

from cua_bench.reward import (
    DEFAULT_SUCCESS_THRESHOLD,
    coerce_reward,
    emit_result_envelope,
    parse_reward,
    try_parse_reward,
)


def test_default_success_threshold_is_half():
    assert DEFAULT_SUCCESS_THRESHOLD == 0.5


def test_emit_result_envelope_round_trips():
    line = emit_result_envelope(0.8)
    assert line == 'CUA_BENCH_RESULT {"reward": 0.8}'
    assert try_parse_reward(line) == pytest.approx(0.8)


def test_emit_result_envelope_round_trips_alongside_legacy_line():
    stdout = "\n".join(
        [
            "✓ Evaluation result: [0.8]",
            emit_result_envelope(0.8),
        ]
    )
    assert try_parse_reward(stdout) == pytest.approx(0.8)


def test_emit_result_envelope_preferred_over_later_legacy_line():
    stdout = "\n".join(
        [
            emit_result_envelope(0.25),
            "✓ Evaluation result: [0.9]",
        ]
    )
    assert try_parse_reward(stdout) == pytest.approx(0.25)


@pytest.mark.parametrize(
    "result,expected",
    [
        ([0.8], 0.8),
        (0.6, 0.6),
        ({"reward": 0.4}, 0.4),
        ({"score": 0.9, "success": True}, 0.9),
        ([], None),
        (True, None),
    ],
)
def test_coerce_reward(result, expected):
    value = coerce_reward(result)
    if expected is None:
        assert value is None
    else:
        assert value == pytest.approx(expected)


@pytest.mark.parametrize(
    "stdout,expected",
    [
        ("✓ Evaluation result: [0.8]", 0.8),
        ("✓ Evaluation result: 0.25", 0.25),
        ("some log line\nEvaluation result: [1.0]\nmore", 1.0),
        ('CUA_BENCH_RESULT {"reward": 0.91}', 0.91),
        ('noise\nCUA_BENCH_RESULT {"score": 1.0}\nmore', 1.0),
        ('CUA_BENCH_RESULT: {"success": true, "score": 0.3}', 0.3),
    ],
)
def test_parse_reward_success(stdout, expected):
    assert parse_reward(stdout) == pytest.approx(expected)


def test_structured_envelope_preferred_over_text():
    stdout = 'Evaluation result: [0.2]\nCUA_BENCH_RESULT {"reward": 0.9}'
    assert parse_reward(stdout) == pytest.approx(0.9)


def test_last_structured_envelope_wins():
    stdout = "\n".join(
        [
            'CUA_BENCH_RESULT {"reward": 0.1}',
            'CUA_BENCH_RESULT {"reward": 0.7}',
        ]
    )
    assert parse_reward(stdout) == pytest.approx(0.7)


def test_unrelated_json_score_does_not_override_evaluator_result():
    stdout = "\n".join(
        [
            'tool_call: {"score": 0.01, "note": "model confidence"}',
            'ocr: {"score": 0.98}',
            "✓ Evaluation result: [0.8]",
        ]
    )
    assert parse_reward(stdout) == pytest.approx(0.8)


def test_unrelated_json_reward_does_not_override_evaluator_result():
    stdout = "\n".join(
        [
            'provider response {"reward": 1234.0}',
            "Evaluation result: 0.6",
        ]
    )
    assert parse_reward(stdout) == pytest.approx(0.6)


def test_marker_without_reward_falls_back_to_text():
    stdout = 'CUA_BENCH_RESULT {"status": "ok"}\nEvaluation result: [0.4]'
    assert parse_reward(stdout) == pytest.approx(0.4)


@pytest.mark.parametrize(
    "stdout",
    [
        "",
        "no reward here",
        '{"reward": 0.9}',
        '{"evaluation": {"score": 1.0}}',
        '{"result": [{"score": 0.3}]}',
        "CUA_BENCH_RESULT not-json",
    ],
)
def test_parse_reward_raises_when_absent(stdout):
    with pytest.raises(ValueError):
        parse_reward(stdout)


def test_try_parse_reward_returns_none_when_absent():
    assert try_parse_reward("no reward here") is None
    assert try_parse_reward("✓ Evaluation result: [0.5]") == pytest.approx(0.5)
