"""Unit tests for config and auth modules."""

import os

from cua_sandbox._config import (
    _global_config,
    configure,
    get_api_key,
    get_base_url,
    get_client_id,
    get_client_secret,
    get_fleet_base_url,
    get_fleet_token,
    get_token_url,
    has_fleet_auth,
)


class TestConfig:
    def setup_method(self):
        _global_config.api_key = None
        _global_config.base_url = "https://api.cua.ai"
        _global_config.fleet_base_url = "https://run.cua.ai"
        _global_config.token_url = (
            "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
        )
        _global_config.client_id = None
        _global_config.client_secret = None
        _global_config.fleet_token = None
        os.environ.pop("FLEETS_TOKEN", None)
        os.environ.pop("CUA_CLIENT_ID", None)
        os.environ.pop("CUA_CLIENT_SECRET", None)

    def test_configure_client_credentials_uses_fleet_defaults(self):
        configure(client_id="client-id", client_secret="client-secret")

        assert get_client_id() == "client-id"
        assert get_client_secret() == "client-secret"
        assert get_base_url() == "https://api.cua.ai"
        assert get_fleet_base_url() == "https://run.cua.ai"
        assert (
            get_token_url() == "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
        )

    def test_configure_api_key(self):
        configure(api_key="sk-test-123")
        assert get_api_key() == "sk-test-123"

    def test_configure_base_url(self):
        configure(base_url="http://localhost:9000")
        assert get_base_url() == "http://localhost:9000"

    def test_override_takes_priority(self):
        configure(api_key="sk-global")
        assert get_api_key("sk-override") == "sk-override"

    def test_env_var_fallback(self, monkeypatch):
        monkeypatch.setenv("CUA_API_KEY", "sk-env")
        assert get_api_key() == "sk-env"

    def test_configure_overrides_env(self, monkeypatch):
        monkeypatch.setenv("CUA_API_KEY", "sk-env")
        configure(api_key="sk-configured")
        assert get_api_key() == "sk-configured"

    def test_get_fleet_token_prefers_configured_token(self, monkeypatch):
        monkeypatch.setenv("FLEETS_TOKEN", " env-token ")
        configure(fleet_token=" configured-token ")

        assert get_fleet_token() == "configured-token"

    def test_get_fleet_token_uses_trimmed_environment_token(self, monkeypatch):
        monkeypatch.setenv("FLEETS_TOKEN", " env-token ")

        assert get_fleet_token() == "env-token"

    def test_get_fleet_token_treats_blank_configured_token_as_unset(self, monkeypatch):
        monkeypatch.setenv("FLEETS_TOKEN", "env-token")
        configure(fleet_token=" \t ")

        assert get_fleet_token() == "env-token"

    def test_get_fleet_token_treats_blank_environment_token_as_unset(self, monkeypatch):
        monkeypatch.setenv("FLEETS_TOKEN", " \n ")

        assert get_fleet_token() is None

    def test_has_fleet_auth_with_static_token(self):
        configure(fleet_token="fleet-token")

        assert has_fleet_auth() is True

    def test_has_fleet_auth_with_complete_client_credentials(self):
        configure(client_id="client-id", client_secret="client-secret")

        assert has_fleet_auth() is True

    def test_has_fleet_auth_requires_complete_credentials_without_token(self):
        configure(client_id="client-id")

        assert has_fleet_auth() is False
