"""The local QEMU path resolves built-in images to the same pinned containerDisk."""

from pathlib import Path

import pytest
from cua_sandbox.builder import build
from cua_sandbox.image import (
    DEFAULT_LINUX_REGISTRY_IMAGE,
    DEFAULT_WINDOWS_REGISTRY_IMAGE,
    Image,
)


@pytest.fixture
def pulled(monkeypatch, tmp_path):
    """Capture the refs handed to the containerDisk puller."""
    refs: list[str] = []
    disk = tmp_path / "container.qcow2"
    disk.write_bytes(b"qcow2")

    def pull_container_disk(ref, **kwargs):
        refs.append(ref)
        return disk

    monkeypatch.setattr(
        "cua_sandbox.registry.container_disk.pull_container_disk", pull_container_disk
    )
    return refs, disk


@pytest.mark.parametrize(
    "image, expected",
    [
        (Image.windows(), DEFAULT_WINDOWS_REGISTRY_IMAGE),
        (Image.linux(), DEFAULT_LINUX_REGISTRY_IMAGE),
    ],
)
async def test_builtin_images_boot_the_pinned_container_disk(pulled, image, expected):
    refs, disk = pulled

    assert await build.resolve_backing_disk(image) == disk
    assert refs == [expected]


@pytest.mark.parametrize("image", [Image.windows("11"), Image.windows("10")])
async def test_images_without_a_pinned_disk_fall_back_to_a_local_build(pulled, monkeypatch, image):
    refs, _ = pulled
    built = []

    async def ensure_base_image(os_type, version):
        built.append((os_type, version))
        return Path("/tmp/base.qcow2")

    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)

    assert await build.resolve_backing_disk(image) == Path("/tmp/base.qcow2")
    assert built == [("windows", image.version)]
    assert refs == []


async def test_a_non_container_disk_ref_falls_back_to_a_local_build(monkeypatch, tmp_path):
    """A lume/tart VM image in the registry is not a containerDisk; don't die on it."""
    built = []

    def pull_container_disk(ref, **kwargs):
        raise FileNotFoundError(f"OCI image {ref!r} does not contain /disk/disk.img")

    async def ensure_base_image(os_type, version):
        built.append((os_type, version))
        return Path("/tmp/base.qcow2")

    monkeypatch.setattr(
        "cua_sandbox.registry.container_disk.pull_container_disk", pull_container_disk
    )
    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)

    assert await build.resolve_backing_disk(Image.windows()) == Path("/tmp/base.qcow2")
    assert built == [("windows", "2022")]


async def test_session_disk_overlays_the_pinned_container_disk(pulled, monkeypatch, tmp_path):
    """Windows no longer detours through the ISO-install base builder."""
    refs, disk = pulled
    session = tmp_path / "session.qcow2"
    overlays = []

    async def ensure_base_image(os_type, version):
        raise AssertionError("a pinned containerDisk must not trigger an ISO install")

    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)
    monkeypatch.setattr(build, "session_overlay_path", lambda name: session)
    monkeypatch.setattr(
        build,
        "create_overlay",
        lambda backing, destination: overlays.append((backing, destination)),
    )

    result = await build.create_session_disk(Image.windows(), "demo")

    assert result == session
    assert refs == [DEFAULT_WINDOWS_REGISTRY_IMAGE]
    assert overlays == [(disk, session)]
