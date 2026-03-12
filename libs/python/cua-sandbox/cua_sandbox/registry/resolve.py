"""Resolve image kind from registry manifest and coordinate pulls.

Uses oras-py for manifest fetching. Delegates actual image pull to:
  - docker pull  (for containers)
  - lume pull    (for macOS VMs on macOS hosts)
  - self-managed download (for VM disk images on other hosts)
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Optional

from cua_sandbox.image import Image
from cua_sandbox.registry.cache import ImageCache
from cua_sandbox.registry.manifest import (
    ImageFormat,
    detect_format,
    detect_kind,
    detect_os,
    detect_os_from_config,
    get_manifest,
)
from cua_sandbox.registry.ref import parse_ref

logger = logging.getLogger(__name__)


def resolve_image_kind(image: Image) -> Image:
    """Fetch manifest for a registry image and return a new Image with kind/os resolved.

    If kind is already set, returns unchanged.
    """
    if image.kind is not None:
        return image
    if not image._registry:
        raise ValueError("Cannot resolve kind: image has no registry reference")

    manifest = get_manifest(image._registry)
    kind = detect_kind(manifest)
    os_type = detect_os_from_config(image._registry, manifest) or image.os_type

    return Image(
        os_type=os_type,
        distro=image.distro,
        version=image.version,
        kind=kind,
        _layers=image._layers,
        _env=image._env,
        _ports=image._ports,
        _files=image._files,
        _registry=image._registry,
        _disk_path=image._disk_path,
    )


def pull_image(
    image: Image,
    *,
    cache: Optional[ImageCache] = None,
    force: bool = False,
) -> tuple[Image, Path]:
    """Pull an image and return (resolved_image, local_path).

    Resolves kind from manifest, then delegates pull to the appropriate backend:
      - container → docker pull (returns the image ref, caller uses DockerRuntime)
      - vm + macOS host → lume pull
      - vm + other host → download blobs via oras
    """
    if not image._registry:
        raise ValueError("Cannot pull: image has no registry reference")

    cache = cache or ImageCache()
    registry, org, name, tag = parse_ref(image._registry)

    # Fetch manifest (always via oras — lightweight)
    manifest = get_manifest(image._registry)
    kind = detect_kind(manifest)
    os_type = detect_os_from_config(image._registry, manifest) or image.os_type
    fmt = detect_format(manifest)

    resolved = Image(
        os_type=os_type,
        distro=image.distro,
        version=image.version,
        kind=kind,
        _layers=image._layers,
        _env=image._env,
        _ports=image._ports,
        _files=image._files,
        _registry=image._registry,
        _disk_path=image._disk_path,
    )

    # Cache the manifest
    dest_dir = cache.save_manifest(registry, org, name, tag, manifest)

    if kind == "container":
        # For containers, docker pull handles everything — just return the ref
        logger.info(f"Container image {image._registry} — use docker pull")
        return resolved, dest_dir

    # QEMU format — use dedicated pull
    if fmt == ImageFormat.QEMU:
        from cua_sandbox.registry.qemu_builder import pull_qemu_image
        disk_path = dest_dir / "disk.qcow2"
        if not force and disk_path.exists():
            logger.info(f"Using cached QEMU image at {dest_dir}")
            return resolved, dest_dir
        logger.info(f"Pulling QEMU image {image._registry}")
        pull_qemu_image(image._registry, dest_dir)
        return resolved, dest_dir

    # Other VM formats — check cache
    disk_path = dest_dir / "disk.img"
    if not force and disk_path.exists():
        logger.info(f"Using cached VM image at {dest_dir}")
        return resolved, dest_dir

    # Pull VM image blobs
    logger.info(f"Pulling VM image {image._registry} ({fmt.value}, {kind})")
    _pull_vm_blobs(image._registry, manifest, dest_dir)

    return resolved, dest_dir


def _pull_vm_blobs(ref: str, manifest: dict, dest_dir: Path) -> None:
    """Download VM disk blobs using oras and reassemble."""
    import oras.provider

    registry, org, name, tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"

    r = oras.provider.Registry()
    layers = manifest.get("layers", [])

    # Separate disk parts from other layers (aux, etc.)
    disk_parts: list[tuple[int, dict]] = []
    other_layers: list[dict] = []

    for layer in layers:
        mt = layer.get("mediaType", "")
        annot = layer.get("annotations", {})

        # Detect part number from annotations or media type
        part_num = annot.get("org.trycua.lume.part.number")
        if part_num is None and "part.number=" in mt:
            for seg in mt.split(";"):
                if seg.startswith("part.number="):
                    part_num = seg.split("=", 1)[1]

        if part_num is not None:
            disk_parts.append((int(part_num), layer))
        else:
            other_layers.append(layer)

    # Download and reassemble disk parts
    if disk_parts:
        disk_parts.sort(key=lambda x: x[0])
        disk_path = dest_dir / "disk.img"
        logger.info(f"  downloading {len(disk_parts)} disk parts...")

        with open(disk_path, "wb") as f:
            for part_num, layer in disk_parts:
                digest = layer["digest"]
                size = layer.get("size", 0)
                logger.info(f"  part {part_num}/{disk_parts[-1][0]}: {size} bytes")

                resp = r.get_blob(full_repo, digest)
                f.write(resp.content)

        logger.info(f"  disk image: {disk_path} ({disk_path.stat().st_size} bytes)")

    # Download other layers (aux image, etc.)
    for layer in other_layers:
        mt = layer.get("mediaType", "")
        digest = layer["digest"]
        title = layer.get("annotations", {}).get("org.opencontainers.image.title", "")

        if "aux" in mt.lower() or "aux" in title.lower():
            out = dest_dir / "aux.img"
        elif title:
            out = dest_dir / title
        else:
            out = dest_dir / f"blob_{digest[:16]}"

        logger.info(f"  downloading {title or mt} ({layer.get('size', '?')} bytes)")
        resp = r.get_blob(full_repo, digest)
        out.write_bytes(resp.content)

    # Download config
    config = manifest.get("config", {})
    if config.get("digest"):
        resp = r.get_blob(full_repo, config["digest"])
        (dest_dir / "config.json").write_bytes(resp.content)
