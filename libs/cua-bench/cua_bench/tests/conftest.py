"""Hermetic defaults for every cua-bench test.

``cb`` follows the user's placement defaults (``CUA_DEFAULT_ON`` /
``CUA_DEFAULT_KIND`` / ``CUA_DEFAULT_RUNTIME``, then ``$CUA_HOME/config.toml``).
Outside ``tests/live`` they come from an empty temp ``CUA_HOME`` and a clean
environment, so a developer's ``cua config set default.on cloud`` never
changes what a unit test sees, and no test reads or writes the real ``~/.cua``.
"""

from __future__ import annotations

import os

import pytest

# Tests must never send real telemetry.
os.environ["CUA_TELEMETRY"] = "0"

_LIVE_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "live") + os.sep
_DEFAULT_VARS = (
    "CUA_DEFAULT_ON",
    "CUA_DEFAULT_KIND",
    "CUA_DEFAULT_RUNTIME",
    "CUA_FLEET_WARM",
    "CUA_BENCH_IMAGE",
)


@pytest.fixture(autouse=True)
def _hermetic_placement_defaults(request, monkeypatch, tmp_path_factory):
    if str(request.node.path).startswith(_LIVE_DIR):
        return
    for name in _DEFAULT_VARS:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("CUA_HOME", str(tmp_path_factory.mktemp("cua-home")))
    # Fake sessions draw a flat PNG: no desktop to wait for (test_desktop_wait
    # covers the wait itself).
    monkeypatch.setenv("CUA_BENCH_DESKTOP_READY_S", "0")
