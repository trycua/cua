import gzip
import hashlib
import io
import tarfile
import threading
from types import SimpleNamespace

import pytest
import requests
from cua_sandbox.image import DEFAULT_LINUX_REGISTRY_IMAGE, Image
from cua_sandbox.registry import container_disk
from cua_sandbox.registry.container_disk import (
    _LOCK_POLL_INTERVAL_SECONDS,
    _auth_backend_for,
    pull_container_disk,
)

# The pinned image really is served as a two-child OCI index: the linux/amd64 disk and a
# buildx provenance manifest that reports platform unknown/unknown. Taking "the first"
# or "any" child would grab the attestation and fail confusingly.
AMD64_CHILD = "sha256:85b3f5022bf9ecc864472f5fade5e2f1f54ab88ce429af7873a1415d668dc2ea"
ATTESTATION_CHILD = "sha256:a961974dc086404082c2299cfbdb6bac77dbf930da9846f6a05437f60592e426"

INDEX_MANIFEST = {
    "schemaVersion": 2,
    "mediaType": "application/vnd.oci.image.index.v1+json",
    "manifests": [
        {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": AMD64_CHILD,
            "size": 483,
            "platform": {"architecture": "amd64", "os": "linux"},
        },
        {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": ATTESTATION_CHILD,
            "size": 563,
            "annotations": {
                "vnd.docker.reference.digest": AMD64_CHILD,
                "vnd.docker.reference.type": "attestation-manifest",
            },
            "platform": {"architecture": "unknown", "os": "unknown"},
        },
    ],
}


def _layer_with_disk(contents: bytes, *, path: str = "disk/disk.img") -> bytes:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        info = tarfile.TarInfo(path)
        info.size = len(contents)
        archive.addfile(info, io.BytesIO(contents))
    return gzip.compress(raw.getvalue())


class TestAuthBackendSelection:
    """Registries disagree about the challenge scheme; oras cannot negotiate one."""

    def _challenge(self, monkeypatch, header):
        def get(url, **kwargs):
            return SimpleNamespace(headers={"Www-Authenticate": header} if header else {})

        monkeypatch.setattr(container_disk.requests, "get", get)

    def test_a_basic_challenge_selects_basic_auth(self, monkeypatch):
        """Private ECR answers with Basic; oras' token backend cannot respond to it."""
        self._challenge(monkeypatch, 'Basic realm="https://12345.dkr.ecr.us-west-2.amazonaws.com/"')

        assert _auth_backend_for("12345.dkr.ecr.us-west-2.amazonaws.com/workspace:tag") == "basic"

    @pytest.mark.parametrize(
        "header",
        [
            'Bearer realm="https://public.ecr.aws/token/",service="public.ecr.aws"',
            'Bearer realm="https://ghcr.io/token",service="ghcr.io"',
            "",
        ],
    )
    def test_a_bearer_or_absent_challenge_selects_token_auth(self, monkeypatch, header):
        self._challenge(monkeypatch, header)

        assert _auth_backend_for("public.ecr.aws/example/workspace:tag") == "token"

    def test_an_unreachable_registry_falls_back_to_token_auth(self, monkeypatch):
        def get(url, **kwargs):
            raise requests.ConnectionError("no route to host")

        monkeypatch.setattr(container_disk.requests, "get", get)

        assert _auth_backend_for("registry.example/workspace:tag") == "token"


def test_explicit_auth_backend_skips_the_challenge_probe(tmp_path, monkeypatch):
    calls = []

    def boom(url, **kwargs):
        raise AssertionError("an explicit auth_backend must not trigger a probe")

    monkeypatch.setattr(container_disk.requests, "get", boom)

    class Registry:
        def __init__(self, *, auth_backend):
            calls.append(("init", auth_backend))
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            return {"layers": [{"digest": "sha256:layer"}]}

        def get_blob(self, container, digest, *, stream):
            return SimpleNamespace(raw=io.BytesIO(_layer_with_disk(b"qcow2")))

    disk = pull_container_disk(
        "registry.example/workspace:latest",
        cache_root=tmp_path,
        auth_backend="basic",
        registry_factory=Registry,
    )

    assert disk.read_bytes() == b"qcow2"
    assert calls == [("init", "basic")]


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
            self.auth = SimpleNamespace(
                load_configs=lambda container: calls.append(("auth", container))
            )

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
        auth_backend="token",
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
            auth_backend="token",
            registry_factory=Registry,
        )
        == disk
    )
    assert calls == []


def test_pull_probes_for_an_auth_backend_when_none_is_given(tmp_path, monkeypatch):
    """Production omits auth_backend, so the challenge probe decides."""
    calls = []
    monkeypatch.setattr(container_disk, "_auth_backend_for", lambda ref: "basic")

    class Registry:
        def __init__(self, *, auth_backend):
            calls.append(("init", auth_backend))
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            return {"layers": [{"digest": "sha256:layer"}]}

        def get_blob(self, container, digest, *, stream):
            return SimpleNamespace(raw=io.BytesIO(_layer_with_disk(b"qcow2")))

    pull_container_disk(
        "registry.example/workspace:latest", cache_root=tmp_path, registry_factory=Registry
    )

    assert calls == [("init", "basic")]


def test_pull_container_disk_resolves_platform_manifest_from_index(tmp_path):
    """A multi-arch index has no ``layers`` — the platform child must be followed."""
    requested = []
    child = {
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [
            {
                "digest": "sha256:amd64layer",
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            }
        ],
    }

    class Registry:
        def __init__(self, *, auth_backend):
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            requested.append(ref)
            return INDEX_MANIFEST if ref == DEFAULT_LINUX_REGISTRY_IMAGE else child

        def get_blob(self, container, digest, *, stream):
            assert digest == "sha256:amd64layer"
            return SimpleNamespace(raw=io.BytesIO(_layer_with_disk(b"qcow2")))

    disk = pull_container_disk(
        DEFAULT_LINUX_REGISTRY_IMAGE,
        cache_root=tmp_path,
        architecture="amd64",
        auth_backend="token",
        registry_factory=Registry,
    )

    assert disk.read_bytes() == b"qcow2"
    repository = DEFAULT_LINUX_REGISTRY_IMAGE.rsplit(":", 1)[0]
    # The linux/amd64 child, never the unknown/unknown attestation sibling.
    assert requested == [DEFAULT_LINUX_REGISTRY_IMAGE, f"{repository}@{AMD64_CHILD}"]


def test_index_descent_never_selects_the_attestation_child(tmp_path):
    """Even listed first, the buildx provenance manifest must not be mistaken for a disk."""
    requested = []
    attestation_first = {
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [
            INDEX_MANIFEST["manifests"][1],  # unknown/unknown attestation
            INDEX_MANIFEST["manifests"][0],  # linux/amd64 disk
        ],
    }

    class Registry:
        def __init__(self, *, auth_backend):
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            requested.append(ref)
            if not ref.endswith(AMD64_CHILD):
                return attestation_first
            return {"layers": [{"digest": "sha256:amd64layer"}]}

        def get_blob(self, container, digest, *, stream):
            return SimpleNamespace(raw=io.BytesIO(_layer_with_disk(b"qcow2")))

    disk = pull_container_disk(
        "registry.example/workspace:latest",
        cache_root=tmp_path,
        architecture="amd64",
        auth_backend="token",
        registry_factory=Registry,
    )

    assert disk.read_bytes() == b"qcow2"
    assert requested == [
        "registry.example/workspace:latest",
        f"registry.example/workspace@{AMD64_CHILD}",
    ]
    assert ATTESTATION_CHILD not in " ".join(requested)


def test_pull_container_disk_rejects_index_without_matching_platform(tmp_path):
    class Registry:
        def __init__(self, *, auth_backend):
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            return {
                "mediaType": "application/vnd.oci.image.index.v1+json",
                "manifests": [
                    {
                        "digest": "sha256:amd64child",
                        "platform": {"architecture": "amd64", "os": "linux"},
                    }
                ],
            }

        def get_blob(self, container, digest, *, stream):
            raise AssertionError("no blob should be fetched without a platform match")

    with pytest.raises(FileNotFoundError, match="no linux/arm64 manifest"):
        pull_container_disk(
            DEFAULT_LINUX_REGISTRY_IMAGE,
            cache_root=tmp_path,
            architecture="arm64",
            auth_backend="token",
            registry_factory=Registry,
        )


def test_pull_container_disk_skips_vm_disk_layers(tmp_path):
    """Chunked VM images (lume/tart/qemu) are not containerDisks — don't stream them."""

    class Registry:
        def __init__(self, *, auth_backend):
            self.auth = SimpleNamespace(load_configs=lambda container: None)

        def get_container(self, ref):
            return "container"

        def get_manifest(self, ref):
            return {
                "layers": [
                    {
                        "digest": "sha256:chunk",
                        "mediaType": "application/vnd.trycua.lume.disk.chunk.lz4",
                    }
                ]
            }

        def get_blob(self, container, digest, *, stream):
            raise AssertionError("VM disk chunks must not be downloaded")

    with pytest.raises(FileNotFoundError, match="does not contain /disk/disk.img"):
        pull_container_disk(
            "registry.example/lume-vm:latest",
            cache_root=tmp_path,
            auth_backend="token",
            registry_factory=Registry,
        )


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

    disk = pull_container_disk(
        "registry.example/workspace:latest",
        cache_root=tmp_path,
        auth_backend="token",
        registry_factory=Registry,
    )

    assert disk.read_bytes() == b"qcow2"


def test_pull_container_disk_waits_for_existing_lock(tmp_path, monkeypatch):
    destination = (
        tmp_path / hashlib.sha256(DEFAULT_LINUX_REGISTRY_IMAGE.encode()).hexdigest() / "disk.qcow2"
    )
    lock_path = destination.with_suffix(".lock")
    destination.parent.mkdir(parents=True, exist_ok=True)
    lock_path.write_text("locked")
    destination_bytes = b"cached"
    sleeps = []

    class Registry:
        def __init__(self, *, auth_backend):
            raise AssertionError("registry should not be used when another process fills the cache")

    def fake_sleep(interval):
        sleeps.append(interval)
        destination.write_bytes(destination_bytes)
        lock_path.unlink()

    monkeypatch.setattr("cua_sandbox.registry.container_disk.time.sleep", fake_sleep)

    disk = pull_container_disk(
        DEFAULT_LINUX_REGISTRY_IMAGE,
        cache_root=tmp_path,
        auth_backend="token",
        registry_factory=Registry,
    )

    assert disk == destination
    assert disk.read_bytes() == destination_bytes
    assert sleeps == [_LOCK_POLL_INTERVAL_SECONDS]


async def test_default_linux_session_overlays_pulled_container_disk(tmp_path, monkeypatch):
    """Local Image.linux() must boot the same containerDisk Fleet cloud boots."""
    from cua_sandbox.builder import build
    from cua_sandbox.registry import container_disk

    pulled = tmp_path / "container-disk.qcow2"
    pulled.write_bytes(b"containerdisk")
    session_disk = tmp_path / "session.qcow2"
    calls = []

    def fake_pull(ref):
        calls.append(("pull", ref, threading.get_ident()))
        return pulled

    async def ensure_base_image(os_type, version):
        raise AssertionError("built-in Linux must not fall back to a locally built base")

    monkeypatch.setattr(container_disk, "pull_container_disk", fake_pull)
    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)
    monkeypatch.setattr(build, "session_overlay_path", lambda name: session_disk)
    monkeypatch.setattr(
        build,
        "create_overlay",
        lambda backing, destination: calls.append(("overlay", backing, destination)),
    )

    result = await build.create_session_disk(Image.linux(), "demo")

    assert result == session_disk
    assert [call[0] for call in calls] == ["pull", "overlay"]
    assert calls[0][1] == DEFAULT_LINUX_REGISTRY_IMAGE
    # The pull does network + multi-GB I/O, so it must not run on the event loop thread.
    assert calls[0][2] != threading.get_ident()
    assert calls[1][1:] == (pulled, session_disk)


async def test_non_container_disk_registry_image_falls_back_to_base(tmp_path, monkeypatch):
    from cua_sandbox.builder import build
    from cua_sandbox.registry import container_disk

    base_disk = tmp_path / "base.qcow2"
    base_disk.write_bytes(b"base")
    session_disk = tmp_path / "session.qcow2"
    calls = []

    def fake_pull(ref):
        raise FileNotFoundError(f"OCI image {ref!r} does not contain /disk/disk.img")

    async def ensure_base_image(os_type, version):
        calls.append(("base", os_type, version))
        return base_disk

    monkeypatch.setattr(container_disk, "pull_container_disk", fake_pull)
    monkeypatch.setattr(build, "ensure_base_image", ensure_base_image)
    monkeypatch.setattr(build, "session_overlay_path", lambda name: session_disk)
    monkeypatch.setattr(
        build,
        "create_overlay",
        lambda backing, destination: calls.append(("overlay", backing, destination)),
    )

    image = Image.from_registry("registry.example/lume-vm:latest")
    result = await build.create_session_disk(image, "demo")

    assert result == session_disk
    assert calls == [
        ("base", "linux", "latest"),
        ("overlay", base_disk, session_disk),
    ]
