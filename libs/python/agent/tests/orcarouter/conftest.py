"""Shared fixtures for OrcaRouter provider tests.

Every fixture uses fake credentials only: no test in this file reaches the
network or holds a real ``sk-orca-`` key.
"""

import pytest

from cua_agent.orcarouter import (
    ApiKeyCredentialProvider,
    ModelCatalog,
    OrcaRouterConnectSession,
    OrcaRouterCredentialStore,
    OrcaRouterProvider,
)

FAKE_API_KEY = "sk-orca-testonly-0000000000000000"
FAKE_PKCE_KEY = "sk-orca-pkcetest-1111111111111111"

#: A catalog shaped like the live one, covering every capability the filters
#: distinguish: text-only chat, image-input chat, embedding, image generation,
#: video, rerank, and a record with no capability metadata at all.
CATALOG_FIXTURE = {
    "object": "list",
    "data": [
        {
            "id": "openai/gpt-5.5",
            "object": "model",
            "created": 0,
            "owned_by": "openai",
            "supported_endpoint_types": ["openai", "openai-response", "anthropic"],
            "context_length": 400000,
            "name": "OpenAI: GPT-5.5",
            "architecture": {"input_modalities": ["text", "image"]},
            "reasoning_efforts": ["low", "medium", "high", "xhigh"],
        },
        {
            "id": "deepseek/deepseek-v4-pro",
            "object": "model",
            "created": 0,
            "owned_by": "deepseek",
            "supported_endpoint_types": ["openai", "openai-response"],
            "context_length": 1048576,
            "name": "DeepSeek: DeepSeek V4 Pro",
            "architecture": {"input_modalities": ["text"]},
        },
        {
            "id": "orcarouter/auto",
            "object": "model",
            "created": 0,
            "owned_by": "orcarouter",
            "supported_endpoint_types": ["openai", "anthropic", "gemini", "openai-response"],
        },
        {
            "id": "google/gemini-3.5-flash",
            "object": "model",
            "created": 0,
            "owned_by": "google",
            "supported_endpoint_types": ["gemini", "openai"],
            "context_length": 1000000,
            "architecture": {"input_modalities": ["text", "image", "audio", "video"]},
        },
        {
            "id": "openai/text-embedding-3-large",
            "object": "model",
            "created": 0,
            "owned_by": "openai",
            "supported_endpoint_types": ["embeddings"],
            "architecture": {"input_modalities": ["text"], "output_modalities": ["embedding"]},
        },
        {
            "id": "google/gemini-3-pro-image-preview",
            "object": "model",
            "created": 0,
            "owned_by": "google",
            "supported_endpoint_types": ["image-generation"],
            "architecture": {"input_modalities": ["text"], "output_modalities": ["image"]},
        },
        {
            "id": "openai/sora-2",
            "object": "model",
            "created": 0,
            "owned_by": "openai",
            "supported_endpoint_types": ["openai-video"],
        },
        {
            "id": "jina/jina-reranker-v3",
            "object": "model",
            "created": 0,
            "owned_by": "jina",
            "supported_endpoint_types": ["jina-rerank"],
        },
        {
            "id": "mystery/model-without-metadata",
            "object": "model",
            "created": 0,
            "owned_by": "mystery",
        },
    ],
}


def make_provider(tmp_path, *, catalog_payload=None, api_key=None, fetcher=None):
    """Build an isolated provider whose catalog never touches the network."""
    environ: dict = {}
    if api_key is not None:
        environ["ORCA_KEY"] = api_key
        environ["ORCA_KEY_SOURCE"] = "api_key"
    api_key_provider = ApiKeyCredentialProvider(environ=environ)
    pkce_store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ=environ)
    payload = CATALOG_FIXTURE if catalog_payload is None else catalog_payload
    calls = []

    def default_fetcher(url, key):
        calls.append((url, key))
        return payload

    catalog = ModelCatalog(
        api_base_url="https://api.orcarouter.ai/v1",
        fetcher=fetcher or default_fetcher,
    )
    provider = OrcaRouterProvider(
        api_base_url="https://api.orcarouter.ai/v1",
        auth_base_url="https://www.orcarouter.ai",
        api_key_provider=api_key_provider,
        pkce_store=pkce_store,
        catalog=catalog,
    )
    provider.catalog_calls = calls  # type: ignore[attr-defined]
    return provider


@pytest.fixture
def provider(tmp_path):
    return make_provider(tmp_path)


@pytest.fixture
def session(provider):
    return OrcaRouterConnectSession(provider=provider)
