"""Unit tests for config and auth modules."""

import warnings

import pytest
from cua_sandbox._config import (
    _global_config,
    configure,
    get_api_key,
    get_base_url,
)

_CONFIG_ENV_VARS = (
    "CUA_API_KEY",
    "CUA_BASE_URL",
)


@pytest.fixture(autouse=True)
def isolate_config(monkeypatch):
    original_config = vars(_global_config).copy()
    clean_config = type(_global_config)()
    vars(_global_config).update(vars(clean_config))
    for variable_name in _CONFIG_ENV_VARS:
        monkeypatch.delenv(variable_name, raising=False)

    try:
        yield
    finally:
        vars(_global_config).clear()
        vars(_global_config).update(original_config)


class TestConfig:
    def test_default_base_url(self):
        assert get_base_url() == "https://api.cua.ai"

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

    def test_cloud_options_are_accepted_and_ignored_with_a_warning(self):
        """Cua Cloud has ended: its options keep parsing but change nothing."""
        with pytest.warns(DeprecationWarning, match="Cua Cloud has ended"):
            configure(
                fleet_base_url="https://fleet.example.test",
                token_url="https://auth.example.test/token",
                client_id="client-id",
                client_secret="client-secret",
                fleet_token="fleet-token",
            )

        assert get_base_url() == "https://api.cua.ai"
        assert get_api_key() is None
        assert not hasattr(_global_config, "client_id")

    def test_api_key_and_base_url_do_not_warn(self):
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            configure(api_key="sk-test-123", base_url="http://localhost:9000")
