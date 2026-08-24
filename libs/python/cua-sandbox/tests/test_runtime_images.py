from cua_sandbox.runtime.images import internal_ports, resolve_image


def test_linux_docker_uses_unified_multiarch_rootfs() -> None:
    image = resolve_image("linux")

    assert image == "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest"
    assert internal_ports(image) == (8000, 6080)
