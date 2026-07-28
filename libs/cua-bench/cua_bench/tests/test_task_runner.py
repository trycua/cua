from unittest.mock import AsyncMock, patch

import pytest

from cua_bench.platforms import PLATFORMS
from cua_bench.runner.task_runner import TaskRunner


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("golden_name", "expected_image"),
    [
        ("linux-docker", "trycua/cua-xfce:latest"),
        ("ghcr.io/example/custom-xfce@sha256:abc", "ghcr.io/example/custom-xfce@sha256:abc"),
    ],
)
async def test_linux_docker_container_uses_resolved_image(
    golden_name: str,
    expected_image: str,
) -> None:
    runner = TaskRunner()

    with patch(
        "cua_bench.runner.task_runner.start_container",
        new_callable=AsyncMock,
    ) as start_container:
        await runner._start_env_container(
            task_id="task-id",
            network_name="network",
            env_type="linux-docker",
            golden_name=golden_name,
            config=PLATFORMS["linux-docker"],
            memory="8G",
            cpus="8",
            vnc_port=None,
            api_port=None,
        )

        assert start_container.await_args.kwargs["image"] == expected_image


def test_resolve_container_image_qemu_ignores_disk_name() -> None:
    from cua_bench.platforms import resolve_container_image

    assert resolve_container_image("linux-qemu", image_name="my-golden-disk") == PLATFORMS["linux-qemu"][
        "image"
    ]


def test_resolve_container_image_unknown_platform_raises() -> None:
    from cua_bench.platforms import resolve_container_image

    with pytest.raises(ValueError, match=r"Unknown container platform"):
        resolve_container_image("not-a-platform")
