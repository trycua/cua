"""A fake Fleet API for the native pool manager (``cua-test-fixtures``)."""

from __future__ import annotations

import importlib
import json
import os
import subprocess
from pathlib import Path

import pytest
from cua_sandbox import Sandbox, _autopool, sandbox_state
from cua_sandbox.pool import _FleetClient

sandbox_module = importlib.import_module("cua_sandbox.sandbox")


def _fixtures_binary() -> Path | None:
    if os.environ.get("CUA_TEST_FIXTURES"):
        return Path(os.environ["CUA_TEST_FIXTURES"])
    root = Path(__file__).resolve().parents[3] / "cua" / "target"
    for profile in ("debug", "release"):
        candidate = root / profile / "cua-test-fixtures"
        if candidate.exists():
            return candidate
    return None


@pytest.fixture
def fleet(monkeypatch, tmp_path):
    """A fresh fake Fleet API per test; yields cua-sandbox's Fleet client."""
    binary = _fixtures_binary()
    if binary is None:
        pytest.skip("cua-test-fixtures is not built")
    proc = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("cua-test-fixtures exited before printing endpoints")
        endpoints = json.loads(line)
        monkeypatch.setenv("CUA_FLEET_BASE_URL", endpoints["fleet_base_url"])
        monkeypatch.setenv("FLEETS_TOKEN", endpoints["fleet_token"])
        for variable in (
            "CUA_FLEET_MAX_POOL_SIZE",
            "CUA_FLEET_CLAIM_TTL",
            "CUA_FLEET_WARM",
            "CUA_CLIENT_ID",
            "CUA_CLIENT_SECRET",
        ):
            monkeypatch.delenv(variable, raising=False)
        # The native manager's automatic GC stays out of these tests.
        monkeypatch.setenv("CUA_FLEET_POOL_IDLE_GC", "off")
        monkeypatch.setattr(_autopool, "CUA_DIR", tmp_path / "cua")
        monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path / "sandboxes")
        monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: api_key is None))
        monkeypatch.setattr(sandbox_module, "_TELEMETRY_AVAILABLE", False)
        yield _FleetClient
    finally:
        proc.stdin.close()
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()
