"""Build and push/pull pre-baked Android AVD directories as OCI artifacts.

An Android AVD (Android Virtual Device) is a directory that contains all state
needed to boot the emulator — system image overlays, user data, app snapshots,
SD card, and hardware config.  Pre-baking an AVD with apps already installed
(e.g. AndroidWorld's 20 apps + their state snapshots) eliminates the ~15 min
one-time setup cost on every fresh run.

OCI format
----------
Each arch variant (arm64 / amd64) is a separate manifest:
  config blob : JSON   — application/vnd.trycua.android.avd.config.v1+json
  layers      : tar.gz — application/vnd.trycua.android.avd.tar.v1+gzip
                         (chunked in 500 MB parts with part.number annotations)

The top-level ref is a manifest *index* that points to both arch manifests so
that Image.from_registry("ghcr.io/trycua/androidworld-avd:latest") auto-selects
the right variant on arm64 (Apple Silicon) and amd64 (Linux/Windows x86_64).

Multi-arch push workflow
------------------------
  # 1. Bake the AVD on Apple Silicon (produces arm64-v8a userdata):
  python -m cua_sandbox.registry.avd_builder push \\
      --avd-dir ~/.cua/android-avd/androidworld.avd \\
      --ref     ghcr.io/trycua/androidworld-avd:latest \\
      --arch    arm64 \\
      --agent-type androidworld

  # 2. Bake the AVD on x86_64 Linux/Windows and push similarly:
  python -m cua_sandbox.registry.avd_builder push \\
      --avd-dir ~/.cua/android-avd/androidworld.avd \\
      --ref     ghcr.io/trycua/androidworld-avd:latest \\
      --arch    amd64 \\
      --agent-type androidworld

  # Both push commands automatically update the manifest index at :latest.

Pull / extraction
-----------------
  config, avd_dir = pull_avd("ghcr.io/trycua/androidworld-avd:latest")
  # → ~/.cua/android-avd/androidworld.avd extracted and ready to boot
"""

from __future__ import annotations

import gzip
import hashlib
import json
import logging
import platform as _plat
import shutil
import tarfile
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)

CHUNK_SIZE = 500 * 1024 * 1024  # 500 MB
_AVD_HOME = Path.home() / ".cua" / "android-avd"
AGENT_TYPE_ANNOTATION = "org.trycua.agent_type"


@dataclass
class AVDConfig:
    """Metadata for a pre-baked AVD image, stored as the OCI config blob."""

    api_level: int = 33
    img_type: str = "google_apis"
    device_id: str = "pixel_6"
    arch: str = "arm64-v8a"  # "arm64-v8a" | "x86_64"
    android_world: bool = False
    """True when AndroidWorld apps are pre-installed in this AVD."""
    agent_type: Optional[str] = None
    """Agent type hint — copied to manifest annotation (e.g. 'androidworld')."""
    extra: dict = field(default_factory=dict)


# ── Host arch detection ───────────────────────────────────────────────────────


def _host_oci_arch() -> str:
    """Return the OCI architecture string for the current host."""
    machine = _plat.machine().lower()
    if machine in ("arm64", "aarch64"):
        return "arm64"
    return "amd64"


def _avd_arch_for_oci(oci_arch: str) -> str:
    """Map OCI architecture to Android ABI string."""
    return "arm64-v8a" if oci_arch == "arm64" else "x86_64"


# ── Push ─────────────────────────────────────────────────────────────────────


def push_avd(
    avd_dir: str | Path,
    ref: str,
    *,
    arch: Optional[str] = None,
    config: Optional[AVDConfig] = None,
    agent_type: Optional[str] = None,
) -> None:
    """Tar, chunk, and push an AVD directory to an OCI registry.

    After pushing the per-arch manifest, creates or updates the multi-arch
    manifest index at ``ref`` so that from_registry() can select the right
    variant automatically.

    Args:
        avd_dir: Path to the ``<name>.avd`` directory.
        ref: Full OCI reference, e.g. ``ghcr.io/trycua/androidworld-avd:latest``.
        arch: OCI architecture string: ``"arm64"`` or ``"amd64"``.
              Defaults to the host architecture.
        config: AVDConfig metadata to store in the config blob.
        agent_type: Value for the ``org.trycua.agent_type`` annotation.
    """
    import oras.provider
    from cua_sandbox.registry.media_types import (
        ANDROID_AVD_CONFIG,
        ANDROID_AVD_TAR_GZIP,
    )
    from cua_sandbox.registry.ref import parse_ref

    avd_dir = Path(avd_dir)
    if not avd_dir.is_dir():
        raise FileNotFoundError(f"AVD directory not found: {avd_dir}")

    oci_arch = arch or _host_oci_arch()
    avd_arch = _avd_arch_for_oci(oci_arch)

    if config is None:
        config = AVDConfig(arch=avd_arch, android_world=(agent_type == "androidworld"))
    if agent_type:
        config.agent_type = agent_type

    import os

    registry, org, name, tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"
    r = oras.provider.Registry(hostname=registry)

    # Authenticate if credentials are available
    username = os.environ.get("ORAS_USERNAME") or os.environ.get("REGISTRY_USERNAME")
    password = os.environ.get("ORAS_PASSWORD") or os.environ.get("REGISTRY_PASSWORD")
    if username and password:
        r.login(username=username, password=password, hostname=registry)
        logger.info(f"Authenticated to {registry} as {username}")

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)

        # 1. Create config blob
        config_data = json.dumps(asdict(config)).encode()
        config_digest = f"sha256:{hashlib.sha256(config_data).hexdigest()}"
        config_path = tmp_dir / "config.json"
        config_path.write_bytes(config_data)

        # 2. Tar the AVD directory
        tar_path = tmp_dir / "avd.tar"
        logger.info(f"Archiving AVD {avd_dir} ...")
        with tarfile.open(tar_path, "w") as tf:
            tf.add(avd_dir, arcname=avd_dir.name)

        tar_size = tar_path.stat().st_size
        logger.info(f"AVD archive: {tar_size / 1024 / 1024:.0f} MB uncompressed")

        # 3. Chunk-compress the tar
        layers = []
        part_total = (tar_size + CHUNK_SIZE - 1) // CHUNK_SIZE

        with open(tar_path, "rb") as f:
            for part_num in range(1, part_total + 1):
                chunk = f.read(CHUNK_SIZE)
                if not chunk:
                    break
                compressed = gzip.compress(chunk, compresslevel=6)
                chunk_path = tmp_dir / f"avd.part.{part_num:04d}.tar.gz"
                chunk_path.write_bytes(compressed)
                chunk_digest = f"sha256:{hashlib.sha256(compressed).hexdigest()}"

                layers.append(
                    {
                        "mediaType": ANDROID_AVD_TAR_GZIP,
                        "digest": chunk_digest,
                        "size": len(compressed),
                        "annotations": {
                            "org.trycua.android.avd.part.number": str(part_num),
                            "org.trycua.android.avd.part.total": str(part_total),
                            "org.trycua.android.avd.uncompressed-size": str(len(chunk)),
                            "org.opencontainers.image.title": f"avd.tar.part.{part_num:04d}",
                        },
                    }
                )
                logger.info(
                    f"  part {part_num}/{part_total}: {len(compressed) / 1024 / 1024:.1f} MB"
                )
                r.upload_blob(str(chunk_path), full_repo, layers[-1])

        # 4. Push config blob
        r.upload_blob(
            str(config_path),
            full_repo,
            {"digest": config_digest, "size": len(config_data), "mediaType": ANDROID_AVD_CONFIG},
        )

        # 5. Build per-arch manifest
        annotations = {
            "org.trycua.android.avd.arch": oci_arch,
            "org.trycua.android.avd.api-level": str(config.api_level),
        }
        if agent_type:
            annotations[AGENT_TYPE_ANNOTATION] = agent_type

        manifest = {
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": ANDROID_AVD_CONFIG,
                "digest": config_digest,
                "size": len(config_data),
            },
            "layers": layers,
            "annotations": annotations,
        }
        manifest_json = json.dumps(manifest)
        manifest_digest = f"sha256:{hashlib.sha256(manifest_json.encode()).hexdigest()}"
        manifest_size = len(manifest_json.encode())

        # Push per-arch manifest as a tagged ref (e.g. :latest-arm64 / :latest-amd64)
        arch_tag = f"{tag}-{oci_arch}"
        import oras.container as _oras_container

        r.upload_manifest(manifest, _oras_container.Container(f"{full_repo}:{arch_tag}"))
        logger.info(f"Pushed {full_repo}:{arch_tag} ({part_total} layers, {oci_arch})")

    # 6. Update / create the manifest index at :tag
    _upsert_manifest_index(
        r=r,
        full_repo=full_repo,
        tag=tag,
        arch=oci_arch,
        manifest_digest=manifest_digest,
        manifest_size=manifest_size,
        agent_type=agent_type,
    )
    logger.info(f"Manifest index updated: {full_repo}:{tag}")


def _put_manifest_raw(r, full_repo: str, tag: str, manifest: dict) -> None:
    """PUT an OCI manifest (any mediaType) via raw HTTP, bypassing oras-py schema validation."""
    import json as _json

    import oras.container as _oras_container

    body = _json.dumps(manifest).encode()
    container = _oras_container.Container(f"{full_repo}:{tag}")
    url = f"{r.prefix}://{container.manifest_url()}"
    headers = {
        "Content-Type": manifest.get("mediaType", "application/vnd.oci.image.index.v1+json"),
        "Content-Length": str(len(body)),
    }
    resp = r.do_request(url, "PUT", headers=headers, data=body)
    if resp.status_code not in (200, 201):
        raise RuntimeError(f"PUT manifest failed ({resp.status_code}): {resp.text[:300]}")
    logger.info(f"PUT manifest → {full_repo}:{tag} ({resp.status_code})")


def _upsert_manifest_index(
    r,
    full_repo: str,
    tag: str,
    arch: str,
    manifest_digest: str,
    manifest_size: int,
    agent_type: Optional[str],
) -> None:
    """Create or update a multi-arch OCI manifest index.

    Fetches the existing index (if any), replaces or adds the entry for
    ``arch``, then pushes the updated index.
    """
    existing_manifests: list[dict] = []

    # Try to fetch existing index
    try:
        existing = r.get_manifest(f"{full_repo}:{tag}")
        if existing.get("mediaType") == "application/vnd.oci.image.index.v1+json":
            # Filter out any stale entry for this arch
            existing_manifests = [
                m
                for m in existing.get("manifests", [])
                if m.get("platform", {}).get("architecture") != arch
            ]
    except Exception:
        pass

    new_entry: dict = {
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "digest": manifest_digest,
        "size": manifest_size,
        "platform": {"os": "linux", "architecture": arch},
    }
    if agent_type:
        new_entry["annotations"] = {AGENT_TYPE_ANNOTATION: agent_type}

    index_annotations: dict = {}
    if agent_type:
        index_annotations[AGENT_TYPE_ANNOTATION] = agent_type

    index = {
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": existing_manifests + [new_entry],
        "annotations": index_annotations,
    }
    # oras-py upload_manifest validates against the regular manifest schema
    # (requires 'config'), which manifest indexes don't have. Use raw PUT.
    _put_manifest_raw(r, full_repo, tag, index)


# ── Pull ─────────────────────────────────────────────────────────────────────


def pull_avd(
    ref: str,
    dest_avd_home: Optional[Path] = None,
    *,
    force: bool = False,
) -> tuple[AVDConfig, Path]:
    """Pull an AVD OCI image and extract it into dest_avd_home.

    The manifest index is resolved to the correct arch variant automatically.
    Returns (config, avd_dir) where avd_dir is the extracted ``.avd`` directory.
    """
    import oras.provider
    from cua_sandbox.registry.manifest import get_manifest
    from cua_sandbox.registry.media_types import (
        ANDROID_AVD_TAR_GZIP,
    )
    from cua_sandbox.registry.ref import parse_ref

    dest_avd_home = dest_avd_home or _AVD_HOME
    dest_avd_home.mkdir(parents=True, exist_ok=True)

    registry, org, name, tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"

    # get_manifest already resolves manifest indexes to the host arch
    manifest = get_manifest(ref)

    # Parse config
    r = oras.provider.Registry()
    config_blob = manifest.get("config", {})
    config = AVDConfig()
    if config_blob.get("digest"):
        try:
            resp = r.get_blob(full_repo, config_blob["digest"])
            data = resp.json() if hasattr(resp, "json") else {}
            config = AVDConfig(
                **{k: v for k, v in data.items() if k in AVDConfig.__dataclass_fields__}
            )
        except Exception as e:
            logger.warning(f"Could not parse AVD config blob: {e}")

    # Determine extraction target — use AVD dir name from config or manifest name
    avd_name = f"{name}.avd"
    avd_dir = dest_avd_home / avd_name

    if avd_dir.exists() and not force:
        logger.info(f"Using cached AVD at {avd_dir}")
        return config, avd_dir

    # Collect and sort tar chunks
    layers = manifest.get("layers", [])
    parts: list[tuple[int, dict]] = []
    for layer in layers:
        if layer.get("mediaType") != ANDROID_AVD_TAR_GZIP:
            continue
        annot = layer.get("annotations", {})
        part_num = int(annot.get("org.trycua.android.avd.part.number", 1))
        parts.append((part_num, layer))

    if not parts:
        raise RuntimeError(f"No AVD tar layers found in manifest for {ref}")

    parts.sort(key=lambda x: x[0])

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)
        tar_path = tmp_dir / "avd.tar"

        logger.info(f"Downloading {len(parts)} AVD parts for {ref} ({config.arch}) ...")
        with open(tar_path, "wb") as out:
            for part_num, layer in parts:
                digest = layer["digest"]
                size = layer.get("size", 0)
                logger.info(f"  part {part_num}/{parts[-1][0]}: {size / 1024 / 1024:.1f} MB")
                resp = r.get_blob(full_repo, digest)
                out.write(gzip.decompress(resp.content))

        # Extract the tar into dest_avd_home
        logger.info(f"Extracting AVD to {dest_avd_home} ...")
        if avd_dir.exists():
            shutil.rmtree(avd_dir)
        with tarfile.open(tar_path, "r") as tf:
            tf.extractall(dest_avd_home)

    if not avd_dir.exists():
        raise RuntimeError(f"AVD extraction failed — {avd_dir} not found after extract")

    logger.info(f"AVD ready at {avd_dir}")
    return config, avd_dir


# ── CLI ───────────────────────────────────────────────────────────────────────


def _main() -> None:
    import argparse

    parser = argparse.ArgumentParser(
        description="Push or pull a pre-baked Android AVD to/from an OCI registry."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    push_p = sub.add_parser("push", help="Push an AVD directory to OCI")
    push_p.add_argument("--avd-dir", required=True, help="Path to <name>.avd directory")
    push_p.add_argument(
        "--ref", required=True, help="OCI ref, e.g. ghcr.io/trycua/androidworld-avd:latest"
    )
    push_p.add_argument(
        "--arch",
        default=None,
        choices=["arm64", "amd64"],
        help="OCI architecture (default: auto-detect host)",
    )
    push_p.add_argument(
        "--agent-type", default=None, help="Agent type annotation, e.g. androidworld"
    )
    push_p.add_argument("--api-level", type=int, default=33)
    push_p.add_argument("--device-id", default="pixel_6")

    pull_p = sub.add_parser("pull", help="Pull an AVD image from OCI and extract it")
    pull_p.add_argument("--ref", required=True)
    pull_p.add_argument(
        "--dest", default=None, help="AVD home directory (default: ~/.cua/android-avd)"
    )
    pull_p.add_argument("--force", action="store_true")

    args = parser.parse_args()

    import logging as _logging

    _logging.basicConfig(level=_logging.INFO, format="%(message)s")

    if args.command == "push":
        cfg = AVDConfig(
            api_level=args.api_level,
            device_id=args.device_id,
            arch=_avd_arch_for_oci(args.arch or _host_oci_arch()),
            android_world=(args.agent_type == "androidworld"),
            agent_type=args.agent_type,
        )
        push_avd(
            args.avd_dir,
            args.ref,
            arch=args.arch,
            config=cfg,
            agent_type=args.agent_type,
        )
        print(f"Done: {args.ref}")

    elif args.command == "pull":
        dest = Path(args.dest) if args.dest else None
        config, avd_dir = pull_avd(args.ref, dest, force=args.force)
        print(f"Extracted: {avd_dir}  (api={config.api_level}, arch={config.arch})")


if __name__ == "__main__":
    _main()
