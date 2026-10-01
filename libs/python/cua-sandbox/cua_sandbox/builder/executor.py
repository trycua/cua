"""Layer executor — runs Image layers through cua-spacesd.

Given a sandbox's cua-spacesd (a ``cua.SpacesdClient``, a ``cua.Sandbox``
handle, or its URL), translates each Image layer dict into shell commands and
file uploads (ProcessService / FilesystemService) and executes them in order.
"""

from __future__ import annotations

import asyncio
import base64
import logging
import os
import time
from typing import Any, Optional

logger = logging.getLogger(__name__)

_OS_EXT = {"linux": "sh", "macos": "sh", "android": "sh", "windows": "ps1"}


def _find_app_install_script(app_id: str, os_type: str) -> str | None:
    """Locate the install script for *app_id* from cua-sandbox-apps.

    Returns the script text, or None if not found.
    """
    ext = _OS_EXT.get(os_type, "sh")
    # Try cua_sandbox_apps package path first
    try:
        from pathlib import Path

        import cua_sandbox_apps

        apps_dir = Path(cua_sandbox_apps.__file__).parent / "apps"
        script_path = apps_dir / app_id / os_type / f"install.{ext}"
        if script_path.exists():
            return script_path.read_text(encoding="utf-8")
    except (ImportError, Exception):
        pass
    return None


def _find_app_launch_script(app_id: str, os_type: str) -> str | None:
    """Locate the launch script for *app_id* from cua-sandbox-apps."""
    ext = _OS_EXT.get(os_type, "sh")
    try:
        from pathlib import Path

        import cua_sandbox_apps

        apps_dir = Path(cua_sandbox_apps.__file__).parent / "apps"
        script_path = apps_dir / app_id / os_type / f"launch.{ext}"
        if script_path.exists():
            return script_path.read_text(encoding="utf-8")
    except (ImportError, Exception):
        pass
    return None


class LayerExecutor:
    """Execute Image layer specs against a running cua-spacesd."""

    def __init__(
        self,
        target: Any,
        timeout: float = 600,
        os_type: str = "linux",
        *,
        token: Optional[str] = None,
        ready_timeout: float = 300,
    ):
        """
        Args:
            target: the spacesd URL (``http://host:3211``) or a connected
                ``cua.SpacesdClient``.
            timeout: default per-command timeout in seconds.
            os_type: guest OS; ``run`` layers are wrapped per OS.
            token: spacesd token for a URL target.
            ready_timeout: how long to wait for the driver to answer (a VM
                that just booted starts it late).
        """
        self.os_type = os_type  # "linux", "macos", "windows", "android"
        self.timeout = timeout
        self._token = token
        self._ready_timeout = ready_timeout
        if isinstance(target, str):
            self.base_url = target.rstrip("/")
            self._env: Any = None
        else:
            self.base_url = None
            self._env = target

    @classmethod
    async def for_sandbox(
        cls, handle: Any, *, os_type: str = "linux", ready_timeout: float = 300
    ) -> "LayerExecutor":
        """An executor for a ``cua.Sandbox`` handle (waits for the driver)."""
        env = await _wait_for_env(lambda: handle.spacesd(15_000), ready_timeout)
        return cls(env, os_type=os_type, ready_timeout=ready_timeout)

    async def _client(self) -> Any:
        if self._env is None:
            from cua_sandbox._sdk import connect_url

            url = self.base_url
            assert url is not None
            sandbox = await connect_url(url, self._token)
            self._env = await _wait_for_env(lambda: sandbox.spacesd(15_000), self._ready_timeout)
        return self._env

    async def run_command(self, command: str, timeout: float | None = None) -> dict:
        """Run a shell command in the guest and return the result dict."""
        from cua_sandbox.transport.env import _output_dict

        t = timeout or self.timeout
        env = await self._client()
        return _output_dict(await env.sh(command, int(t * 1000)))

    async def write_file(self, path: str, content_b64: str, timeout: float | None = None) -> dict:
        """Upload a file to the guest (chunked, SHA-256 verified)."""
        env = await self._client()
        try:
            await env.upload(path, base64.b64decode(content_b64), None)
        except Exception as error:  # noqa: BLE001 - reported like a failed layer
            return {"success": False, "return_code": 1, "error": str(error)}
        return {"success": True, "return_code": 0}

    async def execute_layer(self, layer: dict) -> dict:
        """Execute a single Image layer and return the result."""
        lt = layer["type"]
        handler = getattr(self, f"_exec_{lt}", None)
        if handler is None:
            raise ValueError(f"Unknown layer type: {lt}")
        return await handler(layer)

    async def execute_layers(self, layers: list[dict]) -> list[dict]:
        """Execute all layers sequentially. Raises on first failure."""
        results = []
        for i, layer in enumerate(layers):
            lt = layer["type"]
            logger.info(f"Executing layer {i + 1}/{len(layers)}: {lt}")
            result = await self.execute_layer(layer)
            rc = result.get("return_code", result.get("returncode", -1))
            success = result.get("success", rc == 0)
            if not success or rc not in (0, None):
                logger.error(f"Layer {lt} failed (rc={rc}): {result.get('stderr', '')}")
                raise RuntimeError(
                    f"Layer {i + 1} ({lt}) failed with exit code {rc}: "
                    f"{result.get('stderr', '')}"
                )
            logger.info(f"Layer {lt} completed successfully")
            results.append(result)
        return results

    # ── Per-layer-type handlers ──────────────────────────────────────────

    def _is_windows(self) -> bool:
        return self.os_type == "windows"

    async def _exec_run(self, layer: dict) -> dict:
        cmd = layer["command"]
        if self._is_windows():
            pass
        elif self.os_type == "linux":
            # Linux containers run as a non-root user; use sudo for root access
            cmd = f"sudo bash -c '. /etc/profile.d/cua-env.sh 2>/dev/null; {_bash_escape(cmd)}'"
        elif self.os_type == "macos":
            # macOS VMs: default password is "lume"; pipe it to sudo -S for root access
            cmd = (
                f"echo lume | sudo -S bash -c "
                f"'. /etc/profile.d/cua-env.sh 2>/dev/null; {_bash_escape(cmd)}'"
            )
        else:
            # Android: stock Android uses mksh/sh, not bash; env file is pushed by android_emulator
            cmd = f"sh -c '. /data/local/tmp/.cua_env 2>/dev/null; {_bash_escape(cmd)}'"
        return await self.run_command(cmd)

    async def _exec_apt_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        return await self.run_command(
            f"sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq && "
            f"sudo DEBIAN_FRONTEND=noninteractive apt-get install -y {pkgs}"
        )

    async def _exec_brew_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        return await self.run_command(f"brew install {pkgs}", timeout=900)

    async def _exec_choco_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        return await self.run_command(f"choco install -y {pkgs}", timeout=900)

    async def _exec_winget_install(self, layer: dict) -> dict:
        cmds = []
        for pkg in layer["packages"]:
            cmds.append(
                f"winget install --accept-source-agreements "
                f"--accept-package-agreements -e --id {pkg}"
            )
        combined = " && ".join(cmds)
        return await self.run_command(combined, timeout=900)

    async def _exec_uv_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        if self._is_windows():
            return await self.run_command(
                f"uv add --directory %USERPROFILE%\\cua-server {pkgs}",
                timeout=600,
            )
        # Linux/macOS: install uv if missing, then install packages system-wide
        return await self.run_command(
            f"command -v uv >/dev/null 2>&1 || "
            f"(curl -LsSf https://astral.sh/uv/install.sh | sh && "
            f'export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH") && '
            f"uv pip install --system {pkgs}",
            timeout=600,
        )

    async def _exec_pip_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        if self._is_windows():
            return await self.run_command(f"pip install {pkgs}", timeout=600)
        return await self.run_command(f"pip3 install --break-system-packages {pkgs}", timeout=600)

    async def _exec_env(self, layer: dict) -> dict:
        import re as _re

        variables = layer.get("variables", {})
        if not variables:
            return {"success": True, "return_code": 0}
        if self._is_windows():
            cmds = [f'setx {k} "{v}"' for k, v in variables.items()]
            return await self.run_command(" && ".join(cmds))
        # Validate keys before composing any shell commands
        for k in variables:
            if not _re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                raise ValueError(f"Unsafe env var name: {k!r}")
        # Linux/macOS: append to /etc/environment for persistence.
        # Write each line as a separate printf call so values are treated as
        # literals — no heredoc expansion, no quote stripping.
        sudo = "echo lume | sudo -S" if self.os_type == "macos" else "sudo"
        result: dict = {"success": True, "return_code": 0}
        for k, v in variables.items():
            safe_v = v.replace("'", "'\\''")
            result = await self.run_command(
                f"printf '%s=%s\\n' '{k}' '{safe_v}' | {sudo} tee -a /etc/environment > /dev/null"
            )
            if not result.get("success"):
                return result
        return result

    async def _exec_copy(self, layer: dict) -> dict:
        src = layer["src"]
        dst = layer["dst"]
        if not os.path.exists(src):
            return {"success": False, "return_code": 1, "error": f"Source not found: {src}"}
        with open(src, "rb") as f:
            content_b64 = base64.b64encode(f.read()).decode()
        if self._is_windows():
            dst_dir = dst.rsplit("\\", 1)[0] if "\\" in dst else dst.rsplit("/", 1)[0]
            if dst_dir:
                await self.run_command(f'mkdir "{dst_dir}"')
            result = await self.write_file(dst, content_b64)
            if not result.get("success", False):
                return {"success": False, "return_code": 1, "error": result.get("error", "")}
            return {"success": True, "return_code": 0}
        # Linux/macOS: write to temp then sudo mv to handle root-owned destinations
        import posixpath

        basename = posixpath.basename(dst)
        tmp_path = f"/tmp/_cua_copy_{basename}"
        result = await self.write_file(tmp_path, content_b64)
        if not result.get("success", False):
            return {"success": False, "return_code": 1, "error": result.get("error", "")}
        sudo = "echo lume | sudo -S" if self.os_type == "macos" else "sudo"
        dst_dir = posixpath.dirname(dst)
        if dst_dir and dst_dir != "/":
            await self.run_command(f"{sudo} mkdir -p {_sh_quote(dst_dir)}")
        r = await self.run_command(f"{sudo} mv {_sh_quote(tmp_path)} {_sh_quote(dst)}")
        rc = r.get("return_code", r.get("returncode", -1))
        return {"success": rc == 0, "return_code": rc, "stderr": r.get("stderr", "")}

    async def _exec_app_install(self, layer: dict) -> dict:
        """Install an app from cua-sandbox-apps. Reads its install.sh and runs it."""
        app_id = layer["app_id"]
        # Locate the install script from the apps catalog
        script = _find_app_install_script(app_id, self.os_type)
        if script is None:
            return {
                "success": False,
                "return_code": 1,
                "stderr": f"No install script found for app '{app_id}' on {self.os_type}. "
                f"Install cua-sandbox-apps or run 'cua-sandbox-apps generate' first.",
            }
        return await self.run_command(f"bash -c {_sh_quote(script)}", timeout=900)

    async def _exec_expose(self, layer: dict) -> dict:
        # Expose is a no-op at layer execution time — ports are mapped by the runtime
        return {"success": True, "return_code": 0}

    async def _exec_apk_install(self, layer: dict) -> dict:
        # APK install: transfer the APK file to the device and install via adb
        apk_paths = layer.get("packages", [])
        results = []
        for apk_path in apk_paths:
            if not os.path.exists(apk_path):
                return {
                    "success": False,
                    "return_code": 1,
                    "error": f"APK not found: {apk_path}",
                }
            remote_path = f"/data/local/tmp/{os.path.basename(apk_path)}"
            with open(apk_path, "rb") as f:
                content_b64 = base64.b64encode(f.read()).decode()
            await self.write_file(remote_path, content_b64)
            r = await self.run_command(f"pm install -r {remote_path}", timeout=120)
            results.append(r)
        return results[-1] if results else {"success": True, "return_code": 0}

    async def _exec_pwa_install(self, layer: dict) -> dict:
        # PWA install — just a placeholder; actual install happens via the sandbox
        return {"success": True, "return_code": 0}


async def _wait_for_env(open_env: Any, ready_timeout: float) -> Any:
    """Connect to cua-spacesd, retrying while it is not up yet."""
    from cua_sandbox._sdk import SpacesdNotAvailable, is_env_not_available

    deadline = time.monotonic() + ready_timeout
    delay = 1.0
    while True:
        try:
            return await open_env()
        except Exception as error:  # noqa: BLE001 - classified below
            if not is_env_not_available(error):
                raise
            if time.monotonic() >= deadline:
                raise SpacesdNotAvailable(
                    f"cua-spacesd did not answer within {ready_timeout:.0f}s: {error}"
                ) from error
            await asyncio.sleep(delay)
            delay = min(delay * 2, 5.0)


def _bash_escape(s: str) -> str:
    """Escape a command string for embedding inside single-quoted bash -c '...'."""
    # Replace single quotes: end the single-quote, add escaped single-quote, restart
    return s.replace("'", "'\\''")


def _sh_quote(s: str) -> str:
    """Wrap string in single quotes, escaping any existing single quotes."""
    return "'" + _bash_escape(s) + "'"
