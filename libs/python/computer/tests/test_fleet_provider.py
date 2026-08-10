"""Tests for the Fleet VM provider.

The provider leases sandboxes it does not own, so most of what matters here is
release behaviour: a claim that escapes holds a pool replica until its TTL and
looks to everyone else like the pool is undersized.

The fakes below deliberately mirror the *generated* ``fleet_sdk`` types rather
than a convenient shape. An earlier version of this file used a permissive
``ClaimSpec`` that accepted any keyword and a ``services`` mapping; every test
passed while the provider could not construct a claim spec at all and crashed
on the first ``get_vm``. A fake that is easier to satisfy than production is
not a test.
"""

from __future__ import annotations

import sys
import types
from typing import Any

import pytest

from computer.providers import fleet as fleet_package
from computer.providers.base import VMProviderType
from computer.providers.fleet import FleetProvider
from computer.providers.fleet import provider as fleet_provider_module


class FakeMetadata:
    def __init__(self, name: str, namespace: str) -> None:
        self.name = name
        self.namespace = namespace


class FakeAutoscaling:
    def __init__(self, max_pool_size: int | None) -> None:
        self.min_pool_size = 0
        self.max_pool_size = max_pool_size


class FakeSandboxTemplateRef:
    """Mirrors ``fleet_sdk.SandboxTemplateRef``: ``name`` is keyword-only."""

    def __init__(self, *, name: str) -> None:
        self.name = name


class FakeSpec:
    def __init__(
        self,
        replicas: int,
        autoscaling: FakeAutoscaling | None,
        sandbox_template_ref: FakeSandboxTemplateRef,
    ) -> None:
        self.replicas = replicas
        self.autoscaling = autoscaling
        self.sandbox_template_ref = sandbox_template_ref


class FakePool:
    def __init__(
        self,
        name: str = "bench-pool",
        namespace: str = "fleet",
        *,
        replicas: int = 2,
        max_pool_size: int | None = 16,
        template_name: str = "bench-template",
    ) -> None:
        self.metadata = FakeMetadata(name, namespace)
        self.spec = FakeSpec(
            replicas,
            FakeAutoscaling(max_pool_size) if max_pool_size is not None else None,
            FakeSandboxTemplateRef(name=template_name),
        )


class FakeBound:
    """Mirrors ``fleet_sdk.Sandbox``.

    ``services`` is a ``List[str]`` of service names, not a mapping: the live
    Fleet returns ``['server']``. It also carries its own ``namespace``, which
    is the one the service proxy route is built from -- a bound sandbox does
    not have to live in the namespace the pool was looked up in.
    """

    def __init__(
        self,
        *,
        namespace: str = "fleet-eu",
        claim: str = "claim-0",
        name: str = "sandbox-abc",
        services: list[str] | None = None,
    ) -> None:
        self.namespace = namespace
        self.claim = claim
        self.name = name
        self.services = ["server"] if services is None else services


class FakeClient:
    """Records claim lifecycle so tests can assert on release."""

    def __init__(self, *, bind_error: Exception | None = None) -> None:
        self.created: list[Any] = []
        self.requests: list[Any] = []
        self.deleted: list[Any] = []
        self.closed = False
        self._bind_error = bind_error

    async def get_pool(self, name: str) -> FakePool:
        return FakePool(name=name)

    async def create_claim(self, request: Any) -> str:
        claim = f"claim-{len(self.created)}"
        self.requests.append(request)
        self.created.append(claim)
        return claim

    async def wait_claim(self, claim: Any) -> FakeBound:
        if self._bind_error is not None:
            raise self._bind_error
        return FakeBound(claim=str(claim))

    async def delete_claim(self, claim: Any) -> None:
        self.deleted.append(claim)

    async def close(self) -> None:
        self.closed = True


class CountingClient(FakeClient):
    """Binds a distinctly named sandbox per claim.

    A restart must produce a different address; a fake that always returned the
    same name would hide exactly the bug this guards against.
    """

    async def wait_claim(self, claim: Any) -> FakeBound:
        return FakeBound(name=f"sandbox-{len(self.created) - 1}", claim=str(claim))


class FakeTokenEndpoint:
    """A Keycloak token endpoint that mints a fresh JWT per exchange.

    ``expires_in`` is settable so a test can make the token expire, which is
    the only way to exercise the refresh path: the real one hands out 900s
    tokens and a bench run outlives that many times over.
    """

    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str]] = []
        self.expires_in = 900

    async def __call__(self, token_url: str, client_id: str, client_secret: str) -> tuple[str, int]:
        self.calls.append((token_url, client_id, client_secret))
        return f"jwt-{len(self.calls)}", self.expires_in


@pytest.fixture(autouse=True)
def fake_cua_sandbox(monkeypatch: pytest.MonkeyPatch) -> None:
    """Stand in for cua-sandbox, which is an optional dependency."""

    class SandboxTemplateRef(FakeSandboxTemplateRef):
        pass

    class ClaimLifecycle:
        """Mirrors ``fleet_sdk.ClaimLifecycle``: snake_case, all required."""

        def __init__(
            self,
            *,
            shutdown_time: str | None,
            shutdown_policy: str | None,
            auto_renew: bool | None,
        ) -> None:
            self.shutdown_time = shutdown_time
            self.shutdown_policy = shutdown_policy
            self.auto_renew = auto_renew

    class ClaimSpec:
        """Mirrors ``fleet_sdk.ClaimSpec``.

        All four arguments are keyword-only and have no defaults, so omitting
        one fails here exactly as it does against the generated bindings.
        """

        def __init__(
            self,
            *,
            sandbox_template_ref: Any,
            warmpool: str | None,
            bind_deadline: int | None,
            lifecycle: Any,
        ) -> None:
            self.sandbox_template_ref = sandbox_template_ref
            self.warmpool = warmpool
            self.bind_deadline = bind_deadline
            self.lifecycle = lifecycle

    class CreateClaimRequest:
        def __init__(self, *, pool: Any, spec: Any, name: str | None = None) -> None:
            self.pool = pool
            self.spec = spec
            self.name = name

    # cua_sandbox re-exports ClaimSpec but not ClaimLifecycle, so the stub
    # keeps them where the real packages keep them.
    bindings = types.ModuleType("fleet_sdk")
    bindings.ClaimSpec = ClaimSpec  # type: ignore[attr-defined]
    bindings.ClaimLifecycle = ClaimLifecycle  # type: ignore[attr-defined]
    bindings.SandboxTemplateRef = SandboxTemplateRef  # type: ignore[attr-defined]

    root = types.ModuleType("cua_sandbox")
    root.ClaimSpec = ClaimSpec  # type: ignore[attr-defined]
    root.SandboxTemplateRef = SandboxTemplateRef  # type: ignore[attr-defined]

    config = types.ModuleType("cua_sandbox._config")
    # Distinct values on purpose: the Fleet proxy is not on the legacy VM API
    # host, and a fake that returned the same string for both would let the
    # wrong one pass unnoticed.
    config.get_fleet_base_url = lambda: "https://fleet.example"  # type: ignore[attr-defined]
    config.get_base_url = lambda: "https://legacy-vm-api.example"  # type: ignore[attr-defined]
    config.get_token_url = lambda: "https://auth.example/token"  # type: ignore[attr-defined]
    config.get_client_id = lambda: None  # type: ignore[attr-defined]
    config.get_client_secret = lambda: None  # type: ignore[attr-defined]
    config.get_fleet_token = lambda: None  # type: ignore[attr-defined]

    transport = types.ModuleType("cua_sandbox.transport")
    fleet_cloud = types.ModuleType("cua_sandbox.transport.fleet_cloud")
    fleet_cloud.CreateClaimRequest = CreateClaimRequest  # type: ignore[attr-defined]
    fleet_cloud._FleetClient = FakeClient  # type: ignore[attr-defined]

    for name, module in {
        "fleet_sdk": bindings,
        "cua_sandbox": root,
        "cua_sandbox._config": config,
        "cua_sandbox.transport": transport,
        "cua_sandbox.transport.fleet_cloud": fleet_cloud,
    }.items():
        monkeypatch.setitem(sys.modules, name, module)

    # Ambient Fleet credentials on the developer's machine must not decide
    # what these tests assert.
    for variable in ("CUA_CLIENT_ID", "CUA_CLIENT_SECRET", "FLEETS_TOKEN", "CUA_TOKEN_URL"):
        monkeypatch.delenv(variable, raising=False)


@pytest.fixture
def token_endpoint(monkeypatch: pytest.MonkeyPatch) -> FakeTokenEndpoint:
    """Replace the OAuth round trip; the exchange itself is not under test."""
    endpoint = FakeTokenEndpoint()
    monkeypatch.setattr(fleet_provider_module, "_request_client_credentials_token", endpoint)
    return endpoint


def make_provider(client: FakeClient | None = None, **kwargs: Any) -> FleetProvider:
    kwargs.setdefault("client_id", "cua-client")
    kwargs.setdefault("client_secret", "cua-secret")
    return FleetProvider(
        "bench-pool",
        base_url="https://fleet.example",
        client=client or FakeClient(),
        **kwargs,
    )


@pytest.mark.asyncio
async def test_default_base_url_is_the_fleet_endpoint_not_the_vm_api() -> None:
    """The /api/svc proxy lives on the Fleet host, not the legacy VM API.

    Verified against production: cyclops-cs serves run.cua.ai while
    api.cua.ai serves the API-key cloud operations. Picking the wrong one
    yields URLs that resolve and then 404.
    """
    provider = FleetProvider("bench-pool", client=FakeClient())
    async with provider:
        await provider.run_vm("", "vm-1", {})
        url = await provider.get_api_base_url("vm-1")

    assert url.startswith("https://fleet.example/")
    assert "legacy-vm-api" not in url


@pytest.mark.asyncio
async def test_provider_type_is_fleet() -> None:
    assert make_provider().provider_type == VMProviderType.FLEET


@pytest.mark.asyncio
async def test_run_vm_claims_and_reports_the_proxy_url() -> None:
    client = FakeClient()
    provider = make_provider(client)

    async with provider:
        info = await provider.run_vm("ignored-image", "vm-1", {})

        assert client.created == ["claim-0"]
        assert info["status"] == "running"
        # The sandbox is addressed through the control plane, not directly.
        assert (
            await provider.get_api_base_url("vm-1")
            == "https://fleet.example/api/svc/fleet-eu/sandbox-abc-server/"
        )

    # Leaving the context must release the claim even without stop_vm.
    assert client.deleted == ["claim-0"]


@pytest.mark.asyncio
async def test_api_base_url_uses_the_bound_sandbox_namespace() -> None:
    """The route is keyed on the sandbox's namespace, not the pool's.

    ``_FleetClient.service_url`` builds
    ``{base}/api/svc/{sandbox.namespace}/{sandbox.name}-{service}/``. Using
    the namespace the pool happened to be looked up in produces a URL for a
    sandbox that is not there.
    """

    class ElsewhereClient(FakeClient):
        async def wait_claim(self, claim: Any) -> FakeBound:
            return FakeBound(namespace="tenant-42", claim=str(claim))

    # The pool resolves to namespace "fleet"; the bound sandbox says
    # "tenant-42". The sandbox must win.
    provider = make_provider(ElsewhereClient())
    async with provider:
        await provider.run_vm("", "vm-1", {})
        assert (
            await provider.get_api_base_url("vm-1")
            == "https://fleet.example/api/svc/tenant-42/sandbox-abc-server/"
        )


@pytest.mark.asyncio
async def test_get_vm_reports_services_as_a_list() -> None:
    """``fleet_sdk.Sandbox.services`` is a ``List[str]``.

    Coercing it with ``dict(...)`` raises ``ValueError: dictionary update
    sequence element #0 has length 6; 2 is required`` -- and because
    ``run_vm`` ends in ``get_vm``, that lands *after* a claim has been bound,
    leaving a live claim behind an exception.
    """
    client = FakeClient()
    provider = make_provider(client)

    async with provider:
        info = await provider.run_vm("", "vm-1", {})
        assert info["services"] == ["server"]
        assert (await provider.get_vm("vm-1"))["services"] == ["server"]


@pytest.mark.asyncio
async def test_run_vm_builds_a_complete_claim_spec() -> None:
    """``ClaimSpec`` requires all four keyword-only arguments.

    The template ref is taken from the pool's own spec rather than derived
    from the pool's name: the SDK does the same, because a ref naming a
    template that does not exist makes the bind queue lookup miss forever and
    the claim times out with no useful error.
    """
    client = FakeClient()
    provider = make_provider(client, bind_timeout_seconds=420)

    async with provider:
        await provider.run_vm("", "vm-1", {"ttl_seconds": 1800})

    spec = client.requests[0].spec
    assert spec.sandbox_template_ref.name == "bench-template"
    assert spec.bind_deadline == 420
    # snake_case on the binding; the wire form is camelCase but that is the
    # serializer's job, not ours.
    assert spec.lifecycle.shutdown_time.endswith("Z")
    assert client.requests[0].pool is provider._pool


@pytest.mark.asyncio
async def test_stop_vm_releases_the_claim() -> None:
    client = FakeClient()
    provider = make_provider(client)

    async with provider:
        await provider.run_vm("", "vm-1", {})
        await provider.stop_vm("vm-1")
        assert client.deleted == ["claim-0"]
        # Already released; the context exit must not double-delete.
        assert await provider.get_vm("vm-1") == {"name": "vm-1", "status": "stopped"}

    assert client.deleted == ["claim-0"]


@pytest.mark.asyncio
async def test_failed_binding_releases_the_claim() -> None:
    """An unbound claim still occupies the pool, so it must be cleaned up."""
    client = FakeClient(bind_error=RuntimeError("pool exhausted"))
    provider = make_provider(client)

    async with provider:
        with pytest.raises(RuntimeError, match="pool exhausted"):
            await provider.run_vm("", "vm-1", {})

    assert client.created == ["claim-0"]
    assert client.deleted == ["claim-0"]


@pytest.mark.asyncio
async def test_api_headers_exchange_client_credentials_for_a_jwt(
    token_endpoint: FakeTokenEndpoint,
) -> None:
    """There is no opaque API key in Fleet.

    cyclops-cs validates the bearer as a Keycloak JWT (JWKS signature, issuer
    match, RS256/RS512/ES256); anything else is rejected with
    ``auth token is invalid``. What the product calls an "API key" is a
    client_id/client_secret pair that is exchanged for a JWT at the realm's
    token endpoint. The same header authenticates the WebSocket upgrade, not
    just REST.
    """
    provider = make_provider(client_id="fleet-app", client_secret="hunter2")

    assert await provider.api_headers() == {"Authorization": "Bearer jwt-1"}
    assert token_endpoint.calls == [("https://auth.example/token", "fleet-app", "hunter2")]


@pytest.mark.asyncio
async def test_api_headers_reuse_a_token_that_is_still_valid(
    token_endpoint: FakeTokenEndpoint,
) -> None:
    """A valid token is not re-minted on every call."""
    provider = make_provider()

    assert await provider.api_headers() == {"Authorization": "Bearer jwt-1"}
    assert await provider.api_headers() == {"Authorization": "Bearer jwt-1"}
    assert len(token_endpoint.calls) == 1


@pytest.mark.asyncio
async def test_api_headers_refresh_an_expired_token(
    token_endpoint: FakeTokenEndpoint,
) -> None:
    """Fleet access tokens live 900 seconds; bench runs live much longer.

    A header dict captured once is a bearer that goes stale mid-session, and
    the failure surfaces as a redirect on the next reconnect rather than as
    anything that names auth.
    """
    token_endpoint.expires_in = 0
    provider = make_provider()

    assert await provider.api_headers() == {"Authorization": "Bearer jwt-1"}
    assert await provider.api_headers() == {"Authorization": "Bearer jwt-2"}
    assert len(token_endpoint.calls) == 2


@pytest.mark.asyncio
async def test_explicit_access_token_is_used_verbatim(
    token_endpoint: FakeTokenEndpoint,
) -> None:
    """A caller holding its own workload token must not need a secret."""
    provider = FleetProvider(
        "bench-pool",
        base_url="https://fleet.example",
        access_token="  supplied-jwt  ",
        client=FakeClient(),
    )

    assert await provider.api_headers() == {"Authorization": "Bearer supplied-jwt"}
    assert token_endpoint.calls == []


@pytest.mark.asyncio
async def test_missing_credentials_fail_loudly() -> None:
    """Sending no Authorization header 302s to a login page.

    An empty header dict turns an auth misconfiguration into a confusing
    redirect on the first request, so refuse up front and name the inputs.
    """
    provider = FleetProvider("bench-pool", base_url="https://fleet.example", client=FakeClient())

    with pytest.raises(RuntimeError) as error:
        await provider.api_headers()

    assert "CUA_CLIENT_ID" in str(error.value)
    assert "CUA_CLIENT_SECRET" in str(error.value)


@pytest.mark.asyncio
async def test_list_vms_reports_only_held_claims() -> None:
    provider = make_provider()
    async with provider:
        assert await provider.list_vms() == []
        await provider.run_vm("", "vm-1", {})
        listed = await provider.list_vms()

    assert [vm["name"] for vm in listed] == ["vm-1"]


@pytest.mark.asyncio
async def test_update_vm_raises_and_names_the_fix() -> None:
    """Fails loudly: a silent no-op would let a caller believe it resized.

    The message has to say where the shape actually comes from, or the reader
    is left thinking the provider is simply incomplete.
    """
    provider = make_provider()
    async with provider:
        with pytest.raises(NotImplementedError) as error:
            await provider.update_vm("vm-1", {"memory": "8GB", "cpu": 4})

    message = str(error.value)
    assert "cannot resize 'vm-1'" in message
    assert "pool template" in message
    assert "reconcile the pool" in message
    assert "['cpu', 'memory']" in message


@pytest.mark.asyncio
async def test_get_ip_gives_up_instead_of_hanging() -> None:
    """A name that was never claimed will never arrive.

    The wait exists for the claim landing slightly after the call; without a
    deadline a caller's typo becomes an indefinite hang rather than an error.
    """
    provider = make_provider(bind_timeout_seconds=0)
    async with provider:
        with pytest.raises(TimeoutError, match="No Fleet claim is held for 'never-claimed'"):
            await provider.get_ip("never-claimed", retry_delay=0)


@pytest.mark.asyncio
async def test_get_api_base_url_without_a_claim_is_an_error() -> None:
    provider = make_provider()
    async with provider:
        with pytest.raises(RuntimeError, match="No Fleet claim"):
            await provider.get_api_base_url("missing")


@pytest.mark.asyncio
async def test_restart_raises_and_leaves_the_claim_intact() -> None:
    """Restart-in-place does not exist for a bound claim.

    Silently re-claiming would change which sandbox the caller is talking to
    while looking like a restart, and the released address can be picked up by
    someone else rather than going dead. Refusing sends callers to stop/start,
    which re-resolves the address.
    """
    client = CountingClient()
    provider = make_provider(client)

    async with provider:
        await provider.run_vm("", "vm-1", {})
        before = await provider.get_api_base_url("vm-1")

        with pytest.raises(NotImplementedError) as error:
            await provider.restart_vm("vm-1")

        assert "cannot restart 'vm-1'" in str(error.value)
        assert "stop and start" in str(error.value)
        # A refused restart must not disturb the claim it declined to touch.
        assert client.deleted == []
        assert await provider.get_api_base_url("vm-1") == before

    assert client.deleted == ["claim-0"]


@pytest.mark.asyncio
async def test_max_concurrency_reports_the_autoscaling_ceiling() -> None:
    """Callers should size their worker count from the pool, not guess.

    Over-subscribing does not fail fast -- the excess claims block until
    others are released, which looks like a hang rather than saturation.
    """
    provider = make_provider()
    async with provider:
        assert await provider.max_concurrency() == 16


@pytest.mark.asyncio
async def test_max_concurrency_falls_back_to_replicas_without_autoscaling() -> None:
    """A pool that cannot grow cannot serve more than it has."""

    class FixedPoolClient(FakeClient):
        async def get_pool(self, name: str) -> FakePool:
            return FakePool(name=name, replicas=3, max_pool_size=None)

    provider = make_provider(FixedPoolClient())
    async with provider:
        assert await provider.max_concurrency() == 3


def test_factory_requires_a_pool() -> None:
    from computer.providers.factory import VMProviderFactory

    with pytest.raises(ValueError, match="requires a pool"):
        VMProviderFactory.create_provider("fleet")


def test_factory_passes_fleet_credentials_not_an_api_key() -> None:
    """``api_key`` is not a credential this system has.

    The factory has to forward the OAuth client pair, or a caller configuring
    the provider through it ends up unauthenticated no matter what it passes.
    """
    from computer.providers.factory import VMProviderFactory

    provider = VMProviderFactory.create_provider(
        "fleet",
        pool="bench-pool",
        client_id="factory-client",
        client_secret="factory-secret",
    )

    assert provider.client_id == "factory-client"
    assert provider.client_secret == "factory-secret"


def test_fleet_package_exports_the_provider() -> None:
    assert fleet_package.FleetProvider is FleetProvider


@pytest.mark.asyncio
async def test_computer_hands_the_interface_a_refreshable_header_source(
    monkeypatch: pytest.MonkeyPatch,
    token_endpoint: FakeTokenEndpoint,
) -> None:
    """The interface must be able to re-ask, not just be told once.

    ``Computer`` resolves the bearer while connecting. If it passes the
    resulting dict, the interface's reconnect loop replays that one token for
    the life of the session and every reconnect after ~15 minutes is rejected.
    Passing a callable is what makes the interface's per-attempt resolution
    reach the provider.
    """
    from computer.computer import Computer
    from computer.interface.factory import InterfaceFactory

    captured: dict[str, Any] = {}

    class FakeInterface:
        async def wait_for_ready(self, timeout: int = 60) -> None:
            return None

        def close(self) -> None:
            return None

    def fake_create_interface_for_os(**kwargs: Any) -> FakeInterface:
        captured.update(kwargs)
        return FakeInterface()

    monkeypatch.setattr(
        InterfaceFactory,
        "create_interface_for_os",
        staticmethod(fake_create_interface_for_os),
    )

    # Every token is born expired, so a source that re-resolves hands out a
    # new one each time and a captured dict cannot.
    token_endpoint.expires_in = 0
    provider = make_provider()
    computer = Computer(
        os_type="linux",
        name="vm-1",
        provider_type="fleet",
        telemetry_enabled=False,
    )

    async with provider:
        await provider.run_vm("", "vm-1", {})
        # The provider is already running; hand it to Computer rather than
        # letting run() build one, which is how a Fleet caller wires it up.
        computer.config.vm_provider = provider
        computer._provider_context = provider
        try:
            await computer.run()
        finally:
            if computer._stop_event is not None:
                computer._stop_event.set()

    source = captured["api_headers"]
    assert callable(source), "Computer passed a static dict; reconnects will replay it"
    first = await source()
    second = await source()
    assert first != second
    assert first["Authorization"].startswith("Bearer jwt-")
    assert second["Authorization"].startswith("Bearer jwt-")
