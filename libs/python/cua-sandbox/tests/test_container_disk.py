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
    _detect_auth_backend,
    pull_container_disk,
)

BEARER_CHALLENGE = 'Bearer realm="https://public.ecr.aws/token/",service="public.ecr.aws"'
BASIC_CHALLENGE = 'Basic realm="https://296062593712.dkr.ecr.us-west-2.amazonaws.com/"'


def _fake_ping(monkeypatch, challenge, *, probes=None):
    """Stub the /v2/ auth-challenge probe."""

    def fake_get(url, timeout=None):
        if probes is not None:
            probes.append(url)
        return SimpleNamespace(headers={"WWW-Authenticate": challenge} if challenge else {})

    monkeypatch.setattr(container_disk.requests, "get", fake_get)


@pytest.fixture(autouse=True)
def _never_probe_a_real_registry(monkeypatch):
    """Keep the auth-challenge probe off the network unless a test opts in."""
    _fake_ping(monkeypatch, BEARER_CHALLENGE)


# The real ECR image is a multi-arch index whose children are the per-platform manifests
# plus a buildx attestation manifest.
INDEX_MANIFEST = {
    "schemaVersion": 2,
    "mediaType": "application/vnd.oci.image.index.v1+json",
    "manifests": [
        {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": "sha256:arm64child",
            "platform": {"architecture": "arm64", "os": "linux"},
        },
        {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": "sha256:amd64child",
            "platform": {"architecture": "amd64", "os": "linux"},
        },
        {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": "sha256:attestation",
            "annotations": {"vnd.docker.reference.type": "attestation-manifest"},
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


@pytest.mark.parametrize(
    ("ref", "challenge", "expected"),
    [
        # public.ecr.aws is anonymously readable but still uses the Bearer token flow.
        ("public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-e5d853a9", BEARER_CHALLENGE, "token"),
        # Private ECR answers with Basic and rejects oras' token backend.
        ("296062593712.dkr.ecr.us-west-2.amazonaws.com/duo:main", BASIC_CHALLENGE, "basic"),
        ("ghcr.io/trycua/workspace:latest", BEARER_CHALLENGE, "token"),
        # No challenge at all (open registry) — assume the OCI-standard bearer flow.
        ("registry.example/workspace:latest", "", "token"),
    ],
)
def test_auth_backend_follows_the_registry_challenge(monkeypatch, ref, challenge, expected):
    probes = []
    _fake_ping(monkeypatch, challenge, probes=probes)

    assert _detect_auth_backend(ref) == expected
    assert probes == [f"https://{ref.split('/', 1)[0]}/v2/"]


def test_auth_backend_defaults_to_token_when_the_probe_fails(monkeypatch):
    def boom(url, timeout=None):
        raise requests.ConnectionError("no route to host")

    monkeypatch.setattr(container_disk.requests, "get", boom)

    assert _detect_auth_backend("registry.example/workspace:latest") == "token"


def test_auth_backend_skips_the_probe_for_a_hostless_ref(monkeypatch):
    def boom(url, timeout=None):
        raise AssertionError("a ref without a registry host must not be probed")

    monkeypatch.setattr(container_disk.requests, "get", boom)

    assert _detect_auth_backend("workspace:latest") == "token"


def test_explicit_auth_backend_overrides_detection(tmp_path, monkeypatch):
    calls = []

    def boom(url, timeout=None):
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


def test_pull_container_disk_uses_oras_credentials_and_caches_qcow2(tmp_path, monkeypatch):
    _fake_ping(monkeypatch, BEARER_CHALLENGE)
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
        registry_factory=Registry,
    )

    assert disk.name == "disk.qcow2"
    assert disk.read_bytes() == b"qcow2"
    # The backend comes from the registry's challenge — hardcoding either one breaks
    # half the registries (see test_auth_backend_follows_the_registry_challenge).
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
        registry_factory=Registry,
    )

    assert disk.read_bytes() == b"qcow2"
    repository = DEFAULT_LINUX_REGISTRY_IMAGE.rsplit(":", 1)[0]
    assert requested == [DEFAULT_LINUX_REGISTRY_IMAGE, f"{repository}@sha256:amd64child"]


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
        "registry.example/workspace:latest", cache_root=tmp_path, registry_factory=Registry
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
