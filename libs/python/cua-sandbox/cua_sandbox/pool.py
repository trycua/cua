"""Fleet pool API for reusable cloud sandboxes."""

from __future__ import annotations

import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Any, cast

from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.fleet import FleetTransport
from cua_sandbox.transport.fleet_cloud import _FleetClient
from fleet_sdk import (
    ClaimSpec,
    CreateClaimRequest,
    CreatePoolRequest,
    SandboxTemplateRef,
)

logger = logging.getLogger(__name__)


class Pool:
    """A reconciled Fleet pool that can lease cloud sandboxes.

    ``reconcile`` creates a pool when it is absent and updates its desired
    configuration when it already exists. The supplied ``image`` must be an
    :meth:`Image.from_registry` image; Fleet does not support locally-built
    images, layers, snapshots, or custom disks.

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

    @asynccontextmanager
    async def claim(self, *, bind_deadline: int | None = None) -> AsyncIterator[Sandbox]:
        """Lease a sandbox and release its Fleet claim when the block exits.

        ``bind_deadline`` overrides the Fleet operator deadline in seconds.
        Leave it as ``None`` to use the operator default.

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
            spec = None
            if bind_deadline is not None:
                spec = ClaimSpec(
                    sandbox_template_ref=SandboxTemplateRef(name=self.name),
                    warmpool=None,
                    bind_deadline=bind_deadline,
                    lifecycle=None,
                )
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
