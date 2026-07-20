"""Fleet-backed implementation of the public cloud sandbox transport."""

from __future__ import annotations

import asyncio
import threading
import time
from typing import Any, Optional

import httpx

from cua_sandbox._config import get_base_url, get_client_id, get_client_secret, get_token_url
from cua_sandbox.image import Image
from cua_sandbox.transport.fleet import FleetTransport


class _FleetClient:
    """Minimal synchronous Fleet client used by the async transport wrapper."""

    def __init__(self) -> None:
        client_id = get_client_id()
        client_secret = get_client_secret()
        if not client_id or not client_secret:
            raise ValueError(
                "Fleet cloud sandboxes require CUA_CLIENT_ID and CUA_CLIENT_SECRET, "
                "or cua.configure(client_id=..., client_secret=...)."
            )
        token_response = httpx.post(
            get_token_url(),
            data={"grant_type": "client_credentials", "client_id": client_id, "client_secret": client_secret},
            headers={"Accept": "application/json"},
            timeout=30.0,
        )
        token_response.raise_for_status()
        self._base_url = get_base_url().rstrip("/")
        self._headers = {"Authorization": f"Bearer {token_response.json()['access_token']}"}

    def create_pool(self, request: dict[str, Any]) -> dict[str, Any]:
        namespace = request["namespace"]
        namespace_response = httpx.post(
            f"{self._base_url}/api/namespaces",
            json={"name": namespace},
            headers=self._headers,
            timeout=30.0,
        )
        if namespace_response.status_code not in {200, 201, 202, 409}:
            namespace_response.raise_for_status()
        namespace_created = namespace_response.status_code != 409
        body = {
            "apiVersion": "cua.ai/v1",
            "kind": "OSGymWorkspacePool",
            "metadata": {"name": namespace, "namespace": namespace},
            "spec": request["spec"],
        }
        response = httpx.post(self._pool_url(namespace), json=body, headers=self._headers, timeout=30.0)
        try:
            response.raise_for_status()
        except BaseException:
            if namespace_created:
                self.delete_namespace(namespace)
            raise
        return response.json()

    def create_claim(self, request: dict[str, Any]) -> dict[str, Any]:
        pool = self.wait_pool_ready(request["pool"])
        namespace = pool["metadata"]["namespace"]
        name = f"{pool['metadata']['name']}-claim"
        body = {
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxClaim",
            "metadata": {"name": name, "namespace": namespace},
            "spec": {"sandboxTemplateRef": {"name": f"{pool['metadata']['name']}-template"}},
        }
        response = httpx.post(self._claim_url(namespace), json=body, headers=self._headers, timeout=30.0)
        response.raise_for_status()
        return response.json()

    def wait_claim(self, claim: dict[str, Any]) -> dict[str, Any]:
        namespace = claim["metadata"]["namespace"]
        name = claim["metadata"]["name"]
        deadline = time.monotonic() + 600.0
        while True:
            response = httpx.get(f"{self._claim_url(namespace)}/{name}", headers=self._headers, timeout=30.0)
            response.raise_for_status()
            current = response.json()
            status = current.get("status") or {}
            phase = status.get("phase", "Pending")
            sandbox = (status.get("sandbox") or {}).get("name")
            if phase == "Bound" and sandbox:
                return {"namespace": namespace, "sandbox": sandbox, "services": ["server"]}
            if phase in {"Failed", "Error", "Expired"}:
                raise RuntimeError(f"Fleet claim {name!r} failed: {status}")
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Fleet claim {name!r} did not bind within 600 seconds")
            time.sleep(1)

    def delete_claim(self, claim: dict[str, Any]) -> None:
        metadata = claim["metadata"]
        httpx.delete(f"{self._claim_url(metadata['namespace'])}/{metadata['name']}", headers=self._headers, timeout=30.0).raise_for_status()

    def delete_pool(self, pool: dict[str, Any]) -> None:
        metadata = pool["metadata"]
        response = httpx.delete(
            f"{self._pool_url(metadata['namespace'])}/{metadata['name']}",
            headers=self._headers,
            timeout=30.0,
        )
        if response.status_code not in {200, 202, 204, 404}:
            response.raise_for_status()
        self.delete_namespace(metadata["namespace"])

    def delete_namespace(self, namespace: str) -> None:
        response = httpx.delete(
            f"{self._base_url}/api/namespaces/{namespace}",
            headers=self._headers,
            timeout=30.0,
        )
        if response.status_code not in {200, 202, 204, 404}:
            response.raise_for_status()

    def wait_pool_ready(self, pool: dict[str, Any]) -> dict[str, Any]:
        metadata = pool["metadata"]
        deadline = time.monotonic() + 600.0
        while True:
            response = httpx.get(
                f"{self._pool_url(metadata['namespace'])}/{metadata['name']}",
                headers=self._headers,
                timeout=30.0,
            )
            response.raise_for_status()
            current = response.json()
            if (current.get("status") or {}).get("availableCount", 0) > 0:
                return current
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Fleet pool {metadata['name']!r} did not become available within 600 seconds")
            time.sleep(2)

    def wait_service_ready(self, sandbox: dict[str, Any], service: str) -> None:
        client = self.service_client(sandbox, service)
        deadline = time.monotonic() + 600.0
        try:
            while True:
                response = client.get("/status")
                if response.status_code not in {502, 503, 504}:
                    response.raise_for_status()
                    return
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"Fleet service {service!r} did not become ready within 600 seconds")
                time.sleep(2)
        finally:
            client.close()

    def service_client(self, sandbox: dict[str, Any], service: str) -> httpx.Client:
        if service not in sandbox["services"]:
            raise ValueError(f"Fleet sandbox does not expose service {service!r}")
        return httpx.Client(
            base_url=f"{self._base_url}/api/svc/{sandbox['namespace']}/{sandbox['sandbox']}-{service}/",
            headers=self._headers,
            timeout=30.0,
        )

    def _pool_url(self, namespace: str) -> str:
        return f"{self._base_url}/api/k8s/apis/cua.ai/v1/namespaces/{namespace}/osgymworkspacepools"

    def _claim_url(self, namespace: str) -> str:
        return f"{self._base_url}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/{namespace}/osgymsandboxclaims"


class FleetCloudTransport(FleetTransport):
    """Provision a registry image through Fleet, then route computer-server calls."""

    def __init__(self, *, image: Image, name: str, cpu: Optional[int] = None, memory_mb: Optional[int] = None, request_timeout: Optional[float] = None) -> None:
        self._image = image
        self._name = name
        self._cpu = cpu
        self._memory_mb = memory_mb
        self._request_timeout = request_timeout or 30.0
        self._provisioned = False
        self._pool: Any = None
        self._claim: Any = None
        self._sdk: Any = None
        self._call_lock = threading.Lock()

    @property
    def name(self) -> str:
        return self._name

    async def connect(self) -> None:
        if not self._provisioned:
            self._validate_image(self._image)
            self._sdk = _FleetClient()
            self._pool = await self._run(self._sdk.create_pool, self._pool_request())
            try:
                self._claim = await self._run(self._sdk.create_claim, {"pool": self._pool})
                bound = await self._run(self._sdk.wait_claim, self._claim)
            except BaseException:
                await self._cleanup_resources()
                raise
            self._sdk.wait_service_ready(bound, "server")
            FleetTransport.__init__(self, sdk=self._sdk, bound=bound, service_name="server", timeout=self._request_timeout, call_lock=self._call_lock)
            self._provisioned = True
        await FleetTransport.connect(self)

    async def delete_vm(self) -> None:
        await self._cleanup_resources()

    async def _cleanup_resources(self) -> None:
        cleanup_error: Optional[BaseException] = None
        if self._claim is not None:
            try:
                await self._run(self._sdk.delete_claim, self._claim)
            except BaseException as error:
                cleanup_error = error
            finally:
                self._claim = None
        if self._pool is not None:
            try:
                await self._run(self._sdk.delete_pool, self._pool)
            except BaseException as error:
                if cleanup_error is None:
                    cleanup_error = error
            finally:
                self._pool = None
        if cleanup_error is not None:
            raise cleanup_error

    def _pool_request(self) -> dict[str, Any]:
        template: dict[str, Any] = {
            "containerDiskImage": self._image._registry,
            "imagePullSecret": "ecr-credentials",
            "probes": {"readinessProbe": {"tcpSocket": {"port": 8000}}},
        }
        if self._cpu is not None:
            template["cpuCores"] = self._cpu
        if self._memory_mb is not None:
            template["memory"] = f"{self._memory_mb}Mi"
        services = [{"name": "server", "targetPort": 8000, "protocol": "TCP"}]
        services.extend({"name": f"port-{port}", "targetPort": port, "protocol": "TCP"} for port in self._image._ports if port != 8000)
        return {"namespace": self._name, "spec": {"replicas": 1, "services": services, "template": template}}

    async def _run(self, operation: Any, *args: Any, **kwargs: Any) -> Any:
        return await asyncio.to_thread(self._call, operation, args, kwargs)

    def _call(self, operation: Any, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        with self._call_lock:
            return operation(*args, **kwargs)

    @staticmethod
    def _validate_image(image: Image) -> None:
        if not image._registry:
            raise ValueError("Fleet cloud sandboxes require Image.from_registry(...)")
        if image._layers or image._env or image._files or image._snapshot_source or image._disk_path:
            raise ValueError("Fleet cloud supports registry images with optional exposed services only")
