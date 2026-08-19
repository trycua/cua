"""Fleet-backed implementation of the public cloud sandbox transport."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
from collections.abc import Awaitable, Callable
from typing import TYPE_CHECKING, Any, Mapping, Optional
from urllib.parse import parse_qsl, urlencode, urlparse, urlsplit, urlunsplit

import httpx
from cua_sandbox._config import (
    get_client_id,
    get_client_secret,
    get_fleet_base_url,
    get_fleet_token,
    get_token_url,
)
from cua_sandbox.image import Image, cloud_registry_image
from cua_sandbox.transport.cyclops_http_client import CyclopsHttpClient
from cua_sandbox.transport.fleet import FleetTransport, build_http_request
from fleet_sdk import (
    AccessTokenProvider,
    AccessTokenProviderError,
    CreateClaimRequest,
    CreatePoolRequest,
    CreatePoolRequestBuilder,
    CreateTemplateRequest,
    CreateTemplateRequestBuilder,
    CyclopsClient,
    CyclopsConfiguration,
    CyclopsCredentials,
    CyclopsTokenProviderConfiguration,
    Firmware,
    HttpRequest,
    OsGymSandboxTemplateSpecBuilder,
    OsGymSandboxWarmPoolSpecBuilder,
    PreservedJson,
    SandboxServiceBuilder,
    SandboxTemplateRefBuilder,
    SdkError,
    ServiceProtocol,
    VmTemplateBuilder,
    WarmPoolAutoscaling,
)

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo

logger = logging.getLogger(__name__)

_DNS_LABEL_MAX_LENGTH = 63
_CLAIM_HASH_LENGTH = 16
# Bounds each readiness probe so one stalled /status round-trip cannot eat the
# whole wait_service_ready deadline. Ignored by cua-fleet <= 0.1.8, whose
# native client applies the same 30-second default.
_READINESS_PROBE_TIMEOUT_SECS = 30


# Newer Fleet SDKs raise SdkError.PoolAccessDenied from the Rust core for 403s
# on pool-namespace writes, so every language binding shares one message. Use
# that type when the installed cua-fleet provides it; otherwise map the plain
# 403 Status errors older SDKs raise into an equivalent local error.
_UPSTREAM_POOL_ACCESS_DENIED = getattr(SdkError, "PoolAccessDenied", None)

if _UPSTREAM_POOL_ACCESS_DENIED is not None:
    PoolAccessDeniedError = _UPSTREAM_POOL_ACCESS_DENIED
else:

    class PoolAccessDeniedError(PermissionError):  # type: ignore[no-redef]
        """Fleet refused a pool or template operation for this credential."""


# Catch tuple for pool-access denials raised natively by newer Fleet SDKs;
# empty when the installed cua-fleet predates the upstream variant.
_NATIVE_POOL_ACCESS_DENIED = (
    () if _UPSTREAM_POOL_ACCESS_DENIED is None else (_UPSTREAM_POOL_ACCESS_DENIED,)
)


def _pool_access_denied_message(operation: str, namespace: str, status: int, body: str) -> str:
    return (
        f"Fleet denied {operation} on pool namespace '{namespace}' "
        f"(HTTP {status}: {body}). Pool names are globally unique "
        "across accounts, so this name may already be taken — try a new pool "
        "name. If that does not work, contact support on Discord: "
        "https://discord.gg/mVnXXpdE85"
    )


def _canonicalize_pool_access_denied(error: Exception) -> Exception:
    # The uniffi-generated exception renders as a field dump
    # (operation=..., namespace=..., ...); restore the Rust Display message so
    # both raise paths read identically.
    error.args = (
        _pool_access_denied_message(error.operation, error.namespace, error.status, error.body),
    )
    return error


def _pool_access_denied(namespace: str, error: SdkError.Status) -> Exception:
    if _UPSTREAM_POOL_ACCESS_DENIED is not None:
        return _canonicalize_pool_access_denied(
            _UPSTREAM_POOL_ACCESS_DENIED(
                operation=error.operation,
                namespace=namespace,
                status=error.status,
                body=error.body,
            )
        )
    return PoolAccessDeniedError(
        _pool_access_denied_message(error.operation, namespace, error.status, error.body)
    )


def _claim_name(pool_name: str) -> str:
    normalized_name = pool_name.lower()
    legacy_name = f"{normalized_name}-claim"
    if len(legacy_name) <= _DNS_LABEL_MAX_LENGTH:
        return legacy_name

    hash_suffix = hashlib.sha256(normalized_name.encode()).hexdigest()[:_CLAIM_HASH_LENGTH]
    prefix_length = _DNS_LABEL_MAX_LENGTH - len(hash_suffix) - 1
    prefix = normalized_name[:prefix_length].rstrip("-")
    return f"{prefix}-{hash_suffix}"


_GITHUB_WIF_AUDIENCE = "fleets"
_GitHubTokenRequest = Callable[[str, dict[str, str]], Awaitable[tuple[int, Mapping[str, Any]]]]


async def _httpx_json_get(url: str, headers: dict[str, str]) -> tuple[int, Mapping[str, Any]]:
    async with httpx.AsyncClient(timeout=15) as client:
        response = await client.get(url, headers=headers)
    payload = response.json()
    if not isinstance(payload, Mapping):
        raise ValueError("GitHub OIDC endpoint returned an invalid response")
    return response.status_code, payload


def _with_audience(request_url: str, audience: str) -> str:
    parts = urlsplit(request_url)
    query = [
        (key, value)
        for key, value in parse_qsl(parts.query, keep_blank_values=True)
        if key != "audience"
    ]
    query.append(("audience", audience))
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


class _GitHubActionsAccessTokenProvider(AccessTokenProvider):
    """Refresh a Fleet workload token through GitHub Actions OIDC after a 401."""

    def __init__(
        self,
        token: str,
        *,
        environ: Mapping[str, str] | None = None,
        request: _GitHubTokenRequest = _httpx_json_get,
    ) -> None:
        self._token = token
        self._environ = os.environ if environ is None else environ
        self._request = request

    async def get_access_token(self, force_refresh: bool) -> str:
        if not force_refresh:
            return self._token

        request_url = self._environ.get("ACTIONS_ID_TOKEN_REQUEST_URL", "").strip()
        request_token = self._environ.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "").strip()
        if not request_url or not request_token:
            raise AccessTokenProviderError.Failed(
                "GitHub Actions OIDC environment is unavailable for token refresh"
            )

        try:
            status, payload = await self._request(
                _with_audience(request_url, _GITHUB_WIF_AUDIENCE),
                {
                    "Accept": "application/json",
                    "Authorization": f"bearer {request_token}",
                },
            )
        except AccessTokenProviderError:
            raise
        except Exception as error:
            raise AccessTokenProviderError.Failed("GitHub OIDC token refresh failed") from error

        if status != 200:
            raise AccessTokenProviderError.Failed(
                f"GitHub OIDC token refresh failed with HTTP {status}"
            )
        token = payload.get("value")
        if not isinstance(token, str) or not token.strip():
            raise AccessTokenProviderError.Failed(
                "GitHub OIDC token refresh returned an empty token"
            )
        self._token = token.strip()
        return self._token


class _StaticAccessTokenProvider(AccessTokenProvider):
    """Return a configured Fleet workload token without attempting refresh."""

    def __init__(self, token: str) -> None:
        self._token = token

    async def get_access_token(self, force_refresh: bool) -> str:
        return self._token


class _FleetClient:
    """Thin async facade over the generated Cyclops SDK."""

    def __init__(self) -> None:
        self._closed = False
        fleet_token = get_fleet_token()
        if not fleet_token:
            client_id = get_client_id()
            client_secret = get_client_secret()
            if not client_id or not client_secret:
                raise ValueError(
                    "Fleet cloud sandboxes require CUA_CLIENT_ID and CUA_CLIENT_SECRET, "
                    "or cua.configure(client_id=..., client_secret=...)."
                )
        self._base_url = get_fleet_base_url().rstrip("/")
        self._http_client = CyclopsHttpClient()
        if fleet_token:
            configuration = CyclopsTokenProviderConfiguration(
                base_url=self._base_url,
                pool_poll_interval_ms=2000,
                pool_poll_limit=300,
                claim_poll_interval_ms=2000,
                claim_poll_limit=300,
            )
            self._client = CyclopsClient.connect_with_access_token_provider(
                configuration,
                (
                    _GitHubActionsAccessTokenProvider(fleet_token)
                    if os.environ.get("ACTIONS_ID_TOKEN_REQUEST_URL", "").strip()
                    and os.environ.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "").strip()
                    else _StaticAccessTokenProvider(fleet_token)
                ),
                self._http_client,
            )
            return
        configuration = CyclopsConfiguration(
            base_url=self._base_url,
            token_url=get_token_url(),
            credentials=CyclopsCredentials(client_id, client_secret),
            pool_poll_interval_ms=2000,
            pool_poll_limit=300,
            claim_poll_interval_ms=2000,
            claim_poll_limit=300,
        )
        self._client = CyclopsClient.connect(configuration, self._http_client)

    async def close(self) -> None:
        if self._closed:
            return
        await self._http_client.aclose()
        self._closed = True

    async def create_pool(self, request: CreatePoolRequest) -> Any:
        return await self._client.create_pool(request)

    async def reconcile_pool(self, request: CreatePoolRequest) -> Any:
        return await self._client.reconcile_pool(request)

    async def create_template(self, request: CreateTemplateRequest) -> Any:
        return await self._client.create_template(request)

    async def reconcile_template(self, request: CreateTemplateRequest) -> Any:
        return await self._client.reconcile_template(request)

    async def get_template(self, namespace: str, name: str) -> Any:
        return await self._client.get_template(namespace, name)

    async def delete_template(self, template: Any) -> None:
        await self._client.delete_template(template)

    async def get_namespace(self, name: str) -> Any:
        return await self._client.get_namespace(name)

    async def create_namespace(self, name: str) -> Any:
        return await self._client.create_namespace(name)

    async def delete_namespace(self, name: str) -> None:
        await self._client.delete_namespace(name)

    async def create_claim(self, request: CreateClaimRequest) -> Any:
        return await self._client.create_claim(request)

    async def wait_pool(self, pool: Any, timeout: float = 900.0, poll_interval: float = 5.0) -> Any:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            pools = await self._client.list_pools(pool.metadata.namespace)
            for current_pool in pools:
                if current_pool.metadata.name != pool.metadata.name:
                    continue
                if current_pool.status and (current_pool.status.ready_replicas or 0) >= 1:
                    return current_pool
                break
            if asyncio.get_running_loop().time() >= deadline:
                raise TimeoutError(
                    f"Timed out waiting for Fleet pool {pool.metadata.name!r} to warm up"
                )
            await asyncio.sleep(poll_interval)

    async def wait_claim(self, claim: Any) -> Any:
        return await self._client.wait_claim(claim)

    async def renew_claim(self, claim: Any, shutdown_time: str) -> Any:
        renew = getattr(self._client, "renew_claim", None)
        if renew is None:
            raise RuntimeError(
                "the installed cua-fleet release does not support claim renewal; "
                "upgrade to a build whose CyclopsClient exposes renew_claim"
            )
        return await renew(claim, shutdown_time)

    async def delete_claim(self, claim: Any) -> None:
        await self._client.delete_claim(claim)

    async def delete_pool(self, pool: Any) -> None:
        await self._client.delete_pool(pool)

    async def update_pool(self, pool: Any) -> Any:
        return await self._client.update_pool(pool)

    async def service_request(
        self, sandbox: Any, service: str, path: str, request: HttpRequest
    ) -> Any:
        return await self._client.service_request(sandbox, service, path, request)

    async def get_pool(self, name: str) -> Any:
        return await self._client.get_pool(name)

    async def list_claims(self, namespace: str) -> list[Any]:
        return await self._client.list_claims(namespace)

    async def get_claim(self, pool: Any, name: str | None = None) -> Any:
        expected = name or _claim_name(pool.metadata.name)
        for claim in await self._client.list_claims(pool.metadata.namespace):
            if claim.metadata.name == expected:
                return claim
        raise LookupError(f"Fleet claim {expected!r} was not found")

    async def list_pools(self) -> list[Any]:
        raise NotImplementedError(
            "Fleet sandbox listing requires namespace discovery; use an exact sandbox name instead"
        )

    async def set_pool_replicas(self, pool: Any, replicas: int) -> Any:
        pool.spec.replicas = replicas
        return await self._client.update_pool(pool)

    async def wait_service_ready(
        self, sandbox: Any, service: str, time_to_start: Optional[float] = None
    ) -> None:
        timeout = time_to_start if time_to_start is not None else 600.0
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            response = await self.service_request(
                sandbox,
                service,
                "/status",
                build_http_request(
                    method="GET",
                    url="https://service.invalid/status",
                    timeout_secs=_READINESS_PROBE_TIMEOUT_SECS,
                ),
            )
            if 200 <= response.status < 500:
                return
            if asyncio.get_running_loop().time() >= deadline:
                raise TimeoutError(
                    f"Fleet service {service!r} did not become ready within {timeout} seconds"
                )
            await asyncio.sleep(2)

    def service_url(self, sandbox: Any, service: str) -> str:
        if service not in sandbox.services:
            raise ValueError(f"Fleet sandbox does not expose service {service!r}")
        return f"{self._base_url}/api/svc/{sandbox.namespace}/{sandbox.name}-{service}/"


_ECR_HOST_SUFFIX = ".amazonaws.com"
_ECR_HOST_MARKER = ".dkr.ecr."


def _needs_ecr_pull_secret(image: "str | None") -> bool:
    """True only for the account's private ECR, which the secret authenticates.

    Public registries (public.ecr.aws, ghcr.io, quay.io, Docker Hub) are pulled
    anonymously; attaching a credential for them makes the gateway enforce its
    private-registry allowlist against an image that never needed one.
    """
    if not image:
        return False
    host = image.split("/", 1)[0]
    return _ECR_HOST_MARKER in host and host.endswith(_ECR_HOST_SUFFIX)


_TTL_SECONDS_MAX = 2**32 - 1


def validate_ttl_seconds_after_created(value: "int | None") -> None:
    if value is not None and (
        isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= _TTL_SECONDS_MAX
    ):
        raise ValueError(
            f"ttl_seconds_after_created must be an integer between 0 and {_TTL_SECONDS_MAX}"
        )


class FleetCloudTransport(FleetTransport):
    """Provision image-backed pools or claim pre-created pools through Fleet."""

    def __init__(
        self,
        *,
        image: Optional[Image],
        name: str,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: int = 8000,
        pool_name: str | None = None,
        create_claim: bool = False,
        replicas: int = 1,
        services: Mapping[str, int] | None = None,
        autoscaling: Optional[WarmPoolAutoscaling] = None,
        ttl_seconds_after_created: Optional[int] = None,
    ) -> None:
        if (
            isinstance(server_port, bool)
            or not isinstance(server_port, int)
            or server_port < 1
            or server_port > 65535
        ):
            raise ValueError("server_port must be an integer between 1 and 65535")
        if disk_gb is not None:
            raise ValueError("disk_gb is not supported by the Fleet cloud transport")
        if region != "us-east-1":
            raise ValueError("Fleet cloud sandboxes currently support only region='us-east-1'")
        if isinstance(replicas, bool) or not isinstance(replicas, int) or replicas < 1:
            raise ValueError("replicas must be a positive integer")
        if services is not None and (
            not isinstance(services, Mapping)
            or not services
            or any(
                not isinstance(name, str)
                or not name
                or not isinstance(port, int)
                or port < 1
                or port > 65535
                for name, port in services.items()
            )
        ):
            raise ValueError("services must map non-empty names to TCP ports")
        if autoscaling is not None:
            if not isinstance(autoscaling, WarmPoolAutoscaling):
                raise TypeError("autoscaling must be a fleet_sdk.WarmPoolAutoscaling")
            for field, minimum in (
                ("min_pool_size", 0),
                ("initial_pool_size", 0),
                ("max_pool_size", 1),
            ):
                value = getattr(autoscaling, field)
                if value is not None and (
                    isinstance(value, bool) or not isinstance(value, int) or value < minimum
                ):
                    raise ValueError(f"autoscaling.{field} must be an integer >= {minimum}")
            if (
                autoscaling.min_pool_size is not None
                and autoscaling.max_pool_size is not None
                and autoscaling.min_pool_size > autoscaling.max_pool_size
            ):
                raise ValueError(
                    "autoscaling.min_pool_size must not exceed autoscaling.max_pool_size"
                )
        validate_ttl_seconds_after_created(ttl_seconds_after_created)
        self._image = image
        self._name = name
        self._explicit_pool = pool_name is not None
        self._pool_name = pool_name or name
        self._create_claim = create_claim
        self._claim_name = name if pool_name else _claim_name(name)
        self._cpu = cpu
        self._memory_mb = memory_mb
        self._time_to_start = time_to_start if time_to_start is not None else 600.0
        self._request_timeout = request_timeout or 30.0
        self._server_port = server_port
        self._replicas = replicas
        self._services = dict(services) if services is not None else None
        self._autoscaling = autoscaling
        self._ttl_seconds_after_created = ttl_seconds_after_created
        self._provisioned = False
        self._owns_resources = image is not None or create_claim
        self._template: Any = None
        self._pool: Any = None
        self._claim: Any = None
        self._sdk: Any = None

    @property
    def name(self) -> str:
        return self._name

    async def connect(self) -> None:
        if not self._provisioned:
            if self._sdk is None:
                self._sdk = _FleetClient()
            try:
                if self._pool is None:
                    if self._image is None:
                        self._pool = await self._sdk.get_pool(self._pool_name)
                        if self._create_claim:
                            self._pool = await self._sdk.set_pool_replicas(self._pool, 1)
                            self._pool = await self._sdk.wait_pool(self._pool)
                    else:
                        self._validate_image(self._image)
                        try:
                            self._pool = await self._sdk.reconcile_pool(self._pool_request())
                            self._template = await self._sdk.reconcile_template(
                                self._template_request()
                            )
                        except _NATIVE_POOL_ACCESS_DENIED as error:
                            raise _canonicalize_pool_access_denied(error)
                        except SdkError.Status as error:
                            if error.status == 403:
                                raise _pool_access_denied(self._pool_name, error) from error
                            raise
                        self._pool = await self._sdk.wait_pool(self._pool)
                if self._claim is None:
                    if self._image is None and not self._create_claim:
                        self._claim = await self._get_claim()
                    else:
                        try:
                            self._claim = await self._sdk.create_claim(
                                CreateClaimRequest(
                                    pool=self._pool, spec=None, name=self._claim_name
                                )
                            )
                        except Exception as create_error:
                            try:
                                self._claim = await self._get_claim()
                            except Exception as lookup_error:
                                raise create_error from lookup_error
                bound = await self._sdk.wait_claim(self._claim)
                await self._sdk.wait_service_ready(bound, "server", self._time_to_start)
            except BaseException as provisioning_error:
                cleanup_error: BaseException | None = None
                try:
                    if self._owns_resources:
                        await self._cleanup_resources()
                except BaseException as error:
                    cleanup_error = error
                finally:
                    if self._owns_resources:
                        self._pool = None
                        self._template = None
                if cleanup_error is not None:
                    logger.warning(
                        "Failed to clean up Fleet sandbox %r: %s", self._name, cleanup_error
                    )
                    raise provisioning_error from cleanup_error
                raise
            FleetTransport.__init__(
                self,
                sdk=self._sdk,
                bound=bound,
                service_name="server",
                timeout=self._request_timeout,
            )
            self._provisioned = True
        await FleetTransport.connect(self)

    async def create_snapshot(self, **_: Any) -> dict[str, Any]:
        raise NotImplementedError("Snapshots are not supported by the Fleet cloud transport")

    async def forward_tunnel(self, sandbox_port: int | str) -> "TunnelInfo":
        if not isinstance(sandbox_port, int):
            raise ValueError("Fleet services can only expose numeric TCP ports")
        if not self._provisioned:
            raise ValueError("Transport not connected")
        service = "server" if sandbox_port == self._server_port else f"port-{sandbox_port}"
        from cua_sandbox.interfaces.tunnel import TunnelInfo

        endpoint = self._sdk.service_url(self._bound, service)
        parsed = urlparse(endpoint)
        return TunnelInfo(
            parsed.hostname or "",
            parsed.port or (443 if parsed.scheme == "https" else 80),
            sandbox_port,
            url=endpoint,
        )

    async def delete_vm(self) -> None:
        await self._cleanup_resources()

    async def _cleanup_resources(self) -> None:
        if self._claim is not None:
            await self._sdk.delete_claim(self._claim)
            self._claim = None
        self._provisioned = False

    @classmethod
    async def list_sandboxes(cls) -> list[Any]:
        sdk = _FleetClient()
        try:
            return await sdk.list_pools()
        finally:
            await sdk.close()

    @classmethod
    async def get_sandbox_info(cls, name: str) -> Any:
        sdk = _FleetClient()
        try:
            return await sdk.get_pool(name)
        finally:
            await sdk.close()

    @classmethod
    async def suspend_sandbox(cls, name: str) -> None:
        sdk = _FleetClient()
        try:
            await sdk.set_pool_replicas(await sdk.get_pool(name), 0)
        finally:
            await sdk.close()

    @classmethod
    async def resume_sandbox(cls, name: str, time_to_start: Optional[float] = None) -> None:
        del time_to_start
        sdk = _FleetClient()
        try:
            await sdk.set_pool_replicas(await sdk.get_pool(name), 1)
        finally:
            await sdk.close()

    @classmethod
    async def restart_sandbox(cls, name: str, time_to_start: Optional[float] = None) -> None:
        await cls.suspend_sandbox(name)
        await cls.resume_sandbox(name, time_to_start)

    @classmethod
    async def delete_sandbox(cls, name: str, *, pool_name: str | None = None) -> None:
        sdk = _FleetClient()
        try:
            pool = await sdk.get_pool(pool_name or name)
            if pool_name is not None:
                claim = await sdk.get_claim(pool, name)
            else:
                claim = await sdk.get_claim(pool)
            await sdk.delete_claim(claim)
        finally:
            await sdk.close()

    async def _get_claim(self) -> Any:
        if self._explicit_pool:
            return await self._sdk.get_claim(self._pool, self._claim_name)
        return await self._sdk.get_claim(self._pool)

    def _template_request(self) -> CreateTemplateRequest:
        assert self._image is not None
        if self._services is not None:
            service_ports = {
                "server": self._server_port,
                **{name: port for name, port in self._services.items() if name != "server"},
            }
        else:
            service_ports = {
                "server": self._server_port,
                **{
                    f"port-{port}": port for port in self._image._ports if port != self._server_port
                },
            }
        services = [
            SandboxServiceBuilder()
            .name(name)
            .target_port(port)
            .protocol(ServiceProtocol.TCP)
            .build()
            for name, port in service_ports.items()
        ]
        container_disk_image = cloud_registry_image(self._image)
        vm_template_builder = (
            VmTemplateBuilder()
            .container_disk_image(container_disk_image)
            .probes(
                PreservedJson.from_json(
                    json.dumps({"readinessProbe": {"tcpSocket": {"port": self._server_port}}})
                )
            )
            .services(services)
        )
        # The pull secret authenticates the account's private ECR and nothing else.
        # Attaching it to a public image is not merely redundant: the gateway's
        # admission policy reads its presence as "enforce the ECR allowlist", so a
        # public image the cluster can pull anonymously was refused outright.
        if _needs_ecr_pull_secret(container_disk_image):
            vm_template_builder = vm_template_builder.image_pull_secret("ecr-credentials")

        # Windows guest disks are built UEFI-only (see registry/qemu_builder.py), and the
        # Fleet schema defaults firmware to BIOS, so a Windows image left at the default
        # boots SeaBIOS against a GPT/ESP disk and never reaches the readiness probe.
        # The local QEMU runtime keys off the same os_type check.
        if self._image.os_type == "windows":
            vm_template_builder = vm_template_builder.firmware(Firmware.EFI)
        if self._cpu is not None:
            vm_template_builder = vm_template_builder.cpu_cores(self._cpu)
        if self._memory_mb is not None:
            vm_template_builder = vm_template_builder.memory(f"{self._memory_mb}Mi")

        template_spec = (
            OsGymSandboxTemplateSpecBuilder().vm_template(vm_template_builder.build()).build()
        )
        return (
            CreateTemplateRequestBuilder()
            .namespace(self._pool_name)
            .name(self._pool_name)
            .spec(template_spec)
            .build()
        )

    def _pool_request(self) -> CreatePoolRequest:
        template_ref = SandboxTemplateRefBuilder().name(self._pool_name).build()
        pool_spec_builder = (
            OsGymSandboxWarmPoolSpecBuilder()
            .replicas(self._replicas)
            .sandbox_template_ref(template_ref)
        )
        if self._autoscaling is not None:
            pool_spec_builder = pool_spec_builder.autoscaling(self._autoscaling)
        if self._ttl_seconds_after_created is not None:
            pool_spec_builder = pool_spec_builder.ttl_seconds_after_created(
                self._ttl_seconds_after_created
            )
        return (
            CreatePoolRequestBuilder()
            .namespace(self._pool_name)
            .spec(pool_spec_builder.build())
            .build()
        )

    @staticmethod
    def _service_names(template: Any) -> list[str]:
        return [service.name for service in template.spec.vm_template.services or []] or ["server"]

    @staticmethod
    def _validate_image(image: Image) -> None:
        if not cloud_registry_image(image):
            raise NotImplementedError(
                "Fleet cloud sandboxes require a supported built-in image or Image.from_registry(...)"
            )
        if (
            image._layers
            or image._env
            or image._files
            or image._snapshot_source
            or image._disk_path
        ):
            raise NotImplementedError(
                "Fleet cloud supports registry images with optional exposed services only"
            )
