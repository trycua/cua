import pytest
from cua_sandbox import Image
from cua_sandbox.transport import fleet_cloud
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
from fleet_sdk import Sandbox


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


def test_pool_request_uses_the_sandbox_name_and_requested_replicas():
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
    pool = type("Pool", (), {"metadata": type("Metadata", (), {"name": "demo"})()})()
    bound = Sandbox(namespace="demo", claim="demo-claim", name="sandbox", services=["server"])

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

        async def delete_namespace(self, name):
            raise AssertionError("Fleet sandbox transport must not delete namespaces")

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    transport = FleetCloudTransport(
        image=Image.from_registry("example:latest"), name="demo"
    )
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
async def test_connect_without_image_uses_existing_pool_and_claim_by_name(monkeypatch):
    calls = []
    pool = type("Pool", (), {"metadata": type("Metadata", (), {"name": "demo"})()})()
    bound = Sandbox(namespace="demo", claim="demo-claim", name="sandbox", services=["server"])

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
async def test_cleanup_deletes_claim_pool_and_template_but_not_namespace():
    calls = []

    class Client:
        async def delete_claim(self, claim):
            calls.append(("delete_claim", claim))

        async def delete_pool(self, pool):
            calls.append(("delete_pool", pool))

        async def delete_template(self, template):
            calls.append(("delete_template", template))

        async def delete_namespace(self, name):
            raise AssertionError("Fleet sandbox cleanup must not delete namespaces")

    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    transport._sdk = Client()
    transport._claim = "claim"
    transport._pool = "pool"
    transport._template = "template"

    await transport._cleanup_resources()

    assert calls == [
        ("delete_claim", "claim"),
        ("delete_pool", "pool"),
        ("delete_template", "template"),
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
