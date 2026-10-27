"""Unit tests of check.py (no Spaces): `python3 -m pytest test_check.py`."""
from check import check

GOOD = {"desktop_create_p50_ms": 3000, "headless_create_p50_ms": 200, "headless_b16_wall_ms": 1800,
        "create_failures": 0, "idle_spacesd_cpu_pct": 0, "rollout_steps_per_sec": 350,
        "rollout_errors": 0, "pool_claim_batch_p50_ms": 90}


def test_good_numbers_pass():
    assert check(GOOD) == []
    assert check(GOOD, dict(GOOD)) == []


def test_each_invariant_fails():
    assert check(dict(GOOD, create_failures=3))
    assert check(dict(GOOD, rollout_errors=1))
    assert check(dict(GOOD, headless_create_p50_ms=2000))
    assert check(dict(GOOD, idle_spacesd_cpu_pct=9))
    assert check(dict(GOOD, pool_claim_batch_p50_ms=700))
    assert check({k: v for k, v in GOOD.items() if k != "rollout_steps_per_sec"})


def test_baseline_threshold():
    assert check(dict(GOOD, headless_b16_wall_ms=2800), GOOD), "1.55x slower"
    assert not check(dict(GOOD, headless_b16_wall_ms=2600), GOOD), "1.44x is within"
    assert check(dict(GOOD, rollout_steps_per_sec=200), GOOD)
