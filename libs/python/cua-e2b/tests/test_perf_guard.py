"""Shim overhead guard: the hermetic figures stay within 1.5x of the committed
baseline (`perf/baseline.json`). The live comparison against the direct SDK on
a real Space is `python -m perf.overhead --live` (nightly lane / a Mac Studio (M2 Max))."""

import asyncio
import os

import pytest
from perf import overhead


@pytest.mark.perf
@pytest.mark.skipif(
    not os.environ.get("CUA_E2B_PERF"),
    reason="timing guard: set CUA_E2B_PERF=1 on a quiet machine (the hermetic CI job and the nightly lane do)",
)
def test_translation_overhead_is_within_1_5x_of_the_baseline():
    import json

    measured = overhead.best_of()
    baseline = json.loads(overhead.BASELINE.read_text())
    assert set(baseline) <= set(measured)
    assert overhead.check(measured, baseline) == []


def test_the_threshold_check_flags_a_regression():
    assert overhead.check({"x": 0.05}, {"x": 0.01}) == ["x: 0.05 units > 1.5x baseline 0.01"]
    assert overhead.check({"x": 0.0149}, {"x": 0.01}) == []
    assert overhead.check({"x": 0.003}, {"x": 0.0001}) == []  # timer noise
