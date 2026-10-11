"""Lume runtime — macOS (and Linux arm64) VMs via Apple Virtualization.framework.

Starts, suspends, resumes and deletes through the cua SDK's Lume backend
(cua-vmm), which installs/starts ``lume serve`` when needed and does the
pull-once + APFS-clone dance. Checkpoint/fork primitives and the noVNC config
delivery talk to ``lume serve`` directly. The guest daemon is cua-spacesd.
"""

from __future__ import annotations

import asyncio
import logging
import os
import sys
from typing import TYPE_CHECKING, Optional

import httpx
from cua_sandbox.image import Image

if TYPE_CHECKING:
    pass
from cua_sandbox.runtime.base import RuntimeInfo
from cua_sandbox.runtime.images import LUME_API_PORT, LUME_PROVIDER_PORT
from cua_sandbox.runtime.native import NativeRuntime

logger = logging.getLogger(__name__)


def _macos_ref(image: Image) -> str:
    """The Lume image: the registry ref, else the canonical macOS image."""
    from cua_sandbox.image import canonical_image

    return image._registry or canonical_image("macos", image.version or None)


# launchd label of the in-guest daemon installed by libs/lume/scripts/setup-cua.sh.
SPACESD_LAUNCH_AGENT = "com.trycua.spacesd"
# The label images set up before the cua-spacesd rename use.
LEGACY_SPACESD_LAUNCH_AGENT = "com.trycua.env_driver"
# Shell prelude (run in the guest) that sets L to whichever label is
# installed and P to its plist.
_SPACESD_AGENT_SH = (
    f"L={SPACESD_LAUNCH_AGENT}; "
    f'[ -f "$HOME/Library/LaunchAgents/$L.plist" ] || L={LEGACY_SPACESD_LAUNCH_AGENT}; '
    'P="$HOME/Library/LaunchAgents/$L.plist"; '
)


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


def _base_vm_name(oci_ref: str) -> str:
    """Return a stable base-VM name for an OCI image ref (12-char sha256 prefix)."""
    import hashlib

    digest = hashlib.sha256(oci_ref.encode()).hexdigest()[:12]
    return f"cua-base-{digest}"


def _resp_detail(resp: "httpx.Response") -> str:
    try:
        return str(resp.json())
    except Exception:
        return resp.text


def _has_lume() -> bool:
    return _lume_path() is not None


class LumeRuntime(NativeRuntime):
    """Runs macOS VMs via Lume, through the cua SDK."""

    runtime_type = "lume"
    env_ready_timeout = 300.0

    def __init__(
        self,
        *,
        lume_host: str = "localhost",
        lume_port: int = LUME_PROVIDER_PORT,
        api_port: int = LUME_API_PORT,
        ephemeral: bool = True,
        cpus: Optional[int] = None,
        memory_mb: Optional[int] = None,
    ):
        super().__init__(ephemeral=ephemeral, cpus=cpus, memory_mb=memory_mb)
        self.lume_host = lume_host
        self.lume_port = lume_port
        self.api_port = api_port

    @property
    def _lume_url(self) -> str:
        return f"http://{self.lume_host}:{self.lume_port}"

    def _os(self, image: Image) -> str:
        return image.os_type or "macos"

    async def _image_ref(self, image: Image, name: str, **opts) -> str:
        oci_ref = _macos_ref(image)
        return f"lume:{oci_ref}"

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        info = await super().start(image, name, **opts)
        await self._deliver_vnc_config(name, self._lume_url)
        return info

    async def _run_vm(self, name: str, lume_url: str, opts: dict) -> None:
        """POST /lume/vms/{name}/run and raise on failure."""
        async with httpx.AsyncClient(timeout=120) as client:
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

    # ── Checkpoint / fork primitives ─────────────────────────────────────────

    async def ensure_base(self, image: Image, base_name: str):  # -> CheckpointInfo
        """Pull an OCI image into a stopped golden base VM, if it doesn't exist yet.

        Idempotent — if *base_name* already exists and is stopped, returns
        immediately.  Raises if the base VM is running, because cloning a live VM
        can produce dirty or inconsistent clones (same constraint as cloud snapshots).
        The base VM is never started; it only serves as a fork source.
        """
        import time

        from cua_sandbox.runtime.base import CheckpointInfo

        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.get(f"{lume_url}/lume/vms/{base_name}")
        if resp.status_code == 200:
            vm = resp.json()
            if vm.get("status") == "running":
                raise RuntimeError(
                    f"Base VM '{base_name}' is running — stop it before using as a clone source"
                )
            if vm.get("status") in ("stopped", "suspended"):
                logger.info(f"Base VM '{base_name}' already exists (stopped) — skipping pull")
                return CheckpointInfo(name=base_name, runtime_type="lume", created_at=time.time())

        oci_ref = _macos_ref(image)
        logger.info(f"Pulling base image '{oci_ref}' → '{base_name}' (first time only)...")
        pull_payload: dict = {"image": oci_ref, "name": base_name}
        parts = oci_ref.split("/")
        if len(parts) >= 3 and "." in parts[0]:
            pull_payload = {
                "image": "/".join(parts[2:]),
                "name": base_name,
                "registry": parts[0],
                "organization": parts[1],
            }
        await self._pull_vm(base_name, lume_url, pull_payload)
        return CheckpointInfo(name=base_name, runtime_type="lume", created_at=time.time())

    async def fork(self, base_name: str, new_name: str, **opts) -> None:
        """Clone a stopped VM *base_name* → *new_name* via APFS clonefile (instant).

        The source VM must be stopped before calling this — cloning a running VM
        risks dirty state or clone failures (mirrors cloud snapshot behaviour).
        """
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        logger.info(f"Cloning base VM '{base_name}' → '{new_name}'")
        async with httpx.AsyncClient(timeout=60) as client:
            resp = await client.post(
                f"{lume_url}/lume/vms/clone",
                json={"name": base_name, "newName": new_name},
                timeout=60,
            )
        if resp.status_code >= 400:
            try:
                detail = resp.json()
            except Exception:
                detail = resp.text
            raise RuntimeError(f"Lume clone '{base_name}' → '{new_name}' failed: {detail}")
        logger.info(f"Cloned '{base_name}' → '{new_name}' successfully")

    # Prefix used to distinguish checkpoint VMs from ordinary sandboxes
    _CHECKPOINT_PREFIX = "cua-ckpt-"

    async def checkpoint(self, name: str, checkpoint_name: str, **opts):  # -> CheckpointInfo
        """Stop *name*, clone it into a checkpoint, then restart *name*.

        Mirrors the cloud behaviour (stop → snapshot → restart) so clones are
        always taken from a clean stopped state.
        """
        import time

        from cua_sandbox.runtime.base import CheckpointInfo

        lume_url = f"http://{self.lume_host}:{self.lume_port}"

        # Enforce the naming prefix so list/delete_checkpoint stay scoped
        if not checkpoint_name.startswith(
            self._CHECKPOINT_PREFIX
        ) and not checkpoint_name.startswith("cua-base-"):
            checkpoint_name = f"{self._CHECKPOINT_PREFIX}{checkpoint_name}"

        # Determine whether the source VM is running so we can restart it after
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.get(f"{lume_url}/lume/vms/{name}")
            vm = resp.json() if resp.status_code == 200 else {}
        was_running = vm.get("status") == "running"

        if was_running:
            logger.info(f"Stopping '{name}' before checkpoint clone")
            await self.stop(name)

        try:
            await self.fork(name, checkpoint_name)
        finally:
            if was_running:
                logger.info(f"Restarting '{name}' after checkpoint clone")
                await self._run_vm(name, lume_url, opts)

        return CheckpointInfo(
            name=checkpoint_name,
            runtime_type="lume",
            created_at=time.time(),
            metadata={"source": name},
        )

    async def list_checkpoints(self):  # -> list[CheckpointInfo]
        """Return VMs that are checkpoints (cua-base-* or cua-ckpt-* prefix)."""
        import time

        from cua_sandbox.runtime.base import CheckpointInfo

        vms = await self.list()
        return [
            CheckpointInfo(name=v["name"], runtime_type="lume", created_at=time.time())
            for v in vms
            if v["name"].startswith("cua-base-") or v["name"].startswith(self._CHECKPOINT_PREFIX)
        ]

    async def delete_checkpoint(self, checkpoint_name: str) -> None:
        """Delete a checkpoint VM. Only operates on cua-base-* / cua-ckpt-* names."""
        if not checkpoint_name.startswith("cua-base-") and not checkpoint_name.startswith(
            self._CHECKPOINT_PREFIX
        ):
            raise ValueError(
                f"'{checkpoint_name}' does not look like a checkpoint "
                f"(expected 'cua-base-' or '{self._CHECKPOINT_PREFIX}' prefix)"
            )
        await self.delete(checkpoint_name)

    # ── Internal pull helper ──────────────────────────────────────────────────

    async def _pull_vm(self, name: str, lume_url: str, pull_payload: dict) -> None:
        """Pull an OCI image into a stopped VM named *name*.

        Tries /lume/pull/start (async with progress polling) first, falls back
        to the synchronous /lume/pull for older lume versions.
        """
        async with httpx.AsyncClient(timeout=30) as client:
            try:
                start_resp = await self._pull_start_with_retry(lume_url, pull_payload)
            except (httpx.ReadError, httpx.RemoteProtocolError):
                # lume v0.3.x — sync pull
                logger.info("Lume /pull/start unavailable, falling back to synchronous pull...")
                print("\rPulling macOS image...", end="", flush=True, file=sys.stderr)
                try:
                    sync_resp = await client.post(
                        f"{lume_url}/lume/pull", json=pull_payload, timeout=1800
                    )
                    if sync_resp.status_code >= 400:
                        raise RuntimeError(
                            f"Lume pull failed for '{name}': {_resp_detail(sync_resp)}"
                        )
                except (httpx.ReadError, httpx.RemoteProtocolError):
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
                print(file=sys.stderr)
                return

            if start_resp.status_code == 404:
                # Older lume without /pull/start
                try:
                    pull_resp = await client.post(
                        f"{lume_url}/lume/pull", json=pull_payload, timeout=1800
                    )
                except (httpx.ReadError, httpx.RemoteProtocolError):
                    check = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                    if check.status_code != 200 or check.json().get("status") in (
                        "",
                        None,
                        "error",
                    ):
                        raise RuntimeError(
                            f"Lume pull failed for '{name}': connection dropped and VM not found"
                        )
                    return
                if pull_resp.status_code >= 400:
                    raise RuntimeError(f"Lume pull failed for '{name}': {_resp_detail(pull_resp)}")
                return

            if start_resp.status_code >= 400:
                raise RuntimeError(
                    f"Lume pull/start failed for '{name}': {_resp_detail(start_resp)}"
                )

            # Poll until pull completes
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
                    if "error" in status.lower():
                        raise RuntimeError(
                            f"Lume pull failed for '{name}': {data.get('message', status)}"
                        )
                    break
                elif poll.status_code >= 400:
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
                print(file=sys.stderr)

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

    async def _apply_layers(self, info: RuntimeInfo, image: "Image") -> None:
        """Apply image layers (run, brew_install, env, copy, etc.) via cua-spacesd."""
        env_items = getattr(image, "_env", ())
        file_items = getattr(image, "_files", ())
        has_work = image._layers or file_items
        if not has_work and not env_items:
            return
        from cua_sandbox.builder.executor import LayerExecutor

        executor = await LayerExecutor.for_sandbox(
            info.native, os_type=image.os_type, ready_timeout=self.env_ready_timeout
        )
        if env_items and image.os_type != "windows":
            if image.os_type == "macos":
                # macOS: the guest daemon (cua-spacesd) runs under launchd
                # with an explicit EnvironmentVariables dict in its plist.
                # launchctl setenv is ignored by such services, so we must add
                # vars to the plist directly via PlistBuddy, then reload it.
                import re as _re

                for k, v in env_items:
                    # Validate key: must be a safe identifier (alphanumeric + _)
                    if not _re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                        raise ValueError(f"Unsafe env var name: {k!r}")
                    # Escape value for PlistBuddy: single-quote the shell arg,
                    # and escape any single quotes within the value itself.
                    safe_v = v.replace("'", "'\\''")
                    # Use Set if key exists, Add otherwise
                    await executor.run_command(
                        _SPACESD_AGENT_SH
                        + f"/usr/libexec/PlistBuddy -c 'Set :EnvironmentVariables:{k} {safe_v}' \"$P\" 2>/dev/null || "
                        f"/usr/libexec/PlistBuddy -c 'Add :EnvironmentVariables:{k} string {safe_v}' \"$P\""
                    )
                # Reload via lume ssh from the host — bootout from inside the
                # guest would kill the daemon that is executing this command.
                lume_bin = _lume_path() or "lume"
                reload_cmd = (
                    _SPACESD_AGENT_SH + 'launchctl bootout "gui/$(id -u)/$L" 2>/dev/null; '
                    'launchctl bootstrap "gui/$(id -u)" "$P"'
                )
                proc = await asyncio.create_subprocess_exec(
                    lume_bin,
                    "ssh",
                    info.name,
                    "--",
                    reload_cmd,
                    stdout=asyncio.subprocess.DEVNULL,
                    stderr=asyncio.subprocess.DEVNULL,
                )
                await asyncio.wait_for(proc.wait(), timeout=30)
                # The driver restarted: reconnect once it answers again.
                executor = await LayerExecutor.for_sandbox(
                    info.native, os_type=image.os_type, ready_timeout=self.env_ready_timeout
                )
            else:
                import re as _re
                import shlex as _shlex

                sudo = "sudo"
                # Validate all keys up front
                for k, _ in env_items:
                    if not _re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                        raise ValueError(f"Unsafe env var name: {k!r}")
                await executor.run_command(
                    f"printf '#!/bin/sh\\n' | {sudo} tee /etc/profile.d/cua-env.sh > /dev/null"
                )
                for k, v in env_items:
                    quoted_v = _shlex.quote(v)
                    await executor.run_command(
                        f"printf 'export {k}=%s\\n' {quoted_v} "
                        f"| {sudo} tee -a /etc/profile.d/cua-env.sh > /dev/null"
                    )
        for src, dst in file_items:
            await executor.execute_layers([{"type": "copy", "src": src, "dst": dst}])
        if image._layers:
            await executor.execute_layers(list(image._layers))

    async def _deliver_vnc_config(self, name: str, lume_url: str) -> None:
        """Write the current VNC port/password into ~/.vnc.env inside the VM.

        Lume v0.3.x doesn't auto-deliver VNC config via VirtioFS, so the noVNC
        proxy (websockify) inside the VM may use a stale cached port from a
        previous run.  We push the live values via `lume ssh` and restart
        websockify so noVNC always connects to the correct VNC endpoint.
        cua-spacesd captures the display natively and needs no VNC config.
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
            # Write vnc.env and kill websockify so supervisord revives it with
            # the new config.  launchctl/supervisorctl restarts are unreliable
            # from a non-GUI SSH session, so pkill is more reliable.
            proc = await asyncio.create_subprocess_exec(
                _lume_path() or "lume",
                "ssh",
                name,
                "--",
                f"printf '{vnc_env}' > ~/.vnc.env && "
                "pkill -f 'python.*websockify' 2>/dev/null || true",
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
