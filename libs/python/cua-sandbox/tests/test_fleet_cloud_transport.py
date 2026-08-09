from types import SimpleNamespace

import pytest
from cua_sandbox import Image
from cua_sandbox.transport import fleet_cloud
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
from fleet_sdk import Sandbox


def _pool(name="demo"):
    return SimpleNamespace(
        metadata=SimpleNamespace(name=name, namespace=name),
        spec=SimpleNamespace(sandbox_template_ref=SimpleNamespace(name=name)),
    )


def _bound():
    return Sandbox(namespace="demo", claim="demo-claim", name="sandbox", services=["server"])


def test_registry_image_becomes_typed_template_request():
    request = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace@sha256:abc").expose(3000),
        name="demo",
        cpu=4,
        memory_mb=8192,
    )._template_request()

    assert (request.namespace, request.name) == ("demo", "demo")
    vm_template = request.spec.vm_template
    assert vm_template.container_disk_image == "registry.example/workspace@sha256:abc"
    assert vm_template.cpu_cores == 4
    assert vm_template.memory == "8192Mi"
    assert [(service.name, service.target_port) for service in vm_template.services] == [
        ("server", 8000),
        ("port-3000", 3000),
    ]


def test_pool_request_uses_the_single_sandbox_name_and_requested_replicas():
    request = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace@sha256:abc"),
        name="demo",
        replicas=3,
    )._pool_request()

    assert request.namespace == "demo"
    assert request.spec.replicas == 3
    assert request.spec.sandbox_template_ref.name == "demo"
    assert request.spec.autoscaling is None


@pytest.mark.parametrize(
    "image", [Image.linux(), Image.from_registry("example:latest").apt_install("curl")]
)
def test_rejects_unsupported_images(image):
    with pytest.raises(ValueError):
        FleetCloudTransport._validate_image(image)


@pytest.mark.asyncio
async def test_image_connect_reconciles_named_resources_without_namespace_calls(monkeypatch):
    calls = []
    pool = _pool()
    bound = _bound()

    class Client:
        async def reconcile_pool(self, request):
            calls.append(("reconcile_pool", request))
            return pool

        async def reconcile_template(self, request):
            calls.append(("reconcile_template", request))
            return "template"

        async def wait_pool(self, reconciled_pool):
            calls.append(("wait_pool", reconciled_pool))
            return reconciled_pool

        async def create_claim(self, request):
            calls.append(("create_claim", request))
            return "claim"

        async def wait_claim(self, claim):
            calls.append(("wait_claim", claim))
            return bound

        async def wait_service_ready(self, sandbox, service, time_to_start):
            calls.append(("wait_service_ready", sandbox, service, time_to_start))

        async def get_namespace(self, name):
            raise AssertionError("Fleet sandbox transport must not get namespaces")

        async def create_namespace(self, name):
            raise AssertionError("Fleet sandbox transport must not create namespaces")

        async def create_pool(self, request):
            raise AssertionError("Fleet sandbox transport must reconcile pools")

        async def create_template(self, request):
            raise AssertionError("Fleet sandbox transport must reconcile templates")

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    await transport.connect()

    assert [call[0] for call in calls] == [
        "reconcile_pool",
        "reconcile_template",
        "wait_pool",
        "create_claim",
        "wait_claim",
        "wait_service_ready",
    ]
    pool_request = calls[0][1]
    template_request = calls[1][1]
    assert pool_request.namespace == "demo"
    assert pool_request.spec.sandbox_template_ref.name == "demo"
    assert pool_request.spec.replicas == 1
    assert (template_request.namespace, template_request.name) == ("demo", "demo")
    assert calls[-1] == ("wait_service_ready", bound, "server", 600.0)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "failure_stage",
    [
        "reconcile_pool",
        "reconcile_template",
        "wait_pool",
        "create_claim",
        "wait_claim",
        "service_ready",
    ],
)
async def test_connect_failure_releases_only_created_claim(monkeypatch, failure_stage):
    calls = []
    pool = _pool()
    bound = _bound()

    class Client:
        async def reconcile_pool(self, request):
            calls.append("reconcile_pool")
            if failure_stage == "reconcile_pool":
                raise RuntimeError("reconcile_pool failed")
            return pool

        async def reconcile_template(self, request):
            calls.append("reconcile_template")
            if failure_stage == "reconcile_template":
                raise RuntimeError("reconcile_template failed")
            return "template"

        async def wait_pool(self, reconciled_pool):
            calls.append("wait_pool")
            if failure_stage == "wait_pool":
                raise RuntimeError("wait_pool failed")
            return reconciled_pool

        async def create_claim(self, request):
            calls.append("create_claim")
            if failure_stage == "create_claim":
                raise RuntimeError("create_claim failed")
            return "claim"

        async def wait_claim(self, claim):
            calls.append("wait_claim")
            if failure_stage == "wait_claim":
                raise RuntimeError("wait_claim failed")
            return bound

        async def wait_service_ready(self, sandbox, service, time_to_start):
            calls.append("service_ready")
            if failure_stage == "service_ready":
                raise RuntimeError("service_ready failed")

        async def delete_claim(self, claim):
            calls.append("delete_claim")

        async def delete_pool(self, reconciled_pool):
            calls.append("delete_pool")

        async def delete_template(self, template):
            calls.append("delete_template")

        async def delete_namespace(self, name):
            calls.append("delete_namespace")

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")

    with pytest.raises(RuntimeError, match=f"{failure_stage} failed"):
        await transport.connect()

    claim_was_created = failure_stage in {"wait_claim", "service_ready"}
    assert ("delete_claim" in calls) is claim_was_created
    assert "delete_pool" not in calls
    assert "delete_template" not in calls
    assert "delete_namespace" not in calls
    assert transport._sdk is client
    assert transport._pool is (None if failure_stage == "reconcile_pool" else pool)
    template_reconciled = failure_stage not in {"reconcile_pool", "reconcile_template"}
    assert transport._template == ("template" if template_reconciled else None)
    assert transport._claim is None
    assert not transport._provisioned


@pytest.mark.asyncio
async def test_connect_failure_preserves_claim_when_claim_cleanup_fails(monkeypatch):
    pool = _pool()
    calls = []

    class Client:
        async def reconcile_pool(self, request):
            return pool

        async def reconcile_template(self, request):
            return "template"

        async def wait_pool(self, reconciled_pool):
            return reconciled_pool

        async def create_claim(self, request):
            return "claim"

        async def wait_claim(self, claim):
            raise RuntimeError("claim wait failed")

        async def delete_claim(self, claim):
            calls.append(("delete_claim", claim))
            raise RuntimeError("claim delete failed")

        async def delete_pool(self, reconciled_pool):
            raise AssertionError("failed claim cleanup must not delete the pool")

        async def delete_template(self, template):
            raise AssertionError("failed claim cleanup must not delete the template")

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")

    with pytest.raises(RuntimeError, match="claim wait failed") as error:
        await transport.connect()

    assert isinstance(error.value.__cause__, RuntimeError)
    assert str(error.value.__cause__) == "claim delete failed"
    assert calls == [("delete_claim", "claim")]
    assert transport._claim == "claim"
    assert transport._pool is pool
    assert transport._template == "template"


@pytest.mark.asyncio
async def test_connect_without_image_uses_existing_pool_and_claim_by_name(monkeypatch):
    calls = []
    pool = _pool()
    bound = _bound()

    class Client:
        async def get_pool(self, name):
            calls.append(("get_pool", name))
            return pool

        async def get_claim(self, existing_pool):
            calls.append(("get_claim", existing_pool))
            return "claim"

        async def wait_claim(self, claim):
            calls.append(("wait_claim", claim))
            return bound

        async def wait_service_ready(self, sandbox, service, time_to_start):
            calls.append(("wait_service_ready", sandbox, service, time_to_start))

        async def get_namespace(self, name):
            raise AssertionError("existing Fleet sandboxes must not get namespaces")

        async def create_namespace(self, name):
            raise AssertionError("existing Fleet sandboxes must not create namespaces")

        async def reconcile_pool(self, request):
            raise AssertionError("existing Fleet sandboxes must not reconcile pools")

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    await FleetCloudTransport(image=None, name="demo").connect()

    assert calls == [
        ("get_pool", "demo"),
        ("get_claim", pool),
        ("wait_claim", "claim"),
        ("wait_service_ready", bound, "server", 600.0),
    ]


@pytest.mark.asyncio
async def test_instance_cleanup_deletes_only_claim_and_preserves_infrastructure_state():
    calls = []

    class Client:
        async def delete_claim(self, claim):
            calls.append(("delete_claim", claim))

        async def delete_pool(self, pool):
            calls.append(("delete_pool", pool))

        async def delete_template(self, template):
            calls.append(("delete_template", template))

        async def delete_namespace(self, name):
            calls.append(("delete_namespace", name))

    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    transport._sdk = Client()
    transport._claim = "claim"
    transport._pool = "pool"
    transport._template = "template"
    transport._provisioned = True

    await transport._cleanup_resources()

    assert calls == [("delete_claim", "claim")]
    assert transport._claim is None
    assert transport._pool == "pool"
    assert transport._template == "template"
    assert not transport._provisioned


@pytest.mark.asyncio
async def test_instance_cleanup_stops_after_claim_failure_and_preserves_state():
    calls = []

    class Client:
        async def delete_claim(self, claim):
            calls.append(("delete_claim", claim))
            raise RuntimeError("claim delete failed")

        async def delete_pool(self, pool):
            raise AssertionError("claim cleanup failure must not delete the pool")

        async def delete_template(self, template):
            raise AssertionError("claim cleanup failure must not delete the template")

    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    transport._sdk = Client()
    transport._claim = "claim"
    transport._pool = "pool"
    transport._template = "template"
    transport._provisioned = True

    with pytest.raises(RuntimeError, match="claim delete failed"):
        await transport._cleanup_resources()

    assert calls == [("delete_claim", "claim")]
    assert transport._claim == "claim"
    assert transport._pool == "pool"
    assert transport._template == "template"
    assert transport._provisioned


@pytest.mark.asyncio
async def test_delete_sandbox_resolves_and_deletes_only_deterministic_claim(monkeypatch):
    calls = []
    pool = _pool("demo")

    class Client:
        async def get_pool(self, name):
            calls.append(("get_pool", name))
            return pool

        async def get_claim(self, existing_pool):
            calls.append(("get_claim", existing_pool))
            return "demo-claim"

        async def delete_claim(self, claim):
            calls.append(("delete_claim", claim))

        async def delete_pool(self, existing_pool):
            calls.append(("delete_pool", existing_pool))

        async def delete_template(self, template):
            calls.append(("delete_template", template))

        async def delete_namespace(self, name):
            calls.append(("delete_namespace", name))

        async def close(self):
            calls.append(("close",))

    monkeypatch.setattr(fleet_cloud, "_FleetClient", Client)

    await FleetCloudTransport.delete_sandbox("demo")

    assert calls == [
        ("get_pool", "demo"),
        ("get_claim", pool),
        ("delete_claim", "demo-claim"),
        ("close",),
    ]


@pytest.mark.asyncio
async def test_delete_sandbox_closes_sdk_when_claim_delete_fails(monkeypatch):
    calls = []
    pool = _pool("demo")

    class Client:
        async def get_pool(self, name):
            calls.append(("get_pool", name))
            return pool

        async def get_claim(self, existing_pool):
            calls.append(("get_claim", existing_pool))
            return "demo-claim"

        async def delete_claim(self, claim):
            calls.append(("delete_claim", claim))
            raise RuntimeError("claim delete failed")

        async def delete_pool(self, existing_pool):
            raise AssertionError("class delete must not delete pools")

        async def delete_template(self, template):
            raise AssertionError("class delete must not delete templates")

        async def delete_namespace(self, name):
            raise AssertionError("class delete must not delete namespaces")

        async def close(self):
            calls.append(("close",))

    monkeypatch.setattr(fleet_cloud, "_FleetClient", Client)

    with pytest.raises(RuntimeError, match="claim delete failed"):
        await FleetCloudTransport.delete_sandbox("demo")

    assert calls == [
        ("get_pool", "demo"),
        ("get_claim", pool),
        ("delete_claim", "demo-claim"),
        ("close",),
    ]


@pytest.mark.asyncio
async def test_forward_tunnel_uses_named_service_url():
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    transport._provisioned = True
    transport._bound = Sandbox(
        namespace="demo", claim="claim", name="sandbox", services=["port-3000"]
    )

    class Client:
        def service_url(self, sandbox, service):
            assert service == "port-3000"
            return "https://run.cua.ai/api/svc/demo/sandbox-port-3000/"

    transport._sdk = Client()
    tunnel = await transport.forward_tunnel(3000)
    assert tunnel.url == "https://run.cua.ai/api/svc/demo/sandbox-port-3000/"


@pytest.mark.asyncio
async def test_snapshot_is_unsupported():
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    with pytest.raises(NotImplementedError, match="Snapshots are not supported"):
        await transport.create_snapshot()
