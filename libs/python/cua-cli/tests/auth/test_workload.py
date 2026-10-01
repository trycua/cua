"""Tests for workload-token authentication."""

from pathlib import Path
from unittest.mock import AsyncMock, patch

import cua_sandbox
from cua_cli.auth.workload import get_fleets_token
from cua_cli.commands import sandbox as sandbox_commands
from cua_sandbox import Sandbox
from cua_sandbox import _config as sandbox_config


def test_get_fleets_token_trims_value(monkeypatch):
    monkeypatch.setenv("FLEETS_TOKEN", "  github-token  ")

    assert get_fleets_token() == "github-token"


def test_get_fleets_token_ignores_blank_value(monkeypatch):
    monkeypatch.setenv("FLEETS_TOKEN", "  \t ")

    assert get_fleets_token() is None


async def test_cli_wif_uses_local_fleet_sandbox_without_legacy_key(monkeypatch):
    config = sandbox_config._global_config
    original_config = vars(config).copy()
    try:
        for field in ("api_key", "client_id", "client_secret", "fleet_token"):
            if hasattr(config, field):
                setattr(config, field, None)
        for variable in (
            "CUA_API_KEY",
            "CUA_CLIENT_ID",
            "CUA_CLIENT_SECRET",
            "CUA_FLEET_CLIENT_ID",
            "CUA_FLEET_CLIENT_SECRET",
        ):
            monkeypatch.delenv(variable, raising=False)
        monkeypatch.setenv("FLEETS_TOKEN", "integration-token")

        assert Sandbox._uses_fleet(None) is True
        local_package = Path(__file__).resolve().parents[3] / "cua-sandbox" / "cua_sandbox"
        assert Path(cua_sandbox.__file__).resolve().is_relative_to(local_package)
        with patch.object(
            sandbox_commands, "get_access_token", new_callable=AsyncMock
        ) as get_access_token:
            assert await sandbox_commands._cloud_auth_kwargs() == {}
        get_access_token.assert_not_awaited()
    finally:
        vars(config).clear()
        vars(config).update(original_config)
