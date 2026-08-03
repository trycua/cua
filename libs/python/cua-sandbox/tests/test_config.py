"""Unit tests for config and auth modules."""

from cua_sandbox._config import (
    _global_config,
    configure,
    get_access_token,
    get_api_key,
    get_base_url,
    get_client_id,
    get_client_secret,
    get_fleet_base_url,
    get_token_url,
)
from cua_sandbox.sandbox import Sandbox


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
        _global_config.access_token = None

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

    def test_configure_static_access_token(self):
        configure(access_token="temporary-device-token")

        assert get_access_token() == "temporary-device-token"

    def test_fleet_authentication_precedence(self, monkeypatch):
        monkeypatch.delenv("CUA_CLIENT_ID", raising=False)
        monkeypatch.delenv("CUA_CLIENT_SECRET", raising=False)

        assert not Sandbox._uses_fleet(None)

        configure(client_id="client-id", client_secret="client-secret")
        assert Sandbox._uses_fleet(None)

        configure(access_token="temporary-device-token")
        assert Sandbox._uses_fleet(None)
        assert not Sandbox._uses_fleet("sk-explicit-legacy-key")

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
