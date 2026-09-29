"""Model catalog tests: parsing, capability filters, fallback, invalidation.

Fixtures cover text-only chat, image-input chat, embedding, image generation,
video and rerank records, so each entry point's filter is exercised directly.
"""

import pytest

from cua_agent.orcarouter import (
    VERIFIED_FALLBACK_MODELS,
    ModelCatalog,
    filter_models,
    is_still_compatible,
    parse_catalog,
    with_reasoning_efforts,
)
from tests.orcarouter.conftest import CATALOG_FIXTURE, make_provider


def catalog_models(payload=None):
    return parse_catalog(payload if payload is not None else CATALOG_FIXTURE)


class TestParsing:
    def test_parses_the_live_response_shape(self):
        models = catalog_models()
        ids = [m.id for m in models]
        assert "openai/gpt-5.5" in ids
        assert all(m.source == "live" for m in models)

    def test_preserves_vendor_model_namespace_verbatim(self):
        ids = [m.id for m in catalog_models()]
        assert "deepseek/deepseek-v4-pro" in ids
        assert "orcarouter/auto" in ids

    def test_rejects_records_without_a_usable_id(self):
        models = parse_catalog({"data": [{"object": "model"}, {"id": "   "}, {"id": "ok/one"}]})
        assert [m.id for m in models] == ["ok/one"]

    def test_survives_a_non_object_response(self):
        assert parse_catalog(None) == []
        assert parse_catalog("nope") == []
        assert parse_catalog({"data": "nope"}) == []

    def test_bounds_the_number_of_records(self):
        from cua_agent.orcarouter.catalog import MAX_MODELS

        payload = {"data": [{"id": f"v/m{i}"} for i in range(MAX_MODELS + 500)]}
        assert len(parse_catalog(payload)) == MAX_MODELS

    def test_reads_context_and_modalities(self):
        models = {m.id: m for m in catalog_models()}
        gpt55 = models["openai/gpt-5.5"]
        assert gpt55.context_length == 400000
        assert gpt55.input_modalities == ("text", "image")
        assert gpt55.reasoning_efforts == ("low", "medium", "high", "xhigh")

    def test_ignores_a_bogus_context_length(self):
        models = parse_catalog({"data": [{"id": "v/m", "context_length": -5}]})
        assert models[0].context_length is None


class TestCapabilityFilters:
    def test_chat_requires_a_supported_endpoint_type(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="chat")]
        assert "openai/gpt-5.5" in ids
        assert "orcarouter/auto" in ids  # endpoint types, no modality metadata
        assert "google/gemini-3.5-flash" in ids

    def test_chat_excludes_non_text_specialists(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="chat")]
        assert "openai/text-embedding-3-large" not in ids
        assert "google/gemini-3-pro-image-preview" not in ids
        assert "openai/sora-2" not in ids
        assert "jina/jina-reranker-v3" not in ids

    def test_a_record_without_metadata_is_not_chat(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="chat")]
        assert "mystery/model-without-metadata" not in ids

    def test_multimodal_requires_an_explicit_image_declaration(self):
        """Fail closed: an undeclared modality is excluded."""
        ids = [
            m.id
            for m in filter_models(
                catalog_models(), capability="chat", required_input_modalities=("image",)
            )
        ]
        assert "openai/gpt-5.5" in ids
        assert "google/gemini-3.5-flash" in ids
        assert "deepseek/deepseek-v4-pro" not in ids  # text-only
        assert "orcarouter/auto" not in ids  # declares no modalities

    def test_audio_requirement_excludes_image_only_models(self):
        ids = [
            m.id
            for m in filter_models(
                catalog_models(), capability="chat", required_input_modalities=("audio",)
            )
        ]
        assert ids == ["google/gemini-3.5-flash"]

    def test_embedding_matches_only_the_embedding_endpoint(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="embedding")]
        assert ids == ["openai/text-embedding-3-large"]

    def test_image_generation_matches_only_image_generation(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="image")]
        assert ids == ["google/gemini-3-pro-image-preview"]

    def test_video_matches_only_openai_video(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="video")]
        assert ids == ["openai/sora-2"]

    def test_rerank_matches_only_jina_rerank(self):
        ids = [m.id for m in filter_models(catalog_models(), capability="rerank")]
        assert ids == ["jina/jina-reranker-v3"]

    def test_never_guesses_a_capability_from_the_model_name(self):
        """A name that looks generative still needs a matching endpoint type."""
        payload = {
            "data": [
                {"id": "vendor/stable-diffusion-xl", "supported_endpoint_types": ["openai"]},
                {"id": "vendor/gpt-5-embedding", "supported_endpoint_types": []},
            ]
        }
        models = parse_catalog(payload)
        assert [m.id for m in filter_models(models, capability="image")] == []
        assert [m.id for m in filter_models(models, capability="embedding")] == []
        # The first one does declare a chat-capable endpoint, so it stays chat.
        assert [m.id for m in filter_models(models, capability="chat")] == [
            "vendor/stable-diffusion-xl"
        ]


class TestStaleSelection:
    def test_unknown_model_is_not_compatible(self):
        models = catalog_models()
        assert not is_still_compatible("nope/removed", models, capability="chat")
        assert is_still_compatible("openai/gpt-5.5", models, capability="chat")

    def test_text_model_becomes_incompatible_once_image_is_required(self):
        models = catalog_models()
        assert is_still_compatible("deepseek/deepseek-v4-pro", models, capability="chat")
        assert not is_still_compatible(
            "deepseek/deepseek-v4-pro",
            models,
            capability="chat",
            required_input_modalities=("image",),
        )


class TestReasoningMetadata:
    def test_live_record_keeps_its_own_ladder(self):
        models = with_reasoning_efforts(catalog_models())
        gpt55 = next(m for m in models if m.id == "openai/gpt-5.5")
        assert gpt55.reasoning_efforts == ("low", "medium", "high", "xhigh")

    def test_known_ladder_is_restored_when_live_metadata_omits_it(self):
        payload = {"data": [{"id": "openai/gpt-5.5", "supported_endpoint_types": ["openai"]}]}
        models = with_reasoning_efforts(parse_catalog(payload))
        assert models[0].reasoning_efforts == ("low", "medium", "high", "xhigh")

    def test_unknown_model_is_not_given_invented_efforts(self):
        payload = {"data": [{"id": "vendor/unknown", "supported_endpoint_types": ["openai"]}]}
        models = with_reasoning_efforts(parse_catalog(payload))
        assert models[0].reasoning_efforts == ()


class TestFallbackAndDegradation:
    def test_live_success_is_authoritative(self, tmp_path):
        provider = make_provider(tmp_path)
        result = provider.load_catalog(capability="chat")
        assert result.origin == "live"
        assert result.degraded is False

    def test_failure_falls_back_to_the_verified_seed(self, tmp_path):
        def failing_fetcher(url, key):
            raise OSError("catalog unreachable")

        provider = make_provider(tmp_path, fetcher=failing_fetcher)
        result = provider.load_catalog(capability="chat")
        assert result.origin == "fallback"
        assert result.degraded is True
        assert result.degraded_reason
        ids = [m.id for m in result.models]
        assert "openai/gpt-5.5" in ids
        assert "orcarouter/auto" in ids

    def test_seed_keeps_reasoning_and_modality_metadata(self, tmp_path):
        def failing_fetcher(url, key):
            raise OSError("down")

        provider = make_provider(tmp_path, fetcher=failing_fetcher)
        result = provider.load_catalog(capability="chat")
        gpt55 = next(m for m in result.models if m.id == "openai/gpt-5.5")
        assert gpt55.reasoning_efforts == ("low", "medium", "high", "xhigh")
        assert "image" in gpt55.input_modalities
        assert gpt55.context_length is not None

    def test_fallback_is_still_capability_filtered(self, tmp_path):
        def failing_fetcher(url, key):
            raise OSError("down")

        provider = make_provider(tmp_path, fetcher=failing_fetcher)
        # The seed has no embedding-capable entry, so the list is empty rather
        # than padded with unrelated models.
        assert provider.load_catalog(capability="embedding").models == ()
        chat_ids = [m.id for m in provider.load_catalog(capability="chat").models]
        assert chat_ids, "the verified seed must remain usable during an outage"

    def test_last_known_good_is_preferred_over_the_seed(self, tmp_path):
        provider = make_provider(tmp_path)
        live = provider.load_catalog(capability=None)
        assert live.origin == "live"

        def failing_fetcher(url, key):
            raise OSError("down")

        provider.catalog.fetcher = failing_fetcher
        provider.catalog.invalidate()
        degraded = provider.load_catalog(capability=None)
        assert degraded.from_cache is True
        assert [m.id for m in degraded.models] == [m.id for m in live.models]

    def test_empty_live_response_is_treated_as_a_failure(self, tmp_path):
        provider = make_provider(tmp_path, catalog_payload={"data": []})
        result = provider.load_catalog(capability="chat")
        assert result.origin == "fallback"

    def test_refresh_invalidates_the_cache(self, tmp_path):
        provider = make_provider(tmp_path)
        provider.load_catalog(capability="chat")
        calls_before = len(provider.catalog_calls)
        provider.load_catalog(capability="chat")
        assert len(provider.catalog_calls) == calls_before  # served from cache

        provider.catalog.invalidate()
        provider.load_catalog(capability="chat")
        assert len(provider.catalog_calls) == calls_before + 1

    def test_fallback_seed_ids_match_the_documented_set(self):
        assert [m.id for m in VERIFIED_FALLBACK_MODELS] == [
            "openai/gpt-5.5",
            "anthropic/claude-opus-4.8",
            "google/gemini-3.5-flash",
            "deepseek/deepseek-v4-pro",
            "orcarouter/auto",
        ]


class TestCatalogRequest:
    def test_requests_the_capability_query_parameter(self, tmp_path):
        provider = make_provider(tmp_path)
        provider.load_catalog(capability="embedding")
        url, _key = provider.catalog_calls[-1]
        assert url == "https://api.orcarouter.ai/v1/models?capability=embedding"

    def test_sends_bearer_auth_when_a_credential_exists(self, tmp_path):
        provider = make_provider(tmp_path, api_key="sk-orca-fake")
        provider.load_catalog(capability="chat")
        _url, key = provider.catalog_calls[-1]
        assert key == "sk-orca-fake"

    def test_ui_metadata_never_contains_the_credential(self, tmp_path):
        provider = make_provider(tmp_path, api_key="sk-orca-fake")
        metadata = provider.model_metadata(capability="chat")
        assert metadata
        assert "sk-orca-fake" not in repr(metadata)

    def test_metadata_exposes_the_fields_a_selector_needs(self, tmp_path):
        provider = make_provider(tmp_path)
        entry = next(m for m in provider.model_metadata() if m["id"] == "openai/gpt-5.5")
        assert entry["name"] == "OpenAI: GPT-5.5"
        assert entry["context_length"] == 400000
        assert entry["input_modalities"] == ["text", "image"]
        assert entry["reasoning_efforts"] == ["low", "medium", "high", "xhigh"]

    def test_request_timeout_is_bounded(self, tmp_path):
        assert ModelCatalog(api_base_url="https://api.orcarouter.ai/v1").timeout <= 10
