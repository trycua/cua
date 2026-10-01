"""Tests for structured reward extraction (cua_bench.reward)."""

import pytest

from cua_bench.reward import (
    DEFAULT_SUCCESS_THRESHOLD,
    parse_reward,
    try_parse_reward,
)


def test_default_success_threshold_is_half():
    assert DEFAULT_SUCCESS_THRESHOLD == 0.5


@pytest.mark.parametrize(
    "stdout,expected",
    [
        ("✓ Evaluation result: [0.8]", 0.8),
        ("✓ Evaluation result: 0.25", 0.25),
        ("some log line\nEvaluation result: [1.0]\nmore", 1.0),
        ("prefix {\"reward\": 0.91} suffix", 0.91),
        ("{\"evaluation\": {\"score\": 1.0}}", 1.0),
        ("{\"result\": [{\"score\": 0.3}]}", 0.3),
    ],
)
def test_parse_reward_success(stdout, expected):
    assert parse_reward(stdout) == pytest.approx(expected)


def test_json_envelope_preferred_over_text():
    stdout = "Evaluation result: [0.2]\n{\"reward\": 0.9}"
    assert parse_reward(stdout) == pytest.approx(0.9)


@pytest.mark.parametrize("stdout", ["", "no reward here", "{\"other\": 1}"])
def test_parse_reward_raises_when_absent(stdout):
    with pytest.raises(ValueError):
        parse_reward(stdout)


def test_try_parse_reward_returns_none_when_absent():
    assert try_parse_reward("no reward here") is None
    assert try_parse_reward("✓ Evaluation result: [0.5]") == pytest.approx(0.5)
