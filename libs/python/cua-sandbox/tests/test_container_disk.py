import gzip
import io
import tarfile
from types import SimpleNamespace

from cua_sandbox.image import DEFAULT_LINUX_REGISTRY_IMAGE
from cua_sandbox.registry.container_disk import pull_container_disk


def _layer_with_disk(contents: bytes, *, path: str = "disk/disk.img") -> bytes:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        info = tarfile.TarInfo(path)
        info.size = len(contents)
        archive.addfile(info, io.BytesIO(contents))
    return gzip.compress(raw.getvalue())


def test_pull_container_disk_uses_oras_credentials_and_caches_qcow2(tmp_path):
    calls = []
    manifest = {
        "layers": [
            {
                "digest": "sha256:layer",
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            }
        ]
    }

    class Registry:
        def __init__(self, *, auth_backend):
            calls.append(("init", auth_backend))
            self.auth = SimpleNamespace(load_configs=lambda container: calls.append(("auth", container)))

        def get_container(self, ref):
            calls.append(("container", ref))
            return "container"

        def get_manifest(self, ref):
            calls.append(("manifest", ref))
            return manifest

        def get_blob(self, container, digest, *, stream):
            calls.append(("blob", container, digest, stream))
            return SimpleNamespace(raw=io.BytesIO(_layer_with_disk(b"qcow2")))

    disk = pull_container_disk(
        DEFAULT_LINUX_REGISTRY_IMAGE,
        cache_root=tmp_path,
        registry_factory=Registry,
    )

    assert disk.name == "disk.qcow2"
    assert disk.read_bytes() == b"qcow2"
    assert calls == [
        ("init", "token"),
        ("container", DEFAULT_LINUX_REGISTRY_IMAGE),
        ("auth", "container"),
        ("manifest", DEFAULT_LINUX_REGISTRY_IMAGE),
        ("blob", "container", "sha256:layer", True),
    ]

    calls.clear()
    assert (
        pull_container_disk(
            DEFAULT_LINUX_REGISTRY_IMAGE,
            cache_root=tmp_path,
            registry_factory=Registry,
        )
        == disk
    )
    assert calls == []


def test_pull_container_disk_searches_all_layers(tmp_path):
    empty_layer = _layer_with_disk(b"ignored", path="etc/example")
    disk_layer = _layer_with_disk(b"qcow2")

    class Registry:
        def __init__(self, *, auth_backend):
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            return {
                "layers": [
                    {"digest": "sha256:empty"},
                    {"digest": "sha256:disk"},
                ]
            }

        def get_blob(self, container, digest, *, stream):
            payload = empty_layer if digest == "sha256:empty" else disk_layer
            return SimpleNamespace(raw=io.BytesIO(payload))

    disk = pull_container_disk("registry.example/workspace:latest", cache_root=tmp_path, registry_factory=Registry)

    assert disk.read_bytes() == b"qcow2"


async def test_default_linux_session_uses_container_disk_as_qemu_backing(tmp_path, monkeypatch):
    from cua_sandbox.builder import build
    from cua_sandbox.image import Image

    base_disk = tmp_path / "base.qcow2"
    base_disk.write_bytes(b"base")
    session_disk = tmp_path / "session.qcow2"
    calls = []

    monkeypatch.setattr(
        "cua_sandbox.registry.container_disk.pull_container_disk",
        lambda ref: calls.append(("pull", ref)) or base_disk,
    )
    monkeypatch.setattr(build, "session_overlay_path", lambda name: session_disk)
    monkeypatch.setattr(
        build,
        "create_overlay",
        lambda backing, destination: calls.append(("overlay", backing, destination)),
    )

    result = await build.create_session_disk(Image.linux(), "demo")

    assert result == session_disk
    assert calls == [
        ("pull", DEFAULT_LINUX_REGISTRY_IMAGE),
        ("overlay", base_disk, session_disk),
    ]
