"""OCI media type constants for VM and container image detection.

VM image formats in the wild:

  OCI-layered (agoda):  gzip-compressed disk chunks, agoda media types,
                        annotations with part.number/offset/total
  Legacy Lume (LZ4):    LZ4-chunked disk, trycua media types
  Chunked-parts:        standard OCI layer type with ;part.number= in media type string
  Tart (Cirrus Labs):   compressed disk chunks, cirruslabs media types,
                        OCI config with os/architecture fields
  QEMU (cua):           qcow2 disk chunks, trycua.qemu media types,
                        config with guest_os/cpu/ram/tpm

Standard container images use OCI/Docker layer media types.
"""

# ── QEMU VM (cua) ────────────────────────────────────────────────────────────

QEMU_CONFIG = "application/vnd.trycua.qemu.config.v1+json"
QEMU_DISK = "application/vnd.trycua.qemu.disk.v1"
QEMU_DISK_GZIP = "application/vnd.trycua.qemu.disk.v1+gzip"

# ── Tart (Cirrus Labs) ───────────────────────────────────────────────────────

TART_CONFIG = "application/vnd.cirruslabs.tart.config.v1"
TART_DISK = "application/vnd.cirruslabs.tart.disk.v2"
TART_NVRAM = "application/vnd.cirruslabs.tart.nvram.v1"

# ── OCI-compliant VM (agoda / kubelet) ──────────────────────────────────────

OCI_VM_CONFIG = "application/vnd.agoda.macosvz.config.v1+json"
OCI_VM_DISK = "application/vnd.agoda.macosvz.disk.image.v1"
OCI_VM_AUX = "application/vnd.agoda.macosvz.aux.image.v1"

# Backward-compat: existing images on ghcr.io still use the agoda media types
OCI_VM_CONFIG_LEGACY = "application/vnd.agoda.macosvz.config.v1+json"
OCI_VM_DISK_LEGACY = "application/vnd.agoda.macosvz.disk.image.v1"
OCI_VM_AUX_LEGACY = "application/vnd.agoda.macosvz.aux.image.v1"

# ── Legacy Lume (LZ4-chunked) ──────────────────────────────────────────────

LEGACY_DISK_CHUNK = "application/vnd.trycua.lume.disk.chunk.lz4"
LEGACY_AUX = "application/vnd.trycua.lume.aux.image.v1"
LEGACY_CONFIG = "application/vnd.trycua.lume.config.v1+json"

# ── Standard OCI / Docker container ────────────────────────────────────────

OCI_IMAGE_LAYER = "application/vnd.oci.image.layer.v1.tar+gzip"
OCI_IMAGE_LAYER_NONDIST = "application/vnd.oci.image.layer.nondistributable.v1.tar+gzip"
DOCKER_IMAGE_LAYER = "application/vnd.docker.image.rootfs.diff.tar.gzip"
OCI_IMAGE_CONFIG = "application/vnd.oci.image.config.v1+json"
DOCKER_IMAGE_CONFIG = "application/vnd.docker.container.image.v1+json"

# ── Sets ────────────────────────────────────────────────────────────────────

VM_MEDIA_TYPES = frozenset(
    {
        QEMU_CONFIG,
        QEMU_DISK,
        QEMU_DISK_GZIP,
        OCI_VM_CONFIG,
        OCI_VM_DISK,
        OCI_VM_AUX,
        OCI_VM_CONFIG_LEGACY,
        OCI_VM_DISK_LEGACY,
        OCI_VM_AUX_LEGACY,
        LEGACY_DISK_CHUNK,
        LEGACY_AUX,
        LEGACY_CONFIG,
        TART_CONFIG,
        TART_DISK,
        TART_NVRAM,
    }
)

CONTAINER_LAYER_TYPES = frozenset({
    OCI_IMAGE_LAYER, OCI_IMAGE_LAYER_NONDIST, DOCKER_IMAGE_LAYER,
})

CONTAINER_CONFIG_TYPES = frozenset({
    OCI_IMAGE_CONFIG, DOCKER_IMAGE_CONFIG,
})
