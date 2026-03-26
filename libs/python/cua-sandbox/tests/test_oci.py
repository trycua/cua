"""OCI registry tests — manifest inspection, kind detection, image formats.

    pytest tests/test_oci.py -v -s

Unit tests use synthetic manifests. Integration tests (marked `oci_live`)
fetch real manifests from ghcr.io and require network access.
"""

from __future__ import annotations

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
    LEGACY_DISK_CHUNK,
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
# Live registry tests (require network)
# ═════════════════════════════════════════════════════════════════════════════

oci_live = pytest.mark.skipif(
    not pytest.importorskip("oras", reason="oras-py not installed"),
    reason="oras-py not installed",
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
        disk_parts = [l for l in info if l["part_number"] is not None]
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
