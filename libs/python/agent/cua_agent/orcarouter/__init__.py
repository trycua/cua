"""First-class OrcaRouter provider: authentication, catalog and inference.

OrcaRouter is an OpenAI-compatible AI gateway that routes many providers behind
one endpoint. It is exposed here as a named provider (``orcarouter/…`` model
strings) with two explicit authentication choices that both end in the same
durable API key:

* paste an existing ``sk-orca-…`` API key (``ORCA_KEY``);
* ``Connect with OrcaRouter`` using OAuth 2.0 + PKCE.

Inference and model discovery use ``https://api.orcarouter.ai/v1``; authorization
and code exchange use ``https://www.orcarouter.ai``.
"""

from __future__ import annotations

from .catalog import (
    KNOWN_REASONING_EFFORTS,
    VERIFIED_FALLBACK_MODELS,
    CatalogResult,
    ModelCatalog,
    OrcaRouterModel,
    filter_models,
    is_still_compatible,
    parse_catalog,
    with_reasoning_efforts,
)
from .connect_session import (
    ConnectSnapshot,
    OrcaRouterConnectSession,
    OrcaRouterLoginBusy,
)
from .credentials import (
    ORCA_KEY_SOURCE_ENV,
    REDACTED,
    ApiKeyCredentialProvider,
    CredentialProvider,
    OrcaRouterCredential,
    OrcaRouterCredentialError,
    looks_like_orca_key,
    redact,
    same_secret,
)
from .origins import (
    DEFAULT_API_BASE_URL,
    DEFAULT_AUTH_BASE_URL,
    ORCA_API_BASE_URL_ENV,
    ORCA_AUTH_BASE_URL_ENV,
    ORCA_BASE_URL_ENV,
    ORCA_KEY_ENV,
    OrcaRouterConfigError,
    authorize_url,
    exchange_url,
    models_url,
    resolve_api_base_url,
    resolve_auth_base_url,
)
from .pkce import (
    APP_NAME,
    LoopbackListener,
    PkceAttempt,
    OrcaRouterAuthError,
    OrcaRouterCredentialStore,
    build_authorize_url,
    code_challenge_for,
    connect,
    connect_via_loopback,
    connect_via_out_of_band,
    format_granted_scope_warning,
    generate_state,
    generate_verifier,
    parse_exchange_payload,
    post_exchange,
)
from .provider import (
    ORCAROUTER_LOOP,
    PROVIDER_PREFIX,
    CredentialStatus,
    OrcaRouterAdapter,
    OrcaRouterAuthRequired,
    OrcaRouterAuthState,
    OrcaRouterProvider,
    configured_origins,
    credential_env_names,
    get_default_provider,
    install_orcarouter_provider,
    is_orcarouter_model,
    set_default_provider,
    strip_orcarouter_prefix,
)

__all__ = [
    "APP_NAME",
    "ApiKeyCredentialProvider",
    "CatalogResult",
    "ConnectSnapshot",
    "CredentialProvider",
    "CredentialStatus",
    "DEFAULT_API_BASE_URL",
    "DEFAULT_AUTH_BASE_URL",
    "KNOWN_REASONING_EFFORTS",
    "LoopbackListener",
    "ModelCatalog",
    "ORCA_API_BASE_URL_ENV",
    "ORCA_AUTH_BASE_URL_ENV",
    "ORCA_BASE_URL_ENV",
    "ORCA_KEY_ENV",
    "ORCA_KEY_SOURCE_ENV",
    "ORCAROUTER_LOOP",
    "OrcaRouterAdapter",
    "OrcaRouterAuthError",
    "OrcaRouterAuthRequired",
    "OrcaRouterAuthState",
    "OrcaRouterConfigError",
    "OrcaRouterConnectSession",
    "OrcaRouterCredential",
    "OrcaRouterCredentialError",
    "OrcaRouterCredentialStore",
    "OrcaRouterLoginBusy",
    "OrcaRouterModel",
    "OrcaRouterProvider",
    "PROVIDER_PREFIX",
    "PkceAttempt",
    "REDACTED",
    "VERIFIED_FALLBACK_MODELS",
    "authorize_url",
    "build_authorize_url",
    "code_challenge_for",
    "configured_origins",
    "connect",
    "connect_via_loopback",
    "connect_via_out_of_band",
    "credential_env_names",
    "exchange_url",
    "filter_models",
    "format_granted_scope_warning",
    "generate_state",
    "generate_verifier",
    "get_default_provider",
    "install_orcarouter_provider",
    "is_orcarouter_model",
    "is_still_compatible",
    "looks_like_orca_key",
    "models_url",
    "parse_catalog",
    "parse_exchange_payload",
    "post_exchange",
    "redact",
    "resolve_api_base_url",
    "resolve_auth_base_url",
    "same_secret",
    "set_default_provider",
    "strip_orcarouter_prefix",
    "with_reasoning_efforts",
]
