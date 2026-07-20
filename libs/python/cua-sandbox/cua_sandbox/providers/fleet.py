"""Fleet-backed Sandbox provider using the Cyclops Python SDK."""

from __future__ import annotations

import asyncio
import copy
import logging
import threading
from dataclasses import dataclass
from typing import Any, Mapping, Optional

from cua_sandbox.image import Image
from cua_sandbox.providers.base import ProvisionedSandbox
from cua_sandbox.transport.fleet import FleetTransport

logger = logging.getLogger(__name__)


@dataclass
class FleetSandboxHandle:
    """Fleet resources owned by one Sandbox instance."""

    pool: Any
    claim: Any
    bound: Any


class FleetProvider:
    """Provision one Fleet pool and claim for each Sandbox.

    The caller owns the supplied Cyclops SDK and must close it after all
    sandboxes using this provider have been disconnected or destroyed.
    """

    def __init__(
        self,
        *,
        sdk: Any,
        templates: Mapping[str, Mapping[str, Any]],
        service_name: str = "api",
        service_port: int = 8000,
    ) -> None:
        self._sdk = sdk
        self._templates = {
            os_type: copy.deepcopy(template) for os_type, template in templates.items()
        }
        self._service_name = service_name
        self._service_port = service_port
        self._call_lock = threading.Lock()

    async def create(
        self,
        image: Image,
        *,
        name: str,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
    ) -> ProvisionedSandbox:
        self._validate_image(image)
        if not name:
            raise ValueError("Fleet sandboxes require a name")
        if disk_gb is not None:
            raise ValueError("disk_gb is not supported by FleetProvider")
        if region != "us-east-1":
            raise ValueError("region is not supported by FleetProvider")
        if time_to_start is not None:
            raise ValueError("time_to_start is not configurable through FleetProvider")

        configured_template = self._templates.get(image.os_type)
        if configured_template is None:
            raise ValueError(f"No Fleet template configured for {image.os_type!r}")
        template = copy.deepcopy(configured_template)
        if cpu is not None:
            template["cpuCores"] = cpu
        if memory_mb is not None:
            template["memory"] = f"{memory_mb}Mi"

        pool_request = {
            "namespace": name,
            "spec": {
                "replicas": 1,
                "services": [
                    {
                        "name": self._service_name,
                        "targetPort": self._service_port,
                        "protocol": "TCP",
                    }
                ],
                "template": template,
            },
        }

        pool = await self._run(self._sdk.create_pool, pool_request)
        claim = None
        try:
            claim = await self._run(self._sdk.create_claim, {"pool": pool})
            bound = await self._run(self._sdk.wait_claim, claim)
        except BaseException:
            if claim is not None:
                await self._best_effort(self._sdk.delete_claim, claim, resource="claim")
            await self._best_effort(self._sdk.delete_pool, pool, resource="pool")
            raise

        handle = FleetSandboxHandle(pool=pool, claim=claim, bound=bound)
        transport = FleetTransport(
            sdk=self._sdk,
            bound=bound,
            service_name=self._service_name,
            timeout=request_timeout or 30.0,
            call_lock=self._call_lock,
        )
        return ProvisionedSandbox(name=name, transport=transport, handle=handle)

    async def destroy(self, handle: FleetSandboxHandle) -> None:
        claim_error = None
        try:
            await self._run(self._sdk.delete_claim, handle.claim)
        except Exception as error:
            claim_error = error
        try:
            await self._run(self._sdk.delete_pool, handle.pool)
        except Exception:
            if claim_error is not None:
                logger.warning("Failed to delete Fleet claim before pool cleanup: %s", claim_error)
            raise
        if claim_error is not None:
            raise claim_error

    async def _best_effort(self, operation: Any, resource_value: Any, *, resource: str) -> None:
        try:
            await self._run(operation, resource_value)
        except Exception:
            logger.warning("Failed to clean up Fleet %s after provisioning error", resource)

    async def _run(self, operation: Any, *args: Any, **kwargs: Any) -> Any:
        return await asyncio.to_thread(self._call_locked, operation, args, kwargs)

    def _call_locked(self, operation: Any, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        with self._call_lock:
            return operation(*args, **kwargs)

    @staticmethod
    def _validate_image(image: Image) -> None:
        if image._snapshot_source:
            raise ValueError("FleetProvider does not support snapshot images")
        if image._layers or image._env or image._ports or image._files:
            raise ValueError("FleetProvider does not yet support Image mutations")
        if image._disk_path:
            raise ValueError("FleetProvider does not support local disk images")
        if image._registry:
            raise ValueError("FleetProvider uses explicit templates, not registry Images")
