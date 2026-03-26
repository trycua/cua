"""Push VM disk images and container images to an OCI registry.

Supports three image kinds:
  - vm        : qcow2 disk → chunked QEMU OCI artifact (uses qemu_builder.push_image)
  - lume      : macOS VM → pushed via `lume push` CLI, then annotated
  - container : Docker image → pushed via `docker push`, then annotated with
                oras to stamp org.trycua.agent_type on the manifest

The agent_type annotation (org.trycua.agent_type) is embedded in the OCI
manifest so that Image.from_registry() can auto-detect it without the caller
having to pass agent_type= explicitly.

CLI usage:
  # Push a QEMU VM disk (osworld)
  python -m cua_sandbox.registry.push \\
      --ref ghcr.io/trycua/osworld:latest \\
      --source /path/to/disk.qcow2 \\
      --kind vm \\
      --agent-type osworld \\
      --guest-os linux

  # Push an AndroidWorld container image
  python -m cua_sandbox.registry.push \\
      --ref ghcr.io/trycua/androidworld:latest \\
      --source androidworld:latest \\
      --kind container \\
      --agent-type androidworld

  # Push a macOS lume VM (standard cua-computer-server image — no agent-type needed)
  python -m cua_sandbox.registry.push \\
      --ref ghcr.io/trycua/macos-tahoe-cua:latest \\
      --source macos-tahoe-cua \\
      --kind lume
"""

from __future__ import annotations

import logging
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)

AGENT_TYPE_ANNOTATION = "org.trycua.agent_type"


@dataclass
class PushConfig:
    """Parameters for pushing an image to an OCI registry."""

    ref: str
    """Full OCI reference, e.g. ghcr.io/trycua/osworld:latest."""

    source: str
    """Local disk path (for vm kind) or Docker image name (for container kind)."""

    kind: str
    """'vm', 'lume', 'avd', or 'container'."""

    agent_type: Optional[str] = None
    """Agent type to embed as org.trycua.agent_type manifest annotation."""

    # VM-specific options
    guest_os: str = "linux"
    cpu: int = 4
    ram_mb: int = 8192
    disk_size_gb: int = 64

    # Extra manifest annotations
    extra_annotations: dict = field(default_factory=dict)


def push_vm_image(cfg: PushConfig) -> None:
    """Push a qcow2 disk image to an OCI registry as a QEMU artifact.

    Chunks the disk into gzip-compressed layers and stamps the manifest
    with the agent_type annotation.
    """
    from cua_sandbox.registry.qemu_builder import QEMUImageConfig, push_image

    disk_path = Path(cfg.source)
    if not disk_path.exists():
        raise FileNotFoundError(f"Disk image not found: {disk_path}")

    vm_cfg = QEMUImageConfig(
        guest_os=cfg.guest_os,
        cpu=cfg.cpu,
        ram_mb=cfg.ram_mb,
        disk_size_gb=cfg.disk_size_gb,
    )

    # push_image from qemu_builder does the chunking and OCI push.
    # We extend the manifest annotations afterwards with agent_type.
    push_image(disk_path, cfg.ref, config=vm_cfg)

    if cfg.agent_type:
        _annotate_manifest(cfg.ref, cfg.agent_type, cfg.extra_annotations)
        logger.info(f"Annotated {cfg.ref} with {AGENT_TYPE_ANNOTATION}={cfg.agent_type}")


def push_container_image(cfg: PushConfig) -> None:
    """Push a Docker container image to an OCI registry and annotate it.

    Assumes the image is already built locally under cfg.source.
    Tags it as cfg.ref, pushes via docker, then annotates via oras.
    """
    source = cfg.source
    ref = cfg.ref

    # Tag the local image as the target ref (no-op if source == ref)
    if source != ref:
        logger.info(f"Tagging {source} → {ref}")
        _run(["docker", "tag", source, ref])

    logger.info(f"Pushing {ref} via docker push...")
    _run(["docker", "push", ref])

    if cfg.agent_type:
        _annotate_manifest(ref, cfg.agent_type, cfg.extra_annotations)
        logger.info(f"Annotated {ref} with {AGENT_TYPE_ANNOTATION}={cfg.agent_type}")


def push_lume_image(cfg: PushConfig) -> None:
    """Push a macOS lume VM to an OCI registry via the `lume push` CLI.

    cfg.source is the lume VM name (as shown by `lume list`).
    The ref is parsed into registry/organization/name:tag components that
    lume expects separately.

    Standard cua images (macos-tahoe-cua, etc.) run cua-computer-server and
    do not need an agent_type annotation — leave cfg.agent_type as None for
    those. Only set it for custom benchmark images (e.g. agent_type="osworld").
    """
    lume = _which("lume")
    if not lume:
        raise RuntimeError(
            "lume CLI not found. Install from https://github.com/trycua/cua/tree/main/libs/lume"
        )

    from cua_sandbox.registry.ref import parse_ref

    registry, org, name, tag = parse_ref(cfg.ref)

    logger.info(f"Pushing lume VM '{cfg.source}' → {cfg.ref} via lume CLI...")
    _run(
        [
            lume,
            "push",
            cfg.source,
            f"{name}:{tag}",
            "--registry",
            registry,
            "--organization",
            org,
        ]
    )

    if cfg.agent_type:
        _annotate_manifest(cfg.ref, cfg.agent_type, cfg.extra_annotations)
        logger.info(f"Annotated {cfg.ref} with {AGENT_TYPE_ANNOTATION}={cfg.agent_type}")


def push_avd_image(cfg: PushConfig) -> None:
    """Push a pre-baked Android AVD directory to an OCI registry.

    cfg.source is the path to the ``<name>.avd`` directory.
    Creates / updates a multi-arch manifest index at cfg.ref.
    """
    from cua_sandbox.registry.avd_builder import AVDConfig, push_avd

    avd_dir = Path(cfg.source)
    if not avd_dir.is_dir():
        raise FileNotFoundError(f"AVD directory not found: {avd_dir}")

    avd_cfg = AVDConfig(
        api_level=33,
        device_id="pixel_6",
        android_world=(cfg.agent_type == "androidworld"),
        agent_type=cfg.agent_type,
    )
    push_avd(avd_dir, cfg.ref, config=avd_cfg, agent_type=cfg.agent_type)


def push(cfg: PushConfig) -> None:
    """Dispatch to the appropriate push function based on cfg.kind."""
    if cfg.kind == "vm":
        push_vm_image(cfg)
    elif cfg.kind == "lume":
        push_lume_image(cfg)
    elif cfg.kind == "avd":
        push_avd_image(cfg)
    elif cfg.kind == "container":
        push_container_image(cfg)
    else:
        raise ValueError(
            f"Unknown kind {cfg.kind!r}. Expected 'vm', 'lume', 'avd', or 'container'."
        )


# ── Internal helpers ─────────────────────────────────────────────────────────


def _annotate_manifest(ref: str, agent_type: str, extra: dict) -> None:
    """Attach org.trycua.agent_type (and any extra annotations) to an OCI manifest.

    Uses `oras manifest annotate` which rewrites the manifest in-registry.
    Falls back to a pure-Python oras approach if the CLI is unavailable.
    """
    annotations = {AGENT_TYPE_ANNOTATION: agent_type, **extra}

    # Try oras CLI first (most reliable for arbitrary registries)
    oras_cli = _which("oras")
    if oras_cli:
        args = [oras_cli, "manifest", "annotate", ref]
        for k, v in annotations.items():
            args += ["--annotation", f"{k}={v}"]
        _run(args)
        return

    # Fallback: patch manifest directly via oras-py
    _annotate_via_oras_py(ref, annotations)


def _annotate_via_oras_py(ref: str, annotations: dict) -> None:
    """Patch OCI manifest annotations using oras-py."""

    import oras.container as _oras_container
    import oras.provider
    from cua_sandbox.registry.manifest import get_manifest
    from cua_sandbox.registry.ref import parse_ref

    registry, org, name, tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"
    full_ref = f"{full_repo}:{tag}"

    r = oras.provider.Registry()
    manifest = get_manifest(ref)

    existing = manifest.get("annotations") or {}
    manifest["annotations"] = {**existing, **annotations}

    r.upload_manifest(manifest, _oras_container.Container(full_ref))
    logger.info(f"Patched manifest annotations on {full_ref}")


def _run(cmd: list[str]) -> None:
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"Command failed: {' '.join(cmd)}\n" f"stdout: {result.stdout}\nstderr: {result.stderr}"
        )


def _which(name: str) -> Optional[str]:
    import shutil

    return shutil.which(name)


# ── CLI entry point ──────────────────────────────────────────────────────────


def _main() -> None:
    import argparse

    parser = argparse.ArgumentParser(
        description="Push a VM disk or container image to an OCI registry with agent_type annotation."
    )
    parser.add_argument("--ref", required=True, help="OCI ref, e.g. ghcr.io/trycua/osworld:latest")
    parser.add_argument(
        "--source", required=True, help="Local disk path (vm) or Docker image name (container)"
    )
    parser.add_argument(
        "--kind",
        required=True,
        choices=["vm", "lume", "avd", "container"],
        help="Image kind: vm (QEMU qcow2), lume (macOS via lume CLI), avd (Android AVD dir), or container (Docker)",
    )
    parser.add_argument(
        "--agent-type", default=None, help="Agent type to annotate, e.g. osworld or androidworld"
    )
    parser.add_argument(
        "--guest-os", default="linux", help="Guest OS for VM config (default: linux)"
    )
    parser.add_argument("--cpu", type=int, default=4)
    parser.add_argument("--ram-mb", type=int, default=8192)
    parser.add_argument("--disk-size-gb", type=int, default=64)

    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(message)s")

    cfg = PushConfig(
        ref=args.ref,
        source=args.source,
        kind=args.kind,
        agent_type=args.agent_type,
        guest_os=args.guest_os,
        cpu=args.cpu,
        ram_mb=args.ram_mb,
        disk_size_gb=args.disk_size_gb,
    )
    push(cfg)
    print(f"Done: {args.ref}")


if __name__ == "__main__":
    _main()
