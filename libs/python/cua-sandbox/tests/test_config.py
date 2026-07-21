"""Unit tests for config and auth modules."""

from cua_sandbox._config import _global_config, configure, get_api_key, get_base_url


class TestConfig:
    def setup_method(self):
        _global_config.api_key = None
        _global_config.base_url = "https://api.trycua.com"
        _global_config.sandbox_base_url = "https://run.cua.ai"
        _global_config.sandbox_client_id = None
        _global_config.sandbox_client_secret = None

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


def test_sandbox_base_url_defaults_to_run_cua(monkeypatch):
    from cua_sandbox._config import get_sandbox_base_url

    monkeypatch.delenv("CUA_SANDBOX_BASE_URL", raising=False)
    assert get_sandbox_base_url() == "https://run.cua.ai"


def test_configure_sandbox_base_url():
    from cua_sandbox._config import configure, get_sandbox_base_url

    configure(sandbox_base_url="http://localhost:8080")
    assert get_sandbox_base_url() == "http://localhost:8080"


def test_sandbox_base_url_env_override(monkeypatch):
    from cua_sandbox._config import configure, get_sandbox_base_url

    configure(sandbox_base_url="http://configured")
    monkeypatch.setenv("CUA_SANDBOX_BASE_URL", "http://environment")
    assert get_sandbox_base_url() == "http://environment"


def test_configure_sandbox_client_credentials():
    from cua_sandbox._config import get_sandbox_client_credentials

    configure(sandbox_client_id="ukey-demo", sandbox_client_secret="secret")
    assert get_sandbox_client_credentials() == ("ukey-demo", "secret")


def test_sandbox_client_credentials_env_override(monkeypatch):
    from cua_sandbox._config import get_sandbox_client_credentials

    configure(sandbox_client_id="ukey-configured", sandbox_client_secret="configured")
    monkeypatch.setenv("CUA_CLIENT_ID", "ukey-environment")
    monkeypatch.setenv("CUA_CLIENT_SECRET", "environment")
    assert get_sandbox_client_credentials() == ("ukey-environment", "environment")
