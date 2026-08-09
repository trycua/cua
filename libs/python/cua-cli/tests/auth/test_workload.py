"""Tests for workload-token authentication."""

from cua_cli.auth.workload import get_fleets_token


def test_get_fleets_token_trims_value(monkeypatch):
    monkeypatch.setenv("FLEETS_TOKEN", "  github-token  ")

    assert get_fleets_token() == "github-token"


def test_get_fleets_token_ignores_blank_value(monkeypatch):
    monkeypatch.setenv("FLEETS_TOKEN", "  \t ")

    assert get_fleets_token() is None
