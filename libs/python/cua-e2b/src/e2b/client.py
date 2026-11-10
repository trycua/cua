"""The `E2B` client: the resource classes with connection params bound.

    client = E2B(api_key="e2b_...", domain="e2b.dev")
    sandbox = client.Sandbox.create()

`client.Sandbox`, `AsyncSandbox`, `Volume`, `AsyncVolume`, `Template`,
`AsyncTemplate`, `Secret` and `AsyncSecret` are subclasses of the top-level classes
carrying the params as `_bound_api_params`; per-call params win over them, and
clients are isolated from each other.

The bound params are kept and merged exactly as e2b does and are validated by the
`ConnectionConfig` every call builds. The ones that only configure E2B's REST/HTTP
transport (`api_key`, `domain`, `api_url`, `proxy`, `http_version`, `retries`,
`headers`, `api_headers`) stay on that config and change nothing about how a Space
is reached: cua-e2b has no REST control plane. `request_timeout` and `debug`
apply.
"""

from __future__ import annotations

from typing import Dict, Type, TypeVar, cast

from e2b._typing import Unpack
from e2b.connection_config import ApiParams
from e2b.sandbox_async.main import AsyncSandbox
from e2b.sandbox_sync.main import Sandbox
from e2b.secret import AsyncSecret, Secret
from e2b.template_async import AsyncTemplate
from e2b.template_sync import Template
from e2b.volume import AsyncVolume
from e2b.volume.volume_sync import Volume

T = TypeVar("T")


class E2BClientParams(ApiParams, total=False):
    """Params bound to an `E2B` client, the defaults of every call made through its
    resource classes. Same shape as `ApiParams`."""


def _bind(cls: Type[T], api_params: ApiParams) -> Type[T]:
    """A subclass of `cls` carrying `api_params` as its bound params."""
    return cast(
        Type[T],
        type(cls.__name__, (cls,), {"_bound_api_params": api_params}),
    )


class E2B:
    """E2B client with an explicitly bound connection configuration.

    The resource classes it exposes behave like the top-level exports of the same
    name, except that the client's params are the defaults instead of the
    environment variables. Per-call params still take precedence.
    """

    def __init__(self, **opts: Unpack[E2BClientParams]):
        # Copied, so later changes to the caller's dicts cannot change the bound
        # configuration.
        api_params = cast(ApiParams, dict(cast(Dict[str, object], opts)))

        self.Sandbox = _bind(Sandbox, api_params)
        """`Sandbox` bound to this client's connection configuration."""

        self.AsyncSandbox = _bind(AsyncSandbox, api_params)
        """`AsyncSandbox` bound to this client's connection configuration."""

        self.Volume = _bind(Volume, api_params)
        """`Volume` bound to this client's connection configuration."""

        self.AsyncVolume = _bind(AsyncVolume, api_params)
        """`AsyncVolume` bound to this client's connection configuration."""

        self.Template = _bind(Template, api_params)
        """`Template` bound to this client's connection configuration."""

        self.AsyncTemplate = _bind(AsyncTemplate, api_params)
        """`AsyncTemplate` bound to this client's connection configuration."""

        self.Secret = _bind(Secret, api_params)
        """`Secret` bound to this client's connection configuration."""

        self.AsyncSecret = _bind(AsyncSecret, api_params)
        """`AsyncSecret` bound to this client's connection configuration."""
