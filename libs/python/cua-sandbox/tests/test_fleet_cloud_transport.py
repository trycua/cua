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
                {"name": "api", "targetPort": 8000, "protocol": "TCP"},
                {"name": "port-3000", "targetPort": 3000, "protocol": "TCP"},
            ],
            "template": {
                "containerDiskImage": "registry.example/workspace@sha256:abc",
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
