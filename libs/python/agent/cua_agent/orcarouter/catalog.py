"""OrcaRouter model catalog: live discovery, capability filtering, fallback seed.

The authoritative catalog is ``GET {api_base}/models`` on the configured
OrcaRouter origin. Capability filters are applied *server-side* through the
``capability`` query parameter and then enforced again client-side, so a model
that does not declare the modality an entry point actually uploads is never
offered (fail closed).
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal

from .credentials import OrcaRouterCredential
from .origins import models_url

Capability = Literal["chat", "embedding", "image", "video", "rerank"]

#: ``supported_endpoint_types`` values that can serve text chat/tool calling.
CHAT_ENDPOINT_TYPES = frozenset({"openai", "anthropic", "gemini", "openai-response"})
#: Endpoint types that are never textual chat, whatever else they advertise.
NON_CHAT_ENDPOINT_TYPES = frozenset(
    {"image-generation", "openai-video", "jina-rerank", "embeddings"}
)
EMBEDDING_ENDPOINT_TYPES = frozenset({"embeddings", "openai-embedding"})
IMAGE_ENDPOINT_TYPES = frozenset({"image-generation"})
VIDEO_ENDPOINT_TYPES = frozenset({"openai-video"})
RERANK_ENDPOINT_TYPES = frozenset({"jina-rerank"})

#: Bounds so a catalog response cannot consume unbounded memory.
MAX_MODELS = 2000
MAX_RESPONSE_BYTES = 4 * 1024 * 1024
DEFAULT_TIMEOUT_SECONDS = 10.0


@dataclass(frozen=True)
class OrcaRouterModel:
    """One catalog entry, bound to the metadata the entry points rely on."""

    id: str
    name: str | None = None
    context_length: int | None = None
    max_completion_tokens: int | None = None
    supported_endpoint_types: tuple[str, ...] = ()
    input_modalities: tuple[str, ...] = ()
    reasoning_efforts: tuple[str, ...] = ()
    source: Literal["live", "fallback"] = "live"

    @property
    def label(self) -> str:
        return self.name or self.id

    def supports_chat(self) -> bool:
        endpoints = set(self.supported_endpoint_types)
        if endpoints & NON_CHAT_ENDPOINT_TYPES and not endpoints & CHAT_ENDPOINT_TYPES:
            return False
        return bool(endpoints & CHAT_ENDPOINT_TYPES)

    def supports_embedding(self) -> bool:
        return bool(set(self.supported_endpoint_types) & EMBEDDING_ENDPOINT_TYPES)

    def supports_image_generation(self) -> bool:
        return bool(set(self.supported_endpoint_types) & IMAGE_ENDPOINT_TYPES)

    def supports_video(self) -> bool:
        return bool(set(self.supported_endpoint_types) & VIDEO_ENDPOINT_TYPES)

    def supports_rerank(self) -> bool:
        return bool(set(self.supported_endpoint_types) & RERANK_ENDPOINT_TYPES)

    def supports_input_modalities(self, required: Iterable[str]) -> bool:
        """Fail closed: a model that does not declare the modality is excluded."""
        declared = set(self.input_modalities)
        for modality in required:
            if modality not in declared:
                return False
        return True

    def to_metadata(self) -> dict[str, Any]:
        """Minimal metadata handed to a UI layer."""
        return {
            "id": self.id,
            "name": self.label,
            "context_length": self.context_length,
            "max_completion_tokens": self.max_completion_tokens,
            "supported_endpoint_types": list(self.supported_endpoint_types),
            "input_modalities": list(self.input_modalities),
            "reasoning_efforts": list(self.reasoning_efforts),
            "source": self.source,
        }


def _as_str_tuple(value: Any) -> tuple[str, ...]:
    if isinstance(value, str):
        return (value,)
    if isinstance(value, Sequence):
        return tuple(str(item) for item in value if isinstance(item, (str, int)))
    return ()


def _as_positive_int(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int) and value > 0:
        return value
    return None


def parse_model(
    record: Mapping[str, Any], *, source: Literal["live", "fallback"] = "live"
) -> OrcaRouterModel | None:
    """Parse one catalog record, rejecting anything the client cannot speak."""
    model_id = record.get("id")
    if not isinstance(model_id, str) or not model_id.strip():
        return None
    architecture = record.get("architecture")
    if not isinstance(architecture, Mapping):
        architecture = {}
    efforts = record.get("reasoning_efforts")
    if not isinstance(efforts, Sequence) or isinstance(efforts, str):
        efforts = _efforts_from_architecture(architecture)
    return OrcaRouterModel(
        id=model_id.strip(),
        name=record.get("name") if isinstance(record.get("name"), str) else None,
        context_length=_as_positive_int(record.get("context_length")),
        max_completion_tokens=_as_positive_int(record.get("max_completion_tokens")),
        supported_endpoint_types=_as_str_tuple(record.get("supported_endpoint_types")),
        input_modalities=_as_str_tuple(architecture.get("input_modalities")),
        reasoning_efforts=tuple(str(e) for e in efforts if isinstance(e, str)),
        source=source,
    )


def _efforts_from_architecture(architecture: Mapping[str, Any]) -> tuple[str, ...]:
    reasoning = architecture.get("reasoning")
    if isinstance(reasoning, Mapping):
        return _as_str_tuple(reasoning.get("efforts") or reasoning.get("effort_levels"))
    return ()


def parse_catalog(
    payload: Any, *, source: Literal["live", "fallback"] = "live"
) -> list[OrcaRouterModel]:
    """Parse a ``/models`` response body, bounded and shape-checked."""
    records: Any = payload
    if isinstance(payload, Mapping):
        records = payload.get("data")
    if not isinstance(records, Sequence) or isinstance(records, (str, bytes)):
        return []
    models: list[OrcaRouterModel] = []
    for record in records[:MAX_MODELS]:
        if not isinstance(record, Mapping):
            continue
        parsed = parse_model(record, source=source)
        if parsed is not None:
            models.append(parsed)
    return models


#: Verified cold-start seed. Each entry's metadata was confirmed against the live
#: OrcaRouter catalog; reasoning ladders and modalities are preserved so a
#: discovery outage does not silently reduce capabilities.
VERIFIED_FALLBACK_MODELS: tuple[OrcaRouterModel, ...] = (
    OrcaRouterModel(
        id="openai/gpt-5.5",
        name="OpenAI: GPT-5.5",
        context_length=400000,
        supported_endpoint_types=("openai", "openai-response", "anthropic"),
        input_modalities=("text", "image"),
        reasoning_efforts=("low", "medium", "high", "xhigh"),
        source="fallback",
    ),
    OrcaRouterModel(
        id="anthropic/claude-opus-4.8",
        name="Anthropic: Claude Opus 4.8",
        context_length=200000,
        supported_endpoint_types=("anthropic", "openai"),
        input_modalities=("text", "image"),
        reasoning_efforts=("low", "medium", "high"),
        source="fallback",
    ),
    OrcaRouterModel(
        id="google/gemini-3.5-flash",
        name="Google: Gemini 3.5 Flash",
        context_length=1000000,
        supported_endpoint_types=("gemini", "openai"),
        input_modalities=("text", "image", "audio", "video"),
        reasoning_efforts=("low", "medium", "high"),
        source="fallback",
    ),
    OrcaRouterModel(
        id="deepseek/deepseek-v4-pro",
        name="DeepSeek: DeepSeek V4 Pro",
        context_length=1048576,
        supported_endpoint_types=("openai", "openai-response"),
        input_modalities=("text",),
        source="fallback",
    ),
    OrcaRouterModel(
        id="orcarouter/auto",
        name="OrcaRouter: Auto",
        supported_endpoint_types=("openai", "openai-response", "anthropic", "gemini"),
        input_modalities=("text", "image"),
        source="fallback",
    ),
)

#: Reasoning ladders already used by this project, keyed by model id. Live
#: discovery never removes metadata a verified fallback entry carries.
KNOWN_REASONING_EFFORTS: Mapping[str, tuple[str, ...]] = {
    model.id: model.reasoning_efforts
    for model in VERIFIED_FALLBACK_MODELS
    if model.reasoning_efforts
}


@dataclass(frozen=True)
class CatalogResult:
    """A resolved catalog plus how it was obtained."""

    models: tuple[OrcaRouterModel, ...]
    origin: Literal["live", "fallback"]
    degraded_reason: str | None = None
    #: False when the catalog came from a previous successful discovery.
    from_cache: bool = False

    @property
    def degraded(self) -> bool:
        return self.origin != "live" or self.from_cache

    def ids(self) -> tuple[str, ...]:
        return tuple(model.id for model in self.models)


@dataclass
class ModelCatalog:
    """Fetches, filters and caches the OrcaRouter catalog."""

    api_base_url: str
    fetcher: Callable[[str, str | None], Any] | None = None
    timeout: float = DEFAULT_TIMEOUT_SECONDS
    _cache: dict[str, tuple[OrcaRouterModel, ...]] = field(default_factory=dict)
    _last_good: tuple[OrcaRouterModel, ...] | None = None

    def _fetch(self, capability: str | None, credential: OrcaRouterCredential | None) -> Any:
        url = models_url(self.api_base_url, capability)
        if self.fetcher is not None:
            return self.fetcher(url, credential.api_key if credential else None)
        return _http_get_json(
            url, api_key=credential.api_key if credential else None, timeout=self.timeout
        )

    def load(
        self,
        *,
        capability: Capability | None = None,
        credential: OrcaRouterCredential | None = None,
        use_cache: bool = True,
    ) -> CatalogResult:
        """Return the catalog for ``capability``.

        Live discovery is authoritative. On failure the last known-good catalog
        is reused; failing that, the verified seed. There is no free-text
        fallback.
        """
        cache_key = capability or "all"
        if use_cache and cache_key in self._cache:
            return CatalogResult(models=self._cache[cache_key], origin="live", from_cache=True)
        try:
            payload = self._fetch(capability, credential)
            parsed = tuple(parse_catalog(payload, source="live"))
            # The server filters by `capability`, and the same filter is applied
            # again here so a record the client cannot speak never becomes an
            # option even if the catalog response includes it.
            models = _filter_capability(parsed, capability)
            if not models:
                raise ValueError("catalog response contained no usable models")
        except Exception as error:  # noqa: BLE001 - every failure degrades identically
            return self._degraded(capability, error)
        self._cache[cache_key] = models
        # The unfiltered live catalog becomes the last known-good source.
        self._last_good = parsed
        return CatalogResult(models=models, origin="live")

    def _degraded(self, capability: Capability | None, error: Exception) -> CatalogResult:
        reason = f"{error.__class__.__name__}: {error}"
        if self._last_good is not None:
            return CatalogResult(
                models=_filter_capability(self._last_good, capability),
                origin="fallback",
                degraded_reason=reason,
                from_cache=True,
            )
        return CatalogResult(
            models=_filter_capability(VERIFIED_FALLBACK_MODELS, capability),
            origin="fallback",
            degraded_reason=reason,
        )

    def invalidate(self) -> None:
        self._cache.clear()


def _http_get_json(url: str, *, api_key: str | None, timeout: float) -> Any:
    request = urllib.request.Request(url, method="GET")
    request.add_header("Accept", "application/json")
    if api_key:
        request.add_header("Authorization", f"Bearer {api_key}")
    with urllib.request.urlopen(
        request, timeout=timeout
    ) as response:  # noqa: S310 - fixed https origin
        body = response.read(MAX_RESPONSE_BYTES + 1)
    if len(body) > MAX_RESPONSE_BYTES:
        raise ValueError("model catalog response exceeded the size limit")
    return json.loads(body.decode("utf-8"))


def _filter_capability(
    models: Iterable[OrcaRouterModel], capability: Capability | None
) -> tuple[OrcaRouterModel, ...]:
    if capability is None:
        return tuple(models)
    predicate: Callable[[OrcaRouterModel], bool]
    if capability == "chat":
        predicate = lambda m: m.supports_chat()  # noqa: E731
    elif capability == "embedding":
        predicate = lambda m: m.supports_embedding()  # noqa: E731
    elif capability == "image":
        predicate = lambda m: m.supports_image_generation()  # noqa: E731
    elif capability == "video":
        predicate = lambda m: m.supports_video()  # noqa: E731
    else:
        predicate = lambda m: m.supports_rerank()  # noqa: E731
    return tuple(model for model in models if predicate(model))


def filter_models(
    models: Iterable[OrcaRouterModel],
    *,
    capability: Capability,
    required_input_modalities: Iterable[str] = (),
) -> tuple[OrcaRouterModel, ...]:
    """Apply one entry point's capability filter to a catalog.

    Multimodal understanding requires chat *and* an explicit declaration of the
    modality the entry point uploads; an undeclared modality fails closed.
    """
    selected = _filter_capability(models, capability)
    required = tuple(required_input_modalities)
    if capability == "chat" and required:
        selected = tuple(model for model in selected if model.supports_input_modalities(required))
    return selected


def is_still_compatible(
    model_id: str,
    models: Iterable[OrcaRouterModel],
    *,
    capability: Capability,
    required_input_modalities: Iterable[str] = (),
) -> bool:
    """Re-validate a persisted selection before restoring it."""
    compatible = filter_models(
        models, capability=capability, required_input_modalities=required_input_modalities
    )
    return any(model.id == model_id for model in compatible)


def with_reasoning_efforts(models: Iterable[OrcaRouterModel]) -> tuple[OrcaRouterModel, ...]:
    """Restore known reasoning ladders that a live record did not advertise."""
    restored: list[OrcaRouterModel] = []
    for model in models:
        if model.reasoning_efforts:
            restored.append(model)
            continue
        known = KNOWN_REASONING_EFFORTS.get(model.id)
        if not known:
            restored.append(model)
            continue
        restored.append(
            OrcaRouterModel(
                id=model.id,
                name=model.name,
                context_length=model.context_length,
                max_completion_tokens=model.max_completion_tokens,
                supported_endpoint_types=model.supported_endpoint_types,
                input_modalities=model.input_modalities,
                reasoning_efforts=known,
                source=model.source,
            )
        )
    return tuple(restored)
