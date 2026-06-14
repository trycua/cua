import asyncio
import json
from unittest.mock import patch

import pytest


def test_normalize_container_runtime_aliases():
    from computer.providers.docker.provider import ContainerRuntime, _normalize_runtime

    assert _normalize_runtime("docker") == ContainerRuntime.DOCKER
    assert _normalize_runtime("container") == ContainerRuntime.APPLE_CONTAINER
    assert _normalize_runtime("apple-container") == ContainerRuntime.APPLE_CONTAINER


def test_normalize_container_runtime_rejects_unknown():
    from computer.providers.docker.provider import _normalize_runtime

    with pytest.raises(ValueError):
        _normalize_runtime("podman")


@patch("computer.providers.docker.provider.HAS_DOCKER", True)
@patch("computer.providers.docker.provider.HAS_CONTAINER", True)
def test_docker_runtime_commands_use_docker_cli():
    from computer.providers.docker.provider import DockerProvider

    provider = DockerProvider(image="trycua/cua-xfce:latest", runtime="docker")

    assert provider._run_cmd("desktop") == ["docker", "run", "-d", "--name", "desktop"]
    assert provider._inspect_cmd("desktop") == ["docker", "inspect", "desktop"]
    assert provider._start_cmd("desktop") == ["docker", "start", "desktop"]
    assert provider._stop_cmd("desktop") == ["docker", "stop", "desktop"]
    assert provider._delete_cmd("desktop", force=True) == ["docker", "rm", "-f", "desktop"]


@patch("computer.providers.docker.provider.HAS_DOCKER", True)
@patch("computer.providers.docker.provider.HAS_CONTAINER", True)
def test_apple_container_runtime_commands_use_container_cli():
    from computer.providers.docker.provider import DockerProvider

    provider = DockerProvider(image="trycua/cua-xfce:latest", runtime="container")

    assert provider._run_cmd("desktop") == ["container", "run", "-d", "--name", "desktop"]
    assert provider._inspect_cmd("desktop") == ["container", "inspect", "desktop"]
    assert provider._start_cmd("desktop") == ["container", "start", "desktop"]
    assert provider._stop_cmd("desktop") == ["container", "stop", "desktop"]
    assert provider._delete_cmd("desktop", force=True) == [
        "container",
        "delete",
        "--force",
        "desktop",
    ]


@patch("computer.providers.docker.provider.HAS_DOCKER", True)
@patch("computer.providers.docker.provider.HAS_CONTAINER", True)
def test_parse_apple_container_inspect_json():
    from computer.providers.docker.provider import DockerProvider

    provider = DockerProvider(image="trycua/cua-xfce:latest", runtime="container")
    info = provider._parse_inspect(
        "desktop",
        {
            "status": "running",
            "image": "trycua/cua-xfce:latest",
            "configuration": {"id": "desktop"},
            "networks": [{"address": "192.168.64.3/24"}],
        },
    )

    assert info["name"] == "desktop"
    assert info["status"] == "running"
    assert info["ip_address"] == "192.168.64.3"
    assert info["image"] == "trycua/cua-xfce:latest"
    assert info["provider"] == "container"


@patch("computer.providers.docker.provider.HAS_DOCKER", True)
@patch("computer.providers.docker.provider.HAS_CONTAINER", True)
def test_parse_apple_container_inspect_json_objects():
    from computer.providers.docker.provider import DockerProvider

    provider = DockerProvider(image="trycua/cua-xfce:latest", runtime="container")
    info = provider._parse_inspect(
        "desktop",
        {
            "status": {"state": "running", "networks": []},
            "image": {"reference": "docker.io/trycua/cua-xfce:latest"},
            "configuration": {"id": "desktop"},
            "networks": [{"address": "192.168.64.3/24"}],
        },
    )

    assert info["status"] == "running"
    assert info["image"] == "trycua/cua-xfce:latest"


@patch("computer.providers.docker.provider.HAS_DOCKER", True)
@patch("computer.providers.docker.provider.HAS_CONTAINER", True)
def test_apple_container_list_vms_matches_normalized_image():
    from computer.providers.docker.provider import DockerProvider

    provider = DockerProvider(image="trycua/cua-xfce:latest", runtime="container")
    list_output = json.dumps(
        [
            {
                "id": "desktop",
                "image": {"reference": "docker.io/trycua/cua-xfce:latest"},
            },
            {
                "id": "other",
                "image": {"reference": "docker.io/library/alpine:latest"},
            },
        ]
    )
    inspect_output = json.dumps(
        [
            {
                "status": {"state": "running"},
                "image": {"reference": "docker.io/trycua/cua-xfce:latest"},
                "configuration": {"id": "desktop"},
                "networks": [{"address": "192.168.64.3/24"}],
            }
        ]
    )

    def fake_run(cmd, capture_output=True, text=True, check=False):
        class Result:
            returncode = 0
            stderr = ""

        result = Result()
        if cmd[:2] == ["container", "list"]:
            result.stdout = list_output
        elif cmd[:2] == ["container", "inspect"]:
            result.stdout = inspect_output
        else:
            raise AssertionError(f"unexpected command: {cmd}")
        return result

    with patch("computer.providers.docker.provider.subprocess.run", side_effect=fake_run):
        vms = asyncio.run(provider.list_vms())

    assert len(vms) == 1
    assert vms[0]["name"] == "desktop"
    assert vms[0]["image"] == "trycua/cua-xfce:latest"
