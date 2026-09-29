"""Authentication tests: both adapters, origins, generations, redaction.

Covers the ``api_key`` and ``pkce`` covers for the verification plan: both
adapters produce the same credential result, neither logs a secret, a revoked
key never triggers a fake refresh, and a late failure cannot poison a newer
credential generation.
"""

import logging
import os

import pytest

from cua_agent.orcarouter import (
    ApiKeyCredentialProvider,
    OrcaRouterCredential,
    OrcaRouterCredentialStore,
    OrcaRouterCredentialError,
    OrcaRouterProvider,
    redact,
    resolve_api_base_url,
    resolve_auth_base_url,
)
from tests.orcarouter.conftest import FAKE_API_KEY, FAKE_PKCE_KEY, make_provider


class TestOrigins:
    def test_defaults_use_separate_public_origins(self):
        assert resolve_auth_base_url({}) == "https://www.orcarouter.ai"
        assert resolve_api_base_url({}) == "https://api.orcarouter.ai/v1"

    def test_explicit_overrides_win_over_shared_base(self):
        env = {
            "ORCA_BASE_URL": "https://self-hosted.example.com",
            "ORCA_AUTH_BASE_URL": "https://auth.example.com",
            "ORCA_API_BASE_URL": "https://inference.example.com/v1",
        }
        assert resolve_auth_base_url(env) == "https://auth.example.com"
        assert resolve_api_base_url(env) == "https://inference.example.com/v1"

    def test_shared_self_hosted_base_serves_both_with_one_v1(self):
        env = {"ORCA_BASE_URL": "https://self-hosted.example.com"}
        assert resolve_auth_base_url(env) == "https://self-hosted.example.com"
        assert resolve_api_base_url(env) == "https://self-hosted.example.com/v1"

    def test_shared_base_is_not_double_v1_prefixed(self):
        env = {"ORCA_BASE_URL": "https://self-hosted.example.com/v1"}
        assert resolve_api_base_url(env) == "https://self-hosted.example.com/v1"

    def test_remote_origins_require_https(self):
        with pytest.raises(ValueError):
            resolve_api_base_url({"ORCA_API_BASE_URL": "http://api.orcarouter.ai/v1"})

    def test_loopback_http_is_allowed_for_development(self):
        assert (
            resolve_api_base_url({"ORCA_API_BASE_URL": "http://127.0.0.1:8080/v1"})
            == "http://127.0.0.1:8080/v1"
        )

    def test_exchange_url_is_never_on_the_inference_origin(self):
        import urllib.parse

        from cua_agent.orcarouter import exchange_url

        url = exchange_url("https://www.orcarouter.ai")
        parsed = urllib.parse.urlsplit(url)
        assert parsed.netloc == "www.orcarouter.ai"
        assert parsed.path == "/api/v1/auth/keys"
        # The documented mistake is building it from the relay origin:
        # https://api.orcarouter.ai/v1/auth/keys is a 404.
        assert parsed.netloc != "api.orcarouter.ai"


class TestApiKeyAdapter:
    def test_stores_reads_and_clears(self, tmp_path):
        environ: dict = {}
        adapter = ApiKeyCredentialProvider(environ=environ)
        assert adapter.resolve() is None

        adapter.store(OrcaRouterCredential(api_key=FAKE_API_KEY, source="api_key"))
        credential = adapter.resolve()
        assert credential is not None
        assert credential.api_key == FAKE_API_KEY
        assert credential.source == "api_key"

        assert adapter.clear() is True
        assert adapter.resolve() is None
        assert adapter.clear() is False

    def test_refuses_empty_key(self):
        adapter = ApiKeyCredentialProvider(environ={})
        with pytest.raises(OrcaRouterCredentialError):
            adapter.store(OrcaRouterCredential(api_key="   ", source="api_key"))

    def test_redaction_hides_the_secret_body(self):
        masked = redact(FAKE_API_KEY)
        assert masked.startswith("sk-orca-")
        assert FAKE_API_KEY not in masked
        assert "testonly" not in masked
        assert redact(None) == ""

    def test_credential_repr_never_contains_the_key(self):
        credential = OrcaRouterCredential(api_key=FAKE_API_KEY, source="api_key")
        assert FAKE_API_KEY not in repr(credential)
        assert FAKE_API_KEY not in str(credential)

    def test_secret_not_written_to_the_settings_json(self, tmp_path, monkeypatch):
        """The Gradio settings file must never carry OrcaRouter secret material."""
        import json

        from cua_agent.ui.gradio import app as gradio_app

        settings_file = tmp_path / ".gradio_settings.json"
        monkeypatch.setattr(gradio_app, "SETTINGS_FILE", settings_file)
        gradio_app.save_settings({"agent_loop": "ORCAROUTER", "orcarouter_api_key": FAKE_API_KEY})

        written = json.loads(settings_file.read_text())
        assert "orcarouter_api_key" not in written
        assert FAKE_API_KEY not in settings_file.read_text()


class TestPkceAdapter:
    def test_persists_to_the_project_env_file_and_reads_back(self, tmp_path):
        env_file = tmp_path / ".env"
        environ: dict = {}
        store = OrcaRouterCredentialStore(env_file=str(env_file), environ=environ)

        store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce", granted_scope="api"))

        assert env_file.exists()
        assert "ORCA_KEY" in env_file.read_text()
        credential = store.resolve()
        assert credential is not None
        assert credential.api_key == FAKE_PKCE_KEY
        assert credential.source == "pkce"

    def test_secret_file_is_owner_only(self, tmp_path):
        env_file = tmp_path / ".env"
        store = OrcaRouterCredentialStore(env_file=str(env_file), environ={})
        store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))
        assert os.stat(env_file).st_mode & 0o077 == 0

    def test_clear_removes_the_stored_key(self, tmp_path):
        env_file = tmp_path / ".env"
        store = OrcaRouterCredentialStore(env_file=str(env_file), environ={})
        store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))
        assert store.clear() is True
        assert store.resolve() is None

    def test_pkce_adapter_ignores_a_key_it_did_not_issue(self, tmp_path):
        """A pasted key is not claimed by the PKCE adapter."""
        environ = {"ORCA_KEY": FAKE_API_KEY, "ORCA_KEY_SOURCE": "api_key"}
        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ=environ)
        assert store.resolve() is None


class TestBothAdaptersAgree:
    def test_each_adapter_yields_the_same_credential_type(self, tmp_path):
        entered = OrcaRouterCredential(api_key=FAKE_API_KEY, source="api_key")
        adapter = ApiKeyCredentialProvider(environ={})
        adapter.store(entered)

        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ={})
        store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))

        for credential in (adapter.resolve(), store.resolve()):
            assert isinstance(credential, OrcaRouterCredential)
            assert credential.api_key
            assert credential.source in ("api_key", "pkce")

    def test_provider_prefers_whichever_choice_is_configured(self, tmp_path):
        keyed = make_provider(tmp_path, api_key=FAKE_API_KEY)
        assert keyed.resolve_credential().source == "api_key"

        anonymous = make_provider(tmp_path, api_key=None)
        assert anonymous.resolve_credential() is None
        anonymous.pkce_store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))
        credential = anonymous.resolve_credential()
        assert credential is not None
        assert credential.source == "pkce"

    def test_downstream_does_not_care_which_adapter_supplied_the_key(self, tmp_path):
        """Capability filtering is identical for both authentication choices."""
        from cua_agent.orcarouter import OrcaRouterAdapter

        keyed = make_provider(tmp_path, api_key=FAKE_API_KEY)
        oauth = make_provider(tmp_path, api_key=None)
        oauth.pkce_store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))

        keyed_ids = [m.id for m in keyed.load_catalog(capability="chat").models]
        oauth_ids = [m.id for m in oauth.load_catalog(capability="chat").models]
        assert keyed_ids == oauth_ids

        for provider, expected in ((keyed, FAKE_API_KEY), (oauth, FAKE_PKCE_KEY)):
            adapter = OrcaRouterAdapter(provider=provider)
            assert adapter.resolve_api_key({}) == expected


class TestReauthentication:
    def test_revoked_key_marks_needs_reauth_without_a_refresh_attempt(self, tmp_path, monkeypatch):
        from cua_agent.orcarouter import OrcaRouterAdapter, OrcaRouterAuthRequired

        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        generation = provider.begin_generation()
        adapter = OrcaRouterAdapter(provider=provider)

        class Unauthorized(Exception):
            status_code = 401

        called = []

        async def fake_acompletion(**kwargs):
            called.append(kwargs)
            raise Unauthorized("revoked")

        monkeypatch.setattr("cua_agent.orcarouter.provider.acompletion", fake_acompletion)

        # The 401 is terminal for this credential: it is surfaced as an
        # actionable reauthentication error, never retried or refreshed.
        with pytest.raises(OrcaRouterAuthRequired) as caught:
            import asyncio

            asyncio.run(
                adapter.acompletion(model="orcarouter/deepseek/deepseek-v4-pro", messages=[])
            )
        assert "--connect-orcarouter" in str(caught.value)
        assert FAKE_API_KEY not in str(caught.value)

        status = provider.status()
        assert status.needs_reauth is True
        assert status.generation == generation
        # Only the rejected request ran: no refresh grant, no retry loop.
        assert len(called) == 1
        assert "refresh" not in provider.reauth_message().lower() or "not refreshable" in (
            provider.reauth_message().lower()
        )

    def test_stale_generation_cannot_poison_a_new_credential(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        old_generation = provider.begin_generation()
        old_credential = provider.resolve_credential()
        assert old_credential is not None

        # A new login succeeds and advances the generation.
        provider.pkce_store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))
        new_generation = provider.begin_generation()
        assert new_generation > old_generation

        # The late 401 from the old credential arrives now.
        provider.on_unauthorized(old_credential, old_generation)

        assert provider.status().needs_reauth is False
        assert provider.status().masked == redact(FAKE_PKCE_KEY)

    def test_401_for_a_different_account_does_not_mark_the_current_one(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        generation = provider.begin_generation()
        provider.auth_state._current.account_id = "account-a"

        applied = provider.auth_state.mark_needs_reauth(
            generation=generation, account_id="account-b"
        )
        assert applied is False
        assert provider.status().needs_reauth is False

    def test_retry_after_reauth_keeps_the_old_key_until_replacement(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        generation = provider.begin_generation()
        credential = provider.resolve_credential()
        provider.on_unauthorized(credential, generation)

        # The rejected key is still present, so a failed reconnect is not fatal.
        assert provider.api_key_provider.resolve().api_key == FAKE_API_KEY

    def test_client_error_other_than_401_is_not_treated_as_revocation(self, tmp_path, monkeypatch):
        from cua_agent.orcarouter import OrcaRouterAdapter

        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        provider.begin_generation()
        adapter = OrcaRouterAdapter(provider=provider)

        class ServerError(Exception):
            status_code = 500

        async def fake_acompletion(**kwargs):
            raise ServerError("boom")

        monkeypatch.setattr("cua_agent.orcarouter.provider.acompletion", fake_acompletion)

        import asyncio

        with pytest.raises(ServerError):
            asyncio.run(adapter.acompletion(model="orcarouter/auto", messages=[]))
        assert provider.status().needs_reauth is False


class TestSecretHygiene:
    def test_no_secret_in_logs_when_the_adapter_is_reprd(self, tmp_path, caplog):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        credential = provider.resolve_credential()
        with caplog.at_level(logging.DEBUG):
            logging.getLogger("cua_agent.orcarouter").debug("credential=%r", credential)
            logging.getLogger("cua_agent.orcarouter").debug("status=%r", provider.status())
        assert FAKE_API_KEY not in caplog.text

    def test_provider_env_names_are_documented(self):
        from cua_agent.orcarouter import credential_env_names

        assert credential_env_names()[0] == "ORCA_KEY"


class TestProviderConstruction:
    def test_provider_lists_both_choices(self, tmp_path):
        provider: OrcaRouterProvider = make_provider(tmp_path)
        sources = [choice.source for choice in provider.providers]
        assert sources == ["api_key", "pkce"]

    def test_status_is_empty_before_configuration(self, tmp_path):
        provider = make_provider(tmp_path)
        status = provider.status()
        assert status.source is None
        assert status.masked == ""
        assert status.needs_reauth is False
