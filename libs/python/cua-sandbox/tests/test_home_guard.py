"""The host-safety guard: a test cannot write the user's real ~/.cua."""

from __future__ import annotations

import pytest
from cua_sandbox import _paths, sandbox_state


def test_pytest_is_a_test_process():
    assert _paths.is_test_process()


def test_a_state_write_to_the_real_home_is_refused(monkeypatch):
    real = _paths.real_cua_home()
    if real is None:
        pytest.skip("no account home")
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", real / "sandboxes")
    before = (real / "sandboxes" / "cua-home-guard-probe.json").exists()
    with pytest.raises(PermissionError, match="CUA_HOME"):
        sandbox_state.save_fleet_claim("cua-home-guard-probe", "pool")
    assert (real / "sandboxes" / "cua-home-guard-probe.json").exists() == before


def test_a_temp_home_is_allowed(tmp_path, monkeypatch):
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    sandbox_state.save_fleet_claim("ok", "pool")
    assert (tmp_path / "sandboxes" / "ok.json").exists()
