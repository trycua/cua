"""The local QEMU path boots the canonical image's containerDisk, pulled by the SDK."""

from pathlib import Path
from types import SimpleNamespace

import pytest
from cua_sandbox import _sdk
from cua_sandbox.builder import build
from cua_sandbox.image import Image

LINUX = "ghcr.io/trycua/linux:24.04"
WINDOWS = "ghcr.io/trycua/windows:2022"


class _FakeLocal:
    def __init__(self, disk: Path, error: Exception | None = None):
        self.disk = disk
        self.error = error
        self.refs: list[str] = []

    async def pull_image(self, reference: str):
        self.refs.append(reference)
        if self.error is not None:
            raise self.error
        return SimpleNamespace(location=str(self.disk), kind="vm", reference=reference)


@pytest.fixture
def local(monkeypatch, tmp_path):
    """Capture the refs handed to the SDK's image puller (``Cua.local().pull_image``)."""
    disk = tmp_path / "container.qcow2"
    disk.write_bytes(b"qcow2")
    fake = _FakeLocal(disk)
    monkeypatch.setattr(_sdk, "local_runtime", lambda: SimpleNamespace(local=lambda: fake))
    return fake


@pytest.mark.parametrize("image, expected", [(Image.windows(), WINDOWS), (Image.linux(), LINUX)])
async def test_builtin_images_boot_the_canonical_container_disk(local, image, expected):
    assert await build.resolve_backing_disk(image) == local.disk
    # `vm:` asks the resolver for the containerDisk variant (`-disk` sibling).
    assert local.refs == [f"vm:{expected}"]


@pytest.mark.parametrize("image", [Image.windows("11"), Image.windows("10")])
async def test_images_without_a_canonical_disk_fall_back_to_a_local_build(
    local, monkeypatch, image
):
    built = []

    async def ensure_base_image(os_type, version):
        built.append((os_type, version))
        return Path("/tmp/base.qcow2")

    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)

    assert await build.resolve_backing_disk(image) == Path("/tmp/base.qcow2")
    assert built == [("windows", image.version)]
    assert local.refs == []


async def test_a_non_container_disk_ref_falls_back_to_a_local_build(local, monkeypatch):
    """A lume/tart VM image in the registry is not a containerDisk; don't die on it."""
    n = _sdk.native()
    local.error = n.CuaError.Unsupported("no containerdisk variant for backend vm")
    built = []

    async def ensure_base_image(os_type, version):
        built.append((os_type, version))
        return Path("/tmp/base.qcow2")

    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)

    assert await build.resolve_backing_disk(Image.windows()) == Path("/tmp/base.qcow2")
    assert built == [("windows", "2022")]


async def test_session_disk_overlays_the_container_disk(local, monkeypatch, tmp_path):
    """Windows does not detour through the ISO-install base builder."""
    session = tmp_path / "session.qcow2"
    overlays = []

    async def ensure_base_image(os_type, version):
        raise AssertionError("a canonical containerDisk must not trigger an ISO install")

    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)
    monkeypatch.setattr(build, "session_overlay_path", lambda name: session)
    monkeypatch.setattr(
        build,
        "create_overlay",
        lambda backing, destination: overlays.append((backing, destination)),
    )

    result = await build.create_session_disk(Image.windows(), "demo")

    assert result == session
    assert local.refs == [f"vm:{WINDOWS}"]
    assert overlays == [(local.disk, session)]


@pytest.mark.parametrize(
    "image, variable",
    [
        (Image.linux(), "CUA_IMAGE_LINUX"),
        (Image.windows(), "CUA_IMAGE_WINDOWS"),
        # Deprecated names still work.
        (Image.linux(), "CUA_DEFAULT_LINUX_IMAGE"),
        (Image.windows(), "CUA_DEFAULT_WINDOWS_IMAGE"),
    ],
)
async def test_canonical_images_are_overridable_by_env(local, monkeypatch, image, variable):
    from cua_sandbox.image import cloud_registry_image

    override = "registry.example/override/desktop:vm-1"
    monkeypatch.setenv(variable, override)

    assert cloud_registry_image(image) == override
    await build.resolve_backing_disk(image)
    assert local.refs == [f"vm:{override}"]


@pytest.mark.parametrize("value", ["", "   "])
def test_empty_override_keeps_the_canonical_image(monkeypatch, value):
    from cua_sandbox.image import cloud_registry_image

    monkeypatch.setenv("CUA_IMAGE_LINUX", value)
    assert cloud_registry_image(Image.linux()) == LINUX


def test_override_does_not_touch_explicit_or_non_canonical_images(monkeypatch):
    from cua_sandbox.image import cloud_registry_image

    monkeypatch.setenv("CUA_IMAGE_LINUX", "registry.example/override:1")
    monkeypatch.setenv("CUA_IMAGE_WINDOWS", "registry.example/override:2")
    explicit = Image.from_registry("registry.example/mine:3")
    assert cloud_registry_image(explicit) == "registry.example/mine:3"
    assert cloud_registry_image(Image.windows("11")) is None
