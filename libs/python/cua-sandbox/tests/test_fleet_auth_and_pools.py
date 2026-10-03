"""fleet_auth_source() and the public pools module (no network, no vault)."""

from __future__ import annotations

import cua_sandbox
import pytest
from cua_sandbox import _config


@pytest.fixture
def clean_env(monkeypatch):
    for var in ("FLEETS_TOKEN", "CUA_CLIENT_ID", "CUA_CLIENT_SECRET"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setattr(_config._global_config, "fleet_token", None)
    monkeypatch.setattr(_config._global_config, "client_id", None)
    monkeypatch.setattr(_config._global_config, "client_secret", None)
    monkeypatch.setattr(_config, "has_fleet_session", lambda: False)
    return monkeypatch


def test_token_wins(clean_env):
    clean_env.setenv("FLEETS_TOKEN", "t")
    clean_env.setenv("CUA_CLIENT_ID", "id")
    clean_env.setenv("CUA_CLIENT_SECRET", "s")
    assert cua_sandbox.fleet_auth_source() == "FLEETS_TOKEN"


def test_client_credentials_then_session(clean_env):
    clean_env.setenv("CUA_CLIENT_ID", "id")
    assert cua_sandbox.fleet_auth_source() is None  # secret missing, no session
    clean_env.setenv("CUA_CLIENT_SECRET", "s")
    assert cua_sandbox.fleet_auth_source() == "client credentials"


def test_session_only_when_nothing_else(clean_env):
    looked = []
    clean_env.setattr(_config, "has_fleet_session", lambda: looked.append(1) or True)
    assert cua_sandbox.fleet_auth_source() == "cua auth login session"
    clean_env.setenv("FLEETS_TOKEN", "t")
    assert cua_sandbox.fleet_auth_source() == "FLEETS_TOKEN"
    assert looked == [1]  # the stored session is not read when a token is set


def test_pools_module_is_public():
    from cua_sandbox import pools

    for name in ("list_pools", "list_claims", "gc", "gc_pools", "GcReport", "ManagedPoolInfo"):
        assert hasattr(pools, name)
    assert pools.is_managed_pool_name("cua-auto-abc")


def test_marker_only_mode_never_reads_the_vault(clean_env):
    clean_env.setattr(_config, "has_fleet_session", lambda: pytest.fail("vault read"))
    clean_env.setattr(_config, "may_have_fleet_session", lambda: True)
    assert cua_sandbox.fleet_auth_source(read_session=False) == "cua auth login session"
    clean_env.setattr(_config, "may_have_fleet_session", lambda: False)
    assert cua_sandbox.fleet_auth_source(read_session=False) is None
