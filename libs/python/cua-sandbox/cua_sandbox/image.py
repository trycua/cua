"""Image builder — pure-data immutable chained builder for sandbox images.

Supports Linux, macOS, and Windows constructors. Serializes to a spec dict
for cloud API or cloud-init consumption.

Usage::

    from cua_sandbox import Image

    img = (
        Image.linux("ubuntu", "24.04")
        .apt_install("curl", "git", "build-essential")
        .pip_install("numpy", "pandas")
        .env(MY_VAR="hello")
        .run("echo 'setup complete'")
        .expose(8080)
    )

    spec = img.to_dict()
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

logger = logging.getLogger(__name__)

_IMAGE_CACHE = Path.home() / ".cua" / "cua-sandbox" / "image-cache"


def _download_image(url: str) -> str:
    """Download an image URL to the local cache, extract if zipped.

    Returns the path to the final disk image (qcow2, img, etc.).
    Skips download if the file already exists in the cache.
    """
    import hashlib
    import urllib.request

    _IMAGE_CACHE.mkdir(parents=True, exist_ok=True)

    # Determine filename from URL
    url_filename = url.rsplit("/", 1)[-1].split("?")[0]
    # Use hash prefix to avoid collisions
    url_hash = hashlib.sha256(url.encode()).hexdigest()[:12]
    download_path = _IMAGE_CACHE / f"{url_hash}_{url_filename}"

    # Check if we already have the extracted result
    if download_path.suffix.lower() == ".zip":
        # Look for an already-extracted disk image
        extracted = _find_disk_image(_IMAGE_CACHE / url_hash)
        if extracted:
            logger.info(f"Using cached image: {extracted}")
            return str(extracted)

    if not download_path.exists():
        logger.info(f"Downloading {url} → {download_path}")
        urllib.request.urlretrieve(url, str(download_path))
        logger.info(f"Download complete: {download_path}")

    # Extract zip files
    if download_path.suffix.lower() == ".zip":
        import zipfile

        extract_dir = _IMAGE_CACHE / url_hash
        extract_dir.mkdir(parents=True, exist_ok=True)
        logger.info(f"Extracting {download_path} → {extract_dir}")
        with zipfile.ZipFile(download_path) as zf:
            zf.extractall(extract_dir)
        # Find the disk image inside
        disk = _find_disk_image(extract_dir)
        if not disk:
            raise FileNotFoundError(
                f"No disk image found in {download_path}. "
                f"Contents: {[f.name for f in extract_dir.rglob('*') if f.is_file()]}"
            )
        logger.info(f"Extracted disk image: {disk}")
        return str(disk)

    return str(download_path)


def _find_disk_image(directory: Path) -> Optional[Path]:
    """Find a disk image file in a directory."""
    for ext in (".qcow2", ".img", ".raw", ".vhdx", ".vmdk"):
        for f in directory.rglob(f"*{ext}"):
            return f
    return None


_INSTALL_OS_MAP: Dict[str, Tuple[str, ...]] = {
    "apt_install": ("linux",),
    "brew_install": ("macos",),
    "choco_install": ("windows",),
    "winget_install": ("windows",),
    "apk_install": ("android",),
    "pwa_install": ("android",),
}


@dataclass(frozen=True)
class Image:
    """Immutable, chainable image specification.

    Each mutation method returns a new Image instance so that builders
    can be forked at any point.
    """

    os_type: str  # "linux" | "macos" | "windows" | "android"
    distro: str  # e.g. "ubuntu", "macos", "windows"
    version: str  # e.g. "24.04", "15", "11"
    kind: Optional[str] = None  # "container" | "vm" | None (resolved after registry pull)
    _layers: Tuple[Dict[str, Any], ...] = ()
    _env: Tuple[Tuple[str, str], ...] = ()
    _ports: Tuple[int, ...] = ()
    _files: Tuple[Tuple[str, str], ...] = ()  # (src, dst)
    _registry: Optional[str] = None  # OCI registry reference
    _disk_path: Optional[str] = None  # local disk file path (qcow2, vhdx, raw)
    _agent_type: Optional[str] = None  # e.g. "osworld" for OSWorld Flask server
    _snapshot_source: Optional[Dict[str, Any]] = None  # set by Sandbox.snapshot()

    # ── Constructors ─────────────────────────────────────────────────────

    @classmethod
    def linux(
        cls, distro: str = "ubuntu", version: str = "24.04", kind: str = "container"
    ) -> Image:
        """Linux image. Defaults to 'container' (Docker/XFCE). Use kind='vm' for QEMU."""
        return cls(os_type="linux", distro=distro, version=version, kind=kind)

    @classmethod
    def macos(cls, version: str = "26", kind: str = "vm") -> Image:
        """macOS image. Always a VM (Apple Virtualization / Lume).

        Supported versions: ``"15"`` / ``"sequoia"``, ``"26"`` / ``"tahoe"``.
        """
        from cua_sandbox.runtime.images import MACOS_VERSION_IMAGES

        if version not in MACOS_VERSION_IMAGES:
            supported = ", ".join(f'"{v}"' for v in MACOS_VERSION_IMAGES)
            raise ValueError(f"Unsupported macOS version {version!r}. Supported: {supported}")
        return cls(os_type="macos", distro="macos", version=version, kind=kind)

    @classmethod
    def windows(cls, version: str = "11", kind: str = "vm") -> Image:
        """Windows image. Always a VM (QEMU or Hyper-V)."""
        return cls(os_type="windows", distro="windows", version=version, kind=kind)

    @classmethod
    def android(cls, version: str = "14", kind: str = "vm") -> Image:
        """Android image. Always a VM (QEMU emulator)."""
        return cls(os_type="android", distro="android", version=version, kind=kind)

    @classmethod
    def from_registry(cls, ref: str) -> Image:
        """Create an image from a registry reference. kind is resolved after pull."""
        return cls(os_type="linux", distro="registry", version="latest", kind=None, _registry=ref)

    @classmethod
    def from_file(
        cls,
        path: str,
        *,
        os_type: str = "windows",
        kind: str = "vm",
        agent_type: Optional[str] = None,
    ) -> Image:
        """Create an image from a local disk, ISO file, or URL.

        Supported formats: qcow2, vhdx, raw, img, iso.
        URLs (http/https) are downloaded automatically. Zip files are extracted.
        For ISOs, the runtime will create a qcow2 disk and attach the ISO
        as a CD-ROM for installation/boot.

        Args:
            path: Local file path or URL (http/https).
            os_type: OS type hint ("linux", "windows", "macos", "android").
            kind: "vm" or "container".
            agent_type: Agent type hint (e.g. "osworld" for OSWorld Flask server).
        """
        if path.startswith(("http://", "https://")):
            path = _download_image(path)
        return cls(
            os_type=os_type,
            distro="local",
            version="local",
            kind=kind,
            _disk_path=path,
            _agent_type=agent_type,
        )

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> Image:
        """Reconstruct an Image from a serialized spec dict."""
        img = cls(
            os_type=data["os_type"],
            distro=data["distro"],
            version=data["version"],
            kind=data.get("kind"),
            _registry=data.get("registry"),
        )
        # Replay layers
        for layer in data.get("layers", []):
            img = img._add_layer(layer)
        for k, v in data.get("env", {}).items():
            img = img.env(**{k: v})
        for p in data.get("ports", []):
            img = img.expose(p)
        for src, dst in data.get("files", []):
            img = img.copy(src, dst)
        return img

    def _check_os(self, layer_type: str) -> None:
        """Raise ValueError if this install method is incompatible with os_type."""
        allowed = _INSTALL_OS_MAP.get(layer_type)
        if allowed and self.os_type not in allowed:
            raise ValueError(
                f"{layer_type} is not supported on {self.os_type!r} images. "
                f"Supported OS types: {', '.join(allowed)}"
            )

    # ── Chainable mutations (return new Image) ───────────────────────────

    def _add_layer(self, layer: Dict[str, Any]) -> Image:
        return Image(
            os_type=self.os_type,
            distro=self.distro,
            version=self.version,
            kind=self.kind,
            _layers=self._layers + (layer,),
            _env=self._env,
            _ports=self._ports,
            _files=self._files,
            _registry=self._registry,
            _disk_path=self._disk_path,
            _agent_type=self._agent_type,
            _snapshot_source=self._snapshot_source,
        )

    def _with(self, **kwargs) -> Image:
        """Return a new Image with specific fields overridden."""
        fields = {
            "os_type": self.os_type,
            "distro": self.distro,
            "version": self.version,
            "kind": self.kind,
            "_layers": self._layers,
            "_env": self._env,
            "_ports": self._ports,
            "_files": self._files,
            "_registry": self._registry,
            "_disk_path": self._disk_path,
            "_agent_type": self._agent_type,
            "_snapshot_source": self._snapshot_source,
        }
        fields.update(kwargs)
        return Image(**fields)

    def apt_install(self, *packages: str) -> Image:
        """Install packages via apt (Linux only)."""
        self._check_os("apt_install")
        return self._add_layer({"type": "apt_install", "packages": list(packages)})

    def brew_install(self, *packages: str) -> Image:
        """Install packages via Homebrew (macOS)."""
        self._check_os("brew_install")
        return self._add_layer({"type": "brew_install", "packages": list(packages)})

    def choco_install(self, *packages: str) -> Image:
        """Install packages via Chocolatey (Windows)."""
        self._check_os("choco_install")
        return self._add_layer({"type": "choco_install", "packages": list(packages)})

    def winget_install(self, *packages: str) -> Image:
        """Install packages via winget (Windows)."""
        self._check_os("winget_install")
        return self._add_layer({"type": "winget_install", "packages": list(packages)})

    def apk_install(self, *apk_paths: str) -> Image:
        """Install APK files via adb (Android only)."""
        self._check_os("apk_install")
        return self._add_layer({"type": "apk_install", "packages": list(apk_paths)})

    def pwa_install(
        self,
        manifest_url: str,
        package_name: Optional[str] = None,
        keystore: Optional[str] = None,
        keystore_alias: str = "android",
        keystore_password: str = "android",
    ) -> "Image":
        """Build an APK from a PWA manifest URL via Bubblewrap and install it (Android only).

        Bubblewrap reads the Web App Manifest at *manifest_url*, generates a
        Trusted Web Activity (TWA) APK signed with *keystore*, and installs it
        via adb.  The keystore's SHA-256 fingerprint must match what the server
        serves from ``/.well-known/assetlinks.json`` so Chrome trusts the TWA
        and skips the browser UI entirely.

        Args:
            manifest_url: Full URL to the PWA's ``manifest.json``.
                          Example: ``"http://10.0.2.2:3000/manifest.json"``
            package_name: Android package ID.  Defaults to a reversed-hostname
                          derivation (e.g. ``"com.example.app"``).
            keystore:     Path to a ``*.keystore`` / ``*.jks`` file.  When
                          omitted a fresh keystore is generated and cached under
                          ``~/.cua/cua-sandbox/pwa-cache/``.  Pass the keystore
                          bundled with your PWA repo so the fingerprint is
                          deterministic and pre-loaded into ``assetlinks.json``.
            keystore_alias:    Key alias inside the keystore (default ``"android"``).
            keystore_password: Password for both the store and the key
                               (default ``"android"``).
        """
        self._check_os("pwa_install")
        layer: dict = {"type": "pwa_install", "manifest_url": manifest_url}
        if package_name:
            layer["package_name"] = package_name
        if keystore:
            layer["keystore"] = keystore
        layer["keystore_alias"] = keystore_alias
        layer["keystore_password"] = keystore_password
        return self._add_layer(layer)

    def uv_install(self, *packages: str) -> Image:
        """Install Python packages via uv add into the cua-server project."""
        return self._add_layer({"type": "uv_install", "packages": list(packages)})

    def pip_install(self, *packages: str) -> Image:
        """Install Python packages via pip."""
        return self._add_layer({"type": "pip_install", "packages": list(packages)})

    def run(self, command: str) -> Image:
        """Run a shell command during image build."""
        return self._add_layer({"type": "run", "command": command})

    def env(self, **variables: str) -> Image:
        """Set environment variables."""
        new_env = self._env + tuple(variables.items())
        return self._with(_env=new_env)

    def copy(self, src: str, dst: str) -> Image:
        """Copy a file into the image."""
        new_files = self._files + ((src, dst),)
        return self._with(_files=new_files)

    def expose(self, port: int) -> Image:
        """Expose a port."""
        new_ports = self._ports + (port,)
        return self._with(_ports=new_ports)

    # ── Serialization ────────────────────────────────────────────────────

    def to_dict(self) -> Dict[str, Any]:
        """Serialize to a plain dict suitable for JSON or cloud API."""
        d: Dict[str, Any] = {
            "os_type": self.os_type,
            "distro": self.distro,
            "version": self.version,
            "kind": self.kind,
            "layers": list(self._layers),
        }
        if self._env:
            d["env"] = dict(self._env)
        if self._ports:
            d["ports"] = list(self._ports)
        if self._files:
            d["files"] = [list(f) for f in self._files]
        if self._registry:
            d["registry"] = self._registry
        return d

    def to_cloud_init(self) -> str:
        """Generate a cloud-init user-data script from the image layers."""
        lines = ["#!/bin/bash", "set -e"]
        for k, v in self._env:
            lines.append(f"export {k}={v!r}")
        for layer in self._layers:
            lt = layer["type"]
            if lt == "apt_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"apt-get update && apt-get install -y {pkgs}")
            elif lt == "brew_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"brew install {pkgs}")
            elif lt == "winget_install":
                for pkg in layer["packages"]:
                    lines.append(
                        f"winget install --accept-source-agreements --accept-package-agreements -e --id {pkg}"
                    )
            elif lt == "uv_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"uv add --directory ~/cua-server {pkgs}")
            elif lt == "choco_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"choco install -y {pkgs}")
            elif lt == "pip_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"pip install {pkgs}")
            elif lt == "apk_install":
                for apk in layer["packages"]:
                    lines.append(f"adb install {apk}")
            elif lt == "pwa_install":
                manifest_url = layer["manifest_url"]
                lines.append(
                    # Install bubblewrap CLI (once), init a TWA project from the
                    # manifest URL, build a debug APK, then install it via adb.
                    f"npm install -g @bubblewrap/cli 2>/dev/null || true\n"
                    f"_BWW_DIR=$(mktemp -d)\n"
                    f"(cd \"$_BWW_DIR\" && bubblewrap init --manifest '{manifest_url}' --directory . --skipPwaValidation && bubblewrap build --skipSigning)\n"
                    f'adb install "$_BWW_DIR/app-release-unsigned.apk"'
                )
            elif lt == "run":
                lines.append(layer["command"])
        return "\n".join(lines) + "\n"

    def local_support(self):  # -> RuntimeSupport (lazy import avoids circular dep)
        """Check whether this image can run locally on the current host.

        Returns a :class:`~cua_sandbox.runtime.compat.RuntimeSupport` describing:

        - ``supported``  — runtime is available or auto-installable on this OS
        - ``hw_accel``   — hardware acceleration (HVF / KVM / Hyper-V) is available
        - ``runtime_installed`` — runtime binary found right now (no install needed)
        - ``auto_installable`` — SDK can install the runtime automatically
        - ``reason``     — human-readable explanation

        In tests, use the bundled helper instead of checking manually::

            from cua_sandbox.runtime.compat import skip_if_unsupported

            async def test_something():
                skip_if_unsupported(Image.macos())
                async with Sandbox.ephemeral(Image.macos(), local=True) as sb:
                    ...
        """
        from cua_sandbox.runtime.compat import (  # noqa: F401
            RuntimeSupport,
            check_local_support,
        )

        return check_local_support(self)

    def __repr__(self) -> str:
        reg = f", registry={self._registry!r}" if self._registry else ""
        return (
            f"Image({self.os_type}/{self.distro}:{self.version}, "
            f"kind={self.kind}, {len(self._layers)} layers{reg})"
        )
