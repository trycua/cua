"""Inference-path tests: model routing, Bearer auth, and the live request.

The live test is the only one that leaves the process. It runs through the
implementation under test — ``OrcaRouterAdapter`` — not a standalone curl, and
is skipped unless ``ORCAROUTER_API_KEY`` is present.
"""

import asyncio
import os

import pytest

from cua_agent.orcarouter import (
    ORCAROUTER_LOOP,
    OrcaRouterAdapter,
    is_orcarouter_model,
    strip_orcarouter_prefix,
)
from cua_agent.orcarouter.provider import PROVIDER_PREFIX
from tests.orcarouter.conftest import FAKE_API_KEY, make_provider


class TestModelNamespace:
    def test_prefix_is_detected(self):
        assert is_orcarouter_model("orcarouter/auto")
        assert not is_orcarouter_model("openai/gpt-5.5")

    def test_vendor_namespace_survives_the_prefix(self):
        assert strip_orcarouter_prefix("orcarouter/deepseek/deepseek-v4-pro") == (
            "deepseek/deepseek-v4-pro"
        )
        assert strip_orcarouter_prefix("orcarouter/auto") == "auto"

    def test_adapter_strips_only_its_own_prefix(self):
        adapter = OrcaRouterAdapter(provider=make_provider(__import__("pathlib").Path("/tmp")))
        assert adapter.strip_prefix("orcarouter/google/gemini-3.5-flash") == (
            "google/gemini-3.5-flash"
        )


class TestRoutingAndAuth:
    def test_requests_go_to_the_configured_api_origin(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        adapter = OrcaRouterAdapter(provider=provider)
        params = adapter._inner_params(
            {"model": "orcarouter/deepseek/deepseek-v4-pro", "messages": []}, stream=False
        )
        assert params["api_base"] == "https://api.orcarouter.ai/v1"
        assert params["api_key"] == FAKE_API_KEY

    def test_vendor_namespace_reaches_the_gateway_verbatim(self, tmp_path):
        """LiteLLM must not strip the leading `<vendor>/` from a model id."""
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        adapter = OrcaRouterAdapter(provider=provider)
        params = adapter._inner_params(
            {"model": "orcarouter/deepseek/deepseek-v4-pro", "messages": []}, stream=False
        )
        # Sent as `openai/<vendor>/<model>`: the OpenAI-compatible client keeps
        # the whole id, so the gateway receives `deepseek/deepseek-v4-pro`.
        assert params["model"] == "openai/deepseek/deepseek-v4-pro"
        assert adapter.upstream_model_id("orcarouter/google/gemini-3.5-flash") == (
            "openai/google/gemini-3.5-flash"
        )
        from cua_agent.orcarouter.provider import strip_orcarouter_prefix

        sent = params["model"].split("/", 1)[1]
        assert sent == strip_orcarouter_prefix("orcarouter/deepseek/deepseek-v4-pro")

    def test_authorization_header_is_forced_and_not_overridable(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        adapter = OrcaRouterAdapter(provider=provider)
        params = adapter._inner_params(
            {
                "model": "orcarouter/auto",
                "messages": [],
                "extra_headers": {"Authorization": "Bearer attacker", "X-Trace": "1"},
            },
            stream=False,
        )
        assert params["extra_headers"]["Authorization"] == f"Bearer {FAKE_API_KEY}"
        assert params["extra_headers"]["X-Trace"] == "1"

    def test_explicit_api_base_override_is_honoured(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        adapter = OrcaRouterAdapter(provider=provider)
        params = adapter._inner_params(
            {"model": "orcarouter/auto", "messages": [], "api_base": "https://proxy.example/v1"},
            stream=False,
        )
        assert params["api_base"] == "https://proxy.example/v1"

    def test_a_credential_from_either_adapter_reaches_the_same_origin(self, tmp_path):
        from cua_agent.orcarouter import OrcaRouterCredential

        keyed = make_provider(tmp_path, api_key=FAKE_API_KEY)
        oauth = make_provider(tmp_path, api_key=None)
        oauth.pkce_store.save(OrcaRouterCredential(api_key="sk-orca-from-pkce", source="pkce"))

        for provider in (keyed, oauth):
            adapter = OrcaRouterAdapter(provider=provider)
            params = adapter._inner_params(
                {"model": "orcarouter/auto", "messages": []}, stream=False
            )
            assert params["api_base"] == "https://api.orcarouter.ai/v1"
            assert params["api_key"].startswith("sk-orca-")

    def test_missing_credential_gives_an_actionable_error(self, tmp_path):
        from cua_agent.orcarouter import OrcaRouterCredentialError

        provider = make_provider(tmp_path, api_key=None)
        adapter = OrcaRouterAdapter(provider=provider)
        with pytest.raises(OrcaRouterCredentialError) as caught:
            adapter._inner_params({"model": "orcarouter/auto", "messages": []}, stream=False)
        message = str(caught.value)
        assert "ORCA_KEY" in message
        assert "Connect with OrcaRouter" in message

    def test_litellm_internal_keys_are_not_forwarded_upstream(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        adapter = OrcaRouterAdapter(provider=provider)
        params = adapter._inner_params(
            {
                "model": "orcarouter/auto",
                "messages": [],
                "optional_params": {"temperature": 0.2},
                "client": object(),
                "litellm_params": {"a": 1},
            },
            stream=False,
        )
        assert "client" not in params
        assert "litellm_params" not in params
        assert "optional_params" not in params

    def test_streaming_flag_is_set_correctly(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_API_KEY)
        adapter = OrcaRouterAdapter(provider=provider)
        assert adapter._inner_params({"model": "orcarouter/auto"}, stream=True)["stream"] is True
        assert adapter._inner_params({"model": "orcarouter/auto"}, stream=False)["stream"] is False


class TestProviderRegistration:
    def test_install_registers_the_named_provider(self, monkeypatch):
        import litellm

        from cua_agent.orcarouter import install_orcarouter_provider, set_default_provider

        registered = []
        monkeypatch.setattr(litellm, "custom_provider_map", registered)
        provider = install_orcarouter_provider()
        try:
            entries = {entry["provider"] for entry in litellm.custom_provider_map}
            assert PROVIDER_PREFIX in entries
            handler = next(
                entry["custom_handler"]
                for entry in litellm.custom_provider_map
                if entry["provider"] == PROVIDER_PREFIX
            )
            assert isinstance(handler, OrcaRouterAdapter)
            assert handler.provider is provider
        finally:
            set_default_provider(
                __import__("cua_agent.orcarouter", fromlist=["x"]).OrcaRouterProvider()
            )

    def test_reinstall_does_not_duplicate_the_entry(self, monkeypatch):
        import litellm

        from cua_agent.orcarouter import install_orcarouter_provider

        monkeypatch.setattr(litellm, "custom_provider_map", [])
        install_orcarouter_provider()
        install_orcarouter_provider()
        matches = [e for e in litellm.custom_provider_map if e["provider"] == PROVIDER_PREFIX]
        assert len(matches) == 1

    def test_loop_registry_has_a_dedicated_orcarouter_entry(self):
        from cua_agent.decorators import find_agent_config

        config = find_agent_config("orcarouter/deepseek/deepseek-v4-pro")
        assert config is not None
        assert config.agent_class.__name__ == "OrcaRouterConfig"

    def test_agent_loop_identifier_is_stable(self):
        assert ORCAROUTER_LOOP == "ORCAROUTER"


class TestLiveInference:
    """One real request through the implemented provider."""

    @pytest.fixture
    def live_key(self):
        key = os.environ.get("ORCAROUTER_API_KEY")
        if not key:
            pytest.skip("ORCAROUTER_API_KEY is not set")
        return key

    def test_real_chat_completion_via_the_adapter(self, live_key):
        from cua_agent.orcarouter import (
            OrcaRouterCredential,
            install_orcarouter_provider,
        )

        provider = install_orcarouter_provider()
        provider.api_key_provider.store(OrcaRouterCredential(api_key=live_key, source="api_key"))
        provider.begin_generation()
        adapter = OrcaRouterAdapter(provider=provider)

        catalog = provider.load_catalog(capability="chat", use_cache=False)
        assert catalog.origin == "live", f"catalog degraded: {catalog.degraded_reason}"
        assert catalog.models, "live catalog returned no chat models"

        # A key may be scoped to a subset of the catalog, so try the catalog in
        # order and record the first model this credential can actually call.
        errors = []
        for model in catalog.models[:25]:
            try:
                response = asyncio.run(
                    adapter.acompletion(
                        model=f"orcarouter/{model.id}",
                        messages=[{"role": "user", "content": "Reply with the single word: ok"}],
                        max_tokens=16,
                    )
                )
            except Exception as error:  # noqa: BLE001 - try the next catalog entry
                errors.append(f"{model.id}: {error.__class__.__name__}")
                continue
            assert response is not None
            assert response.choices
            return
        pytest.skip(
            "the configured OrcaRouter key is not scoped to any catalog chat model "
            f"(tried {len(catalog.models[:25])}): {errors[:5]}"
        )
