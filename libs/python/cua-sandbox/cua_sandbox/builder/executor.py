"""Layer executor — boots a QEMU VM and runs Image layers via computer-server API.

Given a running computer-server at some URL, translates each Image layer dict
into shell commands and executes them sequentially.
"""

from __future__ import annotations

import logging
from typing import Any

import httpx

logger = logging.getLogger(__name__)


class LayerExecutor:
    """Execute Image layer specs against a running computer-server."""

    def __init__(self, base_url: str, timeout: float = 600):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout

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
                    import json

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

    async def _exec_run(self, layer: dict) -> dict:
        return await self.run_command(layer["command"])

    async def _exec_apt_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        return await self.run_command(f"apt-get update && apt-get install -y {pkgs}")

    async def _exec_brew_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        return await self.run_command(f"brew install {pkgs}")

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
        return await self.run_command(
            f"uv add --directory %USERPROFILE%\\cua-server {pkgs}",
            timeout=600,
        )

    async def _exec_pip_install(self, layer: dict) -> dict:
        pkgs = " ".join(layer["packages"])
        return await self.run_command(f"pip install {pkgs}", timeout=600)

    async def _exec_env(self, layer: dict) -> dict:
        # Set persistent env vars via setx (Windows) or export (Linux)
        cmds = []
        for k, v in layer.get("variables", {}).items():
            cmds.append(f'setx {k} "{v}"')
        return await self.run_command(" && ".join(cmds)) if cmds else {"returncode": 0}

    async def _exec_copy(self, layer: dict) -> dict:
        # copy is handled differently — needs file transfer, not shell
        raise NotImplementedError("copy layers require file transfer (not yet implemented)")
