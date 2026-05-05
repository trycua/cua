"""Basalt-backed QEMU VM builder — WASM edition.

Translates ``Image`` layer specs into a basalt JSON step file and drives
**basalt-wasmtime** (the basalt build system compiled to WASM) via the
``wasmtime`` Python package.  No external ``basalt`` binary needed — the WASM
module is loaded in-process.

The WASM module is located at one of the well-known paths below, or can be
overridden via the ``BASALT_WASM`` environment variable.  If it is not yet
built, ``BasaltWasmLoader`` can compile it on demand (requires a Rust
toolchain with the ``wasm32-unknown-unknown`` target).

Architecture
------------
The basalt-wasmtime WASM module provides a C ABI with:
  - ``init`` — create / reset the build system
  - ``run_pipeline(json_ptr, json_len)`` — load a step-file JSON and run all
    targets
  - ``result_ptr / result_len`` — read output after success
  - ``error_ptr / error_len`` — read error message after failure

The WASM module calls back into the Python host via three imported functions:
  - ``host_exec(cmd_json_ptr, cmd_json_len) -> exit_code``
  - ``host_exec_stdout_len() -> u32``
  - ``host_exec_stdout_read(dest_ptr)``
  - ``host_exec_stderr_len() -> u32``
  - ``host_exec_stderr_read(dest_ptr)``

For ``qemu/local``, the ``host_exec`` callback runs commands **inside the
mounted qcow2 via chroot** (using ``qemu-nbd`` + ``chroot``), mirroring what
the native Rust ``QemuExecutor`` does.  This requires root / sudo.

Cloud builder stubs (``QEMUCloudBuilder``, ``DockerCloudBuilder``) are
provided as ``NotImplementedError`` placeholders until the cloud build API
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
# WASM module location
# ---------------------------------------------------------------------------

_BASALT_ROOT = (
    # Expected location when cua-sandbox lives alongside the cloud repo
    Path(__file__).resolve().parents[6] / "cloud" / "services" / "basalt"
)
_WASM_CRATE = _BASALT_ROOT / "crates" / "basalt-wasmtime"
_WASM_ARTIFACT = (
    _WASM_CRATE / "target" / "wasm32-unknown-unknown" / "release" / "basalt_wasmtime.wasm"
)

_WASM_SEARCH_PATHS: list[Path] = [
    # User can drop a pre-built .wasm anywhere in these well-known spots
    Path.home() / ".cua" / "basalt" / "basalt_wasmtime.wasm",
    Path("/usr/local/lib/cua/basalt_wasmtime.wasm"),
    Path("/opt/cua/basalt_wasmtime.wasm"),
    _WASM_ARTIFACT,  # built from source (cargo build)
]

_DEFAULT_IMAGES_DIR = Path.home() / ".cua" / "cua-sandbox" / "images"


# ---------------------------------------------------------------------------
# WASM loader
# ---------------------------------------------------------------------------

class BasaltWasmLoader:
    """Locate or build the basalt-wasmtime WASM module.

    Priority:
    1. ``BASALT_WASM`` environment variable (explicit path)
    2. Well-known search paths (pre-built artifact)
    3. Build on demand via ``cargo build --target wasm32-unknown-unknown``
       (requires Rust + wasm32 target installed)
    """

    @classmethod
    def load(cls, build_if_missing: bool = True) -> Path:
        """Return the path to a usable basalt_wasmtime.wasm.

        Args:
            build_if_missing: If True, attempt to build the WASM module from
                source when no pre-built artifact is found.

        Returns:
            Path to the ``.wasm`` file.

        Raises:
            RuntimeError: If the module cannot be found or built.
        """
        env_path = os.environ.get("BASALT_WASM")
        if env_path:
            p = Path(env_path)
            if p.is_file():
                return p
            raise RuntimeError(
                f"BASALT_WASM env var points to non-existent file: {p}"
            )

        for candidate in _WASM_SEARCH_PATHS:
            if candidate.is_file():
                logger.debug("BasaltWasmLoader: found WASM at %s", candidate)
                return candidate

        if build_if_missing:
            return cls._build()

        raise RuntimeError(
            "basalt-wasmtime WASM module not found. "
            "Options:\n"
            "  1. Set BASALT_WASM=/path/to/basalt_wasmtime.wasm\n"
            "  2. Run: cd services/basalt/crates/basalt-wasmtime && "
            "cargo build --target wasm32-unknown-unknown --release\n"
            "  3. pip install wasmtime and let BasaltWasmLoader build it automatically\n"
        )

    @classmethod
    def _build(cls) -> Path:
        """Build basalt-wasmtime via cargo."""
        if not shutil.which("cargo"):
            raise RuntimeError(
                "cargo not found — cannot build basalt-wasmtime. "
                "Install Rust from https://rustup.rs/ then run:\n"
                "  rustup target add wasm32-unknown-unknown"
            )

        if not _WASM_CRATE.is_dir():
            raise RuntimeError(
                f"basalt-wasmtime crate not found at {_WASM_CRATE}. "
                "Clone the cloud repo alongside cua-sandbox."
            )

        logger.info(
            "BasaltWasmLoader: building basalt-wasmtime WASM "
            "(first run may take ~30s)…"
        )
        result = subprocess.run(
            [
                "cargo", "build",
                "--target", "wasm32-unknown-unknown",
                "--release",
            ],
            cwd=str(_WASM_CRATE),
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"cargo build failed:\n{result.stderr}"
            )

        if not _WASM_ARTIFACT.is_file():
            raise RuntimeError(
                f"cargo build succeeded but artifact not found at {_WASM_ARTIFACT}"
            )

        logger.info("BasaltWasmLoader: built → %s", _WASM_ARTIFACT)
        return _WASM_ARTIFACT


# ---------------------------------------------------------------------------
# Wasmtime executor
# ---------------------------------------------------------------------------

class _BasaltWasmRunner:
    """Low-level wrapper around the basalt-wasmtime WASM module.

    Instantiates the module with wasmtime-py and exposes a single
    ``run_pipeline(step_file_dict)`` method.

    The ``host_exec_fn`` callback is called for every command that basalt
    wants to execute.  For ``qemu/local`` it runs commands inside a chroot;
    for tests it can be replaced with any callable.

    Args:
        wasm_path: Path to the basalt_wasmtime.wasm module.
        host_exec_fn: ``(cmd: dict) -> (exit_code: int, stdout: bytes, stderr: bytes)``
    """

    def __init__(
        self,
        wasm_path: Path,
        host_exec_fn: Any,
    ) -> None:
        try:
            import wasmtime as wt
        except ImportError:
            raise ImportError(
                "wasmtime-py is required for WASM-based basalt builds. "
                "Install it with: pip install wasmtime"
            ) from None

        self._wt = wt
        self._wasm_path = wasm_path
        self._host_exec_fn = host_exec_fn
        self._instance = None
        self._store = None

    def _build_instance(self) -> tuple:
        """Instantiate the WASM module with the host imports wired up."""
        wt = self._wt

        engine = wt.Engine()
        store = wt.Store(engine)
        module = wt.Module.from_file(engine, str(self._wasm_path))
        linker = wt.Linker(engine)

        # Host-side buffers for the most recent exec's output
        last_stdout: list[bytes] = [b""]
        last_stderr: list[bytes] = [b""]
        host_exec_fn = self._host_exec_fn

        def host_exec(caller, cmd_json_ptr: int, cmd_json_len: int) -> int:
            mem = caller["memory"]
            raw = mem.read(caller, cmd_json_ptr, cmd_json_ptr + cmd_json_len)
            cmd: dict = json.loads(raw)
            try:
                rc, stdout, stderr = host_exec_fn(cmd)
                last_stdout[0] = stdout if isinstance(stdout, bytes) else stdout.encode()
                last_stderr[0] = stderr if isinstance(stderr, bytes) else stderr.encode()
                return rc
            except Exception as exc:
                last_stdout[0] = b""
                last_stderr[0] = str(exc).encode()
                return 1

        def host_exec_stdout_len(_caller) -> int:
            return len(last_stdout[0])

        def host_exec_stdout_read(caller, dest_ptr: int) -> None:
            caller["memory"].write(caller, last_stdout[0], dest_ptr)

        def host_exec_stderr_len(_caller) -> int:
            return len(last_stderr[0])

        def host_exec_stderr_read(caller, dest_ptr: int) -> None:
            caller["memory"].write(caller, last_stderr[0], dest_ptr)

        i32 = wt.ValType.i32()
        linker.define_func("env", "host_exec",
            wt.FuncType([i32, i32], [i32]), host_exec, access_caller=True)
        linker.define_func("env", "host_exec_stdout_len",
            wt.FuncType([], [i32]), host_exec_stdout_len, access_caller=True)
        linker.define_func("env", "host_exec_stdout_read",
            wt.FuncType([i32], []), host_exec_stdout_read, access_caller=True)
        linker.define_func("env", "host_exec_stderr_len",
            wt.FuncType([], [i32]), host_exec_stderr_len, access_caller=True)
        linker.define_func("env", "host_exec_stderr_read",
            wt.FuncType([i32], []), host_exec_stderr_read, access_caller=True)

        instance = linker.instantiate(store, module)
        return store, instance

    def run_pipeline(self, step_file: Dict[str, Any]) -> Dict[str, Any]:
        """Run a basalt pipeline from a step-file dict.

        Args:
            step_file: Dict matching the basalt step-file JSON schema
                (``{"targets": [...], "steps": {...}}``).

        Returns:
            Dict with ``success: bool``, ``output: str``, ``error: str``.
        """
        store, instance = self._build_instance()
        exports = instance.exports(store)

        json_bytes = json.dumps(step_file).encode()
        json_ptr: int = exports["alloc"](store, len(json_bytes))
        exports["memory"].write(store, json_bytes, json_ptr)

        rc: int = exports["run_pipeline"](store, json_ptr, len(json_bytes))
        exports["dealloc"](store, json_ptr, len(json_bytes))

        if rc == 0:
            rptr = exports["result_ptr"](store)
            rlen = exports["result_len"](store)
            output = ""
            if rlen > 0:
                output = exports["memory"].read(store, rptr, rptr + rlen).decode(
                    errors="replace"
                )
            return {"success": True, "output": output, "error": ""}
        else:
            eptr = exports["error_ptr"](store)
            elen = exports["error_len"](store)
            error = ""
            if elen > 0:
                error = exports["memory"].read(store, eptr, eptr + elen).decode(
                    errors="replace"
                )
            return {"success": False, "output": "", "error": error}


# ---------------------------------------------------------------------------
# Chroot host_exec — runs commands inside a mounted qcow2 rootfs
# ---------------------------------------------------------------------------

def _make_chroot_exec_fn(mount_dir: Path):
    """Return a ``host_exec_fn`` that runs commands via ``chroot`` into *mount_dir*.

    Used for ``qemu/local`` builds where the qcow2 is already mounted at
    *mount_dir* by the caller.

    Commands arriving from basalt are ``CommandSpec`` dicts:
    ``{"program": "...", "args": [...], "env": [[k, v], ...], "cwd": "..."}``
    """

    def host_exec(cmd: dict) -> tuple[int, bytes, bytes]:
        program = cmd.get("program", "")
        args = cmd.get("args", [])
        env_pairs: list = cmd.get("env", [])
        cwd: str = cmd.get("cwd") or "/"

        # Build the full command to run inside the chroot
        # We use: chroot <mount_dir> /usr/bin/env <env pairs...> <program> <args...>
        env_prefix = [f"{k}={v}" for k, v in env_pairs]
        chroot_cmd = [
            "chroot", str(mount_dir),
            "/usr/bin/env", *env_prefix, program, *args,
        ]

        logger.debug("chroot exec: %s", " ".join(chroot_cmd))

        try:
            proc = subprocess.run(
                chroot_cmd,
                capture_output=True,
                timeout=600,
            )
            return proc.returncode, proc.stdout, proc.stderr
        except FileNotFoundError:
            msg = f"command not found in chroot: {program}".encode()
            return 127, b"", msg
        except subprocess.TimeoutExpired:
            return 1, b"", b"command timed out"
        except Exception as exc:
            return 1, b"", str(exc).encode()

    return host_exec


# ---------------------------------------------------------------------------
# Layer → basalt step-file translation
# ---------------------------------------------------------------------------

def _layers_to_basalt_steps(
    layers: list[Dict[str, Any]],
    os_type: str = "linux",
) -> Dict[str, Any]:
    """Translate a list of ``Image`` layer dicts into a basalt step-file dict.

    Each layer becomes a named step whose ``run`` command applies the layer
    inside the chroot.  Step ``n+1`` depends on step ``n``, giving a
    sequential build chain.  A content-hash dep is added per step so any
    change to a layer's spec automatically invalidates the basalt cache.

    The returned dict is ready to be passed to ``_BasaltWasmRunner.run_pipeline``
    or serialised to JSON.
    """
    steps: Dict[str, Any] = {}
    prev_step: Optional[str] = None

    for i, layer in enumerate(layers):
        lt = layer["type"]
        step_name = f"layer-{i:03d}-{lt}"

        shell_cmd = _layer_to_shell(layer, os_type)
        if shell_cmd is None:
            shell_cmd = "true"

        # Encode the step as a basalt CommandSpec (program + args)
        run_spec: Dict[str, Any] = {
            "program": "/bin/sh",
            "args": ["-c", shell_cmd],
        }

        deps: List[Dict[str, Any]] = []
        if prev_step:
            deps.append({"type": "step", "name": prev_step})

        # Content-address the layer spec so cache is invalidated on any change
        layer_hash = hashlib.sha256(
            json.dumps(layer, sort_keys=True).encode()
        ).hexdigest()[:16]
        deps.append({
            "type": "exec_output",
            "program": "echo",
            "args": [f"layer-hash:{layer_hash}"],
        })

        steps[step_name] = {
            "run": run_spec,
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
            f'export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH") && '
            f"uv pip install --system {pkgs}"
        )

    if lt == "brew_install":
        pkgs = " ".join(layer["packages"])
        return f"brew install {pkgs}"

    if lt == "env":
        variables = layer.get("variables", {})
        if not variables:
            return "true"
        # Write each variable to /etc/environment (persistent across boots)
        lines = "\n".join(
            f"printf '%s=%s\\n' '{k}' '{v}' >> /etc/environment"
            for k, v in variables.items()
        )
        return lines

    if lt in ("expose", "apk_install", "pwa_install", "app_install"):
        # Not meaningful inside a chroot builder — no-op
        return "true"

    logger.warning("BasaltQEMUBuilder: unknown layer type %r — inserting no-op", lt)
    return "true"


# ---------------------------------------------------------------------------
# Disk helpers
# ---------------------------------------------------------------------------

def _qemu_img_bin() -> str:
    found = shutil.which("qemu-img")
    if found:
        return found
    raise RuntimeError(
        "qemu-img not found. Install qemu-utils (Ubuntu) or qemu (macOS via Homebrew)."
    )


def _file_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _mount_qcow2(qcow2_path: Path, mount_dir: Path, nbd_device: str) -> None:
    """Mount a qcow2 via qemu-nbd + kpartx + mount (requires root/sudo)."""
    mount_dir.mkdir(parents=True, exist_ok=True)

    subprocess.run(["modprobe", "nbd", "max_part=8"], check=True, capture_output=True)
    subprocess.run(
        ["qemu-nbd", "--connect", nbd_device, str(qcow2_path)],
        check=True, capture_output=True,
    )

    # Give the kernel time to populate partition devices
    import time; time.sleep(0.5)

    # Try common root partition suffixes (p1, p2)
    root_part = None
    for suffix in ["p2", "p1"]:
        candidate = f"{nbd_device}{suffix}"
        if Path(candidate).exists():
            root_part = candidate
            break
    if not root_part:
        root_part = f"{nbd_device}p1"  # fall back

    subprocess.run(
        ["mount", root_part, str(mount_dir)],
        check=True, capture_output=True,
    )
    # Bind-mount /dev, /proc, /sys for commands that need them
    for pseudo in ["dev", "proc", "sys"]:
        subprocess.run(
            ["mount", "--bind", f"/{pseudo}", str(mount_dir / pseudo)],
            check=True, capture_output=True,
        )


def _umount_qcow2(mount_dir: Path, nbd_device: str) -> None:
    """Unmount a qcow2 (best-effort; ignores errors)."""
    for pseudo in ["sys", "proc", "dev"]:
        subprocess.run(
            ["umount", str(mount_dir / pseudo)],
            capture_output=True,
        )
    subprocess.run(["umount", str(mount_dir)], capture_output=True)
    subprocess.run(["qemu-nbd", "--disconnect", nbd_device], capture_output=True)


# ---------------------------------------------------------------------------
# BasaltQEMUBuilder
# ---------------------------------------------------------------------------

class BasaltQEMUBuilder:
    """Incrementally build a qcow2 VM disk using basalt via WASM.

    Uses the basalt-wasmtime WASM module (loaded in-process via ``wasmtime``)
    rather than shelling out to a ``basalt`` CLI binary.  The WASM module is
    the portable, in-process build system; the Python host provides
    ``host_exec`` callbacks that run commands inside the mounted qcow2 via
    ``qemu-nbd`` + ``chroot``.

    Args:
        nbd_device: NBD device to use for qcow2 mounting (default ``/dev/nbd0``).
            Requires the ``nbd`` kernel module and root / sudo.
        images_dir: Directory to store built images (default
            ``~/.cua/cua-sandbox/images``).
        wasm_path: Explicit path to the basalt_wasmtime.wasm module.
            Defaults to auto-discovery via :class:`BasaltWasmLoader`.
    """

    def __init__(
        self,
        nbd_device: str = "/dev/nbd0",
        images_dir: Optional[Path] = None,
        wasm_path: Optional[Path] = None,
    ) -> None:
        self.nbd_device = nbd_device
        self.images_dir = images_dir or _DEFAULT_IMAGES_DIR
        self.images_dir.mkdir(parents=True, exist_ok=True)
        self._wasm_path = wasm_path  # None = auto-discover at build time

    def _output_key(self, image: Image, base_qcow2: Path) -> str:
        """Compute a content-based cache key for this (image, base) combination."""
        base_hash = _file_sha256(base_qcow2)[:16]
        layers_json = json.dumps(list(image._layers), sort_keys=True).encode()
        layer_hash = hashlib.sha256(layers_json).hexdigest()[:16] if image._layers else "nolayers"
        return f"{base_hash}-{layer_hash}"

    async def build(
        self,
        image: Image,
        base_qcow2: Path,
        *,
        force: bool = False,
    ) -> Path:
        """Build a qcow2 from *image* layers on top of *base_qcow2*.

        Uses basalt-wasmtime (in-process WASM) to apply layers incrementally.
        The WASM module calls ``host_exec`` for each command, which runs the
        command inside the mounted qcow2 via ``chroot``.

        Requires:
          - ``wasmtime`` Python package (``pip install wasmtime``)
          - ``qemu-nbd`` / ``nbd`` kernel module
          - Root / sudo (for ``modprobe nbd``, ``qemu-nbd``, ``mount``, ``chroot``)

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

        # Locate the WASM module (auto-build if necessary)
        wasm_path = self._wasm_path or BasaltWasmLoader.load()

        # Make a working copy of the base — we mount and modify it in-place
        work_disk = self.images_dir / f"basalt-work-{cache_key}.qcow2"
        shutil.copy2(str(base_qcow2), str(work_disk))

        mount_dir = self.images_dir / f"mnt-{cache_key}"

        try:
            # Mount the working qcow2
            _mount_qcow2(work_disk, mount_dir, self.nbd_device)
            logger.info("BasaltQEMUBuilder: mounted %s at %s", work_disk, mount_dir)

            # Build the step file from image layers
            step_file = _layers_to_basalt_steps(list(image._layers), image.os_type)

            # Create the WASM runner with a chroot host_exec callback
            host_exec_fn = _make_chroot_exec_fn(mount_dir)
            runner = _BasaltWasmRunner(wasm_path, host_exec_fn)

            logger.info(
                "BasaltQEMUBuilder: running pipeline via basalt-wasmtime (%s)",
                wasm_path.name,
            )
            result = runner.run_pipeline(step_file)

            if not result["success"]:
                raise RuntimeError(
                    f"basalt pipeline failed: {result['error']}\n"
                    f"WASM module: {wasm_path}"
                )

            logger.info("BasaltQEMUBuilder: pipeline complete")

        except Exception:
            _umount_qcow2(mount_dir, self.nbd_device)
            work_disk.unlink(missing_ok=True)
            raise
        else:
            _umount_qcow2(mount_dir, self.nbd_device)

        # Clean up mount point
        try:
            mount_dir.rmdir()
        except OSError:
            pass

        # Rename work disk to cache key on success
        work_disk.rename(out_disk)
        logger.info("BasaltQEMUBuilder: built → %s", out_disk)
        return out_disk


# ---------------------------------------------------------------------------
# Cloud builder stubs
# ---------------------------------------------------------------------------

class QEMUCloudBuilder:
    """Stub: cloud-side QEMU/KubeVirt VMI image builder via basalt build API.

    Will POST Image layer specs to the CUA cloud build API, which runs basalt
    (WASM) against a base VM image and returns an OCI image reference suitable
    for use as a KubeVirt containerDisk.

    Not yet implemented — to be wired up once the cloud build API is available.
    """

    def __init__(self, api_key: Optional[str] = None, api_base: Optional[str] = None) -> None:
        self.api_key = api_key
        self.api_base = api_base or "https://api.cua.ai"

    async def build(self, image: Image, *, region: str = "us-east-1") -> str:
        """Build an image on the CUA cloud and return an OCI image ref.

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

    Will POST Image layer specs to the CUA cloud build API, which builds a
    Docker image and returns an OCI image reference suitable for use as a
    gVisor/Incus container (``instanceType: container``).

    Not yet implemented — to be wired up once the cloud build API is available.
    """

    def __init__(self, api_key: Optional[str] = None, api_base: Optional[str] = None) -> None:
        self.api_key = api_key
        self.api_base = api_base or "https://api.cua.ai"

    async def build(self, image: Image, *, region: str = "us-east-1") -> str:
        """Build a Docker image on the CUA cloud and return an OCI ref.

        Raises:
            NotImplementedError: Always — cloud build API not yet implemented.
        """
        raise NotImplementedError(
            "docker/cloud image building is not yet available. "
            "To launch a pre-built image as a gVisor container, use:\n"
            "  Image.from_registry('myorg/myapp:latest').runtime('docker/cloud')\n"
            "Cloud build API coming soon."
        )
