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
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Mapping, Optional, Tuple

from cua_sandbox._paths import cua_home, patched_or
from cua_sandbox.generated.image_models import ImageFileReference, ImageResource

logger = logging.getLogger(__name__)

#: The canonical images (``cua_image::canonical`` in the native SDK):
#: ``Image.linux()`` is ``ghcr.io/trycua/linux:24.04``, ``Image.windows()``
#: ``ghcr.io/trycua/windows:2022``, ``Image.macos()`` ``ghcr.io/trycua/macos:26``
#: (``"15"``/``"sequoia"``: ``macos:15``). One override per OS:
#: ``CUA_IMAGE_LINUX`` / ``CUA_IMAGE_WINDOWS`` / ``CUA_IMAGE_MACOS`` (the old
#: ``CUA_DEFAULT_LINUX_IMAGE`` / ``CUA_DEFAULT_WINDOWS_IMAGE`` /
#: ``CUA_SANDBOX_LINUX_CONTAINER_IMAGE`` still work, deprecated). The one
#: resolver then picks the variant a backend runs: the rootfs for containers,
#: the ``-disk`` containerDisk for VMs (``kind="vm"``), Lume on a Mac.
CANONICAL_DISTROS: Dict[str, Tuple[str, ...]] = {
    "linux": ("ubuntu",),
    "windows": ("windows",),
    "macos": ("macos",),
}
#: Windows versions with a canonical image; others (``"11"``) install locally
#: from an evaluation ISO.
CANONICAL_WINDOWS_VERSIONS = ("2022",)
MACOS_VERSIONS = ("15", "sequoia", "26", "tahoe")


def canonical_image(os_type: str, version: Optional[str] = None) -> str:
    """The canonical image for ``os_type`` (native ``cua.canonical_image``)."""
    from cua_sandbox._sdk import native

    return native().canonical_image(os_type, version)


def _tier_ref(os_type: str, version: Optional[str], tier: Optional[str]) -> Optional[str]:
    """The registry ref of a non-default tier (``slim``, macOS ``xcode``), or
    ``None`` for the full default. Raises the native
    ``CuaError.ImageNotPublished`` for a tier CI has not published yet."""
    if tier is None or tier.strip().lower() in ("", "full", "default"):
        return None
    from cua_sandbox._sdk import native

    return native().canonical_image_tier(os_type, version, tier)


_IMAGE_CACHE = cua_home() / "cua-sandbox" / "image-cache"
_IMAGE_CACHE_DEFAULT = _IMAGE_CACHE


def _image_cache() -> Path:
    return patched_or(_IMAGE_CACHE, _IMAGE_CACHE_DEFAULT, "cua-sandbox", "image-cache")


def _download_image(url: str) -> str:
    """Download an image URL to the local cache, extract if zipped.

    Returns the path to the final disk image (qcow2, img, etc.).
    Skips download if the file already exists in the cache.
    """
    import hashlib
    import urllib.request

    _image_cache().mkdir(parents=True, exist_ok=True)

    # Determine filename from URL
    url_filename = url.rsplit("/", 1)[-1].split("?")[0]
    # Use hash prefix to avoid collisions
    url_hash = hashlib.sha256(url.encode()).hexdigest()[:12]
    download_path = _image_cache() / f"{url_hash}_{url_filename}"

    # Check if we already have the extracted result
    if download_path.suffix.lower() == ".zip":
        # Look for an already-extracted disk image
        extracted = _find_disk_image(_image_cache() / url_hash)
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

        extract_dir = _image_cache() / url_hash
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
class ImageInfo:
    """The image a sandbox runs, as resolved and pinned at create time.

    ``reference`` is the reference as requested (normalised, e.g.
    ``docker.io/library/python:3.12-slim``), ``pinned_ref`` the
    ``registry/repo@sha256:...`` that ran, ``variant`` one of ``rootfs``,
    ``containerdisk`` or ``lume``, ``arch`` the architecture that
    ran (``amd64``/``arm64``, when known), ``os`` the guest OS and
    ``emulated`` whether it ran under emulation.
    """

    reference: str
    pinned_ref: str
    digest: str
    variant: str
    arch: Optional[str]
    os: str
    emulated: bool

    @classmethod
    def _from_native(cls, obj: Any) -> Optional["ImageInfo"]:
        """From the SDK's ``ImageInfo`` or ``ResolvedImage`` (same fields);
        ``None`` for ``None`` or anything unreadable. Never raises."""
        if obj is None:
            return None
        try:
            pinned = str(obj.pinned_ref)
            digest = getattr(obj, "digest", None) or pinned.partition("@")[2]
            arch = getattr(obj, "arch", None)
            return cls(
                reference=str(obj.reference),
                pinned_ref=pinned,
                digest=str(digest),
                variant=str(obj.variant),
                arch=str(arch) if arch else None,
                os=str(getattr(obj, "os", "") or ""),
                emulated=bool(getattr(obj, "emulated", False)),
            )
        except Exception:  # noqa: BLE001 - informational; never fail the caller
            return None


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
    # Private-registry credentials (RegistrySecret); never serialized.
    _secret: Optional[Any] = field(default=None, compare=False, repr=False)
    # What the native resolver pinned (resolve_image_kind); never serialized.
    _resolved: Optional[ImageInfo] = field(default=None, compare=False, repr=False)

    def __post_init__(self) -> None:
        # ``kind`` is "container", "vm" or None (auto); "auto" means None.
        if isinstance(self.kind, str):
            word = self.kind.strip().lower()
            if word in ("", "auto"):
                object.__setattr__(self, "kind", None)
            elif word in ("container", "vm"):
                object.__setattr__(self, "kind", word)

    # ── Constructors ─────────────────────────────────────────────────────

    @classmethod
    def linux(
        cls,
        distro: str = "ubuntu",
        version: str = "24.04",
        kind: Optional[str] = None,
        tier: Optional[str] = None,
    ) -> Image:
        """Linux: the canonical ``ghcr.io/trycua/linux:<version>`` image.

        ``kind=None`` (or ``"auto"``) runs what the image is (the rootfs: a
        gVisor container locally and in the cloud); ``kind="vm"`` its
        ``-disk`` containerDisk (QEMU locally, KubeVirt in the cloud);
        ``kind="container"`` the rootfs. ``Sandbox.create(kind=...)``
        overrides it.

        ``tier``: ``"full"`` (the default: dev tooling) or ``"slim"``
        (``24.04-slim``: cua-spacesd and Chromium only; what CI runs). A tier
        CI has not published yet raises ``CuaError.ImageNotPublished``; use
        ``Image.from_registry(ref)`` to run it anyway.
        """
        ref = _tier_ref("linux", version, tier)
        return cls(os_type="linux", distro=distro, version=version, kind=kind, _registry=ref)

    @classmethod
    def macos(cls, version: str = "26", kind: str = "vm", tier: Optional[str] = None) -> Image:
        """macOS: the canonical ``ghcr.io/trycua/macos:<version>`` image (Lume locally).

        Supported versions: ``"15"`` / ``"sequoia"``, ``"26"`` / ``"tahoe"``.
        ``tier``: ``"full"`` (the default), ``"slim"`` or ``"xcode"`` /
        ``"xcode-<X.Y>"`` (full plus one pinned Xcode); unpublished tiers
        raise ``CuaError.ImageNotPublished``.
        """
        if version not in MACOS_VERSIONS:
            supported = ", ".join(f'"{v}"' for v in MACOS_VERSIONS)
            raise ValueError(f"Unsupported macOS version {version!r}. Supported: {supported}")
        ref = _tier_ref("macos", version, tier)
        return cls(os_type="macos", distro="macos", version=version, kind=kind, _registry=ref)

    @classmethod
    def windows(cls, version: str = "2022", kind: str = "vm", tier: Optional[str] = None) -> Image:
        """Windows: the canonical ``ghcr.io/trycua/windows:2022`` containerDisk.

        Other versions, including ``"11"``, have no canonical image: on Fleet
        they are unsupported, and locally they are installed from a downloaded
        evaluation ISO. ``tier="slim"`` names ``2022-slim`` once published.
        """
        ref = _tier_ref("windows", version, tier)
        return cls(os_type="windows", distro="windows", version=version, kind=kind, _registry=ref)

    @classmethod
    def omarchy(cls, channel: Optional[str] = None) -> Image:
        """Omarchy (Arch Linux, Hyprland) with cua-spacesd:
        ``ghcr.io/trycua/omarchy:edge`` (or ``"rc"``/``"stable"``).

        An amd64 VM: QEMU locally (emulated on arm64 hosts, so slow there),
        KubeVirt in the cloud. Until CI publishes it this raises
        ``CuaError.ImageNotPublished``; use
        ``Image.from_registry("ghcr.io/trycua/omarchy:edge", kind="vm")`` to
        run it anyway.
        """
        from cua_sandbox._sdk import native

        ref = native().omarchy_image(channel)
        return cls(
            os_type="linux", distro="omarchy", version=channel or "edge", kind="vm", _registry=ref
        )

    @classmethod
    def android(cls, version: str = "14", kind: str = "vm") -> Image:
        """Android image. Always a VM (QEMU emulator)."""
        return cls(os_type="android", distro="android", version=version, kind=kind)

    @classmethod
    def from_registry(
        cls,
        ref: str,
        *,
        os_type: str = "linux",
        kind: Optional[str] = None,
        agent_type: Optional[str] = None,
        secret: Optional[Any] = None,
    ) -> Image:
        """Create an image from a registry reference.

        ``secret`` (a :class:`~cua_sandbox.RegistrySecret`) pulls a private
        image, the same way locally (the pull) and in the cloud (a registry
        pull secret for the sandbox). It is never logged or saved.

        os_type selects the firmware: Windows guest disks are built UEFI-only,
        so a Windows containerDisk pulled from a registry must say so or it is
        handed BIOS and will not boot. kind is resolved after pull when omitted.
        agent_type names the guest control server the disk already runs, the
        same hint as ``from_file``: ``"osworld"`` selects the OSWorld Flask
        server on port 5000 instead of the computer-server on 8000, both for
        local QEMU and for Fleet pools.
        """
        return cls(
            os_type=os_type,
            distro="registry",
            version="latest",
            kind=kind,
            _registry=ref,
            _agent_type=agent_type,
            _secret=secret,
        )

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
            _agent_type=data.get("agent_type"),
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
        return self._with(_layers=self._layers + (layer,))

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
            "_secret": self._secret,
            "_resolved": self._resolved,
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
        builder: str = "pwa2apk",
        push_timeout: Optional[float] = None,
    ) -> "Image":
        """Build an APK from a PWA manifest URL and install it (Android only).

        Two builder backends are supported:

        * ``"pwa2apk"`` (default) — generates a lightweight WebView-based APK.
          No Chrome dependency, no "Running in Chrome" banner, no Digital Asset
          Links needed.  ~10 KB APK.
        * ``"bubblewrap"`` — generates a Chrome Trusted Web Activity (TWA) APK.
          Requires Chrome on the device and a matching
          ``/.well-known/assetlinks.json`` on the server.  Shows a mandatory
          "Running in Chrome" privacy disclosure on every launch.

        Args:
            manifest_url: Full URL to the PWA's ``manifest.json`` or
                          ``manifest.webmanifest``.
                          Example: ``"http://10.0.2.2:3000/manifest.json"``
            package_name: Android package ID.  Defaults to a reversed-hostname
                          derivation (e.g. ``"com.example.app"``).
            keystore:     Path to a ``*.keystore`` / ``*.jks`` file.  When
                          omitted a fresh keystore is generated and cached.
                          Pass the keystore bundled with your PWA repo so the
                          fingerprint is deterministic.
            keystore_alias:    Key alias inside the keystore (default ``"android"``).
            keystore_password: Password for both the store and the key
                               (default ``"android"``).
            builder:      ``"pwa2apk"`` (default) or ``"bubblewrap"``.
            push_timeout: Optional seconds for the ``write_bytes`` ``adb push``
                          of the built APK.  When ``None`` the server default
                          applies.
        """
        if builder not in ("pwa2apk", "bubblewrap"):
            raise ValueError(f"builder must be 'pwa2apk' or 'bubblewrap', got {builder!r}")
        self._check_os("pwa_install")
        layer: dict = {"type": "pwa_install", "manifest_url": manifest_url, "builder": builder}
        if package_name:
            layer["package_name"] = package_name
        if keystore:
            layer["keystore"] = keystore
        layer["keystore_alias"] = keystore_alias
        layer["keystore_password"] = keystore_password
        if push_timeout is not None:
            layer["push_timeout"] = push_timeout
        return self._add_layer(layer)

    def uv_install(self, *packages: str) -> Image:
        """Install Python packages via uv add into the cua-server project."""
        return self._add_layer({"type": "uv_install", "packages": list(packages)})

    def pip_install(self, *packages: str) -> Image:
        """Install Python packages via pip."""
        return self._add_layer({"type": "pip_install", "packages": list(packages)})

    def app_install(self, app_id: str) -> Image:
        """Install an app from the cua-sandbox-apps catalog.

        Requires ``cua-sandbox-apps`` to be installed. The app's install
        script for this image's OS is executed as a build layer.
        """
        return self._add_layer({"type": "app_install", "app_id": app_id})

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

    def to_build_recipe(
        self,
        *,
        name: str,
        namespace: str,
        tags: Mapping[str, str] | None = None,
        timeout_seconds: int | None = None,
        disk_size: str | None = None,
        file_references: Mapping[str, ImageFileReference] | None = None,
    ) -> Dict[str, Any]:
        """Return a CRD-validated Image custom-resource manifest."""
        if self._registry is not None:
            raise ValueError("remote builds do not accept registry-only images")
        if self._disk_path is not None:
            raise ValueError("remote builds do not accept local disk images")
        if self._snapshot_source is not None:
            raise ValueError("remote builds do not accept snapshot source images")
        if self.os_type != "linux" or self.kind not in ("vm", None):
            raise ValueError("remote builds currently support only Linux VM recipes")

        references = dict(file_references or {})
        required_sources = {source for source, _ in self._files}
        missing_sources = sorted(required_sources - references.keys())
        if missing_sources:
            raise ValueError(f"missing file reference for {missing_sources[0]}")
        unused_sources = sorted(references.keys() - required_sources)
        if unused_sources:
            raise ValueError(f"unused file references: {', '.join(unused_sources)}")

        layers = []
        for layer in self._layers:
            if layer["type"] == "app_install":
                layers.append({"type": "app_install", "appId": layer["app_id"]})
            else:
                layers.append(layer)

        recipe: Dict[str, Any] = {
            "osType": self.os_type,
            "distro": self.distro,
            "version": self.version,
            "kind": self.kind or "vm",
            "layers": layers,
        }
        if self._env:
            recipe["env"] = dict(self._env)
        if self._ports:
            recipe["ports"] = list(self._ports)
        if self._files:
            recipe["files"] = [
                {
                    "source": references[source].model_dump(
                        by_alias=True,
                        exclude_none=True,
                        mode="json",
                    ),
                    "destination": destination,
                }
                for source, destination in self._files
            ]

        spec: Dict[str, Any] = {"recipe": recipe}
        if tags:
            spec["metadata"] = {"tags": dict(tags)}
        build: Dict[str, Any] = {}
        if timeout_seconds is not None:
            build["timeoutSeconds"] = timeout_seconds
        if disk_size is not None:
            build["diskSize"] = disk_size
        if build:
            spec["build"] = build

        validated = ImageResource.model_validate(
            {
                "apiVersion": "images.cua.ai/v1alpha1",
                "kind": "Image",
                "metadata": {"name": name, "namespace": namespace},
                "spec": spec,
            }
        )
        return validated.model_dump(by_alias=True, exclude_none=True, mode="json")

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
        if self._agent_type:
            d["agent_type"] = self._agent_type
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
                builder = layer.get("builder", "pwa2apk")
                if builder == "pwa2apk":
                    lines.append(
                        f"# pwa2apk: WebView APK (no Chrome dependency)\n"
                        f"if [ ! -d /tmp/pwa2apk ]; then git clone https://github.com/trycua/pwa2apk.git /tmp/pwa2apk; fi\n"
                        f"node /tmp/pwa2apk/src/cli.js '{manifest_url}' --output /tmp/pwa.apk\n"
                        f"adb install /tmp/pwa.apk"
                    )
                else:
                    lines.append(
                        f"# bubblewrap: Chrome TWA APK\n"
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


def cloud_registry_image(image: Image) -> Optional[str]:
    """The registry reference an image runs from, local or on Fleet.

    An explicit ``Image.from_registry(...)`` reference always wins; the
    built-in descriptors (``Image.linux()``, ``Image.windows()`` for 2022,
    ``Image.macos()``) are the canonical images (see :func:`canonical_image`).
    Other descriptors (custom distros, ``Image.windows("11")``) return ``None``.
    """
    if image._registry is not None:
        return image._registry
    if image.distro not in CANONICAL_DISTROS.get(image.os_type, ()):
        return None
    if image.os_type == "windows" and image.version not in CANONICAL_WINDOWS_VERSIONS:
        return None
    return canonical_image(image.os_type, image.version)


#: What each resolved variant means for :attr:`Image.kind`.
_VARIANT_KIND = {"rootfs": "container", "containerdisk": "vm", "lume": "vm"}


def resolve_image_kind(image: Image) -> Image:
    """Resolve an image's kind (and OS) with the one native resolver.

    Returns the image unchanged when its kind is set or it has no registry
    reference. Short refs are docker.io (``python:3.12-slim``); the registry
    is read with the docker/ghcr/ECR credential chain; a containerDisk runs
    as a VM (QEMU), a rootfs as a container, a Lume image on Lume. A tag the
    registry does not know (one only the local engine has) runs as a
    container. An image no local backend runs raises.
    Blocks while the registry is read.
    """
    if image.kind is not None:
        return image
    ref = cloud_registry_image(image)
    if ref is None:
        return image
    from cua_sandbox._sdk import native

    n = native()
    try:
        if image._secret is not None:
            # A private image: its credentials head the registry auth chain.
            resolved = n.resolve_image_with_secret(ref, "local", None, image._secret.native())
        else:
            resolved = n.resolve_image(ref, "local", None)
    except n.CuaError.NotFound:
        logger.debug("%s is not in a registry; running it as a local container image", ref)
        return image._with(kind="container")
    except n.CuaError.Unauthenticated as error:
        raise PermissionError(
            f"cannot read {ref}: {error}. Log in to its registry (`docker login`), "
            "or set CUA_REGISTRY_USERNAME/CUA_REGISTRY_PASSWORD"
        ) from error
    except n.CuaError.Unsupported as error:
        raise ValueError(str(error)) from error
    except n.CuaError as error:
        # Offline, or resolution switched off (CUA_IMAGE_RESOLVE=0): the
        # local backend decides from the reference (a container).
        logger.warning("could not resolve %s (%s); running it as a container", ref, error)
        return image._with(kind="container")
    kind = _VARIANT_KIND.get(resolved.variant, "container")
    os_type = image.os_type
    if resolved.os != "linux" or resolved.variant == "lume":
        os_type = resolved.os
    # Kept for Sandbox.image_info on runtimes that do not report it.
    return image._with(kind=kind, os_type=os_type, _resolved=ImageInfo._from_native(resolved))
