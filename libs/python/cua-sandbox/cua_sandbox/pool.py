"""Fleet template, pool, and lease APIs for reusable cloud sandboxes.

Two claim styles live here. :meth:`Pool.claim` is scope-bound: an ``async
with`` block leases a sandbox and releases it on exit, which suits scripts and
notebooks. :meth:`Pool.create_claim` returns a :class:`Lease` — a serializable
claim reference whose wait/renew/release steps are explicit, for durable
orchestrators whose claim outlives any single process scope.
"""

from __future__ import annotations

import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Any, cast

from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.fleet import FleetTransport
from cua_sandbox.transport.fleet_cloud import _FleetClient
from fleet_sdk import (
    Claim,
    ClaimSpec,
    CreateClaimRequest,
    CreatePoolRequest,
    CreateTemplateRequest,
    CyclopsClient,
    ResourceMetadata,
)
from fleet_sdk import Sandbox as FleetSandbox
from fleet_sdk import (
    SandboxTemplateRef,
)

logger = logging.getLogger(__name__)


def supports_claim_renewal() -> bool:
    """Whether the installed cua-fleet release can push a claim lease forward.

    ``Lease.renew`` needs ``CyclopsClient.renew_claim``, which older cua-fleet
    wheels do not ship. Callers that hold a claim beyond one lease should check
    this once and, when it is False, size the claim's initial
    ``lifecycle.shutdownTime`` to cover the whole hold instead of renewing.
    """
    return hasattr(CyclopsClient, "renew_claim")


class Template:
    """A reconciled Fleet sandbox template.

    A template holds the VM shape — image, resources, firmware, exposed
    services — that warm pools and claims reference by name through
    ``spec.sandboxTemplateRef``. Reconcile a template before reconciling a
    pool that points at it.

    Template instances retain only Fleet resource metadata; each call opens
    and closes its own Fleet client.
    """

    def __init__(self, resource: Any) -> None:
        self._resource = resource

    @property
    def name(self) -> str:
        """The template name."""
        return cast(str, self._resource.metadata.name)

    @property
    def resource(self) -> Any:
        """The underlying Fleet SDK template resource."""
        return self._resource

    @classmethod
    async def reconcile(cls, request: CreateTemplateRequest) -> "Template":
        """Create or update a Fleet sandbox template from a native request.

        The request is passed unchanged to the generated Fleet client. Import
        ``CreateTemplateRequest`` and its nested schema types from
        ``cua_sandbox`` so callers do not depend on the generated binding
        package directly.
        """
        if not isinstance(request, CreateTemplateRequest):
            raise TypeError("Template.reconcile requires a CreateTemplateRequest")
        client = _FleetClient()
        try:
            return cls(await client.reconcile_template(request))
        finally:
            await client.close()


def _claim_stub(namespace: str, name: str) -> Claim:
    """A minimal Claim carrying only its identity.

    get/delete/renew/wait address a claim purely by metadata (namespace +
    name), so a reference reconstructed from serialized state needs no spec
    content — the placeholder template ref is never sent anywhere.
    """
    return Claim(
        api_version="osgym.cua.ai/v1alpha1",
        kind="OSGymSandboxClaim",
        metadata=ResourceMetadata(namespace=namespace, name=name, labels=None),
        spec=ClaimSpec(
            sandbox_template_ref=SandboxTemplateRef(name=""),
            warmpool=None,
            bind_deadline=None,
            lifecycle=None,
        ),
        status=None,
    )


class Lease:
    """A held Fleet sandbox claim with an explicit, serializable lifecycle.

    :meth:`Pool.claim` scopes a lease to one ``async with`` block — claim and
    release are welded to a single Python scope, which suits scripts. A Lease
    instead carries the claim *by reference* (namespace + claim name, plus the
    bound sandbox record once known), so a durable orchestrator — a Temporal
    workflow, a queue worker — can create the claim in one process, reattach
    from serialized state in another, renew periodically, and release from a
    third. ``to_dict``/``from_dict`` round-trip plain JSON-safe data.

    Nothing is released automatically: the holder owns calling
    :meth:`release`. Guard against holders that die without releasing by
    setting ``lifecycle.shutdownTime`` on the claim spec (the pool operator's
    reaper deletes Bound claims whose deadline passed) and, for holds longer
    than one lease, pushing it forward with :meth:`renew` — available when
    :func:`supports_claim_renewal` is true.

    Each method opens and closes its own Fleet client, except :meth:`wait`,
    whose returned :class:`Sandbox` keeps its client until ``disconnect``.
    """

    def __init__(self, *, namespace: str, name: str, bound: Any = None) -> None:
        self._namespace = namespace
        self._name = name
        self._bound = bound

    @property
    def namespace(self) -> str:
        """The pool namespace the claim lives in."""
        return self._namespace

    @property
    def name(self) -> str:
        """The claim name."""
        return self._name

    def to_dict(self) -> dict:
        """JSON-safe reference to this lease, including the bound sandbox
        record once :meth:`wait` has observed the bind."""
        data: dict = {"namespace": self._namespace, "name": self._name}
        if self._bound is not None:
            data["sandbox"] = {
                "namespace": self._bound.namespace,
                "claim": self._bound.claim,
                "name": self._bound.name,
                "services": list(self._bound.services),
            }
        return data

    @classmethod
    def from_dict(cls, data: dict) -> "Lease":
        """Rebuild a lease reference produced by :meth:`to_dict`."""
        bound = data.get("sandbox")
        return cls(
            namespace=data["namespace"],
            name=data["name"],
            bound=FleetSandbox(**bound) if bound else None,
        )

    async def wait(self, *, service: str = "server") -> Sandbox:
        """Wait for the claim to bind and return a connected :class:`Sandbox`.

        When this lease already carries the bound sandbox record (a reattach
        via :meth:`from_dict`), no polling happens — the transport connects
        directly. ``service`` names the sandbox service the default transport
        pins to; it must be declared by the claim's template. The returned
        sandbox owns its Fleet client: ``await sandbox.disconnect()`` releases
        the client's HTTP resources (the claim itself stays held).
        """
        client = _FleetClient()
        try:
            if self._bound is None:
                self._bound = await client.wait_claim(_claim_stub(self._namespace, self._name))
            sandbox = Sandbox(
                FleetTransport(sdk=client, bound=self._bound, service_name=service, owns_sdk=True),
                name=self._bound.name,
            )
            await sandbox._connect()
            return sandbox
        except BaseException:
            await client.close()
            raise

    async def renew(self, shutdown_time: str) -> None:
        """Push the claim's ``spec.lifecycle.shutdownTime`` forward.

        ``shutdown_time`` is an absolute ISO-8601 UTC expiry. Requires a
        cua-fleet release with ``CyclopsClient.renew_claim`` (see
        :func:`supports_claim_renewal`); raises ``RuntimeError`` otherwise.
        """
        client = _FleetClient()
        try:
            await client.renew_claim(_claim_stub(self._namespace, self._name), shutdown_time)
        finally:
            await client.close()

    async def release(self) -> None:
        """Delete the claim, returning the sandbox to the pool.

        Idempotent: releasing an already-deleted (or reaped) claim succeeds.
        """
        client = _FleetClient()
        try:
            await client.delete_claim(_claim_stub(self._namespace, self._name))
        finally:
            await client.close()


class Pool:
    """A reconciled Fleet pool that can lease cloud sandboxes.

    ``reconcile`` creates a pool when it is absent and updates its desired
    configuration when it already exists. A pool carries no VM shape of its
    own: it names a :class:`Template` through ``spec.sandboxTemplateRef``, so
    reconcile that template first. Fleet templates require an
    :meth:`Image.from_registry` image; locally-built images, layers,
    snapshots, and custom disks are not supported.

    Pool instances retain only Fleet resource metadata. Each call to
    :meth:`claim` creates and closes its own Fleet client, so a Pool may be
    reused for sequential claims without the caller managing client lifetime.
    """

    def __init__(self, resource: Any) -> None:
        self._resource = resource

    @property
    def name(self) -> str:
        """The pool name."""
        return cast(str, self._resource.metadata.name)

    @property
    def resource(self) -> Any:
        """The underlying Fleet SDK pool resource."""
        return self._resource

    @classmethod
    async def reconcile(cls, request: CreatePoolRequest) -> "Pool":
        """Create or update a Fleet pool from a native Fleet request.

        The request is passed unchanged to the generated Fleet client. Import
        ``CreatePoolRequest`` and its nested schema types from ``cua_sandbox``
        so callers do not depend on the generated binding package directly.
        """
        if not isinstance(request, CreatePoolRequest):
            raise TypeError("Pool.reconcile requires a CreatePoolRequest")
        client = _FleetClient()
        try:
            return cls(await client.reconcile_pool(request))
        finally:
            await client.close()

    @classmethod
    async def get(cls, name: str) -> "Pool":
        """Fetch an existing Fleet pool by name without changing its spec.

        Use this when the pool was reconciled elsewhere and the caller only
        needs a handle to claim against — :meth:`reconcile` would PATCH the
        spec, :meth:`get` never writes.
        """
        client = _FleetClient()
        try:
            return cls(await client.get_pool(name))
        finally:
            await client.close()

    async def create_claim(self, *, spec: ClaimSpec | None = None, name: str | None = None) -> Lease:
        """Create a claim against this pool and return it as a :class:`Lease`.

        The claim is created and left held — nothing waits for the bind and
        nothing releases on scope exit; the returned lease's ``wait``/
        ``release`` drive the rest of the lifecycle explicitly, possibly from
        other processes via ``Lease.to_dict``/``from_dict``. Prefer
        :meth:`claim` when a single ``async with`` scope fits the caller.

        A client-supplied ``name`` is used verbatim as the claim name; left
        unset, the client generates a random ``claim-<petname>`` so concurrent
        leases and retries cannot collide. Explicit names need a cua-fleet
        release whose ``CreateClaimRequest`` carries a name field; older wheels
        raise ``RuntimeError`` with an upgrade pointer.
        """
        if name is None:
            request = CreateClaimRequest(pool=self._resource, spec=spec)
        else:
            try:
                request = CreateClaimRequest(pool=self._resource, spec=spec, name=name)
            except TypeError as error:
                raise RuntimeError(
                    "the installed cua-fleet release does not support client-supplied "
                    "claim names; upgrade to a build whose CreateClaimRequest carries "
                    "a name field"
                ) from error
        client = _FleetClient()
        try:
            claim = await client.create_claim(request)
            return Lease(namespace=claim.metadata.namespace, name=claim.metadata.name)
        finally:
            await client.close()

    @asynccontextmanager
    async def claim(self, *, spec: ClaimSpec | None = None) -> AsyncIterator[Sandbox]:
        """Lease a sandbox and release its Fleet claim when the block exits.

        A claim is released after both normal and exceptional block exits. If
        release fails while the block is already raising, the original block
        exception is preserved and the release failure is logged. Otherwise,
        the release failure is raised to the caller.
        """
        client = _FleetClient()
        claim: Any = None
        sandbox: Sandbox | None = None
        primary_error: BaseException | None = None
        cleanup_error: Exception | None = None

        try:
            claim = await client.create_claim(CreateClaimRequest(pool=self._resource, spec=spec))
            bound = await client.wait_claim(claim)
            sandbox = Sandbox(
                FleetTransport(sdk=client, bound=bound, service_name="server"), name=bound.name
            )
            await sandbox._connect()
            yield sandbox
        except BaseException as error:
            primary_error = error
            raise
        finally:
            if sandbox is not None:
                try:
                    await sandbox.disconnect()
                except Exception:
                    logger.exception("Failed to disconnect claimed sandbox %r", sandbox.name)

            if claim is not None:
                try:
                    await client.delete_claim(claim)
                except Exception as error:
                    if primary_error is not None:
                        logger.exception("Failed to release Fleet claim after an earlier error")
                    else:
                        cleanup_error = error

            try:
                await client.close()
            except Exception:
                if primary_error is not None or cleanup_error is not None:
                    logger.exception("Failed to close Fleet client")
                else:
                    raise

            if cleanup_error is not None:
                raise cleanup_error
