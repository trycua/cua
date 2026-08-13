"""OCI registry tests — manifest inspection, kind detection, image formats.

    pytest tests/test_oci.py -v -s

Unit tests use synthetic manifests. Integration tests (marked `oci_live`)
fetch real manifests from ghcr.io and require network access.
"""

from __future__ import annotations

import os

import pytest
from cua_sandbox.registry.manifest import (
    ImageFormat,
    detect_format,
    detect_kind,
    detect_os,
    get_layer_info,
)
from cua_sandbox.registry.media_types import (
    DOCKER_IMAGE_CONFIG,
    DOCKER_IMAGE_LAYER,
    LEGACY_CONFIG,
    LEGACY_DISK_CHUNK,
    LUME_DISK,
    LUME_NVRAM,
    OCI_IMAGE_CONFIG,
    OCI_IMAGE_LAYER,
    OCI_VM_AUX,
    OCI_VM_CONFIG,
    OCI_VM_DISK,
    QEMU_CONFIG,
    QEMU_DISK_GZIP,
)
from cua_sandbox.registry.ref import parse_ref

# ═════════════════════════════════════════════════════════════════════════════
# parse_ref
# ═════════════════════════════════════════════════════════════════════════════


class TestParseRef:
    def test_full_ref(self):
        assert parse_ref("ghcr.io/trycua/macos-sequoia-cua:latest") == (
            "ghcr.io",
            "trycua",
            "macos-sequoia-cua",
            "latest",
        )

    def test_org_name(self):
        assert parse_ref("trycua/cua-xfce:v2") == ("ghcr.io", "trycua", "cua-xfce", "v2")

    def test_short_name(self):
        assert parse_ref("cua-xfce") == ("ghcr.io", "trycua", "cua-xfce", "latest")

    def test_short_name_with_tag(self):
        assert parse_ref("cua-xfce:nightly") == ("ghcr.io", "trycua", "cua-xfce", "nightly")


# ═════════════════════════════════════════════════════════════════════════════
# detect_format / detect_kind — synthetic manifests
# ═════════════════════════════════════════════════════════════════════════════


def _make_manifest(config_mt: str, layer_mts: list[str], **kwargs) -> dict:
    m = {
        "config": {"mediaType": config_mt, "digest": "sha256:aaa", "size": 100},
        "layers": [
            {"mediaType": mt, "digest": f"sha256:{i:03x}", "size": 1000}
            for i, mt in enumerate(layer_mts)
        ],
    }
    m.update(kwargs)
    return m


class TestDetectFormat:
    def test_oci_layered_by_config(self):
        m = _make_manifest(OCI_VM_CONFIG, [OCI_VM_DISK, OCI_VM_AUX])
        assert detect_format(m) == ImageFormat.OCI_LAYERED

    def test_oci_layered_by_layer(self):
        m = _make_manifest("application/json", [OCI_VM_DISK])
        assert detect_format(m) == ImageFormat.OCI_LAYERED

    def test_legacy_lz4_by_layer(self):
        m = _make_manifest("application/json", [LEGACY_DISK_CHUNK, LEGACY_DISK_CHUNK])
        assert detect_format(m) == ImageFormat.LEGACY_LZ4

    def test_chunked_parts(self):
        m = _make_manifest(
            "application/json",
            [
                f"{OCI_IMAGE_LAYER};part.number=1;part.total=3",
                f"{OCI_IMAGE_LAYER};part.number=2;part.total=3",
                f"{OCI_IMAGE_LAYER};part.number=3;part.total=3",
            ],
        )
        assert detect_format(m) == ImageFormat.CHUNKED_PARTS

    def test_container_oci(self):
        m = _make_manifest(OCI_IMAGE_CONFIG, [OCI_IMAGE_LAYER])
        assert detect_format(m) == ImageFormat.CONTAINER

    def test_container_docker(self):
        m = _make_manifest(DOCKER_IMAGE_CONFIG, [DOCKER_IMAGE_LAYER])
        assert detect_format(m) == ImageFormat.CONTAINER

    def test_qemu_by_config(self):
        m = _make_manifest(QEMU_CONFIG, [QEMU_DISK_GZIP])
        assert detect_format(m) == ImageFormat.QEMU

    def test_qemu_by_layer(self):
        m = _make_manifest("application/json", [QEMU_DISK_GZIP])
        assert detect_format(m) == ImageFormat.QEMU

    def test_unknown(self):
        m = _make_manifest("application/json", ["application/octet-stream"])
        assert detect_format(m) == ImageFormat.UNKNOWN


class TestDetectKind:
    def test_vm_oci_layered(self):
        m = _make_manifest(OCI_VM_CONFIG, [OCI_VM_DISK])
        assert detect_kind(m) == "vm"

    def test_vm_legacy(self):
        m = _make_manifest("application/json", [LEGACY_DISK_CHUNK])
        assert detect_kind(m) == "vm"

    def test_vm_chunked(self):
        m = _make_manifest(
            "application/json",
            [
                f"{OCI_IMAGE_LAYER};part.number=1;part.total=2",
            ],
        )
        assert detect_kind(m) == "vm"

    def test_vm_qemu(self):
        m = _make_manifest(QEMU_CONFIG, [QEMU_DISK_GZIP])
        assert detect_kind(m) == "vm"

    def test_container(self):
        m = _make_manifest(OCI_IMAGE_CONFIG, [OCI_IMAGE_LAYER])
        assert detect_kind(m) == "container"


class TestDetectOs:
    def test_agoda_is_macos(self):
        m = _make_manifest(OCI_VM_CONFIG, [OCI_VM_DISK])
        assert detect_os(m) == "macos"

    def test_annotation_linux(self):
        m = _make_manifest(
            "application/json",
            [],
            annotations={
                "org.trycua.lume.os": "Linux",
            },
        )
        assert detect_os(m) == "linux"

    def test_no_os(self):
        m = _make_manifest(OCI_IMAGE_CONFIG, [OCI_IMAGE_LAYER])
        assert detect_os(m) is None


# ═════════════════════════════════════════════════════════════════════════════
# get_layer_info
# ═════════════════════════════════════════════════════════════════════════════


class TestGetLayerInfo:
    def test_part_from_annotations(self):
        m = {
            "layers": [
                {
                    "mediaType": OCI_VM_DISK,
                    "digest": "sha256:abc",
                    "size": 500_000_000,
                    "annotations": {
                        "org.trycua.lume.part.number": "3",
                        "org.trycua.lume.part.total": "10",
                        "org.opencontainers.image.title": "disk.img.003",
                    },
                }
            ],
        }
        info = get_layer_info(m)
        assert len(info) == 1
        assert info[0]["part_number"] == 3
        assert info[0]["part_total"] == 10
        assert info[0]["title"] == "disk.img.003"

    def test_part_from_mediatype(self):
        mt = f"{OCI_IMAGE_LAYER};part.number=7;part.total=164"
        m = {"layers": [{"mediaType": mt, "digest": "sha256:def", "size": 100}]}
        info = get_layer_info(m)
        assert info[0]["part_number"] == 7
        assert info[0]["part_total"] == 164


# ═════════════════════════════════════════════════════════════════════════════
# Real manifest shapes, captured offline
# ═════════════════════════════════════════════════════════════════════════════


def _lume_chunked_manifest() -> dict:
    """Trimmed copy of ghcr.io/trycua/macos-sequoia-cua:latest.

    Two disk parts instead of 160; everything else is verbatim. This is what
    `lume push` produces today: trycua.lume media types with the part index in
    annotations, not in the media type string.
    """
    return {
        "annotations": {"org.trycua.lume.os": "macOS"},
        "config": {
            "mediaType": LEGACY_CONFIG,
            "digest": "sha256:478d66",
            "size": 611,
        },
        "layers": [
            {
                "mediaType": LUME_NVRAM,
                "digest": "sha256:921d54",
                "size": 33579164,
                "annotations": {"org.opencontainers.image.title": "nvram.bin"},
            },
            {
                "mediaType": LUME_DISK,
                "digest": "sha256:14d748",
                "size": 1123260,
                "annotations": {
                    "org.opencontainers.image.title": "disk.img.part.0",
                    "org.trycua.lume.part.number": "0",
                    "org.trycua.lume.part.total": "160",
                },
            },
            {
                "mediaType": LUME_DISK,
                "digest": "sha256:603f15",
                "size": 5728169,
                "annotations": {
                    "org.opencontainers.image.title": "disk.img.part.1",
                    "org.trycua.lume.part.number": "1",
                    "org.trycua.lume.part.total": "160",
                },
            },
        ],
    }


class TestLumeChunkedManifest:
    """A lume-pushed macOS image classified as UNKNOWN, so `cua pull` logged
    "(unknown, vm)" and no format-specific handling could ever match it."""

    def test_format_is_chunked_parts(self):
        assert detect_format(_lume_chunked_manifest()) == ImageFormat.CHUNKED_PARTS

    def test_kind_and_os(self):
        manifest = _lume_chunked_manifest()
        assert detect_kind(manifest) == "vm"
        assert detect_os(manifest) == "macos"

    def test_layer_parts_are_numbered(self):
        info = get_layer_info(_lume_chunked_manifest())
        disk_parts = [layer for layer in info if layer["part_number"] is not None]
        assert [layer["part_number"] for layer in disk_parts] == [0, 1]
        assert all(layer["part_total"] == 160 for layer in disk_parts)

    def test_lz4_still_wins_over_chunked(self):
        """Legacy LZ4 images share the lume config type — they must not be
        reclassified just because the config media type matches."""
        manifest = _lume_chunked_manifest()
        manifest["layers"] = [{"mediaType": LEGACY_DISK_CHUNK, "digest": "sha256:a", "size": 1}]
        assert detect_format(manifest) == ImageFormat.LEGACY_LZ4


# ═════════════════════════════════════════════════════════════════════════════
# Live registry tests (require network)
# ═════════════════════════════════════════════════════════════════════════════

# These reach out to ghcr.io / docker.io over the network. Anonymous pulls from
# a shared CI runner are rate-limited, so the whole class is opt-in; the offline
# regression tests above cover the same detection logic against real manifests.
oci_live = pytest.mark.skipif(
    os.environ.get("CUA_TEST_REGISTRY", "").lower() not in ("1", "true", "yes"),
    reason="hits a live registry; set CUA_TEST_REGISTRY=1 to run",
)


@oci_live
class TestLiveRegistry:
    """Fetch real manifests from ghcr.io. Requires network + oras."""

    def test_macos_sparse_oci_layered(self):
        from cua_sandbox.registry.manifest import get_manifest

        manifest = get_manifest("ghcr.io/trycua/macos-sequoia-cua-sparse:latest-oci-layered")
        assert detect_format(manifest) == ImageFormat.OCI_LAYERED
        assert detect_kind(manifest) == "vm"
        assert detect_os(manifest) == "macos"

        info = get_layer_info(manifest)
        assert len(info) > 0
        # Should have disk layers with part numbers
        disk_parts = [layer for layer in info if layer["part_number"] is not None]
        assert len(disk_parts) > 0

    def test_macos_chunked_parts(self):
        from cua_sandbox.registry.manifest import get_manifest

        manifest = get_manifest("ghcr.io/trycua/macos-sequoia-cua:latest")
        fmt = detect_format(manifest)
        assert fmt in (ImageFormat.CHUNKED_PARTS, ImageFormat.OCI_LAYERED, ImageFormat.LEGACY_LZ4)
        assert detect_kind(manifest) == "vm"

    def test_ubuntu_container(self):
        from cua_sandbox.registry.manifest import get_manifest

        manifest = get_manifest("docker.io/trycua/cua-xfce:latest")
        assert detect_format(manifest) == ImageFormat.CONTAINER
        assert detect_kind(manifest) == "container"
