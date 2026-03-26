"""Unit tests for config and auth modules."""


from cua_sandbox._config import _global_config, configure, get_api_key, get_base_url


class TestConfig:
    def setup_method(self):
        _global_config.api_key = None
        _global_config.base_url = "https://api.trycua.com"

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
