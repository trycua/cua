"""OSWorldTransport — speaks the OSWorld Flask server API (port 5000).

The OSWorld computer-server exposes:
  GET  /screenshot                    → raw PNG bytes
  POST /screen_size                   → {"width": ..., "height": ...}
  POST /execute                       → {"command": [...], "shell": false} → {"output": ...}
  POST /run_bash_script               → {"script": ..., "timeout": ...}
  GET  /accessibility                 → {"AT": ...}
  POST /setup/launch                  → {"command": [...]}
  POST /setup/execute                 → {"command": [...], "shell": false}
  POST /setup/upload                  → multipart file upload

Task lifecycle (OSWorld benchmark):
  setup_task(task_config)   — runs the task's config steps on the VM
                              (launches apps, uploads files, etc.)
  evaluate_task(task_config, qmp_port) — runs postconfig then evaluates
                                          via osworld's Python evaluator

The OSWorld snapshot/reset model uses QEMU QMP savevm/loadvm — the cua
QEMURuntime handles that automatically on sandbox startup. Per-task reset
is done by reverting via QMP before calling setup_task again.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any, Dict, List, Optional

import httpx
from cua_sandbox.transport.base import Transport

logger = logging.getLogger(__name__)


class OSWorldTransport(Transport):
    """Transport for VMs running the OSWorld Flask server (pyautogui-based)."""

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 30.0,
        qmp_host: str = "localhost",
        qmp_port: Optional[int] = None,
    ):
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client: Optional[httpx.AsyncClient] = None
        self._qmp_host = qmp_host
        self._qmp_port = qmp_port
        self._snapshot_name = "osworld-init"
        self._snapshot_saved = False

    async def connect(self) -> None:
        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            timeout=self._timeout,
        )

    async def disconnect(self) -> None:
        if self._client:
            await self._client.aclose()
            self._client = None

    async def send(self, action: str, **params: Any) -> Any:
        assert self._client is not None, "Transport not connected"
        if action in ("execute", "run_bash_script", "run_python"):
            resp = await self._client.post(f"/{action}", json=params)
            resp.raise_for_status()
            return resp.json()
        if action == "accessibility":
            resp = await self._client.get("/accessibility")
            resp.raise_for_status()
            return resp.json()
        if action == "terminal":
            resp = await self._client.get("/terminal")
            resp.raise_for_status()
            return resp.json()
        raise ValueError(f"Unknown action: {action}")

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/screenshot")
        resp.raise_for_status()
        from cua_sandbox.transport.base import convert_screenshot

        return convert_screenshot(resp.content, format, quality)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post("/screen_size")
        resp.raise_for_status()
        data = resp.json()
        return {"width": int(data["width"]), "height": int(data["height"])}

    async def get_environment(self) -> str:
        return "linux"

    # ── OSWorld task lifecycle ────────────────────────────────────────────────

    async def save_snapshot(self, name: Optional[str] = None) -> None:
        """Save a QEMU VM snapshot via QMP for later restore between tasks."""
        if not self._qmp_port:
            raise RuntimeError("qmp_port required for snapshot operations")
        snap = name or self._snapshot_name
        from cua_sandbox.runtime.qemu import _qmp_command

        await _qmp_command(self._qmp_host, self._qmp_port, "stop")
        await _qmp_command(self._qmp_host, self._qmp_port, "savevm", {"name": snap})
        await _qmp_command(self._qmp_host, self._qmp_port, "cont")
        self._snapshot_saved = True
        logger.info(f"Saved QMP snapshot '{snap}'")

    async def restore_snapshot(self, name: Optional[str] = None) -> None:
        """Restore VM to the saved QMP snapshot, then wait for the server to come back."""
        if not self._qmp_port:
            raise RuntimeError("qmp_port required for snapshot operations")
        snap = name or self._snapshot_name
        from cua_sandbox.runtime.qemu import _qmp_command

        await _qmp_command(self._qmp_host, self._qmp_port, "loadvm", {"name": snap})
        await _qmp_command(self._qmp_host, self._qmp_port, "cont")
        logger.info(f"Restored QMP snapshot '{snap}', waiting for server ...")
        await self._wait_for_server(timeout=60)

    async def _wait_for_server(self, timeout: float = 60) -> None:
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    r = await client.get(f"{self._base_url}/screenshot")
                    if r.status_code == 200:
                        return
                except Exception:
                    pass
                await asyncio.sleep(2)
        raise TimeoutError(f"OSWorld server not ready after {timeout}s")

    async def setup_task(self, task_config: Dict[str, Any]) -> None:
        """Run the task's config steps on the VM (launch apps, upload files, etc.).

        task_config is the JSON object from OSWorld's evaluation_examples, e.g.:
            {
              "id": "bb5e4c0d-...",
              "instruction": "Make Bing the default search engine ...",
              "config": [
                {"type": "launch", "parameters": {"command": ["google-chrome", ...]}},
                ...
              ],
              ...
            }

        Mirrors OSWorld's SetupController.setup() but uses direct HTTP to the
        VM's Flask server without needing osworld installed on the host.
        """
        assert self._client is not None, "Transport not connected"
        config_steps: List[Dict[str, Any]] = task_config.get("config", [])
        for step in config_steps:
            step_type = step.get("type", "")
            params = step.get("parameters", {})
            await self._run_setup_step(step_type, params)

    async def _run_setup_step(self, step_type: str, params: Dict[str, Any]) -> None:
        assert self._client is not None
        if step_type == "launch":
            resp = await self._client.post(
                "/setup/launch",
                json=params,
                timeout=60,
            )
            resp.raise_for_status()
        elif step_type in ("execute", "command"):
            resp = await self._client.post(
                "/setup/execute",
                json=params,
                timeout=120,
            )
            resp.raise_for_status()
        elif step_type == "sleep":
            await asyncio.sleep(float(params.get("seconds", 1)))
        elif step_type == "upload":
            # params: {"files": [{"local_path": ..., "dest": ...}]}
            import pathlib

            files_list = params if isinstance(params, list) else params.get("files", [])
            for f in files_list:
                local = pathlib.Path(f["local_path"])
                if local.exists():
                    with open(local, "rb") as fh:
                        resp = await self._client.post(
                            "/setup/upload",
                            files={"file": (local.name, fh)},
                            data={"dest": f.get("dest", f"/root/{local.name}")},
                            timeout=600,
                        )
                        resp.raise_for_status()
        else:
            logger.debug(f"Skipping unhandled setup step type: {step_type!r}")

    async def evaluate_task(
        self,
        task_config: Dict[str, Any],
        *,
        osworld_source_dir: Optional[str] = None,
    ) -> float:
        """Evaluate task completion by running OSWorld's evaluator.

        Requires osworld importable on the host (set osworld_source_dir or
        install it). Runs any postconfig steps first, then calls the evaluator.

        Returns a score in [0.0, 1.0].

        Args:
            task_config: Full OSWorld task JSON (must include "evaluator" key).
            osworld_source_dir: Path to OSWorld source checkout, prepended to
                sys.path so osworld is importable without installation.
        """
        import sys

        if osworld_source_dir and osworld_source_dir not in sys.path:
            sys.path.insert(0, osworld_source_dir)

        # Run postconfig steps (e.g. restart the browser so state is flushed)
        postconfig = task_config.get("evaluator", {}).get("postconfig", [])
        for step in postconfig:
            await self._run_setup_step(step.get("type", ""), step.get("parameters", {}))

        # Import osworld evaluators (host-side, reads VM state via HTTP/adb)
        try:
            from desktop_env.desktop_env import DesktopEnv  # type: ignore

            vm_host = self._base_url.split("://", 1)[-1].split(":")[0]
            vm_port = int(self._base_url.rsplit(":", 1)[-1])
            env = DesktopEnv.__new__(DesktopEnv)
            env.vm_ip = vm_host
            env.server_port = vm_port
            env.http_server = self._base_url
            env.cache_dir = "/tmp/osworld_cache"
            import os

            os.makedirs(env.cache_dir, exist_ok=True)
            from desktop_env.controllers.setup import SetupController  # type: ignore

            env.setup_controller = SetupController(
                vm_ip=vm_host, server_port=vm_port, cache_dir=env.cache_dir
            )
            env._set_task_info(task_config)
            score = env.evaluate()
            return float(score)
        except ImportError:
            raise RuntimeError(
                "osworld (desktop_env) is not importable. "
                "Set osworld_source_dir= or install osworld on the host."
            )
