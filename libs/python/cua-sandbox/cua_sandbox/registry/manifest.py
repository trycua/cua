"""Fetch and classify OCI manifests using oras-py.

This is the single entry point for inspecting any OCI image — VM or container.
All runtime/pull logic delegates here first to figure out what the image is.
"""

from __future__ import annotations

from enum import Enum
from typing import Optional

import oras.provider
from cua_sandbox.registry.media_types import (
    ANDROID_AVD_CONFIG,
    ANDROID_AVD_TAR_GZIP,
    CONTAINER_CONFIG_TYPES,
    CONTAINER_LAYER_TYPES,
    LEGACY_DISK_CHUNK,
    LUME_CONFIG,
    LUME_DISK,
    LUME_NVRAM,
    OCI_VM_CONFIG,
    OCI_VM_CONFIG_LEGACY,
    OCI_VM_DISK,
    OCI_VM_DISK_LEGACY,
    QEMU_CONFIG,
    QEMU_DISK,
    QEMU_DISK_GZIP,
    TART_CONFIG,
    TART_DISK,
    VM_MEDIA_TYPES,
)
from cua_sandbox.registry.ref import parse_ref


class ImageFormat(Enum):
    """Format of a VM image in the registry."""

    ANDROID_AVD = "android-avd"  # trycua Android AVD — gzip-chunked tar, avd media types
    LUME = "lume"  # trycua Lume macOS VM — gzip chunks, lume media types
    OCI_LAYERED = "oci-layered"  # agoda media types, gzip chunks with part annotations
    LEGACY_LZ4 = "legacy-lz4"  # trycua LZ4-chunked
    CHUNKED_PARTS = "chunked-parts"  # standard OCI layer type with ;part.number= in media type
    TART = "tart"  # Cirrus Labs Tart VM disk chunks
    QEMU = "qemu"  # trycua QEMU qcow2 disk + config
    CONTAINER = "container"  # standard Docker/OCI container
    UNKNOWN = "unknown"


def _make_registry(hostname: str) -> oras.provider.Registry:
    """Create an oras Registry, authenticating from env vars or ~/.docker/config.json."""
    import base64
    import json
    import os
    from pathlib import Path

    r = oras.provider.Registry(hostname=hostname)

    # 1. Explicit env vars take priority
    username = os.environ.get("ORAS_USERNAME") or os.environ.get("REGISTRY_USERNAME")
    password = (
        os.environ.get("ORAS_PASSWORD")
        or os.environ.get("REGISTRY_PASSWORD")
        or os.environ.get("GITHUB_TOKEN")
        or os.environ.get("GHCR_TOKEN")
    )
    if username and password:
        r.login(username=username, password=password, hostname=hostname)
        return r

    # 2. Fall back to ~/.docker/config.json
    docker_cfg = Path.home() / ".docker" / "config.json"
    if docker_cfg.exists():
        try:
            cfg = json.loads(docker_cfg.read_text())
            auth_b64 = cfg.get("auths", {}).get(hostname, {}).get("auth", "")
            if auth_b64:
                decoded = base64.b64decode(auth_b64).decode()
                user, token = decoded.split(":", 1)
                r.login(username=user, password=token, hostname=hostname)
        except Exception:
            pass

    return r


def get_manifest(ref: str, platform: Optional[str] = None) -> dict:
    """Fetch the OCI manifest for an image reference.

    If the manifest is a multi-arch index, resolves to the best platform match.

    Args:
        ref: Full or short image reference, e.g.
             'ghcr.io/trycua/macos-sequoia-cua-sparse:latest-oci-layered'
             'trycua/cua-xfce:latest'
        platform: Platform filter e.g. "linux/amd64". Auto-detected if None.
    """
    import platform as _plat

    registry, org, name, tag = parse_ref(ref)
    r = _make_registry(registry)
    full = f"{registry}/{org}/{name}:{tag}"
    manifest = r.get_manifest(full)

    # If it's a manifest index, resolve to a specific platform manifest
    if manifest.get("manifests"):
        arch_map = {"x86_64": "amd64", "AMD64": "amd64", "aarch64": "arm64", "ARM64": "arm64"}
        if platform:
            want_os, want_arch = platform.split("/", 1)
        else:
            want_os = "linux"
            machine = _plat.machine()
            want_arch = arch_map.get(machine, machine.lower())

        # Find best match
        for m in manifest["manifests"]:
            p = m.get("platform", {})
            if p.get("os") == want_os and p.get("architecture") == want_arch:
                # Skip attestation manifests
                annot = m.get("annotations", {})
                if "attestation" in annot.get("vnd.docker.reference.type", ""):
                    continue
                digest = m["digest"]
                full_repo = f"{registry}/{org}/{name}"
                return r.get_manifest(f"{full_repo}@{digest}")

        # Fallback: first non-attestation manifest
        for m in manifest["manifests"]:
            annot = m.get("annotations", {})
            if "attestation" not in annot.get("vnd.docker.reference.type", ""):
                digest = m["digest"]
                full_repo = f"{registry}/{org}/{name}"
                return r.get_manifest(f"{full_repo}@{digest}")

    return manifest


def detect_format(manifest: dict) -> ImageFormat:
    """Determine the image format from a manifest dict."""
    layers = manifest.get("layers", [])
    config = manifest.get("config", {})
    config_mt = config.get("mediaType", "")

    # Android AVD: config uses trycua.android.avd.config type
    if config_mt == ANDROID_AVD_CONFIG:
        return ImageFormat.ANDROID_AVD
    if any(layer.get("mediaType") == ANDROID_AVD_TAR_GZIP for layer in layers):
        return ImageFormat.ANDROID_AVD

    # Lume native OCI format: config uses trycua.lume.config type
    if config_mt == LUME_CONFIG:
        return ImageFormat.LUME
    if any(layer.get("mediaType") in (LUME_DISK, LUME_NVRAM) for layer in layers):
        return ImageFormat.LUME

    # OCI-layered (agoda/kubelet): config or disk layers use agoda media types
    if config_mt in (OCI_VM_CONFIG, OCI_VM_CONFIG_LEGACY):
        return ImageFormat.OCI_LAYERED
    if any(layer.get("mediaType") in (OCI_VM_DISK, OCI_VM_DISK_LEGACY) for layer in layers):
        return ImageFormat.OCI_LAYERED

    # Legacy LZ4
    if config_mt in (LEGACY_DISK_CHUNK,):
        return ImageFormat.LEGACY_LZ4
    if any(layer.get("mediaType") == LEGACY_DISK_CHUNK for layer in layers):
        return ImageFormat.LEGACY_LZ4

    # Chunked-parts: standard OCI layer type but with ;part.number= suffix
    for layer in layers:
        mt = layer.get("mediaType", "")
        if "part.number=" in mt:
            return ImageFormat.CHUNKED_PARTS

    # QEMU (cua): trycua.qemu config or disk layers
    if config_mt == QEMU_CONFIG:
        return ImageFormat.QEMU
    if any(layer.get("mediaType") in (QEMU_DISK, QEMU_DISK_GZIP) for layer in layers):
        return ImageFormat.QEMU

    # Tart (Cirrus Labs): OCI config but cirruslabs disk layers
    if any(layer.get("mediaType") in (TART_DISK, TART_CONFIG) for layer in layers):
        return ImageFormat.TART

    # Standard container
    if config_mt in CONTAINER_CONFIG_TYPES:
        return ImageFormat.CONTAINER
    if any(layer.get("mediaType") in CONTAINER_LAYER_TYPES for layer in layers):
        return ImageFormat.CONTAINER

    return ImageFormat.UNKNOWN


def detect_kind(manifest: dict) -> str:
    """Classify manifest as 'vm' or 'container'."""
    fmt = detect_format(manifest)
    if fmt in (
        ImageFormat.ANDROID_AVD,
        ImageFormat.LUME,
        ImageFormat.OCI_LAYERED,
        ImageFormat.LEGACY_LZ4,
        ImageFormat.CHUNKED_PARTS,
        ImageFormat.TART,
        ImageFormat.QEMU,
    ):
        return "vm"
    if fmt == ImageFormat.CONTAINER:
        return "container"
    # Fallback: check if any layer has a VM media type
    layers = manifest.get("layers", [])
    config_mt = manifest.get("config", {}).get("mediaType", "")
    if config_mt in VM_MEDIA_TYPES or any(
        layer.get("mediaType") in VM_MEDIA_TYPES for layer in layers
    ):
        return "vm"
    return "container"


def detect_os(manifest: dict) -> Optional[str]:
    """Try to infer OS from manifest annotations or media types."""
    # Check top-level annotations
    annot = manifest.get("annotations", {})
    os_val = annot.get("org.trycua.lume.os", "").lower()
    if os_val:
        if "macos" in os_val or "mac" in os_val:
            return "macos"
        if "windows" in os_val:
            return "windows"
        if "linux" in os_val:
            return "linux"

    # lume/agoda media types → macOS
    config_mt = manifest.get("config", {}).get("mediaType", "")
    if config_mt in (OCI_VM_CONFIG, OCI_VM_CONFIG_LEGACY):
        return "macos"
    if any(
        layer.get("mediaType") in (OCI_VM_DISK, OCI_VM_DISK_LEGACY)
        for layer in manifest.get("layers", [])
    ):
        return "macos"

    return None


def detect_os_from_config(ref: str, manifest: dict) -> Optional[str]:
    """Fetch the OCI config blob and read the 'os' field.

    This handles images (like Tart) where OS info is in the config blob
    rather than manifest annotations.  Falls back to detect_os() first.
    """
    os_type = detect_os(manifest)
    if os_type:
        return os_type

    config = manifest.get("config", {})
    digest = config.get("digest")
    if not digest:
        return None

    r = oras.provider.Registry()
    registry, org, name, _tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"

    try:
        resp = r.get_blob(full_repo, digest)
        data = resp.json() if hasattr(resp, "json") else {}
        # Standard OCI: "os" field; QEMU: "guest_os" field
        os_val = (data.get("os") or data.get("guest_os") or "").lower()
        if os_val in ("linux", "windows"):
            return os_val
        if os_val in ("darwin", "macos"):
            return "macos"
    except Exception:
        pass

    # Also check tart config layer
    for layer in manifest.get("layers", []):
        if layer.get("mediaType") == TART_CONFIG:
            try:
                resp = r.get_blob(full_repo, layer["digest"])
                data = resp.json() if hasattr(resp, "json") else {}
                os_val = (data.get("os") or "").lower()
                if os_val in ("linux", "windows"):
                    return os_val
                if os_val in ("darwin", "macos"):
                    return "macos"
            except Exception:
                pass

    return None


def get_layer_info(manifest: dict) -> list[dict]:
    """Extract structured layer info from a manifest.

    Returns a list of dicts with keys: mediaType, digest, size, title,
    part_number, part_total, uncompressed_size.
    """
    result = []
    for layer in manifest.get("layers", []):
        annot = layer.get("annotations", {})
        mt = layer.get("mediaType", "")

        # Parse part info from either annotations or media type string
        part_number = annot.get("org.trycua.lume.part.number")
        part_total = annot.get("org.trycua.lume.part.total")
        if part_number is None and "part.number=" in mt:
            # Parse from media type: "...;part.number=1;part.total=164"
            for segment in mt.split(";"):
                if segment.startswith("part.number="):
                    part_number = segment.split("=", 1)[1]
                elif segment.startswith("part.total="):
                    part_total = segment.split("=", 1)[1]

        result.append(
            {
                "mediaType": mt,
                "digest": layer.get("digest", ""),
                "size": layer.get("size", 0),
                "title": annot.get("org.opencontainers.image.title", ""),
                "part_number": int(part_number) if part_number is not None else None,
                "part_total": int(part_total) if part_total is not None else None,
                "uncompressed_size": int(
                    annot.get("com.agoda.macosvz.content.uncompressed-size", 0)
                )
                or None,
            }
        )
    return result
