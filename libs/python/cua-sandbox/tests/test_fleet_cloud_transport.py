import pytest
from cua_sandbox import Image
from cua_sandbox.transport import fleet_cloud
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
from fleet_sdk import Sandbox, SdkError


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


def test_pool_request_only_references_the_template():
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
async def test_connect_creates_template_before_pool_and_defaults_the_claim(monkeypatch):
    pool = type("Pool", (), {"metadata": type("Metadata", (), {"name": "demo"})()})()
    requests = []
    order = []

    class Client:
        async def get_namespace(self, name):
            order.append("get-namespace")
            raise SdkError.Status("get namespace", 404, "missing")

        async def create_namespace(self, name):
            order.append("create-namespace")
            return "namespace"

        async def create_template(self, request):
            order.append("template")
            assert request.spec.vm_template.container_disk_image == "example:latest"
            return "template"

        async def create_pool(self, request):
            order.append("pool")
            assert request.spec.sandbox_template_ref.name == "demo"
            return pool

        async def wait_pool(self, created_pool):
            return created_pool

        async def create_claim(self, request):
            requests.append(request)
            return "claim"

        async def wait_claim(self, claim):
            return Sandbox(namespace="demo", claim=claim, name="sandbox", services=["server"])

        async def wait_service_ready(self, sandbox, service, time_to_start):
            assert service == "server"

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    await FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo").connect()

    assert order == ["get-namespace", "create-namespace", "template", "pool"]
    assert len(requests) == 1
    assert requests[0].spec is None


@pytest.mark.asyncio
async def test_connect_creates_missing_namespace_before_template(monkeypatch):
    order = []

    class Client:
        async def get_namespace(self, name):
            order.append(("get-namespace", name))
            raise SdkError.Status("get namespace", 404, "missing")

        async def create_namespace(self, name):
            order.append(("create-namespace", name))
            return object()

        async def create_template(self, request):
            order.append(("template", request.name))
            raise RuntimeError("template failed")

        async def delete_namespace(self, namespace):
            order.append(("delete-namespace", namespace))

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="template failed"):
        await FleetCloudTransport(
            image=Image.from_registry("example:latest"), name="demo"
        ).connect()

    assert order == [
        ("get-namespace", "demo"),
        ("create-namespace", "demo"),
        ("template", "demo"),
        ("delete-namespace", "demo"),
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize("status", [403, 500])
async def test_connect_propagates_unexpected_namespace_lookup_errors_without_create(
    monkeypatch, status
):
    calls = []

    class Client:
        async def get_namespace(self, name):
            calls.append(("get-namespace", name))
            raise SdkError.Status("get namespace", status, "unexpected")

        async def create_namespace(self, name):
            calls.append(("create-namespace", name))
            raise AssertionError("unexpected namespace lookup errors must not create namespaces")

    monkeypatch.setattr(fleet_cloud, "_FleetClient", Client)

    with pytest.raises(SdkError.Status) as error:
        await FleetCloudTransport(
            image=Image.from_registry("example:latest"), name="demo"
        ).connect()

    assert error.value.status == status
    assert calls == [("get-namespace", "demo")]


@pytest.mark.asyncio
async def test_connect_propagates_non_conflict_namespace_create_error(monkeypatch):
    calls = []

    class Client:
        async def get_namespace(self, name):
            calls.append(("get-namespace", name))
            raise SdkError.Status("get namespace", 404, "missing")

        async def create_namespace(self, name):
            calls.append(("create-namespace", name))
            raise SdkError.Status("create namespace", 500, "failed")

        async def create_template(self, request):
            calls.append(("create-template", request.name))
            raise AssertionError("failed namespace creation must not provision a template")

    monkeypatch.setattr(fleet_cloud, "_FleetClient", Client)

    with pytest.raises(SdkError.Status) as error:
        await FleetCloudTransport(
            image=Image.from_registry("example:latest"), name="demo"
        ).connect()

    assert error.value.status == 500
    assert calls == [("get-namespace", "demo"), ("create-namespace", "demo")]


@pytest.mark.asyncio
@pytest.mark.parametrize("failure_stage", ["template", "pool"])
async def test_connect_preserves_existing_namespace_after_provisioning_failure(
    monkeypatch, failure_stage
):
    calls = []

    class Client:
        async def get_namespace(self, name):
            calls.append(("get-namespace", name))
            return object()

        async def create_namespace(self, name):
            raise AssertionError("existing namespaces must not be created")

        async def create_template(self, request):
            calls.append(("create-template", request.name))
            if failure_stage == "template":
                raise RuntimeError("template failed")
            return "template"

        async def create_pool(self, request):
            calls.append(("create-pool", request.namespace))
            raise RuntimeError("pool failed")

        async def delete_template(self, template):
            calls.append(("delete-template", template))

        async def delete_namespace(self, name):
            raise AssertionError("existing namespaces must not be deleted")

    monkeypatch.setattr(fleet_cloud, "_FleetClient", Client)

    with pytest.raises(RuntimeError, match=f"{failure_stage} failed"):
        await FleetCloudTransport(
            image=Image.from_registry("example:latest"), name="demo"
        ).connect()

    expected_calls = [("get-namespace", "demo"), ("create-template", "demo")]
    if failure_stage == "pool":
        expected_calls.extend([("create-pool", "demo"), ("delete-template", "template")])
    assert calls == expected_calls


@pytest.mark.asyncio
async def test_connect_refetches_namespace_after_create_race(monkeypatch):
    namespace = "existing-namespace"
    order = []
    pool = type("Pool", (), {"metadata": type("Metadata", (), {"name": "demo"})()})()

    class Client:
        def __init__(self):
            self.get_calls = 0

        async def get_namespace(self, name):
            self.get_calls += 1
            order.append(("get-namespace", name))
            if self.get_calls == 1:
                raise SdkError.Status("get namespace", 404, "missing")
            return namespace

        async def create_namespace(self, name):
            order.append(("create-namespace", name))
            raise SdkError.Status("create namespace", 409, "exists")

        async def create_template(self, request):
            order.append(("template", request.name))
            return "template"

        async def create_pool(self, request):
            order.append(("pool", request.namespace))
            return pool

        async def wait_pool(self, created_pool):
            return created_pool

        async def create_claim(self, request):
            return "claim"

        async def wait_claim(self, claim):
            return Sandbox(namespace="demo", claim=claim, name="sandbox", services=["server"])

        async def wait_service_ready(self, sandbox, service, time_to_start):
            return None

        async def delete_claim(self, claim):
            order.append(("delete-claim", claim))

        async def delete_pool(self, created_pool):
            order.append(("delete-pool", created_pool))

        async def delete_template(self, template):
            order.append(("delete-template", template))

        async def delete_namespace(self, created_namespace):
            order.append(("delete-namespace", created_namespace))

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")

    await transport.connect()
    await transport.delete_vm()

    assert order == [
        ("get-namespace", "demo"),
        ("create-namespace", "demo"),
        ("get-namespace", "demo"),
        ("template", "demo"),
        ("pool", "demo"),
        ("delete-claim", "claim"),
        ("delete-pool", pool),
        ("delete-template", "template"),
    ]


@pytest.mark.asyncio
async def test_connect_rolls_back_template_then_owned_namespace_after_pool_failure(monkeypatch):
    order = []

    class Client:
        async def get_namespace(self, name):
            raise SdkError.Status("get namespace", 404, "missing")

        async def create_namespace(self, name):
            order.append(("create-namespace", name))
            return object()

        async def create_template(self, request):
            order.append(("create-template", request.name))
            return "template"

        async def create_pool(self, request):
            order.append(("create-pool", request.namespace))
            raise RuntimeError("pool failed")

        async def delete_template(self, template):
            order.append(("delete-template", template))

        async def delete_namespace(self, namespace):
            order.append(("delete-namespace", namespace))

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="pool failed"):
        await FleetCloudTransport(
            image=Image.from_registry("example:latest"), name="demo"
        ).connect()

    assert order == [
        ("create-namespace", "demo"),
        ("create-template", "demo"),
        ("create-pool", "demo"),
        ("delete-template", "template"),
        ("delete-namespace", "demo"),
    ]


@pytest.mark.asyncio
async def test_cleanup_preserves_existing_namespace():
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    calls = []

    class Client:
        async def delete_template(self, template):
            calls.append(("template", template))

        async def delete_namespace(self, namespace):
            calls.append(("namespace", namespace))

    transport._sdk = Client()
    transport._template = "template"
    transport._owns_namespace = False

    await transport._cleanup_resources()

    assert calls == [("template", "template")]


@pytest.mark.asyncio
async def test_connect_with_existing_pool_skips_namespace_crud(monkeypatch):
    calls = []
    pool = type("Pool", (), {"metadata": type("Metadata", (), {"name": "demo"})()})()

    class Client:
        async def get_namespace(self, name):
            calls.append(("get-namespace", name))
            raise AssertionError("existing pools must not manage namespaces")

        async def create_namespace(self, name):
            calls.append(("create-namespace", name))
            raise AssertionError("existing pools must not manage namespaces")

        async def delete_namespace(self, namespace):
            calls.append(("delete-namespace", namespace))
            raise AssertionError("existing pools must not manage namespaces")

        async def get_pool(self, name):
            calls.append(("get-pool", name))
            return pool

        async def get_claim(self, existing_pool):
            calls.append(("get-claim", existing_pool))
            return "claim"

        async def wait_claim(self, claim):
            calls.append(("wait-claim", claim))
            return Sandbox(namespace="demo", claim=claim, name="sandbox", services=["server"])

        async def wait_service_ready(self, sandbox, service, time_to_start):
            calls.append(("wait-service-ready", service))

    client = Client()
    monkeypatch.setattr(fleet_cloud, "_FleetClient", lambda: client)

    await FleetCloudTransport(image=None, name="demo").connect()

    assert calls == [
        ("get-pool", "demo"),
        ("get-claim", pool),
        ("wait-claim", "claim"),
        ("wait-service-ready", "server"),
    ]


@pytest.mark.asyncio
async def test_cleanup_stops_after_claim_failure():
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    calls = []

    class Client:
        async def delete_claim(self, claim):
            calls.append(("claim", claim))
            raise RuntimeError("claim delete failed")

        async def delete_pool(self, pool):
            calls.append(("pool", pool))

    transport._sdk = Client()
    transport._claim = "claim"
    transport._pool = "pool"
    with pytest.raises(RuntimeError, match="claim delete failed"):
        await transport._cleanup_resources()
    assert calls == [("claim", "claim")]


@pytest.mark.asyncio
async def test_cleanup_releases_claim_pool_template_then_owned_namespace():
    transport = FleetCloudTransport(image=Image.from_registry("example:latest"), name="demo")
    calls = []

    class Client:
        async def delete_claim(self, claim):
            calls.append(("claim", claim))

        async def delete_pool(self, pool):
            calls.append(("pool", pool))

        async def delete_template(self, template):
            calls.append(("template", template))

        async def delete_namespace(self, name):
            calls.append(("namespace", name))

    transport._sdk = Client()
    transport._claim = "claim"
    transport._pool = "pool"
    transport._template = "template"
    transport._owns_namespace = True
    await transport._cleanup_resources()
    assert calls == [
        ("claim", "claim"),
        ("pool", "pool"),
        ("template", "template"),
        ("namespace", "demo"),
    ]
    assert transport._template is None


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
    with pytest.raises(NotImplementedError, match="Snapshots"):
        await transport.create_snapshot()
