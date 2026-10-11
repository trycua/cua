"""In-memory Fleet for hermetic managed-pool tests.

Implements the ``_FleetClient`` surface cua-sandbox uses, backed by real
``fleet_sdk`` records. Install it through the existing seam::

    fleet = FakeFleet()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", fleet.client)

Nothing here touches the network, the host or real Fleet resources.
"""

from __future__ import annotations

import itertools
import threading
from datetime import datetime, timezone
from typing import Any

from fleet_sdk import (
    Claim,
    ClaimSpec,
    Namespace,
    OsGymSandboxClaimCondition,
    OsGymSandboxClaimStatus,
    OsGymSandboxWarmPoolStatus,
    Pool,
    ResourceMetadata,
)
from fleet_sdk import Sandbox as BoundSandbox
from fleet_sdk import (
    SdkError,
)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _status_error(operation: str, status: int, body: str = "") -> SdkError:
    return SdkError.Status(operation, status, body)


class FakeFleet:
    """Shared state for every client a test creates."""

    def __init__(self, *, tenant: str = "tenant-a") -> None:
        self.tenant = tenant
        self.lock = threading.Lock()
        self.namespaces: dict[str, dict[str, str]] = {}
        self.pools: dict[str, Pool] = {}
        self.templates: dict[str, Any] = {}
        self.claims: dict[tuple[str, str], Claim] = {}
        self.foreign: set[str] = set()  # names held by another account
        self.terminating: set[str] = set()  # own namespaces being deleted
        self.adoption_lag: dict[str, int] = {}  # 403s before Capsule adopts
        self.hidden: dict[str, int] = {}  # list_namespaces calls that miss a name
        self.calls: list[tuple] = []
        self.renewals: list[tuple[str, str]] = []
        self.patches: list[tuple[str, str, str, dict]] = []
        self.ready_replicas: dict[str, int] = {}
        self.fail_bind: dict[str, str] = {}  # pool -> failure reason
        self.clients: list["FakeFleetClient"] = []
        self._ids = itertools.count(1)

    def client(self) -> "FakeFleetClient":
        client = FakeFleetClient(self)
        self.clients.append(client)
        return client

    def seed_pool(
        self,
        name: str,
        *,
        labels: dict[str, str] | None = None,
        created: str | None = None,
        replicas: int = 0,
    ) -> Pool:
        from fleet_sdk import OsGymSandboxWarmPoolSpecBuilder, SandboxTemplateRefBuilder

        self.namespaces[name] = {}
        pool = Pool(
            api_version="osgym.cua.ai/v1alpha1",
            kind="OSGymSandboxWarmPool",
            metadata=ResourceMetadata(
                namespace=name,
                name=name,
                labels=labels,
                creation_timestamp=created or _now(),
            ),
            spec=OsGymSandboxWarmPoolSpecBuilder()
            .replicas(replicas)
            .sandbox_template_ref(SandboxTemplateRefBuilder().name(name).build())
            .build(),
            status=None,
        )
        self.pools[name] = pool
        self.templates[name] = object()
        return pool

    def seed_claim(
        self,
        pool: str,
        name: str,
        *,
        phase: str = "Bound",
        created: str | None = None,
        managed: bool = False,
    ) -> Claim:
        claim = Claim(
            api_version="osgym.cua.ai/v1alpha1",
            kind="OSGymSandboxClaim",
            metadata=ResourceMetadata(
                namespace=pool,
                name=name,
                labels={"cua.ai/managed-by": "cua-sdk"} if managed else None,
                creation_timestamp=created or _now(),
            ),
            spec=ClaimSpec(
                sandbox_template_ref=self.pools[pool].spec.sandbox_template_ref,
                warmpool=pool,
                bind_deadline=None,
                lifecycle=None,
                ttl_seconds_after_created=None,
            ),
            status=OsGymSandboxClaimStatus(phase=phase, conditions=None, sandbox=None),
        )
        self.claims[(pool, name)] = claim
        return claim


class FakeFleetClient:
    def __init__(self, fleet: FakeFleet) -> None:
        self.fleet = fleet
        self._closed = False
        self._authenticated = False

    # --- identity ---

    async def close(self) -> None:
        self._closed = True

    # --- namespaces / pools / templates ---

    async def list_namespaces(self) -> list[Namespace]:
        self._authenticated = True
        self.fleet.calls.append(("list_namespaces",))
        hidden = {name for name, left in self.fleet.hidden.items() if left > 0}
        for name in hidden:
            self.fleet.hidden[name] -= 1
        return [
            Namespace(
                name=name,
                status="Terminating" if name in self.fleet.terminating else "Active",
                created_at=labels.get("__created", _now()) if labels else _now(),
                labels=None,
            )
            for name, labels in self.fleet.namespaces.items()
            if name not in hidden
        ]

    async def get_pool(self, name: str) -> Pool:
        self.fleet.calls.append(("get_pool", name))
        if name in self.fleet.foreign:
            raise _status_error("get pool", 403, "forbidden")
        pool = self.fleet.pools.get(name)
        if pool is None:
            raise _status_error("get pool", 404, "not found")
        ready = self.fleet.ready_replicas.get(name)
        if ready is not None:
            pool.status = OsGymSandboxWarmPoolStatus(
                replicas=ready, ready_replicas=ready, selector=None
            )
        return pool

    async def create_namespace(self, name: str) -> Namespace:
        self.fleet.calls.append(("create_namespace", name))
        if name in self.fleet.foreign or name in self.fleet.namespaces:
            raise _status_error("create namespace", 409, "exists")
        self.fleet.namespaces[name] = {}
        return Namespace(name=name, status="Active", created_at=_now(), labels=None)

    async def create_pool(self, request: Any) -> Pool:
        name = request.namespace
        self.fleet.calls.append(("create_pool", name, request.spec))
        if name in self.fleet.foreign:
            raise SdkError.PoolAccessDenied("create pool", name, 403, "capsule denied")
        if self.fleet.adoption_lag.get(name):
            self.fleet.adoption_lag[name] -= 1
            raise SdkError.PoolAccessDenied("create pool", name, 403, "not adopted yet")
        if name in self.fleet.pools:
            raise _status_error("create pool", 409, "exists")
        self.fleet.namespaces[name] = {}
        pool = Pool(
            api_version="osgym.cua.ai/v1alpha1",
            kind="OSGymSandboxWarmPool",
            metadata=ResourceMetadata(
                namespace=name, name=name, labels=None, creation_timestamp=_now()
            ),
            spec=request.spec,
            status=None,
        )
        self.fleet.pools[name] = pool
        return pool

    async def reconcile_pool(self, request: Any) -> Pool:  # pragma: no cover - must not be used
        raise AssertionError("managed pools must never reconcile (it fights KEDA)")

    async def update_pool(self, pool: Any) -> Pool:  # pragma: no cover - must not be used
        raise AssertionError("managed pools must never rewrite a pool spec")

    async def reconcile_template(self, request: Any) -> Any:
        self.fleet.calls.append(("reconcile_template", request.name))
        self.fleet.templates[request.name] = request
        return request

    async def get_template(self, namespace: str, name: str) -> Any:
        if name not in self.fleet.templates:
            raise _status_error("get template", 404)
        return name

    async def delete_template(self, template: Any) -> None:
        self.fleet.calls.append(("delete_template", template))
        self.fleet.templates.pop(template, None)

    async def delete_pool(self, pool: Any) -> None:
        name = pool.metadata.name
        self.fleet.calls.append(("delete_pool", name))
        self.fleet.pools.pop(name, None)

    async def delete_namespace(self, name: str) -> None:
        self.fleet.calls.append(("delete_namespace", name))
        self.fleet.namespaces.pop(name, None)

    async def create_claim(self, request: Any) -> Claim:
        pool = request.pool
        namespace = pool.metadata.namespace
        name = request.name or f"{namespace}-claim-{next(self.fleet._ids)}"
        self.fleet.calls.append(("create_claim", namespace, name, request.spec))
        if (namespace, name) in self.fleet.claims:
            raise _status_error("create claim", 409)
        claim = Claim(
            api_version="osgym.cua.ai/v1alpha1",
            kind="OSGymSandboxClaim",
            metadata=ResourceMetadata(
                namespace=namespace, name=name, labels=None, creation_timestamp=_now()
            ),
            spec=request.spec
            or ClaimSpec(
                sandbox_template_ref=pool.spec.sandbox_template_ref,
                warmpool=None,
                bind_deadline=None,
                lifecycle=None,
            ),
            status=OsGymSandboxClaimStatus(phase="Pending", conditions=None, sandbox=None),
        )
        self.fleet.claims[(namespace, name)] = claim
        return claim

    async def list_claims(self, namespace: str) -> list[Claim]:
        if namespace not in self.fleet.pools:
            raise _status_error("list claims", 404)
        return [c for (ns, _), c in self.fleet.claims.items() if ns == namespace]

    async def wait_claim(self, claim: Any) -> BoundSandbox:
        namespace, name = claim.metadata.namespace, claim.metadata.name
        self.fleet.calls.append(("wait_claim", namespace, name))
        stored = self.fleet.claims.get((namespace, name))
        if stored is None:
            raise _status_error("wait claim", 404)
        reason = self.fleet.fail_bind.get(namespace)
        if reason is not None:
            stored.status = OsGymSandboxClaimStatus(
                phase="Failed",
                conditions=[
                    OsGymSandboxClaimCondition(
                        type="Bound",
                        status="False",
                        reason=reason,
                        message="no sandbox became available",
                        last_transition_time=None,
                    )
                ],
                sandbox=None,
            )
            raise SdkError.ClaimFailed("Failed", reason)
        stored.status = OsGymSandboxClaimStatus(phase="Bound", conditions=None, sandbox=None)
        return BoundSandbox(
            namespace=namespace, claim=name, name=f"sbx-{name}", services=["env", "server"]
        )

    async def wait_service_ready(self, sandbox: Any, service: str, time_to_start=None) -> None:
        return None

    async def renew_claim(self, claim: Any, shutdown_time: str) -> Any:
        key = (claim.metadata.namespace, claim.metadata.name)
        with self.fleet.lock:
            self.fleet.renewals.append((claim.metadata.name, shutdown_time))
        if key not in self.fleet.claims:
            raise _status_error("renew claim", 404)
        return self.fleet.claims[key]

    async def delete_claim(self, claim: Any) -> None:
        key = (claim.metadata.namespace, claim.metadata.name)
        self.fleet.calls.append(("delete_claim", *key))
        if key not in self.fleet.claims:
            raise _status_error("delete claim", 404)
        del self.fleet.claims[key]

    def service_url(self, sandbox: Any, service: str) -> str:
        return f"https://fleet.invalid/api/svc/{sandbox.namespace}/{sandbox.name}-{service}/"
