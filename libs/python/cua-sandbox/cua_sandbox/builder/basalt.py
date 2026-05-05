"""Basalt-backed QEMU VM builder.

Translates ``Image`` layer specs into a basalt JSON step file and drives the
basalt CLI to incrementally build a qcow2 disk image.  Each layer becomes a
basalt step whose deps include the layer contents, giving free content-addressed
caching across builds.

Basalt handles:
  - Mounting the qcow2 via ``qemu-nbd`` + loopback
  - Executing steps in a ``chroot``
  - Caching intermediate results as named qcow2-internal snapshots
    (``basalt-{step}-{hash8}``)
  - Restoring to the last cache-hit snapshot before executing the first miss

This replaces the hand-rolled ``build.py`` implementation for the
``qemu/local`` runtime path.

Usage::

    from pathlib import Path
    from cua_sandbox.builder.basalt import BasaltQEMUBuilder
    from cua_sandbox import Image

    image = (
        Image.linux("ubuntu", "24.04")
        .apt_install("curl", "git")
        .run("curl -LsSf https://astral.sh/uv/install.sh | sh")
    )

    builder = BasaltQEMUBuilder()
    disk = await builder.build(image, base_qcow2=Path("/base/ubuntu-24.04.qcow2"))
    # disk is a content-addressed qcow2 ready to boot

Cloud builder stubs (``qemu/cloud``, ``docker/cloud``) are provided as
``NotImplementedError`` placeholders to be wired up once the cloud build API
is available.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Optional

from cua_sandbox.image import Image

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Default paths
# ---------------------------------------------------------------------------

_IMAGES_DIR = Path.home() / ".cua" / "cua-sandbox" / "images"
_BASALT_BIN_CANDIDATES = [
    "basalt",                                    # on PATH
    str(Path.home() / ".cargo" / "bin" / "basalt"),
    "/usr/local/bin/basalt",
    "/opt/basalt/bin/basalt",
]


def _find_basalt() -> str:
    """Locate the ``basalt`` binary.  Raises ``RuntimeError`` if not found."""
    for candidate in _BASALT_BIN_CANDIDATES:
        if shutil.which(candidate) or Path(candidate).is_file():
            return candidate
    raise RuntimeError(
        "basalt binary not found. "
        "Install it with: cargo install --path services/basalt  "
        "or set BASALT_BIN environment variable."
    )


def _basalt_bin() -> str:
    """Return the basalt binary path, honouring ``BASALT_BIN`` env var."""
    env_bin = os.environ.get("BASALT_BIN")
    if env_bin:
        return env_bin
    return _find_basalt()


# ---------------------------------------------------------------------------
# Layer → basalt step translation
# ---------------------------------------------------------------------------

def _layers_to_basalt_steps(
    layers: list[Dict[str, Any]],
    os_type: str = "linux",
) -> Dict[str, Any]:
    """Translate a list of ``Image`` layer dicts into a basalt step-file dict.

    Each layer becomes a named step whose ``run`` command applies the layer
    inside the chroot.  Step ``n+1`` depends on step ``n``, giving a
    sequential build chain.

    The returned dict is ready to be serialised to JSON and passed to the
    basalt CLI.
    """
    steps: Dict[str, Any] = {}
    prev_step: Optional[str] = None

    for i, layer in enumerate(layers):
        lt = layer["type"]
        step_name = f"layer-{i:03d}-{lt}"

        cmd = _layer_to_shell(layer, os_type)
        if cmd is None:
            # Layer has no shell equivalent (e.g. expose) — insert a no-op
            cmd = "true"

        deps: List[Dict[str, Any]] = []
        if prev_step:
            deps.append({"type": "step", "name": prev_step})

        # Hash the layer spec itself as a content dep so any change invalidates
        # the cache without needing a file on disk.
        layer_hash = hashlib.sha256(
            json.dumps(layer, sort_keys=True).encode()
        ).hexdigest()[:16]
        # Use a known-stable no-op command whose output encodes the layer hash
        deps.append({
            "type": "exec_output",
            "program": "echo",
            "args": [f"layer-hash:{layer_hash}"],
        })

        steps[step_name] = {
            "run": cmd if isinstance(cmd, list) else [{"program": "sh", "args": ["-c", cmd]}],
            "deps": deps,
        }
        prev_step = step_name

    targets = [prev_step] if prev_step else []
    return {"targets": targets, "steps": steps}


def _layer_to_shell(layer: Dict[str, Any], os_type: str) -> Optional[str]:
    """Convert a single layer dict to a shell command string for chroot execution."""
    lt = layer["type"]

    if lt == "apt_install":
        pkgs = " ".join(layer["packages"])
        return (
            f"DEBIAN_FRONTEND=noninteractive apt-get update -qq && "
            f"DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends {pkgs}"
        )

    if lt == "run":
        return layer["command"]

    if lt == "pip_install":
        pkgs = " ".join(layer["packages"])
        return f"pip3 install {pkgs}"

    if lt == "uv_install":
        pkgs = " ".join(layer["packages"])
        return (
            f"command -v uv >/dev/null 2>&1 || "
            f"(curl -LsSf https://astral.sh/uv/install.sh | sh && "
            f"export PATH=\"$HOME/.cargo/bin:$HOME/.local/bin:$PATH\") && "
            f"uv pip install --system {pkgs}"
        )

    if lt == "brew_install":
        pkgs = " ".join(layer["packages"])
        return f"brew install {pkgs}"

    if lt == "env":
        variables = layer.get("variables", {})
        if not variables:
            return "true"
        lines = "\n".join(
            f"printf '%s=%s\\n' '{k}' '{v}' >> /etc/environment"
            for k, v in variables.items()
        )
        return lines

    if lt in ("expose", "apk_install", "pwa_install", "app_install"):
        # Not meaningful inside a chroot builder — skip (no-op)
        return "true"

    # Unknown layer type — let basalt run a no-op so the build doesn't fail
    logger.warning("BasaltQEMUBuilder: unknown layer type %r — inserting no-op", lt)
    return "true"


# ---------------------------------------------------------------------------
# BasaltQEMUBuilder
# ---------------------------------------------------------------------------

class BasaltQEMUBuilder:
    """Incrementally build a qcow2 VM disk using basalt.

    The builder:
    1. Copies the base qcow2 to a working path (if needed)
    2. Generates a basalt JSON step file from ``image._layers``
    3. Invokes the ``basalt`` CLI in qemu mode
    4. Returns the path to the built qcow2

    The output disk is content-addressed by (base_hash, layers_hash) and
    cached at ``~/.cua/cua-sandbox/images/basalt-{hash}.qcow2``.  Subsequent
    calls with identical inputs return the cached disk immediately.

    Args:
        nbd_device: NBD device to use for qcow2 mounting (default ``/dev/nbd0``).
            Requires the ``nbd`` kernel module and root / sudo.
        images_dir: Directory to store built images (default
            ``~/.cua/cua-sandbox/images``).
    """

    def __init__(
        self,
        nbd_device: str = "/dev/nbd0",
        images_dir: Optional[Path] = None,
    ) -> None:
        self.nbd_device = nbd_device
        self.images_dir = images_dir or _IMAGES_DIR
        self.images_dir.mkdir(parents=True, exist_ok=True)

    def _output_key(self, image: Image, base_qcow2: Path) -> str:
        """Compute a content-based cache key for this (image, base) combination."""
        from cua_sandbox.builder.overlay import layers_hash

        base_hash = _file_sha256(base_qcow2)[:16]
        layer_hash = layers_hash(list(image._layers))[:16] if image._layers else "nolayers"
        return f"{base_hash}-{layer_hash}"

    async def build(
        self,
        image: Image,
        base_qcow2: Path,
        *,
        force: bool = False,
    ) -> Path:
        """Build a qcow2 from *image* layers on top of *base_qcow2*.

        Returns the path to the built disk (may be *base_qcow2* itself if
        the image has no layers).

        If the output disk already exists and *force* is False, the cached
        disk is returned immediately without re-running basalt.

        Requires the ``basalt`` binary and root permissions for qemu-nbd /
        chroot (or a suitably configured sudo).

        Args:
            image: The Image whose layers to apply.
            base_qcow2: Read-only base disk (ubuntu, windows, ...).
            force: Rebuild even if a cached output disk exists.

        Returns:
            Path to the built qcow2.
        """
        if not base_qcow2.exists():
            raise FileNotFoundError(f"Base qcow2 not found: {base_qcow2}")

        if not image._layers:
            logger.info("BasaltQEMUBuilder: no layers — returning base qcow2 as-is")
            return base_qcow2

        cache_key = self._output_key(image, base_qcow2)
        out_disk = self.images_dir / f"basalt-{cache_key}.qcow2"

        if out_disk.exists() and not force:
            logger.info("BasaltQEMUBuilder: cache hit — %s", out_disk)
            return out_disk

        logger.info(
            "BasaltQEMUBuilder: building %d layers on top of %s → %s",
            len(image._layers), base_qcow2, out_disk,
        )

        # Make a working copy of the base — basalt will modify it in-place
        work_disk = self.images_dir / f"basalt-work-{cache_key}.qcow2"
        shutil.copy2(str(base_qcow2), str(work_disk))

        # Generate step file
        step_data = _layers_to_basalt_steps(list(image._layers), image.os_type)

        with tempfile.NamedTemporaryFile(
            mode="w",
            suffix=".basalt.json",
            delete=False,
            dir=self.images_dir,
        ) as f:
            json.dump(step_data, f, indent=2)
            step_file = Path(f.name)

        mount_dir = self.images_dir / f"mnt-{cache_key}"
        mount_dir.mkdir(parents=True, exist_ok=True)

        try:
            await self._run_basalt(step_file, work_disk, mount_dir)
        except Exception:
            # Clean up partial output on failure
            work_disk.unlink(missing_ok=True)
            raise
        finally:
            step_file.unlink(missing_ok=True)
            # Best-effort cleanup of mount point
            try:
                mount_dir.rmdir()
            except OSError:
                pass

        # Rename work disk to cache key on success
        work_disk.rename(out_disk)
        logger.info("BasaltQEMUBuilder: built → %s", out_disk)
        return out_disk

    async def _run_basalt(
        self,
        step_file: Path,
        qcow2_path: Path,
        mount_dir: Path,
    ) -> None:
        """Invoke the basalt CLI in qemu mode and stream output to logger."""
        import asyncio

        bin_path = _basalt_bin()
        cmd = [
            bin_path,
            str(step_file),
            "--qcow2",     str(qcow2_path),
            "--mount-dir", str(mount_dir),
            "--nbd-dev",   self.nbd_device,
        ]

        logger.info("BasaltQEMUBuilder: %s", " ".join(cmd))

        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )

        # Stream basalt output through our logger
        assert proc.stdout
        async for line in proc.stdout:
            text = line.decode(errors="replace").rstrip()
            if text:
                logger.info("[basalt] %s", text)

        rc = await proc.wait()
        if rc != 0:
            raise RuntimeError(
                f"basalt exited with code {rc}. "
                f"Check logs above for details. "
                f"Step file: {step_file}"
            )


# ---------------------------------------------------------------------------
# Cloud builder stubs
# ---------------------------------------------------------------------------

class QEMUCloudBuilder:
    """Stub: cloud-side QEMU/KubeVirt VMI image builder via basalt build API.

    This will POST Image layer specs to the CUA cloud build API, which
    runs basalt against a base VM image and returns an OCI image reference
    suitable for use as a KubeVirt containerDisk.

    Not yet implemented — to be wired up once the cloud build API is available.
    """

    def __init__(self, api_key: Optional[str] = None, api_base: Optional[str] = None) -> None:
        self.api_key = api_key
        self.api_base = api_base or "https://api.cua.ai"

    async def build(self, image: Image, *, region: str = "us-east-1") -> str:
        """Build an image on the CUA cloud and return an OCI image ref.

        Args:
            image: Image with layers to apply.
            region: Cloud region to run the build in.

        Returns:
            OCI image reference (e.g. ``"registry.cua.ai/builds/abc123:latest"``)
            that can be passed as ``dockerImage`` to ``POST /v1/vms``.

        Raises:
            NotImplementedError: Always — cloud build API not yet implemented.
        """
        raise NotImplementedError(
            "qemu/cloud image building is not yet available. "
            "To launch a pre-built image as a KubeVirt VMI, use:\n"
            "  Image.from_registry('myorg/myimage:latest').runtime('qemu/cloud')\n"
            "Cloud build API coming soon."
        )


class DockerCloudBuilder:
    """Stub: cloud-side OCI/gVisor container builder API.

    Will POST Image layer specs to the CUA cloud build API, which builds
    a Docker image and returns an OCI image reference suitable for use
    as a gVisor/Incus container (``instanceType: container``).

    Not yet implemented — to be wired up once the cloud build API is available.
    """

    def __init__(self, api_key: Optional[str] = None, api_base: Optional[str] = None) -> None:
        self.api_key = api_key
        self.api_base = api_base or "https://api.cua.ai"

    async def build(self, image: Image, *, region: str = "us-east-1") -> str:
        """Build a Docker image on the CUA cloud and return an OCI ref.

        Args:
            image: Image with layers to apply.
            region: Cloud region to run the build in.

        Returns:
            OCI image reference (e.g. ``"registry.cua.ai/builds/def456:latest"``)
            that can be passed as ``dockerImage`` to ``POST /v1/vms``
            with ``instanceType: "container"``.

        Raises:
            NotImplementedError: Always — cloud build API not yet implemented.
        """
        raise NotImplementedError(
            "docker/cloud image building is not yet available. "
            "To launch a pre-built image as a gVisor container, use:\n"
            "  Image.from_registry('myorg/myapp:latest').runtime('docker/cloud')\n"
            "Cloud build API coming soon."
        )


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _file_sha256(path: Path) -> str:
    """Compute SHA-256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()
