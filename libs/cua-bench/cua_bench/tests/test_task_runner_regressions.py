from cua_bench.runner.task_runner import (
    DEFAULT_LINUX_DOCKER_IMAGE,
    ENV_CONFIGS,
    resolve_environment_container_image,
)


def test_linux_docker_uses_the_public_cua_image() -> None:
    assert DEFAULT_LINUX_DOCKER_IMAGE == "trycua/cua-xfce:latest"
    assert ENV_CONFIGS["linux-docker"]["image"] == DEFAULT_LINUX_DOCKER_IMAGE
    assert "nikri/" not in ENV_CONFIGS["linux-docker"]["image"]
    assert (
        resolve_environment_container_image(
            "linux-docker", "linux-docker", ENV_CONFIGS["linux-docker"]
        )
        == DEFAULT_LINUX_DOCKER_IMAGE
    )
    assert (
        resolve_environment_container_image(
            "linux-docker",
            "registry.example/custom:latest",
            ENV_CONFIGS["linux-docker"],
        )
        == "registry.example/custom:latest"
    )
