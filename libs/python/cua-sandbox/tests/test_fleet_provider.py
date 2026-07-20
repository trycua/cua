from __future__ import annotations

from copy import deepcopy

import pytest

from cua_sandbox import Image
from cua_sandbox.providers.fleet import FleetProvider
from cua_sandbox.transport.fleet import FleetTransport


class FakeSDK:
    def __init__(self, *, create_claim_error=None, wait_claim_error=None):
        self.calls = []
        self.create_claim_error = create_claim_error
        self.wait_claim_error = wait_claim_error
        self.pool = {
            "apiVersion": "cua.ai/v1",
            "kind": "OSGymWorkspacePool",
            "metadata": {"namespace": "demo", "name": "demo"},
            "spec": {},
        }
        self.claim = {
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxClaim",
            "metadata": {"namespace": "demo", "name": "claim-demo"},
            "spec": {},
        }
        self.bound = {
            "namespace": "demo",
            "claim": "claim-demo",
            "sandbox": "sandbox-demo",
            "services": ["api"],
        }

    def create_pool(self, request):
        self.calls.append(("create_pool", deepcopy(request)))
        pool = deepcopy(self.pool)
        pool["spec"] = deepcopy(request["spec"])
        return pool

    def create_claim(self, request):
        self.calls.append(("create_claim", request))
        if self.create_claim_error:
            raise self.create_claim_error
        return self.claim

    def wait_claim(self, claim):
        self.calls.append(("wait_claim", claim))
        if self.wait_claim_error:
            raise self.wait_claim_error
        return self.bound

    def delete_claim(self, claim):
        self.calls.append(("delete_claim", claim))

    def delete_pool(self, pool):
        self.calls.append(("delete_pool", pool))


@pytest.fixture
def linux_template():
    return {
        "containerDiskImage": "registry.example/desktop-workspace-duo@sha256:abc",
        "cpuCores": 4,
        "memory": "4Gi",
    }


@pytest.mark.asyncio
async def test_create_provisions_one_pool_and_one_claim(linux_template):
    sdk = FakeSDK()
    provider = FleetProvider(sdk=sdk, templates={"linux": linux_template})

    provisioned = await provider.create(
        Image.linux(),
        name="demo",
        cpu=8,
        memory_mb=8192,
        disk_gb=None,
        region="us-east-1",
        request_timeout=45,
    )

    assert [call[0] for call in sdk.calls] == ["create_pool", "create_claim", "wait_claim"]
    pool_request = sdk.calls[0][1]
    assert pool_request == {
        "namespace": "demo",
        "spec": {
            "replicas": 1,
            "services": [{"name": "api", "targetPort": 8000, "protocol": "TCP"}],
            "template": {
                "containerDiskImage": "registry.example/desktop-workspace-duo@sha256:abc",
                "cpuCores": 8,
                "memory": "8192Mi",
            },
        },
    }
    assert linux_template["cpuCores"] == 4
    assert provisioned.name == "demo"
    assert isinstance(provisioned.transport, FleetTransport)
    assert provisioned.handle.pool["metadata"]["name"] == "demo"
    assert provisioned.handle.claim["metadata"]["name"] == "claim-demo"
    assert provisioned.handle.bound["sandbox"] == "sandbox-demo"


@pytest.mark.asyncio
async def test_destroy_deletes_claim_before_pool(linux_template):
    sdk = FakeSDK()
    provider = FleetProvider(sdk=sdk, templates={"linux": linux_template})
    provisioned = await provider.create(Image.linux(), name="demo")
    sdk.calls.clear()

    await provider.destroy(provisioned.handle)

    assert [call[0] for call in sdk.calls] == ["delete_claim", "delete_pool"]


@pytest.mark.asyncio
async def test_claim_creation_failure_rolls_back_pool(linux_template):
    sdk = FakeSDK(create_claim_error=RuntimeError("claim failed"))
    provider = FleetProvider(sdk=sdk, templates={"linux": linux_template})

    with pytest.raises(RuntimeError, match="claim failed"):
        await provider.create(Image.linux(), name="demo")

    assert [call[0] for call in sdk.calls] == ["create_pool", "create_claim", "delete_pool"]


@pytest.mark.asyncio
async def test_claim_wait_failure_deletes_claim_then_pool(linux_template):
    sdk = FakeSDK(wait_claim_error=RuntimeError("bind failed"))
    provider = FleetProvider(sdk=sdk, templates={"linux": linux_template})

    with pytest.raises(RuntimeError, match="bind failed"):
        await provider.create(Image.linux(), name="demo")

    assert [call[0] for call in sdk.calls] == [
        "create_pool",
        "create_claim",
        "wait_claim",
        "delete_claim",
        "delete_pool",
    ]


@pytest.mark.asyncio
async def test_create_requires_explicit_os_template():
    provider = FleetProvider(sdk=FakeSDK(), templates={})

    with pytest.raises(ValueError, match="No Fleet template configured for 'windows'"):
        await provider.create(Image.windows(), name="demo")


@pytest.mark.asyncio
async def test_create_rejects_unsupported_disk_size(linux_template):
    provider = FleetProvider(sdk=FakeSDK(), templates={"linux": linux_template})

    with pytest.raises(ValueError, match="disk_gb is not supported"):
        await provider.create(Image.linux(), name="demo", disk_gb=20)

@pytest.mark.asyncio
async def test_create_rejects_registry_image_in_favor_of_explicit_template(linux_template):
    provider = FleetProvider(sdk=FakeSDK(), templates={"linux": linux_template})

    with pytest.raises(ValueError, match="uses explicit templates"):
        await provider.create(Image.from_registry("registry.example/custom:latest"), name="demo")

@pytest.mark.asyncio
async def test_create_rejects_custom_start_timeout(linux_template):
    provider = FleetProvider(sdk=FakeSDK(), templates={"linux": linux_template})

    with pytest.raises(ValueError, match="time_to_start is not configurable"):
        await provider.create(Image.linux(), name="demo", time_to_start=120)
