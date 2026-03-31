"""CloudTransport — connects to a CUA cloud VM via the platform API.

Resolves VM connection info from the API, optionally creates a new VM,
then delegates all computer control to an inner HTTPTransport pointed at
the VM's computer-server endpoint.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any, Dict, Optional

import httpx
from cua_sandbox._config import get_api_key, get_base_url
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.http import HTTPTransport

logger = logging.getLogger(__name__)

_POLL_INTERVAL = 2.0  # seconds between status polls
_POLL_TIMEOUT = 600.0  # max seconds to wait for VM to be running


class CloudTransport(Transport):
    """Transport that provisions / connects to a CUA cloud VM."""

    _DEFAULT_CPU = 1
    _DEFAULT_MEMORY_MB = 4096
    _DEFAULT_DISK_GB = 64

    def __init__(
        self,
        name: Optional[str] = None,
        *,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        # Creation params (used only when creating a new VM)
        image: Optional[Any] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
    ):
        self._name = name
        self._api_key_override = api_key
        self._base_url = base_url or get_base_url()
        self._image = image
        self._cpu = cpu
        self._memory_mb = memory_mb
        self._disk_gb = disk_gb
        self._region = region
        self._inner: Optional[HTTPTransport] = None
        self._api_client: Optional[httpx.AsyncClient] = None

    # ── Connection lifecycle ────────────────────────────────────────────

    async def connect(self) -> None:
        api_key = get_api_key(self._api_key_override)
        if not api_key:
            raise ValueError(
                "No CUA API key found. Cloud sandboxes are the default — to use one, provide an API key via:\n"
                "  1. cua.configure(api_key='sk-...')\n"
                "  2. Set the CUA_API_KEY environment variable\n"
                "  3. Run cua.login() to authenticate via browser\n"
                "  4. Pass api_key='sk-...' directly to sandbox()\n"
                "\n"
                "For local-only usage (no cloud), use sandbox(local=True) instead."
            )

        self._api_client = httpx.AsyncClient(
            base_url=self._base_url,
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=30.0,
        )

        if self._name:
            logger.debug("[cloud] getting VM info for %r", self._name)
            vm_info = await self._get_vm(self._name)
            logger.debug("[cloud] VM info: status=%r", vm_info.get("status"))
        else:
            logger.debug("[cloud] creating new VM")
            vm_info = await self._create_vm()
            self._name = vm_info["name"]
            logger.debug("[cloud] created VM %r", self._name)

        # Poll until running
        logger.debug(
            "[cloud] waiting for VM %r to be running (status=%r)", self._name, vm_info.get("status")
        )
        vm_info = await self._wait_for_running(vm_info)
        logger.debug("[cloud] VM %r is running", self._name)

        # Resolve computer-server endpoint — auth with CUA API key + container name.
        # In local-dev mode the API initially returns .cua.sh placeholder URLs and
        # replaces them with direct IPs once the container IP is known, so we poll
        # until we get a non-.cua.sh address.  On prod the reverse-proxy .cua.sh URL
        # is the real endpoint, so we skip this loop entirely.
        cs_url = self._resolve_endpoint(vm_info)
        _is_local_dev = not self._base_url.rstrip("/").endswith("cua.sh") and (
            "localhost" in self._base_url
            or "127.0.0.1" in self._base_url
            or "0.0.0.0" in self._base_url
        )
        poll_elapsed = 0.0
        while _is_local_dev and (cs_url.endswith(".cua.sh") or ".cua.sh:" in cs_url):
            if poll_elapsed >= 120:
                break  # Fall through — _wait_for_server_ready will handle the error
            await asyncio.sleep(3)
            poll_elapsed += 3
            vm_info = await self._get_vm(self._name)
            cs_url = self._resolve_endpoint(vm_info)
            logger.debug("[cloud] re-resolving endpoint: %s (%.0fs)", cs_url, poll_elapsed)

        logger.debug("[cloud] resolved endpoint: %s", cs_url)
        self._inner = HTTPTransport(cs_url, api_key=api_key, container_name=self._name)
        logger.debug("[cloud] connecting inner HTTPTransport")
        await self._inner.connect()
        logger.debug("[cloud] HTTPTransport connected")

        # Wait for computer-server to be reachable (it may lag behind VM "running" status)
        logger.debug("[cloud] waiting for computer-server to be ready")
        await self._wait_for_server_ready()
        logger.debug("[cloud] computer-server ready")

        # Apply env vars and image layers (e.g. APK installs) after server is ready
        if self._image and (self._image._layers or self._image._env):
            logger.debug("[cloud] applying image layers")
            await self._apply_image_layers()

    async def disconnect(self) -> None:
        if self._inner:
            await self._inner.disconnect()
            self._inner = None
        if self._api_client:
            await self._api_client.aclose()
            self._api_client = None

    async def create_snapshot(self, name: str | None = None, stateful: bool = False) -> dict:
        """Create a snapshot of this VM. Returns an image descriptor dict."""
        assert self._api_client and self._name
        resp = await self._api_client.post(
            f"/v1/vms/{self._name}/snapshot",
            json={"name": name or "", "stateful": stateful},
            timeout=600.0,  # snapshot can take minutes on dir storage
        )
        resp.raise_for_status()
        data = resp.json()
        # Poll until snapshot is ready
        image_desc = data.get("image", data)
        snap_name = image_desc.get("snapshot", "")
        if snap_name:
            await self._wait_for_snapshot_ready(snap_name)
        return image_desc

    async def _wait_for_snapshot_ready(self, snapshot_name: str, timeout: float = 120) -> None:
        """Wait for snapshot to be ready.

        The API's snapshot endpoint is now synchronous — it blocks until the
        Kopf operator finishes the snapshot.  This method is kept as a no-op
        for compatibility.
        """
        return

    async def delete_vm(self) -> None:
        """Delete the cloud VM via the platform API."""
        api_key = get_api_key(self._api_key_override)
        if not api_key or not self._name:
            return
        async with httpx.AsyncClient(
            base_url=self._base_url,
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=30.0,
        ) as client:
            await client.delete(f"/v1/vms/{self._name}")

    async def suspend_vm(self) -> None:
        """Stop (suspend) the cloud VM."""
        if not self._name:
            return
        assert self._api_client
        await self._api_client.post(f"/v1/vms/{self._name}/stop")

    async def resume_vm(self) -> None:
        """Start (resume) the cloud VM."""
        if not self._name:
            return
        assert self._api_client
        await self._api_client.post(f"/v1/vms/{self._name}/run")

    async def restart_vm(self) -> None:
        """Restart the cloud VM."""
        if not self._name:
            return
        assert self._api_client
        await self._api_client.post(f"/v1/vms/{self._name}/restart")

    # ── Delegated methods ───────────────────────────────────────────────

    async def send(self, action: str, **params: Any) -> Any:
        assert self._inner, "Transport not connected"
        # Source .cua_env before run_command on Android (same as ADB/gRPC transports)
        if (
            action == "run_command"
            and "command" in params
            and self._image
            and self._image.os_type == "android"
        ):
            params = dict(params)
            params["command"] = (
                "[ -f /data/local/tmp/.cua_env ] && . /data/local/tmp/.cua_env; "
                + params["command"]
            )
        return await self._inner.send(action, **params)

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._inner, "Transport not connected"
        return await self._inner.screenshot(format=format, quality=quality)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._inner, "Transport not connected"
        return await self._inner.get_screen_size()

    async def get_environment(self) -> str:
        assert self._inner, "Transport not connected"
        return await self._inner.get_environment()

    async def get_display_url(self, *, share: bool = False) -> str:
        if not self._name:
            raise ValueError("Transport not connected — no VM name available")
        if not share:
            return f"https://cua.ai/connect/{self._name}"
        vm_info = await self._get_vm(self._name)
        password = vm_info.get("password", "")
        for ep in vm_info.get("endpoints", []):
            if ep.get("name") == "vnc":
                host = ep["host"]
                url = f"https://{host}"
                if password:
                    url += f"/?password={password}"
                return url
        raise ValueError(
            f"VM '{self._name}' has no VNC endpoint. "
            "Only Android and desktop VMs expose a VNC endpoint."
        )

    # ── Helpers ─────────────────────────────────────────────────────────

    @property
    def name(self) -> Optional[str]:
        return self._name

    async def _get_vm(self, name: str) -> dict:
        assert self._api_client
        resp = await self._api_client.get(f"/v1/vms/{name}")
        resp.raise_for_status()
        return resp.json()

    async def _create_vm(self) -> dict:
        assert self._api_client
        if not self._image:
            raise ValueError(
                "Cannot create a cloud VM without an image. Use:\n"
                "  Sandbox.create(image=Image.linux())  or  Sandbox.create(image=Image.windows())  or  Sandbox.create(image=Image.macos())\n"
                "Or connect to an existing VM by name: Sandbox.connect(name='my-vm')"
            )

        # Fork path: image came from sb.snapshot() — create VM from snapshot
        snap_source = getattr(self._image, "_snapshot_source", None)
        if snap_source:
            body = {
                "source": "snapshot",
                "instance": snap_source["instance"],
                "snapshot": snap_source["snapshot"],
                "instanceType": snap_source.get("instanceType", "vm"),
            }
            resp = await self._api_client.post("/v1/vms", json=body)
            resp.raise_for_status()
            return resp.json()

        os_type = getattr(self._image, "os_type", None)
        if not os_type:
            raise ValueError(
                "Image must have an os_type. Use Image.linux(), Image.windows(), or Image.macos()."
            )
        body: Dict[str, Any] = {
            "os": os_type,
            "region": self._region,
        }
        # If any resource spec is provided, send explicit specs (defaulting missing to small).
        # Otherwise, send configuration="small" for backwards compat with legacy API.
        if any(v is not None for v in (self._cpu, self._memory_mb, self._disk_gb)):
            body["cpu"] = self._cpu or self._DEFAULT_CPU
            body["memoryMb"] = self._memory_mb or self._DEFAULT_MEMORY_MB
            body["diskGb"] = self._disk_gb or self._DEFAULT_DISK_GB
        else:
            body["configuration"] = "small"
        resp = await self._api_client.post("/v1/vms", json=body)
        resp.raise_for_status()
        return resp.json()

    async def _wait_for_running(self, vm_info: dict) -> dict:
        """Poll until the VM status is 'running' (or 'ready')."""
        elapsed = 0.0
        while vm_info.get("status") not in ("running", "ready"):
            if elapsed >= _POLL_TIMEOUT:
                raise TimeoutError(
                    f"VM {self._name!r} did not become running within {_POLL_TIMEOUT}s "
                    f"(last status: {vm_info.get('status')})"
                )
            await asyncio.sleep(_POLL_INTERVAL)
            elapsed += _POLL_INTERVAL
            vm_info = await self._get_vm(self._name)  # type: ignore[arg-type]
            logger.debug(
                "[cloud] _wait_for_running: elapsed=%.0fs status=%r", elapsed, vm_info.get("status")
            )
        return vm_info

    async def _wait_for_server_ready(self) -> None:
        """Poll the computer-server until it responds (retries on connection/HTTP errors)."""
        assert self._inner
        elapsed = 0.0
        last_err: Optional[Exception] = None
        while elapsed < _POLL_TIMEOUT:
            try:
                await self._inner.get_screen_size()
                return  # Server is ready
            except httpx.HTTPStatusError as e:
                if e.response.status_code < 500 and e.response.status_code != 404:
                    raise  # 4xx errors (except 404) are not transient — fail fast
                last_err = e
                logger.debug("[cloud] _wait_for_server_ready: elapsed=%.0fs err=%r", elapsed, e)
                await asyncio.sleep(_POLL_INTERVAL)
                elapsed += _POLL_INTERVAL
            except Exception as e:
                last_err = e
                logger.debug("[cloud] _wait_for_server_ready: elapsed=%.0fs err=%r", elapsed, e)
                await asyncio.sleep(_POLL_INTERVAL)
                elapsed += _POLL_INTERVAL
        raise TimeoutError(
            f"Computer-server for VM {self._name!r} not reachable within {_POLL_TIMEOUT}s: {last_err}"
        )

    async def _apply_image_layers(self) -> None:
        """Apply env vars and image layers (APK installs, PWA, shell commands) after the VM is ready."""
        import base64

        assert self._inner

        # Apply environment variables by writing a .cua_env sourced file
        if self._image._env:
            import shlex

            lines = []
            for k, v in self._image._env:
                lines.append(f"export {k}={shlex.quote(v)}")
            env_content = "\n".join(lines) + "\n"
            await self._inner.send(
                "write_bytes",
                path="/data/local/tmp/.cua_env",
                content_b64=base64.b64encode(env_content.encode()).decode(),
            )
            logger.debug("[cloud] wrote %d env vars to .cua_env", len(self._image._env))

        for layer in self._image._layers:
            lt = layer["type"]
            if lt == "apk_install":
                for apk in layer["packages"]:
                    await self._install_apk(apk)
            elif lt == "pwa_install":
                await self._install_pwa(layer)
            elif lt == "run":
                await self._inner.send("run_command", command=layer["command"], timeout=60)

    async def _install_apk(self, apk: str) -> None:
        """Download (if URL) and install an APK via the computer-server."""
        import base64
        import hashlib
        import urllib.request
        from pathlib import Path

        dest = "/data/local/tmp/cua_install.apk"
        if apk.startswith(("http://", "https://")):
            cache_dir = Path.home() / ".cua" / "cua-sandbox" / "apk-cache"
            cache_dir.mkdir(parents=True, exist_ok=True)
            cache_file = cache_dir / (hashlib.sha256(apk.encode()).hexdigest()[:16] + ".apk")
            if not cache_file.exists():
                urllib.request.urlretrieve(apk, cache_file)
            apk_bytes = cache_file.read_bytes()
        else:
            apk_bytes = Path(apk).read_bytes()

        await self._inner.send(
            "write_bytes",
            path=dest,
            content_b64=base64.b64encode(apk_bytes).decode(),
        )
        await self._inner.send(
            "run_command",
            command=(
                f"out=$(pm install -r {dest} 2>&1); "
                f'if echo "$out" | grep -q INSTALL_FAILED_UPDATE_INCOMPATIBLE; then '
                f'  pkg=$(echo "$out" | sed -n "s/.*Package \\(\\S*\\) signatures.*/\\1/p"); '
                f'  pm uninstall "$pkg"; pm install -r {dest}; '
                f'else echo "$out"; fi; true'
            ),
            timeout=90,
        )

    async def _install_pwa(self, layer: dict) -> None:
        """Build PWA APK on the host, then push and install.

        Supports two builders:
        - "pwa2apk" (default): WebView-based APK, no Chrome dependency or banners.
        - "bubblewrap": Chrome TWA APK, requires asset links and shows Chrome disclosure.
        """
        import base64
        from pathlib import Path

        builder = layer.get("builder", "pwa2apk")
        manifest_url = layer["manifest_url"]
        pkg = layer.get("package_name")
        ks = Path(layer["keystore"]) if layer.get("keystore") else None
        ks_alias = layer.get("keystore_alias", "android")
        ks_pass = layer.get("keystore_password", "android")

        if builder == "pwa2apk":
            apk_path, fingerprint = await self._build_pwa2apk(
                manifest_url, pkg, ks, ks_alias, ks_pass
            )
        else:
            from cua_sandbox.runtime.android_emulator import (
                AndroidEmulatorRuntime,
                _ensure_sdk,
            )

            _ensure_sdk()
            runtime = AndroidEmulatorRuntime.__new__(AndroidEmulatorRuntime)
            apk_path, fingerprint = await runtime._build_pwa_apk(
                manifest_url, pkg, ks, ks_alias, ks_pass
            )

        logger.info(f"[cloud] PWA APK built ({builder}): {apk_path} (fingerprint: {fingerprint})")

        apk_bytes = Path(apk_path).read_bytes()
        dest = "/data/local/tmp/cua_pwa.apk"
        await self._inner.send(
            "write_bytes",
            path=dest,
            content_b64=base64.b64encode(apk_bytes).decode(),
        )
        await self._inner.send(
            "run_command",
            command=f"pm install -r {dest} 2>&1; true",
            timeout=120,
        )

        # For bubblewrap TWA, suppress Chrome first-run and set asset link bypass.
        # For pwa2apk WebView, none of this is needed.
        if builder == "bubblewrap":
            from urllib.parse import urlparse

            origin = f"{urlparse(manifest_url).scheme}://{urlparse(manifest_url).netloc}"
            for cmd in [
                "am set-debug-app --persistent com.android.chrome",
                "mkdir -p /data/local/tmp && "
                "echo 'chrome --no-first-run --disable-fre --no-default-browser-check "
                f'--disable-digital-asset-link-verification-for-url="{origin}"\' '
                "> /data/local/tmp/chrome-command-line",
            ]:
                await self._inner.send("run_command", command=cmd, timeout=10)

    @staticmethod
    async def _build_pwa2apk(
        manifest_url: str,
        package_name: str | None = None,
        keystore_path: str | None = None,
        keystore_alias: str = "android",
        keystore_password: str = "android",
    ) -> tuple:
        """Build a WebView-based APK using pwa2apk (no Chrome dependency)."""
        import shutil
        import subprocess

        node = shutil.which("node")
        if not node:
            raise RuntimeError("node not found on PATH; required for pwa2apk")

        # Find pwa2apk — check common locations
        pwa2apk_cli = None
        for candidate in [
            shutil.which("pwa2apk"),
            # npm global
            *([] if not shutil.which("npm") else []),
        ]:
            if candidate:
                pwa2apk_cli = candidate
                break

        # Fall back to requiring it as a node module
        if not pwa2apk_cli:
            # Try npx
            npx = shutil.which("npx")
            if npx:
                pwa2apk_cli = npx

        # Build via the pwa2apk Node API directly
        import hashlib
        from pathlib import Path

        # Check if pwa2apk is installed globally or locally
        pwa2apk_dir = None
        for p in [
            Path.home() / ".cua" / "pwa2apk",
            Path("/tmp/pwa2apk"),
        ]:
            if (p / "src" / "index.js").exists():
                pwa2apk_dir = p
                break

        if not pwa2apk_dir:
            # Auto-clone pwa2apk
            logger.info("Cloning pwa2apk...")
            pwa2apk_dir = Path.home() / ".cua" / "pwa2apk"
            pwa2apk_dir.mkdir(parents=True, exist_ok=True)
            clone_result = subprocess.run(
                ["git", "clone", "https://github.com/trycua/pwa2apk.git", str(pwa2apk_dir)],
                capture_output=True,
                text=True,
                timeout=60,
            )
            if clone_result.returncode != 0:
                raise RuntimeError(f"Failed to clone pwa2apk: {clone_result.stderr}")

        # Build the args for the CLI
        import os

        cache_key = hashlib.sha256(f"{manifest_url}|{package_name or ''}".encode()).hexdigest()[:12]
        output_apk = Path.home() / ".cua" / "pwa2apk-cache" / f"{cache_key}.apk"
        output_apk.parent.mkdir(parents=True, exist_ok=True)

        cmd = [
            node,
            str(pwa2apk_dir / "src" / "cli.js"),
            manifest_url,
            "--output",
            str(output_apk),
        ]
        if package_name:
            cmd.extend(["--package", package_name])
        if keystore_path:
            cmd.extend(["--keystore", str(keystore_path)])
            cmd.extend(["--keystore-alias", keystore_alias])
            cmd.extend(["--keystore-password", keystore_password])

        env = {**os.environ}
        if "JAVA_HOME" not in env:
            for jdk in [
                # Linux
                "/usr/lib/jvm/java-17-openjdk-amd64",
                "/usr/lib/jvm/java-21-openjdk-amd64",
                # macOS (Homebrew ARM) — prefer @17/@21 over unversioned (may be JDK 25+)
                "/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home",
                "/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home",
                "/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home",
                # macOS (Homebrew Intel)
                "/usr/local/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home",
                "/usr/local/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home",
                "/usr/local/opt/openjdk/libexec/openjdk.jdk/Contents/Home",
            ]:
                if Path(jdk).exists():
                    env["JAVA_HOME"] = jdk
                    break

        logger.info(f"Building APK with pwa2apk: {manifest_url}")
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=300,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"pwa2apk build failed:\nstdout: {result.stdout}\nstderr: {result.stderr}"
            )

        # Extract fingerprint from output
        fingerprint = ""
        for line in result.stdout.splitlines():
            if "SHA-256:" in line:
                fingerprint = line.split("SHA-256:", 1)[1].strip()
                break

        if not output_apk.exists():
            raise RuntimeError(f"pwa2apk did not produce APK at {output_apk}")

        return output_apk, fingerprint

    @staticmethod
    def _resolve_endpoint(vm_info: dict) -> str:
        """Build the computer-server HTTP URL from VM info."""
        # Prefer explicit endpoints array
        for ep in vm_info.get("endpoints", []):
            if ep.get("name") in ("computer-server", "api"):
                host = ep["host"]
                # cua.sh hosts are behind a reverse proxy — don't append port
                if host.endswith(".cua.sh"):
                    return f"https://{host}"
                return f"http://{host}:{ep['port']}"

        # Fallback: legacy host-based URL
        host = vm_info.get("host")
        if not host:
            raise ValueError(f"Cannot resolve computer-server endpoint from VM info: {vm_info}")
        return f"http://{host}:8000"


async def cloud_list_vms(
    *, api_key: Optional[str] = None, base_url: Optional[str] = None
) -> list[dict]:
    """List all cloud VMs. Returns raw VM dicts from the API."""
    from cua_sandbox._config import get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError("No CUA API key. Set CUA_API_KEY or run cua.login().")
    url = base_url or get_base_url()
    async with httpx.AsyncClient(
        base_url=url,
        headers={"Authorization": f"Bearer {key}"},
        timeout=30.0,
    ) as client:
        resp = await client.get("/v1/vms")
        resp.raise_for_status()
        data = resp.json()
        return data if isinstance(data, list) else data.get("vms", [])


async def cloud_get_vm(
    name: str, *, api_key: Optional[str] = None, base_url: Optional[str] = None
) -> dict:
    """Get info for a single cloud VM by name."""
    from cua_sandbox._config import get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError("No CUA API key. Set CUA_API_KEY or run cua.login().")
    url = base_url or get_base_url()
    async with httpx.AsyncClient(
        base_url=url,
        headers={"Authorization": f"Bearer {key}"},
        timeout=30.0,
    ) as client:
        resp = await client.get(f"/v1/vms/{name}")
        resp.raise_for_status()
        return resp.json()


async def cloud_vm_action(
    name: str,
    action: str,
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> None:
    """POST /v1/vms/{name}/{action}. action is 'stop', 'run', 'restart', or 'delete'."""
    from cua_sandbox._config import get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError("No CUA API key. Set CUA_API_KEY or run cua.login().")
    url = base_url or get_base_url()
    async with httpx.AsyncClient(
        base_url=url,
        headers={"Authorization": f"Bearer {key}"},
        timeout=30.0,
    ) as client:
        if action == "delete":
            await client.delete(f"/v1/vms/{name}")
        else:
            await client.post(f"/v1/vms/{name}/{action}")
