"""Fleet pool API for reusable cloud sandboxes."""

from __future__ import annotations

import logging
from collections.abc import AsyncIterator, Mapping
from contextlib import asynccontextmanager
from typing import Any, cast

from cua_sandbox.image import Image
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.fleet import FleetTransport
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport, _FleetClient
from fleet_sdk import CreateClaimRequest

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
    async def reconcile(cls, config: Mapping[str, Any]) -> "Pool":
        """Create or update a Fleet pool from its desired configuration.

        Repeated calls with the same configuration are idempotent: an existing
        pool is updated in place rather than creating another pool. ``config``
        requires a non-empty ``name`` and an ``image`` created with
        :meth:`Image.from_registry`.

        The Fleet client used to reconcile is closed before this method returns.
        """
        name, image = cls._validate_config(config)
        desired = FleetCloudTransport(
            image=image,
            name=name,
            replicas=config.get("replicas", 1),
            services=config.get("services"),
        )._pool_request()
        client = _FleetClient()
        try:
            return cls(await client.reconcile_pool(desired))
        finally:
            await client.close()

    @asynccontextmanager
    async def claim(self) -> AsyncIterator[Sandbox]:
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
            claim = await client.create_claim(CreateClaimRequest(pool=self._resource, spec=None))
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

    @staticmethod
    def _validate_config(config: Mapping[str, Any]) -> tuple[str, Image]:
        if not isinstance(config, Mapping):
            raise TypeError("Pool configuration must be a mapping")

        name = config.get("name")
        if not isinstance(name, str) or not name:
            raise ValueError("Pool configuration requires a non-empty string 'name'")

        image = config.get("image")
        if not isinstance(image, Image):
            raise TypeError("Pool configuration 'image' must be an Image.from_registry(...) value")
        FleetCloudTransport._validate_image(image)
        return name, image
