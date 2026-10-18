"""Hermetic tests for the conformance runner's refreshing Fleet bearer.

No network, no Fleet: the mint function and the clock are fakes, and the
`cua` package is stubbed so the runner module imports without its native
library.

    python -m pytest tests/e2e/cua-sdk/test_fleet_gateway_conformance_bearer.py
"""

from __future__ import annotations

import importlib.util
import stat
import sys
import types
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load_runner():
    sys.modules.setdefault("cua", types.ModuleType("cua"))
    spec = importlib.util.spec_from_file_location(
        "fleet_gateway_conformance", HERE / "fleet_gateway_conformance.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runner = _load_runner()


class FakeClock:
    def __init__(self) -> None:
        self.now = 1000.0

    def __call__(self) -> float:
        return self.now


def _minter(lifetime: int = 900):
    minted = []

    def mint():
        minted.append(f"bearer-{len(minted)}")
        return minted[-1], lifetime

    return mint, minted


def test_bearer_is_cached_until_80_percent_of_its_lifetime():
    clock = FakeClock()
    mint, minted = _minter(900)
    bearer = runner.RefreshingBearer(mint=mint, clock=clock)

    assert bearer.get() == "bearer-0"
    clock.now += 719  # just under 80% of 900 s
    assert bearer.get() == "bearer-0"
    assert len(minted) == 1

    clock.now += 1  # 720 s = 80%
    assert bearer.get() == "bearer-1"
    assert bearer.lifetime == 900


def test_unknown_lifetime_assumes_900_seconds():
    clock = FakeClock()
    mint, minted = _minter(0)
    bearer = runner.RefreshingBearer(mint=mint, clock=clock)
    bearer.get()
    clock.now += 700
    bearer.get()
    assert len(minted) == 1
    clock.now += 20
    bearer.get()
    assert len(minted) == 2


def test_bearer_file_is_private_rewritten_on_refresh_and_removed_on_stop():
    clock = FakeClock()
    mint, _ = _minter(900)
    bearer = runner.RefreshingBearer(mint=mint, clock=clock)
    path = bearer.start()
    try:
        assert path.read_text() == "bearer-0"
        assert stat.S_IMODE(path.stat().st_mode) == 0o600
        clock.now += 900
        bearer.get()
        assert path.read_text() == "bearer-1"
    finally:
        bearer.stop()
    assert not path.exists()
    assert not path.parent.exists()


def test_background_thread_refreshes_before_expiry():
    # Real clock, 2 s lifetime: the thread re-mints after ~1.6 s.
    mint, minted = _minter(2)
    bearer = runner.RefreshingBearer(mint=mint)
    path = bearer.start()
    try:
        deadline = runner.time.monotonic() + 10
        while len(minted) < 2 and runner.time.monotonic() < deadline:
            runner.time.sleep(0.1)
        assert len(minted) >= 2
        assert path.read_text() == minted[-1]
    finally:
        bearer.stop()
