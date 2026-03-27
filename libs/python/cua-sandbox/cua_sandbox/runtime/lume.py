"""Lume runtime — macOS VMs via Apple Virtualization.framework (macOS hosts only).

Requires the Lume CLI running on the host (default port 7777).
"""

from __future__ import annotations

import asyncio
import logging
import os
import sys
from typing import TYPE_CHECKING

import httpx
from cua_sandbox.image import Image

if TYPE_CHECKING:
    pass
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.images import (
    LUME_API_PORT,
    LUME_PROVIDER_PORT,
    MACOS_SEQUOIA,
    MACOS_VERSION_IMAGES,
)

logger = logging.getLogger(__name__)


def _lume_path() -> str | None:
    """Return the path to the lume binary, or None if not found."""
    # Check PATH first
    import shutil

    if shutil.which("lume"):
        return "lume"
    # Common install location not always in PATH (e.g. ~/.local/bin)
    local_bin = os.path.expanduser("~/.local/bin/lume")
    if os.path.isfile(local_bin) and os.access(local_bin, os.X_OK):
        return local_bin
    return None


def _has_lume() -> bool:
    return _lume_path() is not None


class LumeRuntime(Runtime):
    """Runs macOS VMs via the Lume CLI / API."""

    def __init__(
        self,
        *,
        lume_host: str = "localhost",
        lume_port: int = LUME_PROVIDER_PORT,
        api_port: int = LUME_API_PORT,
    ):
        self.lume_host = lume_host
        self.lume_port = lume_port
        self.api_port = api_port

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if not _has_lume():
            raise RuntimeError(
                "Lume CLI is not installed. "
                "Install from https://github.com/trycua/cua/tree/main/libs/lume"
            )

        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.get(f"{lume_url}/lume/vms/{name}")
            vm = resp.json()

            if vm.get("status") == "running":
                logger.info(f"Lume VM {name} already running")
            else:
                oci_ref = (
                    image._registry
                    or MACOS_VERSION_IMAGES.get(image.version or "")
                    or MACOS_SEQUOIA
                )
                logger.info(f"Pulling {oci_ref} via Lume...")
                # Lume's pull API expects image="name:tag" with registry/organization
                # passed separately — passing a full ref causes double-prefixing of the org.
                pull_payload: dict = {"image": oci_ref, "name": name}
                parts = oci_ref.split("/")
                if len(parts) >= 3 and "." in parts[0]:
                    pull_payload = {
                        "image": "/".join(parts[2:]),
                        "name": name,
                        "registry": parts[0],
                        "organization": parts[1],
                    }
                # Use /lume/pull/start (async, returns 202 immediately) so we can
                # poll progress while the pull runs in the background.
                # Falls back to the synchronous /lume/pull if start returns 404.
                #
                # Use a fresh client for the pull POST — Lume's HTTP server closes
                # the connection after a 4xx GET response, and reusing the same
                # httpx connection causes the POST body to arrive empty.
                try:
                    start_resp = await self._pull_start_with_retry(lume_url, pull_payload)
                except (httpx.ReadError, httpx.RemoteProtocolError):
                    # /pull/start not available in lume v0.3.x — fall back to sync /pull,
                    # then run the VM and return directly (bypassing the async poll below).
                    logger.info(
                        "Lume /pull/start unavailable (v0.3.x), falling back to synchronous pull..."
                    )
                    print("\rPulling macOS image...", end="", flush=True, file=sys.stderr)
                    try:
                        sync_resp = await client.post(
                            f"{lume_url}/lume/pull",
                            json=pull_payload,
                            timeout=1800,
                        )
                        if sync_resp.status_code >= 400:
                            try:
                                detail = sync_resp.json()
                            except Exception:
                                detail = sync_resp.text
                            raise RuntimeError(f"Lume pull failed for '{name}': {detail}")
                    except (httpx.ReadError, httpx.RemoteProtocolError):
                        # Lume v0.3.x drops the sync pull connection when done.
                        # Verify the VM was created.
                        try:
                            check = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                            if check.status_code != 200 or check.json().get("status") in (
                                "",
                                None,
                                "error",
                            ):
                                raise RuntimeError(
                                    f"Lume pull failed for '{name}': connection dropped and VM not found"
                                    " (check GITHUB_TOKEN is set in lume's LaunchAgent plist)"
                                )
                        except httpx.HTTPError as e:
                            raise RuntimeError(f"Lume pull failed for '{name}': {e}") from e
                    print(file=sys.stderr)
                    logger.info(f"Running Lume VM {name} (v0.3.x path)...")
                    run_resp = await client.post(
                        f"{lume_url}/lume/vms/{name}/run",
                        json={
                            "noDisplay": opts.get("no_display", True),
                            "sharedDirectories": opts.get("shared_directories", []),
                        },
                        timeout=120,
                    )
                    if run_resp.status_code >= 400:
                        try:
                            detail = run_resp.json()
                        except Exception:
                            detail = run_resp.text
                        raise RuntimeError(f"Lume failed to run VM '{name}': {detail}")
                    ip = await self._wait_for_ip(name, lume_url)
                    info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
                    await self.is_ready(info)
                    await self._apply_image_layers(image, info)
                    return info
                if start_resp.status_code == 404:
                    # Older lume without /pull/start — fall back to synchronous pull
                    logger.info(
                        "Lume does not support /pull/start, falling back to synchronous pull..."
                    )
                    try:
                        pull_resp = await client.post(
                            f"{lume_url}/lume/pull",
                            json=pull_payload,
                            timeout=1800,
                        )
                    except (httpx.ReadError, httpx.RemoteProtocolError):
                        # Lume v0.3.x drops the HTTP connection after pull
                        # completes. Check if the VM was actually created.
                        logger.info(
                            f"Lume sync pull connection dropped — checking if VM '{name}' was created..."
                        )
                        try:
                            check = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                            if check.status_code == 200:
                                vm_data = check.json()
                                if vm_data.get("status") not in ("", None, "error"):
                                    logger.info(
                                        f"VM '{name}' exists after connection drop — pull succeeded"
                                    )
                                    # fall through to run the VM
                                    pull_resp = check  # dummy, not used further
                            else:
                                raise RuntimeError(
                                    f"Lume pull failed for '{name}': connection dropped and VM not found"
                                    " (check GITHUB_TOKEN is set in lume's LaunchAgent plist)"
                                )
                        except httpx.HTTPError as check_err:
                            raise RuntimeError(
                                f"Lume pull failed for '{name}': connection dropped and could not verify VM: {check_err}"
                            ) from check_err
                    if pull_resp.status_code >= 400:
                        try:
                            detail = pull_resp.json()
                        except Exception:
                            detail = pull_resp.text
                        raise RuntimeError(f"Lume pull failed for '{name}': {detail}")
                elif start_resp.status_code >= 400:
                    try:
                        detail = start_resp.json()
                    except Exception:
                        detail = start_resp.text
                    raise RuntimeError(f"Lume pull/start failed for '{name}': {detail}")
                else:
                    # Poll /lume/vms/{name} until status leaves "pulling"
                    logger.info(f"Pulling image for '{name}'...")
                    pull_deadline = asyncio.get_event_loop().time() + 1800
                    last_progress = -1.0
                    while asyncio.get_event_loop().time() < pull_deadline:
                        try:
                            poll = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                        except (httpx.ReadError, httpx.RemoteProtocolError, httpx.ConnectError):
                            await asyncio.sleep(3)
                            continue
                        if poll.status_code == 200:
                            data = poll.json()
                            status = data.get("status", "")
                            progress = data.get("downloadProgress")
                            if progress is not None and progress != last_progress:
                                print(
                                    f"\rPulling macOS image: {progress:.0f}%",
                                    end="",
                                    flush=True,
                                    file=sys.stderr,
                                )
                                last_progress = progress
                            if status == "pulling":
                                await asyncio.sleep(3)
                                continue
                            # Pull failed — lume surfaced an error via status
                            if "error" in status.lower():
                                raise RuntimeError(
                                    f"Lume pull failed for '{name}': {data.get('message', status)}"
                                )
                            break
                        elif poll.status_code >= 400:
                            # VM not found — pull may have failed before creating the VM
                            data = {}
                            try:
                                data = poll.json()
                            except Exception:
                                pass
                            raise RuntimeError(
                                f"Lume pull failed for '{name}': {data.get('message', poll.text)}"
                            )
                        await asyncio.sleep(3)
                    else:
                        raise TimeoutError(f"Lume pull for '{name}' did not complete within 1800s")
                    if last_progress >= 0:
                        print(file=sys.stderr)  # newline after progress bar
                logger.info(f"Running Lume VM {name}...")
                run_resp = await client.post(
                    f"{lume_url}/lume/vms/{name}/run",
                    json={
                        "noDisplay": opts.get("no_display", True),
                        "sharedDirectories": opts.get("shared_directories", []),
                    },
                    timeout=120,
                )
                if run_resp.status_code >= 400:
                    try:
                        detail = run_resp.json()
                    except Exception:
                        detail = run_resp.text
                    raise RuntimeError(f"Lume failed to run VM '{name}': {detail}")

            ip = await self._wait_for_ip(name, lume_url)

        await self._deliver_vnc_config(name, lume_url)
        info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
        await self.is_ready(info)
        await self._apply_image_layers(image, info)
        return info

    async def _pull_start_with_retry(
        self, lume_url: str, payload: dict, retries: int = 3
    ) -> httpx.Response:
        """POST /lume/pull/start with retry for spurious 'Invalid request body' 400s.

        Lume's custom HTTP server reads with minimumIncompleteLength=1, so on a
        fresh TCP connection the body can arrive after the first receive() returns,
        causing a spurious 400.  A brief pause + retry recovers reliably.
        """
        last_resp: httpx.Response | None = None
        for attempt in range(retries):
            if attempt > 0:
                await asyncio.sleep(1)
            async with httpx.AsyncClient(timeout=30) as pull_client:
                resp = await pull_client.post(
                    f"{lume_url}/lume/pull/start",
                    json=payload,
                    timeout=30,
                )
            if resp.status_code != 400 or "Invalid request body" not in resp.text:
                return resp  # success or a real error
            logger.warning(
                "pull/start got 'Invalid request body' (attempt %d/%d), retrying...",
                attempt + 1,
                retries,
            )
            last_resp = resp
        return last_resp  # type: ignore[return-value]

    async def _apply_image_layers(self, image: "Image", info: RuntimeInfo) -> None:
        """Apply image layers (run, brew_install, env, copy, etc.) via computer-server."""
        env_items = getattr(image, "_env", ())
        file_items = getattr(image, "_files", ())
        has_work = image._layers or file_items
        if not has_work and not env_items:
            return
        from cua_sandbox.builder.executor import LayerExecutor

        executor = LayerExecutor(f"http://{info.host}:{info.api_port}", os_type=image.os_type)
        if env_items and image.os_type != "windows":
            if image.os_type == "macos":
                # macOS: computer-server runs under launchd with an explicit
                # EnvironmentVariables dict in its plist.  launchctl setenv is
                # ignored by such services, so we must add vars to the plist
                # directly via PlistBuddy, then unload/load the service.
                plist = "~/Library/LaunchAgents/com.trycua.computer_server.plist"
                for k, v in env_items:
                    safe_v = v.replace('"', '\\"')
                    # Use Set if key exists, Add otherwise
                    await executor.run_command(
                        f'/usr/libexec/PlistBuddy -c "Set :EnvironmentVariables:{k} {safe_v}" {plist} 2>/dev/null || '
                        f'/usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:{k} string {safe_v}" {plist}'
                    )
                # Reload the launchd service so it picks up the new env
                await executor.run_command(f"launchctl unload {plist} && launchctl load {plist}")
                await self.is_ready(info)  # wait for computer-server to come back
            else:
                sudo = "sudo"
                await executor.run_command(
                    f"printf '#!/bin/sh\\n' | {sudo} tee /etc/profile.d/cua-env.sh > /dev/null"
                )
                for k, v in env_items:
                    safe_v = v.replace("'", "'\\''")
                    await executor.run_command(
                        f"printf 'export {k}=\"{safe_v}\"\\n' "
                        f"| {sudo} tee -a /etc/profile.d/cua-env.sh > /dev/null"
                    )
        for src, dst in file_items:
            await executor.execute_layers([{"type": "copy", "src": src, "dst": dst}])
        if image._layers:
            await executor.execute_layers(list(image._layers))

    async def _deliver_vnc_config(self, name: str, lume_url: str) -> None:
        """Write the current VNC port/password into ~/.vnc.env inside the VM.

        Lume v0.3.x doesn't auto-deliver VNC config via VirtioFS, so the
        computer-server inside the VM may use a stale cached port from a
        previous run.  We push the live values via `lume ssh` so the
        computer-server always connects to the correct VNC endpoint.
        """
        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.get(f"{lume_url}/lume/vms/{name}")
                data = resp.json()
                vnc_url = data.get("vncUrl") or data.get("vnc_url")
                if not vnc_url:
                    return  # no VNC info — nothing to deliver
                # Parse vnc://:PASSWORD@HOST:PORT
                import re

                m = re.match(r"vnc://:([^@]*)@[^:]+:(\d+)", vnc_url)
                if not m:
                    return
                password, port = m.group(1), m.group(2)
            vnc_env = f"VNC_PORT={port}\\nVNC_PASSWORD={password}"
            # Write vnc.env and kill the computer-server process so launchd
            # revives it with the new config.  launchctl kickstart -k fails
            # silently from a non-GUI SSH session, so pkill is more reliable.
            proc = await asyncio.create_subprocess_exec(
                _lume_path() or "lume",
                "ssh",
                name,
                "--",
                f"printf '{vnc_env}' > ~/.vnc.env && "
                "pkill -f 'python.*computer_server' 2>/dev/null || true",
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL,
            )
            await asyncio.wait_for(proc.wait(), timeout=30)
            logger.info(f"Delivered VNC config to '{name}' (port {port})")
        except Exception as exc:
            logger.debug(f"VNC config delivery skipped for '{name}': {exc}")

    async def _wait_for_ip(self, name: str, lume_url: str, timeout: float = 300) -> str:
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=10) as client:
            while asyncio.get_event_loop().time() < deadline:
                resp = await client.get(f"{lume_url}/lume/vms/{name}")
                data = resp.json()
                status = data.get("status", "")
                if status == "stopped":
                    raise RuntimeError(f"Lume VM '{name}' is stopped — failed to start")
                ip = data.get("ip_address") or data.get("ipAddress")
                if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                    return ip
                await asyncio.sleep(3)
        raise TimeoutError(f"Lume VM {name} did not get an IP within {timeout}s")

    async def suspend(self, name: str) -> None:
        """Stop (save state of) a Lume VM."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            await client.post(f"{lume_url}/lume/vms/{name}/stop")

    async def resume(self, image: "Image", name: str, **opts) -> RuntimeInfo:
        """Start a stopped Lume VM and return its RuntimeInfo."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=120) as client:
            await client.post(
                f"{lume_url}/lume/vms/{name}/run",
                json={
                    "noDisplay": opts.get("no_display", True),
                    "sharedDirectories": opts.get("shared_directories", []),
                },
                timeout=120,
            )
        ip = await self._wait_for_ip(name, lume_url)
        await self._deliver_vnc_config(name, lume_url)
        info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
        await self.is_ready(info)
        return info

    async def list(self) -> list[dict]:
        """List all Lume VMs."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.get(f"{lume_url}/lume/vms")
                vms = resp.json()
                if isinstance(vms, dict):
                    vms = vms.get("vms", [])
        except Exception:
            return []
        result = []
        for vm in vms:
            raw = vm.get("status", "").lower()
            if raw == "running":
                status = "running"
            elif raw in ("stopped", "stop"):
                status = "suspended"
            else:
                status = raw
            result.append(
                {
                    "name": vm.get("name", ""),
                    "status": status,
                    "runtime_type": "lume",
                    "os_type": "macos",
                    "ip_address": vm.get("ip_address") or vm.get("ipAddress"),
                }
            )
        return result

    async def stop(self, name: str) -> None:
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            await client.post(f"{lume_url}/lume/vms/{name}/stop")

    async def delete(self, name: str) -> None:
        """Stop and permanently delete a Lume VM."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=60) as client:
            # Stop first (ignore errors — VM may already be stopped)
            await client.post(f"{lume_url}/lume/vms/{name}/stop")
            await client.delete(f"{lume_url}/lume/vms/{name}")

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Lume VM {info.name} computer-server is ready")
                        return True
                except (
                    httpx.ConnectError,
                    httpx.ReadTimeout,
                    httpx.ConnectTimeout,
                    httpx.ReadError,
                ):
                    pass
                await asyncio.sleep(3)
        raise TimeoutError(f"Lume VM {info.name} computer-server not ready after {timeout}s")
