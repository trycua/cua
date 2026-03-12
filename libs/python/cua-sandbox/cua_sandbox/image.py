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

import copy
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Sequence, Tuple, Union


@dataclass(frozen=True)
class Image:
    """Immutable, chainable image specification.

    Each mutation method returns a new Image instance so that builders
    can be forked at any point.
    """

    os_type: str  # "linux" | "macos" | "windows"
    distro: str  # e.g. "ubuntu", "macos", "windows"
    version: str  # e.g. "24.04", "15", "11"
    kind: Optional[str] = None  # "container" | "vm" | None (resolved after registry pull)
    _layers: Tuple[Dict[str, Any], ...] = ()
    _env: Tuple[Tuple[str, str], ...] = ()
    _ports: Tuple[int, ...] = ()
    _files: Tuple[Tuple[str, str], ...] = ()  # (src, dst)
    _registry: Optional[str] = None  # OCI registry reference
    _disk_path: Optional[str] = None  # local disk file path (qcow2, vhdx, raw)

    # ── Constructors ─────────────────────────────────────────────────────

    @classmethod
    def linux(cls, distro: str = "ubuntu", version: str = "24.04", kind: str = "container") -> Image:
        """Linux image. Defaults to 'container' (Docker/XFCE). Use kind='vm' for QEMU."""
        return cls(os_type="linux", distro=distro, version=version, kind=kind)

    @classmethod
    def macos(cls, version: str = "15", kind: str = "vm") -> Image:
        """macOS image. Always a VM (Apple Virtualization / Lume)."""
        return cls(os_type="macos", distro="macos", version=version, kind=kind)

    @classmethod
    def windows(cls, version: str = "11", kind: str = "vm") -> Image:
        """Windows image. Always a VM (QEMU or Hyper-V)."""
        return cls(os_type="windows", distro="windows", version=version, kind=kind)

    @classmethod
    def from_registry(cls, ref: str) -> Image:
        """Create an image from a registry reference. kind is resolved after pull."""
        return cls(os_type="linux", distro="registry", version="latest", kind=None, _registry=ref)

    @classmethod
    def from_file(cls, path: str, *, os_type: str = "windows", kind: str = "vm") -> Image:
        """Create an image from a local disk file (qcow2, vhdx, raw, img).

        For JSON/YAML image specs, use ``Image.from_dict()`` instead.
        """
        return cls(os_type=os_type, distro="local", version="local", kind=kind, _disk_path=path)

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
        }
        fields.update(kwargs)
        return Image(**fields)

    def apt_install(self, *packages: str) -> Image:
        """Install packages via apt (Linux only)."""
        return self._add_layer({"type": "apt_install", "packages": list(packages)})

    def brew_install(self, *packages: str) -> Image:
        """Install packages via Homebrew (macOS)."""
        return self._add_layer({"type": "brew_install", "packages": list(packages)})

    def choco_install(self, *packages: str) -> Image:
        """Install packages via Chocolatey (Windows)."""
        return self._add_layer({"type": "choco_install", "packages": list(packages)})

    def winget_install(self, *packages: str) -> Image:
        """Install packages via winget (Windows)."""
        return self._add_layer({"type": "winget_install", "packages": list(packages)})

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
                    lines.append(f"winget install --accept-source-agreements --accept-package-agreements -e --id {pkg}")
            elif lt == "uv_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"uv add --directory ~/cua-server {pkgs}")
            elif lt == "choco_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"choco install -y {pkgs}")
            elif lt == "pip_install":
                pkgs = " ".join(layer["packages"])
                lines.append(f"pip install {pkgs}")
            elif lt == "run":
                lines.append(layer["command"])
        return "\n".join(lines) + "\n"

    def __repr__(self) -> str:
        reg = f", registry={self._registry!r}" if self._registry else ""
        return (
            f"Image({self.os_type}/{self.distro}:{self.version}, "
            f"kind={self.kind}, {len(self._layers)} layers{reg})"
        )
