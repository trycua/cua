"""First-class OrcaRouter provider for this agent.

The provider is the single seam where an OrcaRouter credential and the
OrcaRouter model catalog are obtained. Both authentication choices — a pasted
API key and the OAuth 2.0 + PKCE connect flow — implement the same
:class:`~cua_agent.orcarouter.credentials.CredentialProvider` interface and
produce the same :class:`~cua_agent.orcarouter.credentials.OrcaRouterCredential`,
so inference, model discovery and every entry point are independent of how the
key was obtained.
"""

from __future__ import annotations

import os
from collections.abc import AsyncIterator, Iterator, Mapping
from dataclasses import dataclass, field
from typing import Any, Literal

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse

from .catalog import CatalogResult, ModelCatalog, OrcaRouterModel, with_reasoning_efforts
from .credentials import (
    ApiKeyCredentialProvider,
    CredentialProvider,
    OrcaRouterCredential,
    OrcaRouterCredentialError,
    redact,
)
from .origins import (
    ORCA_API_BASE_URL_ENV,
    ORCA_BASE_URL_ENV,
    ORCA_KEY_ENV,
    resolve_api_base_url,
    resolve_auth_base_url,
)
from .pkce import OrcaRouterCredentialStore

#: LiteLLM provider prefix. ``orcarouter/<vendor>/<model>`` is forwarded to
#: OrcaRouter with the ``<vendor>/<model>`` namespace intact.
PROVIDER_PREFIX = "orcarouter"

#: Gradio agent-loop identifier for the OrcaRouter provider. The model control
#: for this loop is the live catalog dropdown, not a free-text field.
ORCAROUTER_LOOP = "ORCAROUTER"

AuthMethod = Literal["api_key", "pkce"]


class OrcaRouterAuthRequired(RuntimeError):
    """Raised when the stored credential was rejected and needs reauthentication."""

    def __init__(self, message: str, *, account_id: str | None, generation: int) -> None:
        super().__init__(message)
        self.account_id = account_id
        self.generation = generation


@dataclass
class CredentialStatus:
    """Public, redacted view of the credential for status/UI consumers."""

    source: AuthMethod | None
    account_id: str | None
    generation: int
    needs_reauth: bool
    masked: str


@dataclass
class _Generation:
    account_id: str | None
    source: AuthMethod
    needs_reauth: bool = False


class OrcaRouterAuthState:
    """Generation-safe reauthentication tracking.

    A ``401`` from the relay is terminal for the credential that made the
    request: it marks exactly that account and generation ``needs_reauth``. A
    late failure from an older generation can never mark a newly authorized
    credential as broken.
    """

    def __init__(self) -> None:
        self._generation = 0
        self._current: _Generation | None = None

    @property
    def generation(self) -> int:
        return self._generation

    def begin(self, credential: OrcaRouterCredential) -> int:
        """Record a freshly resolved credential and return its generation."""
        self._generation += 1
        self._current = _Generation(account_id=credential.account_id, source=credential.source)
        return self._generation

    def mark_needs_reauth(self, *, generation: int, account_id: str | None) -> bool:
        """Mark the rejected credential. Returns whether the mark was applied."""
        current = self._current
        if current is None or generation != self._generation:
            return False
        if (
            account_id is not None
            and current.account_id is not None
            and account_id != current.account_id
        ):
            return False
        current.needs_reauth = True
        return True

    @property
    def needs_reauth(self) -> bool:
        return bool(self._current and self._current.needs_reauth)

    def status(self, credential: OrcaRouterCredential | None) -> CredentialStatus:
        return CredentialStatus(
            source=credential.source if credential else None,
            account_id=credential.account_id if credential else None,
            generation=self._generation,
            needs_reauth=self.needs_reauth,
            masked=redact(credential.api_key if credential else None),
        )


@dataclass
class OrcaRouterProvider:
    """The OrcaRouter provider: credential seam + catalog + adapter wiring."""

    api_base_url: str = field(default_factory=resolve_api_base_url)
    auth_base_url: str = field(default_factory=resolve_auth_base_url)
    api_key_provider: CredentialProvider = field(default_factory=ApiKeyCredentialProvider)
    pkce_store: OrcaRouterCredentialStore = field(default_factory=OrcaRouterCredentialStore)
    catalog: ModelCatalog | None = None
    auth_state: OrcaRouterAuthState = field(default_factory=OrcaRouterAuthState)

    def __post_init__(self) -> None:
        if self.catalog is None:
            self.catalog = ModelCatalog(api_base_url=self.api_base_url)

    # -- credential seam -------------------------------------------------

    @property
    def providers(self) -> tuple[CredentialProvider, ...]:
        """Both authentication choices, in preference order."""
        return (self.api_key_provider, self.pkce_store)

    def resolve_credential(self) -> OrcaRouterCredential | None:
        """Return the credential from whichever choice is configured."""
        for provider in self.providers:
            credential = provider.resolve()
            if credential is not None:
                return credential
        return None

    def current_generation(self) -> int:
        return self.auth_state.generation

    def begin_generation(self) -> int:
        """Advance the credential generation after a successful login."""
        credential = self.resolve_credential()
        return self.auth_state.begin(
            credential or OrcaRouterCredential(api_key="", source="api_key")
        )

    def store_api_key(self, api_key: str) -> CredentialStatus:
        """Persist a pasted API key through the API-key adapter."""
        credential = OrcaRouterCredential(api_key=api_key.strip(), source="api_key")
        self.api_key_provider.store(credential)
        self.auth_state.begin(credential)
        return self.auth_state.status(credential)

    def clear_credential(self) -> bool:
        """Clear both choices; returns whether anything was stored."""
        cleared = False
        for provider in self.providers:
            cleared = provider.clear() or cleared
        return cleared

    def status(self) -> CredentialStatus:
        return self.auth_state.status(self.resolve_credential())

    def on_unauthorized(self, credential: OrcaRouterCredential, generation: int) -> None:
        """Apply terminal reauthentication for the exact rejected credential."""
        self.auth_state.mark_needs_reauth(generation=generation, account_id=credential.account_id)

    def reauth_message(self) -> str:
        return (
            "The stored OrcaRouter credential was rejected (HTTP 401). OrcaRouter keys "
            "are durable, not refreshable: reconnect with 'cua-agent --connect-orcarouter', "
            "or paste a new key into the OrcaRouter API Key field. The previous key was "
            "kept until a replacement succeeds."
        )

    # -- catalog ---------------------------------------------------------

    def load_catalog(
        self,
        *,
        capability: str | None = None,
        required_input_modalities: tuple[str, ...] = (),
        use_cache: bool = True,
    ) -> CatalogResult:
        """Return capability-filtered models, live when possible."""
        assert self.catalog is not None
        result = self.catalog.load(
            capability=capability,  # type: ignore[arg-type]
            credential=self.resolve_credential(),
            use_cache=use_cache,
        )
        models: tuple[OrcaRouterModel, ...] = result.models
        if required_input_modalities:
            models = tuple(
                model
                for model in models
                if model.supports_input_modalities(required_input_modalities)
            )
        return CatalogResult(
            models=with_reasoning_efforts(models),
            origin=result.origin,
            degraded_reason=result.degraded_reason,
            from_cache=result.from_cache,
        )

    def model_metadata(
        self,
        *,
        capability: str | None = "chat",
        required_input_modalities: tuple[str, ...] = (),
    ) -> list[dict[str, Any]]:
        """Minimal, credential-free model metadata for a UI layer."""
        return [
            model.to_metadata()
            for model in self.load_catalog(
                capability=capability,
                required_input_modalities=required_input_modalities,
            ).models
        ]


class OrcaRouterAdapter(CustomLLM):
    """LiteLLM custom handler routing ``orcarouter/…`` models to OrcaRouter.

    The model namespace after the prefix is preserved verbatim
    (``orcarouter/deepseek/deepseek-v4-pro`` →
    ``https://api.orcarouter.ai/v1`` with ``deepseek/deepseek-v4-pro``).
    """

    def __init__(self, provider: OrcaRouterProvider | None = None, **_: Any) -> None:
        super().__init__()
        self.provider = provider or get_default_provider()
        self.base_url = self.provider.api_base_url

    # -- helpers ---------------------------------------------------------

    def strip_prefix(self, model: str) -> str:
        prefix = f"{PROVIDER_PREFIX}/"
        return model[len(prefix) :] if model.startswith(prefix) else model

    def resolve_api_key(self, kwargs: Mapping[str, Any] | None) -> str:
        explicit = (kwargs or {}).get("api_key")
        if isinstance(explicit, str) and explicit.strip():
            return explicit.strip()
        credential = self.provider.resolve_credential()
        if credential is not None:
            return credential.api_key
        raise OrcaRouterCredentialError(
            "No OrcaRouter credential is configured. Paste an API key into the "
            "OrcaRouter API Key field (or set ORCA_KEY), or use Connect with "
            "OrcaRouter."
        )

    def resolve_api_base(self, kwargs: Mapping[str, Any] | None) -> str:
        explicit = (kwargs or {}).get("api_base")
        if isinstance(explicit, str) and explicit.strip():
            return explicit.strip()
        return self.base_url

    def _inner_params(self, kwargs: Mapping[str, Any], *, stream: bool) -> dict[str, Any]:
        params: dict[str, Any] = dict(kwargs)
        model = self.strip_prefix(str(params.pop("model", "")))
        api_key = self.resolve_api_key(kwargs)
        api_base = self.resolve_api_base(kwargs)
        params.pop("api_key", None)
        params.pop("api_base", None)
        # LiteLLM supplies these routing internals; never forward them upstream.
        for internal in ("litellm_params", "optional_params", "client", "logging_obj", "stream"):
            params.pop(internal, None)
        params.update(
            {
                # Route through the OpenAI-compatible client so the vendor
                # namespace reaches OrcaRouter verbatim. Without this, LiteLLM
                # treats the leading `deepseek/` (or `openai/`, `google/`, …)
                # as its own provider prefix and strips it, so the gateway sees
                # a model id the key is not scoped to.
                "model": f"openai/{model}",
                "api_base": api_base,
                "api_key": api_key,
                "stream": stream,
                "extra_headers": _auth_headers(kwargs, api_key),
            }
        )
        return params

    def upstream_model_id(self, model: str) -> str:
        """The model id OrcaRouter actually receives, for tests and diagnostics."""
        return f"openai/{self.strip_prefix(model)}"

    def _handle_auth_error(self, error: Exception, kwargs: Mapping[str, Any]) -> None:
        status = getattr(error, "status_code", None)
        if status != 401:
            return
        credential = self.provider.resolve_credential()
        if credential is None:
            return
        self.provider.on_unauthorized(credential, self.provider.current_generation())
        raise OrcaRouterAuthRequired(
            self.provider.reauth_message(),
            account_id=credential.account_id,
            generation=self.provider.current_generation(),
        ) from None

    # -- CustomLLM surface ----------------------------------------------

    def completion(self, *args: Any, **kwargs: Any) -> ModelResponse:
        params = self._inner_params(kwargs, stream=False)
        return completion(**params)  # type: ignore[return-value]

    async def acompletion(self, *args: Any, **kwargs: Any) -> ModelResponse:
        params = self._inner_params(kwargs, stream=False)
        try:
            return await acompletion(**params)  # type: ignore[return-value]
        except Exception as error:  # noqa: BLE001 - terminal reauth classification
            self._handle_auth_error(error, kwargs)
            raise

    def streaming(self, *args: Any, **kwargs: Any) -> Iterator[GenericStreamingChunk]:
        params = self._inner_params(kwargs, stream=True)
        yield from completion(**params)  # type: ignore[misc]

    async def astreaming(self, *args: Any, **kwargs: Any) -> AsyncIterator[GenericStreamingChunk]:
        params = self._inner_params(kwargs, stream=True)
        try:
            async for chunk in await acompletion(**params):
                yield chunk  # type: ignore[misc]
        except Exception as error:  # noqa: BLE001
            self._handle_auth_error(error, kwargs)
            raise


def _auth_headers(kwargs: Mapping[str, Any], api_key: str) -> dict[str, str]:
    headers: dict[str, str] = {}
    existing = kwargs.get("extra_headers")
    if isinstance(existing, Mapping):
        headers.update({str(k): str(v) for k, v in existing.items()})
    # Forced last so Authorization cannot be overridden by a caller header.
    headers["Authorization"] = f"Bearer {api_key}"
    return headers


_DEFAULT_PROVIDER: OrcaRouterProvider | None = None


def get_default_provider() -> OrcaRouterProvider:
    """Return the process-wide provider, creating it on first use."""
    global _DEFAULT_PROVIDER
    if _DEFAULT_PROVIDER is None:
        _DEFAULT_PROVIDER = OrcaRouterProvider()
    return _DEFAULT_PROVIDER


def set_default_provider(provider: OrcaRouterProvider) -> None:
    """Replace the process-wide provider (used by tests and embedding hosts)."""
    global _DEFAULT_PROVIDER
    _DEFAULT_PROVIDER = provider


def install_orcarouter_provider(provider: OrcaRouterProvider | None = None) -> OrcaRouterProvider:
    """Register OrcaRouter with LiteLLM as a named provider.

    Returns the provider so callers can reach the credential seam and catalog.
    """
    import litellm

    active = provider or get_default_provider()
    set_default_provider(active)
    adapter = OrcaRouterAdapter(provider=active)

    existing = list(litellm.custom_provider_map or [])
    existing = [entry for entry in existing if entry.get("provider") != PROVIDER_PREFIX]
    existing.append({"provider": PROVIDER_PREFIX, "custom_handler": adapter})
    litellm.custom_provider_map = existing
    return active


def is_orcarouter_model(model: str) -> bool:
    """Whether ``model`` is an OrcaRouter model string."""
    return model.startswith(f"{PROVIDER_PREFIX}/")


def strip_orcarouter_prefix(model: str) -> str:
    """Strip the provider prefix, keeping the vendor/model namespace."""
    return model[len(f"{PROVIDER_PREFIX}/") :] if is_orcarouter_model(model) else model


def credential_env_names() -> tuple[str, ...]:
    """Environment variables the OrcaRouter provider reads."""
    return (ORCA_KEY_ENV, ORCA_BASE_URL_ENV, ORCA_API_BASE_URL_ENV)


def configured_origins(environ: Mapping[str, str] | None = None) -> tuple[str, str]:
    """Return the resolved ``(auth_base_url, api_base_url)`` pair."""
    source = os.environ if environ is None else environ
    return resolve_auth_base_url(source), resolve_api_base_url(source)
