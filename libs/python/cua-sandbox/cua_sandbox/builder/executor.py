"""Layer executor — runs Image layers via computer-server API.

Given a running computer-server at some URL, translates each Image layer dict
into shell commands and executes them sequentially.
"""

from __future__ import annotations

import base64
import json
import logging
import os
from typing import Any

import httpx

logger = logging.getLogger(__name__)


class LayerExecutor:
    """Execute Image layer specs against a running computer-server."""

    def __init__(self, base_url: str, timeout: float = 600, os_type: str = "linux"):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.os_type = os_type  # "linux", "macos", "windows", "android"

    async def run_command(self, command: str, timeout: float | None = None) -> dict:
        """Run a shell command via computer-server and return the result."""
        t = timeout or self.timeout
        async with httpx.AsyncClient(timeout=t) as client:
            # computer-server uses SSE on /cmd
            resp = await client.post(
                f"{self.base_url}/cmd",
                json={"command": "run_command", "params": {"command": command}},
                timeout=t,
            )
            resp.raise_for_status()

            # Parse SSE response — collect all data lines
            result: dict[str, Any] = {}
            for line in resp.text.splitlines():
                if line.startswith("data: "):
                    try:
                        data = json.loads(line[6:])
                        if isinstance(data, dict):
                            result.update(data)
                    except (json.JSONDecodeError, ValueError):
                        pass

            return result

    async def write_file(self, path: str, content_b64: str, timeout: float | None = None) -> dict:
        """Write a file via computer-server write_bytes command."""
        t = timeout or self.timeout
        async with httpx.AsyncClient(timeout=t) as client:
            resp = await client.post(
                f"{self.base_url}/cmd",
                json={
                    "command": "write_bytes",
                    "params": {"path": path, "content_b64": content_b64},
                },
                timeout=t,
            )
            resp.raise_for_status()
            result: dict[str, Any] = {}
            for line in resp.text.splitlines():
                if line.startswith("data: "):
                    try:
                        data = json.loads(line[6:])
                        if isinstance(data, dict):
                            result.update(data)
                    except (json.JSONDecodeError, ValueError):
                        pass
            return result

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
            # Android: run directly
            cmd = f"bash -c '. /etc/profile.d/cua-env.sh 2>/dev/null; {_bash_escape(cmd)}'"
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
        variables = layer.get("variables", {})
        if not variables:
            return {"success": True, "return_code": 0}
        if self._is_windows():
            cmds = [f'setx {k} "{v}"' for k, v in variables.items()]
            return await self.run_command(" && ".join(cmds))
        # Linux/macOS: append to /etc/environment for persistence
        sudo = "echo lume | sudo -S" if self.os_type == "macos" else "sudo"
        lines = "\n".join(f'{k}="{v}"' for k, v in variables.items())
        cmd = f"{sudo} sh -c 'cat >> /etc/environment << EOF\n{lines}\nEOF'"
        return await self.run_command(cmd)

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
            await self.run_command(f"{sudo} mkdir -p {dst_dir}")
        r = await self.run_command(f"{sudo} mv {tmp_path} {dst}")
        rc = r.get("return_code", r.get("returncode", -1))
        return {"success": rc == 0, "return_code": rc, "stderr": r.get("stderr", "")}

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


def _bash_escape(s: str) -> str:
    """Escape a command string for embedding inside single-quoted bash -c '...'."""
    # Replace single quotes: end the single-quote, add escaped single-quote, restart
    return s.replace("'", "'\\''")


def _sh_quote(s: str) -> str:
    """Wrap string in single quotes, escaping any existing single quotes."""
    return "'" + _bash_escape(s) + "'"
