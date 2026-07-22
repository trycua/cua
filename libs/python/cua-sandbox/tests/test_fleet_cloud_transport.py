import pytest
from cua_sandbox import Image
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport


def test_registry_image_becomes_fleet_template_and_services():
    transport = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace@sha256:abc").expose(3000),
        name="demo",
        cpu=4,
        memory_mb=8192,
    )

    assert transport._pool_request() == {
        "namespace": "demo",
        "spec": {
            "replicas": 1,
            "services": [
                {"name": "server", "targetPort": 8000, "protocol": "TCP"},
                {"name": "port-3000", "targetPort": 3000, "protocol": "TCP"},
            ],
            "template": {
                "containerDiskImage": "registry.example/workspace@sha256:abc",
                "imagePullSecret": "ecr-credentials",
                "probes": {"readinessProbe": {"tcpSocket": {"port": 8000}}},
                "cpuCores": 4,
                "memory": "8192Mi",
            },
        },
    }


@pytest.mark.parametrize(
    "image",
    [
        Image.linux(),
        Image.from_registry("registry.example/workspace:latest").apt_install("curl"),
        Image.from_registry("registry.example/workspace:latest").env(HELLO="world"),
    ],
)
def test_rejects_cloud_images_other_than_unmodified_registry_with_services(image):
    with pytest.raises(ValueError):
        FleetCloudTransport._validate_image(image)


@pytest.mark.asyncio
async def test_cleanup_deletes_pool_when_claim_deletion_fails():
    transport = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace:latest"), name="demo"
    )
    calls = []

    class Client:
        def delete_claim(self, claim):
            calls.append(("claim", claim))
            raise RuntimeError("claim delete failed")

        def delete_pool(self, pool):
            calls.append(("pool", pool))

    transport._sdk = Client()
    transport._claim = {"metadata": {"name": "claim"}}
    transport._pool = {"metadata": {"name": "pool"}}

    with pytest.raises(RuntimeError, match="claim delete failed"):
        await transport._cleanup_resources()

    assert calls == [("claim", {"metadata": {"name": "claim"}})]
    assert transport._claim == {"metadata": {"name": "claim"}}
    assert transport._pool == {"metadata": {"name": "pool"}}


@pytest.mark.asyncio
async def test_inherited_command_forwards_json_and_timeout_to_service_client():
    transport = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace:latest"), name="demo"
    )
    captured = {}

    class Client:
        def post(self, path, **kwargs):
            captured["path"] = path
            captured.update(kwargs)
            request = __import__("httpx").Request("POST", "https://run.cua.ai/cmd")
            return __import__("httpx").Response(
                200, text='data: {"result": "ok"}\n', request=request
            )

    transport._client = Client()
    transport._timeout = 30.0

    assert await transport.send("shell.run", timeout=15) == "ok"
    assert captured["path"] == "/cmd"
    assert captured["json"] == {"command": "shell.run", "params": {"timeout": 15}}
    assert captured["timeout"].read == 25.0


@pytest.mark.asyncio
async def test_connect_keeps_provisioning_error_when_cleanup_fails(monkeypatch):
    transport = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace:latest"), name="demo"
    )

    class Client:
        def create_pool(self, request):
            return {"metadata": {"name": "demo", "namespace": "demo"}}

        def create_claim(self, request):
            return {"metadata": {"name": "demo-claim", "namespace": "demo"}}

        def wait_claim(self, claim, **kwargs):
            raise RuntimeError("provisioning failed")

        def delete_claim(self, claim):
            return None

        def delete_pool(self, pool):
            raise RuntimeError("cleanup failed")

    monkeypatch.setattr("cua_sandbox.transport.fleet_cloud._FleetClient", lambda: Client())

    with pytest.raises(RuntimeError, match="provisioning failed") as error:
        await transport.connect()

    assert isinstance(error.value.__cause__, RuntimeError)
    assert str(error.value.__cause__) == "cleanup failed"


@pytest.mark.asyncio
async def test_forward_tunnel_returns_fleet_service_endpoint():
    transport = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace:latest").expose(3000), name="demo"
    )
    transport._provisioned = True
    transport._bound = {
        "namespace": "demo",
        "sandbox": "sandbox",
        "services": ["server", "port-3000"],
    }

    class Client:
        def service_url(self, bound, service):
            assert service == "port-3000"
            return "https://run.cua.ai/api/svc/demo/sandbox-port-3000/"

    transport._sdk = Client()
    info = await transport.forward_tunnel(3000)

    assert info.host == "run.cua.ai"
    assert info.port == 443
    assert info.url == "https://run.cua.ai/api/svc/demo/sandbox-port-3000/"


@pytest.mark.asyncio
async def test_snapshot_reports_fleet_unsupported():
    transport = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace:latest"), name="demo"
    )

    with pytest.raises(NotImplementedError, match="Snapshots are not supported"):
        await transport.create_snapshot()
