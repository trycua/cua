import pytest
from cua_sandbox import Image
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
from cyclops_sdk import Sandbox


def test_registry_image_becomes_typed_pool_request():
    request = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace@sha256:abc").expose(3000),
        name="demo",
        cpu=4,
        memory_mb=8192,
    )._pool_request()
    assert request.namespace == "demo"
    assert request.spec.template.container_disk_image == "registry.example/workspace@sha256:abc"
    assert request.spec.template.cpu_cores == 4
    assert request.spec.template.memory == "8192Mi"
    assert [(service.name, service.target_port) for service in request.spec.services] == [
        ("server", 8000),
        ("port-3000", 3000),
    ]


@pytest.mark.parametrize(
    "image", [Image.linux(), Image.from_registry("example:latest").apt_install("curl")]
)
def test_rejects_unsupported_images(image):
    with pytest.raises(ValueError):
        FleetCloudTransport._validate_image(image)


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
