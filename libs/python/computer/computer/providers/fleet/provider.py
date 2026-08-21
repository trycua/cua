"""Fleet VM provider implementation.

Leases sandboxes from a Cua Fleet pool instead of running a VM locally. A
"VM" here is a Fleet claim bound to a pooled sandbox; the pool keeps warm
replicas, so binding is fast where a cold boot would not be.

The sandbox is never directly addressable. Fleet exposes each sandbox's named
services through an authenticated proxy on the control plane::

    {base_url}/api/svc/{sandbox_namespace}/{sandbox}-{service}/

That route proxies WebSocket upgrades as well as plain HTTP (noVNC's
websockify already relies on it), so computer-server's REST and WebSocket
traffic both ride it. This is why the provider reports an ``api_base_url``
rather than an IP: there is no IP to report, and the URL works from anywhere
the control plane is reachable rather than only from inside the cluster.

Authentication is OAuth 2.0 client credentials, not an API key. The control
plane validates the bearer strictly as a Keycloak JWT (JWKS signature, issuer
match, RS256/RS512/ES256), so the only thing that works on that proxy is an
access token minted by the realm. What the product calls an "API key" is the
``client_id``/``client_secret`` pair issued by ``POST /api/keys``, which is
exchanged for a JWT at the realm's token endpoint.
"""

from __future__ import annotations

import asyncio
import base64
import inspect
import json
import logging
import os
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any, Callable, Dict, Optional, Tuple, Union

from ..base import BaseVMProvider, VMProviderType
from ..types import ListVMsResponse

logger = logging.getLogger(__name__)

DEFAULT_SERVICE_NAME = "server"
DEFAULT_CLAIM_TTL_SECONDS = 3600
DEFAULT_BIND_TIMEOUT_SECONDS = 600
DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
# The realm hands out 900-second tokens. Renew slightly early so a token that
# is about to lapse is never the one used to open a connection; the Rust SDK's
# transport applies the same 30-second skew to its own cache.
TOKEN_EXPIRY_SKEW_SECONDS = 30
# Minting a token is on the reconnect path; a stalled realm should surface as a
# retryable error rather than as a connection that appears to hang.
TOKEN_REQUEST_TIMEOUT_SECONDS = 30

TokenSource = Union[str, Callable[[], Any]]


async def _request_client_credentials_token(
    token_url: str, client_id: str, client_secret: str
) -> Tuple[str, int]:
    """Exchange client credentials for an access token.

    Mirrors what the Fleet SDK's Rust transport does internally: HTTP Basic
    with the client pair, ``grant_type=client_credentials`` as a form body.
    The SDK caches that token behind its FFI boundary and exposes no accessor,
    so the provider has to run the exchange itself to hand ``Computer`` a
    bearer for the service proxy.

    Returns the token and its lifetime in seconds.
    """
    import aiohttp

    credentials = base64.b64encode(f"{client_id}:{client_secret}".encode()).decode()
    # This runs on the reconnect path, so an unresponsive realm must fail fast
    # rather than hold a reconnect open for aiohttp's five-minute default.
    timeout = aiohttp.ClientTimeout(total=TOKEN_REQUEST_TIMEOUT_SECONDS)
    async with aiohttp.ClientSession(timeout=timeout) as session:
        async with session.post(
            token_url,
            data="grant_type=client_credentials",
            headers={
                "accept": "application/json",
                "content-type": "application/x-www-form-urlencoded",
                "authorization": f"Basic {credentials}",
            },
        ) as response:
            body = await response.text()
            if not 200 <= response.status < 300:
                raise RuntimeError(
                    f"Fleet token request to {token_url} failed with HTTP "
                    f"{response.status}: {body[:512]}"
                )

    payload = json.loads(body)
    token = payload.get("access_token")
    if not token:
        raise RuntimeError(f"Fleet token endpoint {token_url} returned no access_token")
    return str(token), int(payload.get("expires_in") or 0)


@dataclass
class _Lease:
    """A claim held on behalf of one named VM.

    The claim is what must be released; the bound sandbox is what carries the
    name and services used to address it.
    """

    claim: Any
    bound: Any


class FleetProvider(BaseVMProvider):
    """VM provider backed by a Cua Fleet pool.

    Unlike the local providers, this one does not create or own a VM. It leases
    one from a pool and releases the lease afterwards, so an abandoned lease
    holds fleet capacity until its TTL expires. Every exit path therefore
    releases: ``stop_vm`` for the ordinary case and ``__aexit__`` for anything
    that skipped it.
    """

    def __init__(
        self,
        pool: str,
        *,
        client_id: Optional[str] = None,
        client_secret: Optional[str] = None,
        access_token: Optional[TokenSource] = None,
        token_url: Optional[str] = None,
        base_url: Optional[str] = None,
        service_name: str = DEFAULT_SERVICE_NAME,
        claim_ttl_seconds: int = DEFAULT_CLAIM_TTL_SECONDS,
        bind_timeout_seconds: int = DEFAULT_BIND_TIMEOUT_SECONDS,
        client: Optional[Any] = None,
        verbose: bool = False,
    ):
        """Initialize the Fleet provider.

        There is deliberately no ``namespace`` argument: the control plane
        keys a pool's namespace to its name (``create_pool`` enforces
        ``metadata.namespace == metadata.name``), so a caller-supplied
        namespace could never change a lookup or a URL. The namespace used for
        addressing is read from the pool, and overridden per sandbox by the
        bound sandbox's own namespace.

        Args:
            pool: Name of the Fleet pool to claim sandboxes from.
            client_id: OAuth client id. Falls back to the cua-sandbox config
                and then to CUA_CLIENT_ID.
            client_secret: OAuth client secret. Falls back to the cua-sandbox
                config and then to CUA_CLIENT_SECRET.
            access_token: A ready-made access token, used verbatim instead of
                an exchange. Pass a callable (sync or async, returning a
                string) to keep refresh in the caller's hands; a bare string
                is a fixed token and expires with the realm's lifetime.
                Falls back to the configured Fleet workload token and then to
                FLEETS_TOKEN.
            token_url: Realm token endpoint for the client-credentials
                exchange. Falls back to the cua-sandbox config, then to the
                cyclops-cs realm.
            base_url: Fleet control-plane base URL. Falls back to the SDK's
                configured base URL.
            service_name: Sandbox service exposing computer-server.
            claim_ttl_seconds: Lifetime requested for each claim. A claim that
                outlives its holder is reclaimed rather than leaked forever.
            bind_timeout_seconds: How long to wait for a claim to bind, and
                how long the claim itself may sit Pending before Fleet fails
                it.
            client: Fleet client override, primarily for tests.
            verbose: Enable debug logging.
        """
        if not pool:
            raise ValueError("FleetProvider requires a pool name")

        self.pool_name = pool
        # Read from the pool once it resolves; only a fallback for addressing,
        # since a bound sandbox carries its own namespace.
        self.namespace: Optional[str] = None
        self.base_url = base_url
        self.service_name = service_name
        self.claim_ttl_seconds = claim_ttl_seconds
        self.bind_timeout_seconds = bind_timeout_seconds
        self.verbose = verbose

        # There is no opaque API key in Fleet: the control plane only accepts
        # a realm-issued JWT, so what has to be resolved here is either a
        # token or the client pair that mints one.
        self.client_id = (
            client_id or _config_value("get_client_id") or os.environ.get("CUA_CLIENT_ID")
        )
        self.client_secret = (
            client_secret
            or _config_value("get_client_secret")
            or os.environ.get("CUA_CLIENT_SECRET")
        )
        self.token_url = token_url or _config_value("get_token_url") or DEFAULT_TOKEN_URL
        self._token_source: Optional[TokenSource] = (
            access_token or _config_value("get_fleet_token") or os.environ.get("FLEETS_TOKEN")
        )

        self._token: Optional[str] = None
        self._token_expires_at = 0.0
        self._token_lock = asyncio.Lock()

        self._client = client
        self._pool: Any = None
        self._leases: Dict[str, _Lease] = {}

    @property
    def provider_type(self) -> VMProviderType:
        return VMProviderType.FLEET

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def __aenter__(self) -> "FleetProvider":
        if self._client is None:
            self._client = self._default_client()
        if self._pool is None:
            self._pool = await self._client.get_pool(self.pool_name)
            if self.namespace is None:
                self.namespace = _namespace_of(self._pool)
        return self

    async def __aexit__(self, *_: Any) -> None:
        # Release anything the caller left behind. A lease that escapes here
        # occupies a pool replica until its TTL, which looks to everyone else
        # like the pool is undersized.
        for name in list(self._leases):
            try:
                await self.stop_vm(name)
            except Exception:
                logger.exception("Failed to release Fleet claim for %r", name)

        if self._client is not None:
            try:
                await self._client.close()
            except Exception:
                logger.exception("Failed to close the Fleet client")

    def _default_client(self) -> Any:
        """Build the SDK client lazily.

        cua-sandbox is an optional dependency: importing it at module scope
        would make every provider import fail where it is not installed.
        """
        try:
            from cua_sandbox.transport.fleet_cloud import _FleetClient
        except ImportError as error:  # pragma: no cover - import guard
            raise RuntimeError(
                "FleetProvider requires the cua-sandbox package. "
                "Install it with: pip install cua-sandbox"
            ) from error
        return _FleetClient()

    # ------------------------------------------------------------------
    # VM operations
    # ------------------------------------------------------------------

    async def run_vm(
        self, image: str, name: str, run_opts: Dict[str, Any], storage: Optional[str] = None
    ) -> Dict[str, Any]:
        """Claim a sandbox from the pool and bind it to ``name``.

        ``image`` is accepted for interface compatibility and ignored: a pooled
        sandbox is built from the pool's template, so the image is fixed when
        the pool is created rather than chosen per claim.
        """
        if name in self._leases:
            logger.info("Fleet claim for %r already held; reusing it", name)
            return await self.get_vm(name)

        await self.__aenter__()

        ttl = int(run_opts.get("ttl_seconds", self.claim_ttl_seconds))
        spec = self._claim_spec(ttl)
        request = self._claim_request(spec)

        claim = await self._client.create_claim(request)
        try:
            bound = await asyncio.wait_for(
                self._client.wait_claim(claim), timeout=self.bind_timeout_seconds
            )
        except Exception:
            # The claim exists even though binding failed; leaving it behind
            # would hold a replica for its whole TTL.
            try:
                await self._client.delete_claim(claim)
            except Exception:
                logger.exception("Failed to release an unbound Fleet claim")
            raise

        self._leases[name] = _Lease(claim=claim, bound=bound)
        logger.info("Fleet claim for %r bound to sandbox %r", name, _name_of(bound))
        return await self.get_vm(name)

    async def stop_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Release the claim. The sandbox returns to the pool."""
        lease = self._leases.pop(name, None)
        if lease is None:
            return {"name": name, "status": "stopped"}

        await self._client.delete_claim(lease.claim)
        logger.info("Released Fleet claim for %r", name)
        return {"name": name, "status": "stopped"}

    async def restart_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Raise: a bound claim cannot be restarted in place.

        Releasing and re-claiming would look like a restart while silently
        changing which sandbox the caller is talking to, and the old address
        can be re-claimed by someone else rather than going dead. Callers that
        want a fresh environment should stop and start, which re-resolves the
        address. Computer.restart() already falls back to exactly that.
        """
        raise self._unsupported(
            name,
            "restart",
            "a bound claim cannot be restarted in place; stop and start "
            "instead, which claims a new sandbox and re-resolves its address",
        )

    async def update_vm(
        self, name: str, update_opts: Dict[str, Any], storage: Optional[str] = None
    ) -> Dict[str, Any]:
        """Raise: a pooled sandbox's shape comes from the pool template.

        There is nothing on a claim to change, so a resize request cannot be
        honoured. It fails loudly rather than returning, so a caller never
        proceeds believing the VM was resized.
        """
        raise self._unsupported(
            name,
            "resize",
            "a pooled sandbox's shape comes from the pool template; "
            "change the template and reconcile the pool",
            requested=sorted(update_opts),
        )

    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        lease = self._leases.get(name)
        if lease is None:
            return {"name": name, "status": "stopped"}

        sandbox = _name_of(lease.bound)
        return {
            "name": name,
            "sandbox": sandbox,
            "status": "running",
            "api_url": self._api_base_url(lease.bound),
            # fleet_sdk.Sandbox.services is a List[str] of service names, not
            # a mapping. Coercing it with dict() raises, and run_vm ends in
            # get_vm, so that lands after a claim is already bound.
            "services": list(getattr(lease.bound, "services", None) or []),
        }

    async def list_vms(self) -> ListVMsResponse:
        """List the claims this provider currently holds.

        Deliberately not every claim in the namespace: another process's claims
        are not this provider's to report or, worse, to release.
        """
        return [
            {"name": name, "status": "running", "api_url": self._api_base_url(lease.bound)}
            for name, lease in self._leases.items()
        ]

    async def get_ip(self, name: str, storage: Optional[str] = None, retry_delay: int = 2) -> str:
        """Return the control-plane host that fronts the sandbox.

        A Fleet sandbox has no routable address of its own, so this is the
        proxy's host rather than the VM's. The full path lives in
        ``get_api_base_url``; this exists because the base interface requires
        it and callers use it only for logging and validity checks.

        Waits for the claim to appear, then gives up. A name that was never
        claimed will never arrive, and an unbounded wait turns that mistake
        into a hang instead of an error.
        """
        deadline = time.monotonic() + self.bind_timeout_seconds
        while name not in self._leases:
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"No Fleet claim is held for {name!r} after "
                    f"{self.bind_timeout_seconds}s. run_vm must be called first."
                )
            await asyncio.sleep(retry_delay)
        return _host_of(self._control_plane_base_url())

    async def get_api_base_url(self, name: str) -> str:
        """Return the URL computer-server is reachable on for ``name``.

        This is what a caller should hand to ``Computer(api_base_url=...)``.
        REST and WebSocket URIs are both derived from it.
        """
        lease = self._leases.get(name)
        if lease is None:
            raise RuntimeError(f"No Fleet claim is held for {name!r}")
        return self._api_base_url(lease.bound)

    async def max_concurrency(self) -> Optional[int]:
        """How many sandboxes may be claimed at once, or None if unbounded.

        The pool's autoscaling ceiling, so a caller can size its own worker
        count from the pool rather than guessing. Asking for more than this
        does not fail fast: the excess claims simply block until others are
        released, which reads as a hang rather than as saturation.

        Falls back to the pool's fixed replica count when autoscaling is off,
        since a pool that cannot grow cannot serve more than it has.
        """
        await self.__aenter__()

        autoscaling = getattr(getattr(self._pool, "spec", None), "autoscaling", None)
        if autoscaling is not None:
            maximum = getattr(autoscaling, "max_pool_size", None)
            if maximum:
                return int(maximum)

        replicas = getattr(getattr(self._pool, "spec", None), "replicas", None)
        return int(replicas) if replicas else None

    async def api_headers(self) -> Dict[str, str]:
        """Auth headers for both the REST calls and the WebSocket upgrade.

        Async and re-resolved on every call because the bearer is a Keycloak
        access token with a 900-second lifetime, not a durable key. Each call
        returns a token that is valid now, minting a new one when the cached
        one is within :data:`TOKEN_EXPIRY_SKEW_SECONDS` of expiry.

        ``Computer`` hands this to the interface as a callable rather than as
        a dict, so the interface's reconnect loop calls back here on every
        connection attempt. That matters because the proxy authenticates the
        upgrade rather than each frame: an open socket outlives the token
        silently, and only the reconnect after a network blip would surface
        the expiry -- as a rejected upgrade, permanently.
        """
        token = await self._access_token()
        return {"Authorization": f"Bearer {token}"}

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _unsupported(
        self, name: str, operation: str, reason: str, **details: Any
    ) -> NotImplementedError:
        """Build the error for an operation this provider cannot perform.

        Returns the exception rather than raising it so call sites read as
        ``raise self._unsupported(...)``, keeping the failure visible at the
        method that refuses it.
        """
        suffix = f" (requested: {details['requested']})" if details.get("requested") else ""
        return NotImplementedError(f"FleetProvider cannot {operation} {name!r}: {reason}{suffix}")

    async def _access_token(self) -> str:
        """Return an access token that is valid right now.

        Kept behind a lock so a burst of concurrent claims does not mint a
        token each; the loser of the race finds the winner's token cached.
        """
        source = self._token_source
        if callable(source):
            token = source()
            if inspect.isawaitable(token):
                token = await token
            token = str(token).strip()
            if not token:
                raise RuntimeError("The Fleet access-token callable returned an empty token")
            return token
        if source:
            return str(source).strip()

        if not self.client_id or not self.client_secret:
            raise RuntimeError(
                "FleetProvider needs Fleet credentials to authenticate against the "
                "sandbox service proxy. Pass client_id/client_secret (or "
                "access_token), or set CUA_CLIENT_ID and CUA_CLIENT_SECRET. There "
                "is no API key for Fleet: the control plane only accepts a "
                "realm-issued JWT."
            )

        async with self._token_lock:
            if self._token and time.monotonic() < self._token_expires_at:
                return self._token
            token, expires_in = await _request_client_credentials_token(
                self.token_url, self.client_id, self.client_secret
            )
            self._token = token
            self._token_expires_at = time.monotonic() + max(
                0.0, expires_in - TOKEN_EXPIRY_SKEW_SECONDS
            )
            return token

    def _api_base_url(self, bound: Any) -> str:
        """Build the service-proxy URL the way the SDK's own ``service_url`` does.

        The namespace is the *bound sandbox's*, not the pool's: they are
        usually the same, but a claim can bind a sandbox elsewhere, and a URL
        built from the pool's namespace then points at nothing.
        """
        base = self._control_plane_base_url().rstrip("/")
        namespace = getattr(bound, "namespace", None) or self.namespace
        return f"{base}/api/svc/{namespace}/{_name_of(bound)}-{self.service_name}/"

    def _control_plane_base_url(self) -> str:
        """The Fleet endpoint, which is not the same host as the legacy VM API.

        get_base_url() is api.cua.ai and serves the API-key cloud operations;
        the /api/svc proxy that fronts sandbox services lives on the Fleet
        endpoint, run.cua.ai. Using the former builds URLs that resolve and
        then 404, which is a confusing way to fail.
        """
        if self.base_url:
            return self.base_url
        try:
            from cua_sandbox._config import get_fleet_base_url
        except ImportError as error:  # pragma: no cover - import guard
            raise RuntimeError(
                "FleetProvider needs base_url, or the cua-sandbox package to supply it"
            ) from error
        return get_fleet_base_url()

    def _claim_spec(self, ttl_seconds: int) -> Any:
        """Build the claim spec.

        ``ClaimSpec(*, sandbox_template_ref, warmpool, bind_deadline,
        lifecycle)`` and ``ClaimLifecycle(*, shutdown_time, shutdown_policy,
        auto_renew)`` are keyword-only with no defaults, so every field is
        named here even where the value is ``None``.

        The template ref is copied from the pool's own spec rather than
        derived from the pool's name. That is what the SDK does when it
        defaults the spec, and for a stated reason: a hand-built ref naming a
        template that does not exist makes the bind queue lookup miss forever
        and the claim times out with nothing useful to show for it.

        ``warmpool`` stays ``None`` so the operator applies its own default,
        matching the spec the SDK would have built.
        """
        from cua_sandbox import ClaimSpec

        # cua_sandbox re-exports ClaimSpec but not ClaimLifecycle, so take the
        # latter from the binding package cua-sandbox itself depends on.
        from fleet_sdk import ClaimLifecycle

        shutdown_time = (
            (datetime.now(timezone.utc) + timedelta(seconds=ttl_seconds))
            .isoformat()
            .replace("+00:00", "Z")
        )
        return ClaimSpec(
            sandbox_template_ref=self._pool.spec.sandbox_template_ref,
            warmpool=None,
            bind_deadline=int(self.bind_timeout_seconds),
            lifecycle=ClaimLifecycle(
                shutdown_time=shutdown_time,
                shutdown_policy=None,
                auto_renew=None,
            ),
        )

    def _claim_request(self, spec: Any) -> Any:
        from cua_sandbox.transport.fleet_cloud import CreateClaimRequest

        return CreateClaimRequest(pool=self._pool, spec=spec)


def _config_value(getter: str) -> Optional[str]:
    """Read one setting from cua-sandbox's config, if it is installed.

    cua-sandbox is optional, and the provider is constructible without it
    (a test or an embedder can inject a client), so a missing package means
    "no configured value" rather than an error.
    """
    try:
        from cua_sandbox import _config
    except ImportError:
        return None
    resolver = getattr(_config, getter, None)
    return resolver() if resolver is not None else None


def _name_of(resource: Any) -> str:
    name = getattr(resource, "name", None)
    if name:
        return str(name)
    metadata = getattr(resource, "metadata", None)
    if metadata is not None and getattr(metadata, "name", None):
        return str(metadata.name)
    raise ValueError(f"Fleet resource has no name: {resource!r}")


def _namespace_of(resource: Any) -> str:
    metadata = getattr(resource, "metadata", None)
    if metadata is not None and getattr(metadata, "namespace", None):
        return str(metadata.namespace)
    raise ValueError(f"Fleet pool has no namespace: {resource!r}")


def _host_of(url: str) -> str:
    from urllib.parse import urlparse

    parsed = urlparse(url)
    return parsed.hostname or url
