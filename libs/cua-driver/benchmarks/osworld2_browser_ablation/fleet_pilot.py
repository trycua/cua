#!/usr/bin/env python3
"""Provision one disposable Fleet VM for the OSWorld 2 browser-use pilot.

The process owns the complete lifecycle.  It creates one namespace, one
replica, and one claim; exposes authenticated loopback bridges to the guest
services; optionally prepares task 082 and builds Cua Driver at an exact Cua
commit; then waits until interrupted.  Its ``finally`` block deletes the
claim, pool, and namespace and verifies their absence.

Secrets are loaded in memory from the scoped AWS Secrets Manager entries named
by ``.work/local.json``.  They are never written to state or logs.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import contextlib
import hashlib
import json
import logging
import os
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import aiohttp
from aiohttp import web
import httpx
import requests


ROOT = Path(__file__).resolve().parent
WORK_DIR = Path(
    os.environ.get("CUA_OSWORLD2_WORK_DIR", ROOT / ".work")
).expanduser().resolve()
DEFAULT_CONFIG = WORK_DIR / "local.json"
OSWORLD_DIR = WORK_DIR / "OSWorld-V2"
RESULTS_DIR = WORK_DIR / "results"

DEFAULT_FLEET_BASE_URL = "https://run.cua.ai"
DEFAULT_AWS_REGION = "us-west-2"
POOL_READY_TIMEOUT_SECONDS = int(
    os.environ.get("CUA_OSWORLD2_POOL_READY_TIMEOUT_SECONDS", "1800")
)
LOCAL_ASSET_CHUNK_BYTES = 48 * 1024

POOL_GROUP = "cua.ai"
POOL_VERSION = "v1"
POOL_PLURAL = "osgymworkspacepools"
EXT_GROUP = "osgym.cua.ai"
EXT_VERSION = "v1alpha1"
TEMPLATE_PLURAL = "osgymsandboxtemplates"
CLAIM_PLURAL = "osgymsandboxclaims"

CONTROL_PORT = 5000
CDP_PORT = 1337
GUEST_CDP_PORT = 1337
NOVNC_PORT = 8006
VLC_PORT = 8080
DEFAULT_AWS_MOCK_PORT = 31082
GUEST_DRIVER_SOCKET = "/home/user/.cache/cua-driver/osworld2-pilot.sock"
GUEST_DRIVER_SDK_ROOT = "/home/user/.cache/cua-driver-sdk-0.12.6"
GUEST_DRIVER_SDK_PACKAGE = f"{GUEST_DRIVER_SDK_ROOT}/cua_driver"
GUEST_DRIVER_SDK_CALLER = f"{GUEST_DRIVER_SDK_ROOT}/driver_sdk_call.py"
STOP_REQUESTED: threading.Event | None = None

GUEST_HTTP_CODE = (
    "import base64,json,sys,urllib.error,urllib.request;"
    "s=json.loads(base64.b64decode(sys.argv[1]));"
    "b=base64.b64decode(s['body']) if s.get('body') else None;"
    "q=urllib.request.Request(s['url'],data=b,headers=s['headers'],"
    "method=s['method']);"
    "\ntry:\n"
    " r=urllib.request.urlopen(q,timeout=s['timeout']);"
    " status=r.status; h=dict(r.headers); data=r.read()\n"
    "except urllib.error.HTTPError as e:\n"
    " status=e.code; h=dict(e.headers); data=e.read()\n"
    "except urllib.error.URLError as e:\n"
    " print(json.dumps({'transport_error':str(e.reason)})); sys.exit(0)\n"
    "except OSError as e:\n"
    " print(json.dumps({'transport_error':str(e)})); sys.exit(0)\n"
    "print(json.dumps({'status':status,'headers':h,"
    "'body':base64.b64encode(data).decode('ascii')}))"
)
GUEST_DETACHED_LAUNCH_CODE = (
    "import base64,json,subprocess,sys;"
    "argv=json.loads(base64.b64decode(sys.argv[1]));"
    "log=open(sys.argv[2],'ab',buffering=0);"
    "subprocess.Popen(argv,stdin=subprocess.DEVNULL,stdout=log,stderr=log,"
    "start_new_session=True,close_fds=True,cwd='/home/user')"
)


class PilotError(RuntimeError):
    """A bounded, sanitized pilot failure."""


def emit(event: str, **fields: Any) -> None:
    safe = {"event": event, **fields}
    print(json.dumps(safe, sort_keys=True), flush=True)


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise PilotError(f"{path.name} must contain an object")
    return value


def work_relative_path(path: Path, work_dir: Path = WORK_DIR) -> str:
    """Return a stable artifact path without assuming work lives under source."""
    return str(path.resolve().relative_to(work_dir.resolve()))


def isolated_chrome_command(
    command: list[str],
    guest_chrome_profile: str,
) -> list[str]:
    """Apply deterministic first-run suppression to one disposable profile."""

    required = (
        f"--user-data-dir={guest_chrome_profile}",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-search-engine-choice-screen",
    )
    return [
        command[0],
        *(flag for flag in required if flag not in command),
        *command[1:],
    ]


def load_aws_secret(name: str, region: str) -> dict[str, Any]:
    result: subprocess.CompletedProcess[str] | None = None
    for attempt in range(3):
        result = subprocess.run(
            [
                "aws",
                "secretsmanager",
                "get-secret-value",
                "--region",
                region,
                "--secret-id",
                name,
                "--query",
                "SecretString",
                "--output",
                "text",
            ],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if result.returncode == 0:
            break
        if attempt < 2:
            time.sleep(2**attempt)
    assert result is not None
    if result.returncode:
        raise PilotError("unable to load the configured Fleet credential")
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise PilotError("configured Fleet credential is not JSON") from exc
    if not isinstance(value, dict):
        raise PilotError("configured Fleet credential must be a JSON object")
    return value


def pool_url(namespace: str, name: str | None = None) -> str:
    base = (
        f"/api/k8s/apis/{POOL_GROUP}/{POOL_VERSION}/namespaces/"
        f"{namespace}/{POOL_PLURAL}"
    )
    return f"{base}/{name}" if name else base


def template_url(namespace: str, name: str) -> str:
    return (
        f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/"
        f"{namespace}/{TEMPLATE_PLURAL}/{name}"
    )


def claim_url(namespace: str, name: str | None = None) -> str:
    base = (
        f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/"
        f"{namespace}/{CLAIM_PLURAL}"
    )
    return f"{base}/{name}" if name else base


def require_loopback_port(port: int) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            probe.bind(("127.0.0.1", port))
        except OSError as exc:
            raise PilotError(f"required loopback port {port} is already in use") from exc


@dataclass(frozen=True)
class Service:
    name: str
    guest_port: int
    local_port: int


class TokenSource:
    def __init__(self, token_url: str, client_id: str, client_secret: str) -> None:
        self.token_url = token_url
        self.client_id = client_id
        self.client_secret = client_secret
        self._token = ""
        self._refresh_at = 0.0
        self._lock = asyncio.Lock()

    async def get(self, session: aiohttp.ClientSession) -> str:
        async with self._lock:
            if self._token and time.monotonic() < self._refresh_at:
                return self._token
            async with session.post(
                self.token_url,
                data={
                    "grant_type": "client_credentials",
                    "client_id": self.client_id,
                    "client_secret": self.client_secret,
                },
                timeout=aiohttp.ClientTimeout(total=30),
            ) as response:
                if response.status >= 400:
                    raise PilotError("Fleet token refresh failed")
                payload = await response.json()
            token = payload.get("access_token")
            if not isinstance(token, str) or not token:
                raise PilotError("Fleet token response omitted access_token")
            expires_in = int(payload.get("expires_in", 300))
            self._token = token
            self._refresh_at = time.monotonic() + max(30, expires_in - 45)
            return token


class FleetBridge:
    """Thread-owned aiohttp reverse proxies with Fleet bearer injection."""

    def __init__(
        self,
        *,
        base_url: str,
        namespace: str,
        sandbox: str,
        services: tuple[Service, ...],
        token_source: TokenSource,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.namespace = namespace
        self.sandbox = sandbox
        self.services = services
        self.token_source = token_source
        self._thread: threading.Thread | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._ready = threading.Event()
        self._failure: BaseException | None = None
        self._stop_event: asyncio.Event | None = None

    def start(self) -> None:
        for service in self.services:
            require_loopback_port(service.local_port)
        self._thread = threading.Thread(
            target=self._thread_main,
            name="fleet-loopback-bridge",
            daemon=True,
        )
        self._thread.start()
        if not self._ready.wait(timeout=30):
            raise PilotError("timed out starting Fleet loopback bridges")
        if self._failure:
            raise PilotError("unable to start Fleet loopback bridges") from self._failure

    def stop(self) -> None:
        if self._loop and self._stop_event:
            self._loop.call_soon_threadsafe(self._stop_event.set)
        if self._thread:
            self._thread.join(timeout=30)

    def _thread_main(self) -> None:
        try:
            asyncio.run(self._serve())
        except BaseException as exc:
            self._failure = exc
            self._ready.set()

    async def _serve(self) -> None:
        self._loop = asyncio.get_running_loop()
        self._stop_event = asyncio.Event()
        runners: list[web.AppRunner] = []
        timeout = aiohttp.ClientTimeout(total=None, connect=30, sock_read=None)
        async with aiohttp.ClientSession(timeout=timeout) as client:
            for service in self.services:
                app = web.Application(client_max_size=1024**3)
                app["client"] = client
                app["service"] = service
                app.router.add_route("*", "/{tail:.*}", self._handle)
                runner = web.AppRunner(app, access_log=None)
                await runner.setup()
                site = web.TCPSite(runner, "127.0.0.1", service.local_port)
                await site.start()
                runners.append(runner)
            self._ready.set()
            await self._stop_event.wait()
            for runner in reversed(runners):
                await runner.cleanup()

    def _remote_url(self, service: Service, raw_path: str) -> str:
        prefix = (
            f"/api/svc/{self.namespace}/"
            f"{self.sandbox}-{service.name}"
        )
        suffix = raw_path if raw_path.startswith("/") else f"/{raw_path}"
        return f"{self.base_url}{prefix}{suffix}"

    async def _handle(self, request: web.Request) -> web.StreamResponse:
        service: Service = request.app["service"]
        client: aiohttp.ClientSession = request.app["client"]
        token = await self.token_source.get(client)
        remote = self._remote_url(service, request.raw_path)
        if request.headers.get("Upgrade", "").lower() == "websocket":
            return await self._handle_websocket(request, client, remote, token)
        return await self._handle_http(request, client, remote, token)

    async def _handle_http(
        self,
        request: web.Request,
        client: aiohttp.ClientSession,
        remote: str,
        token: str,
    ) -> web.Response:
        ignored = {
            "authorization",
            "connection",
            "content-length",
            "host",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
        }
        headers = {
            key: value
            for key, value in request.headers.items()
            if key.lower() not in ignored
        }
        headers["Authorization"] = f"Bearer {token}"
        body = await request.read()
        async with client.request(
            request.method,
            remote,
            headers=headers,
            data=body,
            allow_redirects=False,
        ) as response:
            response_body = await response.read()
            response_headers = {
                key: value
                for key, value in response.headers.items()
                if key.lower()
                not in {
                    "connection",
                    "content-length",
                    "keep-alive",
                    "transfer-encoding",
                }
            }
            return web.Response(
                status=response.status,
                headers=response_headers,
                body=response_body,
            )

    async def _handle_websocket(
        self,
        request: web.Request,
        client: aiohttp.ClientSession,
        remote: str,
        token: str,
    ) -> web.WebSocketResponse:
        requested_protocols = [
            value.strip()
            for value in request.headers.get("Sec-WebSocket-Protocol", "").split(",")
            if value.strip()
        ]
        downstream = web.WebSocketResponse(protocols=requested_protocols)
        await downstream.prepare(request)
        async with client.ws_connect(
            remote,
            headers={"Authorization": f"Bearer {token}"},
            protocols=requested_protocols,
            max_msg_size=0,
        ) as upstream:

            async def downstream_to_upstream() -> None:
                async for message in downstream:
                    if message.type == aiohttp.WSMsgType.TEXT:
                        await upstream.send_str(message.data)
                    elif message.type == aiohttp.WSMsgType.BINARY:
                        await upstream.send_bytes(message.data)
                    elif message.type == aiohttp.WSMsgType.CLOSE:
                        await upstream.close()
                        break

            async def upstream_to_downstream() -> None:
                async for message in upstream:
                    if message.type == aiohttp.WSMsgType.TEXT:
                        await downstream.send_str(message.data)
                    elif message.type == aiohttp.WSMsgType.BINARY:
                        await downstream.send_bytes(message.data)
                    elif message.type == aiohttp.WSMsgType.CLOSE:
                        await downstream.close()
                        break

            tasks = [
                asyncio.create_task(downstream_to_upstream()),
                asyncio.create_task(upstream_to_downstream()),
            ]
            done, pending = await asyncio.wait(
                tasks, return_when=asyncio.FIRST_COMPLETED
            )
            for task in pending:
                task.cancel()
            for task in done:
                with contextlib.suppress(asyncio.CancelledError):
                    await task
        return downstream


def create_namespace(http: httpx.Client, namespace: str) -> None:
    response = http.post("/api/namespaces", json={"name": namespace})
    if response.status_code not in (200, 201, 202):
        raise PilotError(f"namespace create failed with HTTP {response.status_code}")


def create_pool(
    http: httpx.Client,
    *,
    namespace: str,
    image: str,
    services: tuple[Service, ...],
) -> None:
    body = {
        "apiVersion": f"{POOL_GROUP}/{POOL_VERSION}",
        "kind": "OSGymWorkspacePool",
        "metadata": {
            "name": namespace,
            "labels": {
                "cua.ai/pool": namespace,
                "cua.ai/purpose": "osworld2-browser-pilot",
            },
        },
        "spec": {
            "replicas": 1,
            "template": {
                "containerDiskImage": image,
                "imagePullSecret": "ecr-credentials",
                "cpuCores": 4,
                "memory": "8Gi",
                "probes": {
                    "readinessProbe": {"tcpSocket": {"port": CONTROL_PORT}}
                },
            },
            "services": [
                {
                    "name": service.name,
                    "targetPort": service.guest_port,
                    "protocol": "TCP",
                }
                for service in services
            ],
        },
    }
    response = http.post(pool_url(namespace), json=body)
    if response.status_code not in (200, 201, 202):
        raise PilotError(f"pool create failed with HTTP {response.status_code}")


def wait_for(
    *,
    description: str,
    timeout: float,
    poll: float,
    probe: Callable[[], Any],
    ready: Callable[[Any], bool],
) -> Any:
    deadline = time.monotonic() + timeout
    last_progress = 0.0
    while True:
        if STOP_REQUESTED is not None and STOP_REQUESTED.is_set():
            raise PilotError(f"stop requested while waiting for {description}")
        value = probe()
        if ready(value):
            return value
        now = time.monotonic()
        if now >= deadline:
            raise PilotError(f"timed out waiting for {description}")
        if now - last_progress >= 30:
            emit("waiting", resource=description)
            last_progress = now
        if STOP_REQUESTED is not None:
            STOP_REQUESTED.wait(timeout=poll)
        else:
            time.sleep(poll)


def wait_template(http: Callable[[], httpx.Client], namespace: str) -> bool:
    """Best-effort projection check.

    Some scoped Fleet credentials can create pools and claims but receive 403
    when reading the projected template.  The pool's own status is the
    authoritative readiness gate, so an invisible template must not abort a
    run that can still warm and bind normally.
    """
    try:
        wait_for(
            description="pool template visibility",
            timeout=60,
            poll=5,
            probe=lambda: http().get(
                template_url(namespace, f"{namespace}-template")
            ),
            ready=lambda response: response.status_code == 200,
        )
    except PilotError:
        emit("template_visibility_unavailable", continuing=True)
        return False
    emit("template_visible")
    return True


def wait_pool(http: Callable[[], httpx.Client], namespace: str) -> dict[str, Any]:
    def probe() -> dict[str, Any]:
        response = http().get(pool_url(namespace, namespace))
        response.raise_for_status()
        return response.json().get("status") or {}

    return wait_for(
        description="exactly one warm Fleet VM",
        timeout=POOL_READY_TIMEOUT_SECONDS,
        poll=10,
        probe=probe,
        ready=lambda status: int(status.get("totalCount", 0)) == 1
        and int(status.get("availableCount", 0)) == 1,
    )


def create_claim(http: httpx.Client, namespace: str, claim: str) -> None:
    response = http.post(
        claim_url(namespace),
        json={
            "apiVersion": f"{EXT_GROUP}/{EXT_VERSION}",
            "kind": "OSGymSandboxClaim",
            "metadata": {"name": claim},
            "spec": {
                "sandboxTemplateRef": {"name": f"{namespace}-template"},
                "bindDeadline": 600,
            },
        },
    )
    if response.status_code not in (200, 201, 202):
        raise PilotError(f"claim create failed with HTTP {response.status_code}")


def wait_claim(
    http: Callable[[], httpx.Client], namespace: str, claim: str
) -> str:
    def probe() -> dict[str, Any]:
        response = http().get(claim_url(namespace, claim))
        response.raise_for_status()
        return response.json().get("status") or {}

    status = wait_for(
        description="sandbox claim",
        timeout=600,
        poll=5,
        probe=probe,
        ready=lambda value: value.get("phase") in {"Bound", "Failed"},
    )
    if status.get("phase") != "Bound":
        raise PilotError("sandbox claim failed to bind")
    sandbox = (status.get("sandbox") or {}).get("name")
    if not isinstance(sandbox, str) or not sandbox:
        raise PilotError("bound claim omitted sandbox name")
    return sandbox


def wait_http_ready(url: str, timeout: float = 300) -> int:
    def probe() -> int:
        try:
            return httpx.get(url, timeout=15).status_code
        except httpx.HTTPError:
            return 0

    return wait_for(
        description="guest control bridge",
        timeout=timeout,
        poll=5,
        probe=probe,
        ready=lambda status: status not in (0, 502, 503, 504),
    )


def guest_exec(
    command: list[str] | str,
    *,
    shell: bool = False,
    timeout: int = 120,
) -> dict[str, Any]:
    response = httpx.post(
        f"http://127.0.0.1:{CONTROL_PORT}/setup/execute",
        json={"command": command, "shell": shell, "timeout": timeout},
        timeout=timeout + 60,
    )
    if response.status_code != 200:
        raise PilotError(f"guest command transport failed with HTTP {response.status_code}")
    payload = response.json()
    if int(payload.get("returncode", 1)) != 0:
        error = " ".join(str(payload.get("error", "")).split())
        if len(error) > 400:
            error = error[-400:]
        raise PilotError(
            f"guest command failed with return code {payload.get('returncode')}"
            + (f": {error}" if error else "")
        )
    return payload


def stage_local_file_to_guest(
    local_path: str | Path,
    guest_path: str,
) -> dict[str, Any]:
    """Stage a host-local task asset without one oversized ingress request.

    Fleet ingress rejects sufficiently large multipart uploads with HTTP 413.
    Local assets therefore travel as bounded base64 chunks through the existing
    authenticated guest command transport and are installed only after their
    guest-side size and SHA-256 match the host source.
    """

    source = Path(local_path).expanduser().resolve()
    if not source.is_file():
        raise PilotError("local task asset is not a regular file")
    if not guest_path.startswith("/") or "\x00" in guest_path:
        raise PilotError("guest task asset path must be absolute")

    source_size = source.stat().st_size
    digest = hashlib.sha256()
    with source.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    source_sha256 = digest.hexdigest()
    staging_path = f"/tmp/.osworld2-asset-{uuid.uuid4().hex}.part"
    guest_exec(
        [
            "python3",
            "-c",
            "import pathlib,sys; pathlib.Path(sys.argv[1]).write_bytes(b'')",
            staging_path,
        ],
        timeout=60,
    )
    with source.open("rb") as stream:
        while chunk := stream.read(LOCAL_ASSET_CHUNK_BYTES):
            guest_exec(
                [
                    "python3",
                    "-c",
                    (
                        "import base64,pathlib,sys;"
                        "p=pathlib.Path(sys.argv[2]);"
                        "p.open('ab').write(base64.b64decode(sys.argv[1]))"
                    ),
                    base64.b64encode(chunk).decode("ascii"),
                    staging_path,
                ],
                timeout=60,
            )

    remote_sha256 = str(
        guest_exec(["sha256sum", staging_path], timeout=120).get("output", "")
    ).split()
    remote_size = str(
        guest_exec(["stat", "-c", "%s", staging_path], timeout=60).get(
            "output", ""
        )
    ).strip()
    if (
        not remote_sha256
        or remote_sha256[0] != source_sha256
        or remote_size != str(source_size)
    ):
        raise PilotError("staged local task asset failed guest attestation")
    guest_exec(
        [
            "python3",
            "-c",
            (
                "import os,pathlib,sys;"
                "src=pathlib.Path(sys.argv[1]);"
                "dst=pathlib.Path(sys.argv[2]);"
                "dst.parent.mkdir(parents=True,exist_ok=True);"
                "os.replace(src,dst)"
            ),
            staging_path,
            guest_path,
        ],
        timeout=60,
    )
    emit(
        "local_asset_staged",
        destination=guest_path,
        bytes=source_size,
        sha256=source_sha256,
    )
    return {
        "destination": guest_path,
        "bytes": source_size,
        "sha256": source_sha256,
    }


def guest_launch(command: list[str] | str, *, shell: bool = False) -> None:
    response = httpx.post(
        f"http://127.0.0.1:{CONTROL_PORT}/setup/launch",
        json={"command": command, "shell": shell},
        timeout=60,
    )
    if response.status_code != 200:
        raise PilotError(
            f"guest launch transport failed with HTTP {response.status_code}"
        )


def guest_launch_detached(command: list[str], log_path: str) -> None:
    """Launch an argv-safe guest process beyond the control request lifetime."""

    encoded_command = base64.b64encode(
        json.dumps(command).encode("utf-8")
    ).decode("ascii")
    guest_exec(
        [
            "python3",
            "-c",
            GUEST_DETACHED_LAUNCH_CODE,
            encoded_command,
            log_path,
        ],
        timeout=30,
    )


def wait_guest_chrome_cdp(timeout: float = 90) -> None:
    wait_for(
        description="detached guest Chrome CDP",
        timeout=timeout,
        poll=2,
        probe=lambda: guest_exec(
            [
                "bash",
                "-lc",
                (
                    "if curl -fsS --max-time 5 "
                    "http://127.0.0.1:1337/json/version >/dev/null; "
                    "then printf ready; else printf waiting; fi"
                ),
            ],
            timeout=15,
        ).get("output", "").strip(),
        ready=lambda value: value == "ready",
    )


class GuestLocalRequests:
    """Route one guest-local origin through the OSWorld control service.

    Task 082 starts its simulated AWS console dynamically with nested Docker.
    The browser uses guest localhost directly.  Host-side setup/evaluation
    calls use this small requests-compatible adapter so the exact same origin
    is reached from inside the guest without exposing a dynamic Fleet service.
    """

    RequestException = requests.RequestException

    def __init__(self, port: int) -> None:
        self.origin = f"http://127.0.0.1:{port}"

    def _request(self, method: str, url: str, **kwargs: Any) -> requests.Response:
        if not url.startswith(self.origin):
            return requests.request(method, url, **kwargs)

        body: bytes | None = None
        headers = dict(kwargs.get("headers") or {})
        if "json" in kwargs:
            body = json.dumps(kwargs["json"]).encode("utf-8")
            headers.setdefault("Content-Type", "application/json")
        elif kwargs.get("data") is not None:
            data = kwargs["data"]
            body = data.encode("utf-8") if isinstance(data, str) else bytes(data)

        request_spec = {
            "method": method.upper(),
            "url": url,
            "headers": headers,
            "body": base64.b64encode(body).decode("ascii") if body else None,
            "timeout": float(kwargs.get("timeout", 30)),
        }
        encoded_spec = base64.b64encode(
            json.dumps(request_spec).encode("utf-8")
        ).decode("ascii")
        result = guest_exec(
            ["python3", "-c", GUEST_HTTP_CODE, encoded_spec],
            timeout=max(60, int(request_spec["timeout"]) + 30),
        )
        output = json.loads(result.get("output", ""))
        if output.get("transport_error"):
            raise requests.ConnectionError("guest-local origin is not ready")
        response = requests.Response()
        response.status_code = int(output["status"])
        response.headers.update(output.get("headers") or {})
        response._content = base64.b64decode(output.get("body") or "")
        response.url = url
        response.encoding = requests.utils.get_encoding_from_headers(
            response.headers
        )
        return response

    def get(self, url: str, **kwargs: Any) -> requests.Response:
        return self._request("GET", url, **kwargs)

    def post(self, url: str, **kwargs: Any) -> requests.Response:
        return self._request("POST", url, **kwargs)

    def put(self, url: str, **kwargs: Any) -> requests.Response:
        return self._request("PUT", url, **kwargs)

    def delete(self, url: str, **kwargs: Any) -> requests.Response:
        return self._request("DELETE", url, **kwargs)


def prepare_task_082(aws_mock_port: int, cache_dir: Path) -> None:
    os.environ.setdefault("WEBSITE_HOST_SUFFIX", "web.hku.icu")
    os.environ.setdefault(
        "PROXY_CONFIG_FILE",
        str(
            OSWORLD_DIR
            / "evaluation_examples"
            / "settings"
            / "proxy"
            / "dataimpulse.json"
        ),
    )
    sys.path.insert(0, str(OSWORLD_DIR))
    logging.getLogger("desktopenv.setup").setLevel(logging.WARNING)
    logging.getLogger("desktopenv").setLevel(logging.WARNING)

    from desktop_env.controllers.setup import SetupController
    from evaluation_examples.task_class import task_082

    # Task 082's compose file maps AWS_PORT on both the host and container
    # sides.  The image itself always listens on the task's official port
    # (3000), so rewriting AWS_PORT to an arbitrary host bridge port produces
    # a healthy container behind a dead mapping such as 31082:31082.  The
    # browser and evaluator both reach the service guest-locally, therefore
    # retain the official port and route host-side requests through the
    # authenticated control bridge instead of changing the task.
    official_guest_port = task_082.AWS_PORT
    task_082.requests = GuestLocalRequests(official_guest_port)
    if aws_mock_port != official_guest_port:
        emit(
            "task_guest_port_retained",
            task_id="082",
            guest_port=official_guest_port,
        )

    class FleetSetupController(SetupController):
        def _chrome_open_tabs_setup(self, urls_to_open: list[str]) -> None:
            # The official implementation reaches CDP directly.  Fleet CDP is
            # proxied, so open the same URLs through Chrome's single-instance
            # command-line handoff inside the interactive guest session.
            self.launch(
                ["google-chrome", "--remote-debugging-port=1337", *urls_to_open]
            )

    cache_dir.mkdir(parents=True, exist_ok=True)
    controller = FleetSetupController(
        vm_ip="127.0.0.1",
        server_port=CONTROL_PORT,
        chromium_port=CDP_PORT,
        vlc_port=VLC_PORT,
        cache_dir=str(cache_dir),
        client_password="osworld-public-evaluation",
        screen_width=1920,
        screen_height=1080,
    )
    if not controller.ensure_ready():
        raise PilotError("official OSWorld setup controller did not become ready")
    emit("task_setup_started", task_id="082")
    task_082.Task082().setup(controller)
    emit("task_setup_complete", task_id="082")


def prepare_browser_task(
    task_id: str,
    cache_dir: Path,
    *,
    guest_chrome_profile: str | None = None,
) -> None:
    if len(task_id) != 3 or not task_id.isdigit():
        raise PilotError(f"invalid OSWorld browser task ID: {task_id!r}")
    os.environ.setdefault("WEBSITE_HOST_SUFFIX", "web.hku.icu")
    os.environ.setdefault(
        "PROXY_CONFIG_FILE",
        str(
            OSWORLD_DIR
            / "evaluation_examples"
            / "settings"
            / "proxy"
            / "dataimpulse.json"
        ),
    )
    sys.path.insert(0, str(OSWORLD_DIR))
    logging.getLogger("desktopenv.setup").setLevel(logging.WARNING)
    logging.getLogger("desktopenv").setLevel(logging.WARNING)

    from desktop_env.controllers.setup import SetupController
    from desktop_env.file_source import resolve_local_source

    module = __import__(
        f"evaluation_examples.task_class.task_{task_id}",
        fromlist=[f"Task{task_id}"],
    )
    task_class = getattr(module, f"Task{task_id}")

    class FleetSetupController(SetupController):
        def _download_setup(self, files: list[dict[str, str]]) -> None:
            remote_files: list[dict[str, str]] = []
            for item in files:
                local_source = resolve_local_source(item["url"])
                if local_source is None:
                    remote_files.append(item)
                    continue

                cache_path = Path(self.cache_dir) / (
                    f"{uuid.uuid5(uuid.NAMESPACE_URL, item['url'])}_"
                    f"{Path(item['path']).name}"
                )
                if not cache_path.exists():
                    cache_path.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(local_source, cache_path)
                stage_local_file_to_guest(local_source, item["path"])

            if remote_files:
                super()._download_setup(remote_files)

        def launch(
            self,
            command: str | list[str],
            shell: bool = False,
        ) -> None:
            if (
                guest_chrome_profile
                and isinstance(command, list)
                and command
                and command[0] == "google-chrome"
            ):
                command = isolated_chrome_command(command, guest_chrome_profile)
                guest_launch_detached(
                    command,
                    "/tmp/osworld2-chrome-launch.log",
                )
                wait_guest_chrome_cdp()
                return
            super().launch(command, shell=shell)

        def _chrome_open_tabs_setup(self, urls_to_open: list[str]) -> None:
            self.launch(
                ["google-chrome", "--remote-debugging-port=1337", *urls_to_open]
            )

    cache_dir.mkdir(parents=True, exist_ok=True)
    controller = FleetSetupController(
        vm_ip="127.0.0.1",
        server_port=CONTROL_PORT,
        chromium_port=CDP_PORT,
        vlc_port=VLC_PORT,
        cache_dir=str(cache_dir),
        client_password="osworld-public-evaluation",
        screen_width=1920,
        screen_height=1080,
    )
    if not controller.ensure_ready():
        raise PilotError("official OSWorld setup controller did not become ready")
    emit("task_setup_started", task_id=task_id)
    task_class().setup(controller)
    emit("task_setup_complete", task_id=task_id)


def build_and_start_driver(source_sha: str) -> dict[str, Any]:
    if not source_sha or len(source_sha) != 40:
        raise PilotError("Cua source SHA must be a full 40-character commit")

    emit("driver_build_started", source_sha=source_sha)
    packages = (
        "git curl ca-certificates build-essential pkg-config clang cmake "
        "libssl-dev libdbus-1-dev libx11-dev libxtst-dev libxrandr-dev "
        "libxinerama-dev libxi-dev libxkbcommon-dev libwayland-dev "
        "libpipewire-0.3-dev libasound2-dev libudev-dev"
    )
    guest_exec(
        (
            "set -e; "
            "echo 'osworld-public-evaluation' | sudo -S apt-get update -qq; "
            f"echo 'osworld-public-evaluation' | sudo -S apt-get install -y "
            f"--no-install-recommends {packages}"
        ),
        shell=True,
        timeout=1800,
    )
    guest_exec(
        (
            "set -e; "
            "if ! command -v rustup >/dev/null 2>&1; then "
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs "
            "| sh -s -- -y --profile minimal; fi; "
            "if [ ! -d /home/user/cua-source/.git ]; then "
            "git clone --filter=blob:none https://github.com/trycua/cua.git "
            "/home/user/cua-source; fi; "
            "git -C /home/user/cua-source fetch --force origin "
            f"{source_sha}; "
            f"git -C /home/user/cua-source checkout --detach {source_sha}; "
            "cd /home/user/cua-source/libs/cua-driver/rust; "
            "/home/user/.cargo/bin/cargo build -p cua-driver --release --locked; "
            "echo 'osworld-public-evaluation' | sudo -S install -m 0755 "
            "target/release/cua-driver /usr/local/bin/cua-driver"
        ),
        shell=True,
        timeout=3600,
    )
    provenance = guest_exec(
        [
            "bash",
            "-lc",
            (
                "set -e; "
                "test \"$(git -C /home/user/cua-source rev-parse HEAD)\" = "
                f"\"{source_sha}\"; "
                "printf 'source=%s\\n' "
                "\"$(git -C /home/user/cua-source rev-parse HEAD)\"; "
                "printf 'binary=%s\\n' \"$(command -v cua-driver)\"; "
                "cua-driver --version"
            ),
        ],
        timeout=120,
    )
    guest_launch(
        [
            "bash",
            "-lc",
            (
                "export DISPLAY=\"${DISPLAY:-:0}\"; "
                "runtime_dir=\"/run/user/$(id -u)\"; "
                "if test -d \"$runtime_dir\"; then "
                "export XDG_RUNTIME_DIR=\"$runtime_dir\"; fi; "
                "if test -S \"$runtime_dir/bus\"; then "
                "export DBUS_SESSION_BUS_ADDRESS="
                "\"unix:path=$runtime_dir/bus\"; fi; "
                "exec env CUA_DRIVER_RS_PERMISSIONS_GATE=0 "
                "/usr/local/bin/cua-driver serve "
                f"--socket {GUEST_DRIVER_SOCKET} "
                "--dangerously-bypass-approvals "
                ">/home/user/cua-driver.log 2>&1"
            ),
        ]
    )
    status = guest_exec(
        ["cua-driver", "status", "--socket", GUEST_DRIVER_SOCKET],
        timeout=60,
    )
    emit("driver_ready", source_sha=source_sha)
    return {
        "source_sha": source_sha,
        "provenance": provenance.get("output", "").strip().splitlines(),
        "status": status.get("output", "").strip(),
    }


def install_prebuilt_driver(
    artifact_dir: Path, source_sha: str
) -> dict[str, Any]:
    binary = artifact_dir / "cua-driver-linux-x86_64"
    provenance_path = artifact_dir / "provenance.json"
    checksum_path = artifact_dir / "cua-driver-linux-x86_64.sha256"
    if not binary.is_file() or not provenance_path.is_file() or not checksum_path.is_file():
        raise PilotError("driver artifact directory is incomplete")
    provenance = read_json(provenance_path)
    expected_hash = str(provenance.get("binary_sha256", ""))
    if provenance.get("source_sha") != source_sha:
        raise PilotError("driver artifact source SHA mismatch")
    actual_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    if not expected_hash or actual_hash != expected_hash:
        raise PilotError("driver artifact checksum mismatch")
    checksum_record = checksum_path.read_text(encoding="utf-8").split()
    if not checksum_record or checksum_record[0] != expected_hash:
        raise PilotError("driver checksum record mismatch")

    emit("driver_upload_started", source_sha=source_sha)
    part_paths: list[str] = []
    with binary.open("rb") as stream:
        part_index = 0
        while True:
            chunk = stream.read(512 * 1024)
            if not chunk:
                break
            part_path = f"/home/user/cua-driver-part-{part_index:03d}"
            response = requests.post(
                f"http://127.0.0.1:{CONTROL_PORT}/setup/upload",
                data={"file_path": part_path},
                files={
                    "file_data": (
                        f"cua-driver-part-{part_index:03d}",
                        chunk,
                        "application/octet-stream",
                    )
                },
                timeout=(30, 300),
            )
            if response.status_code != 200:
                raise PilotError(
                    "driver part upload failed "
                    f"at part {part_index} with HTTP {response.status_code}"
                )
            part_paths.append(part_path)
            part_index += 1
            if part_index % 10 == 0:
                emit(
                    "driver_upload_progress",
                    parts=part_index,
                    bytes=part_index * 512 * 1024,
                )
    if not part_paths:
        raise PilotError("driver artifact was empty")
    assemble_code = (
        "import sys;"
        "out=open(sys.argv[1],'wb');"
        "[(out.write(open(path,'rb').read())) for path in sys.argv[2:]];"
        "out.close()"
    )
    guest_exec(
        [
            "python3",
            "-c",
            assemble_code,
            "/home/user/cua-driver-staged",
            *part_paths,
        ],
        timeout=300,
    )
    remote_checksum = guest_exec(
        ["sha256sum", "/home/user/cua-driver-staged"],
        timeout=120,
    ).get("output", "").strip().split()
    if not remote_checksum or remote_checksum[0] != expected_hash:
        raise PilotError("reassembled guest driver checksum mismatch")
    emit("driver_checksum_verified", source_sha=source_sha)
    guest_exec(
        [
            "bash",
            "-lc",
            (
                "set -e; "
                "echo 'osworld-public-evaluation' | sudo -S install -m 0755 "
                "/home/user/cua-driver-staged /usr/local/bin/cua-driver"
            ),
        ],
        timeout=180,
    )
    installed = guest_exec(
        ["/usr/local/bin/cua-driver", "--version"],
        timeout=60,
    )
    emit("driver_version_verified")
    try:
        guest_launch(
            [
                "bash",
                "-lc",
                (
                    "export DISPLAY=\"${DISPLAY:-:0}\"; "
                    "exec env CUA_DRIVER_RS_PERMISSIONS_GATE=0 "
                    "/usr/local/bin/cua-driver serve "
                    f"--socket {GUEST_DRIVER_SOCKET} "
                    "--dangerously-bypass-approvals "
                    ">/home/user/cua-driver.log 2>&1"
                ),
            ]
        )
        wait_for(
            description="Cua Driver isolated socket",
            timeout=180,
            poll=3,
            probe=lambda: guest_exec(
                [
                    "bash",
                    "-lc",
                    (
                        f"if test -S {GUEST_DRIVER_SOCKET}; "
                        "then printf ready; else printf waiting; fi"
                    ),
                ],
                timeout=20,
            ).get("output", "").strip(),
            ready=lambda value: value == "ready",
        )
    except PilotError as exc:
        log_tail = guest_exec(
            [
                "bash",
                "-lc",
                "tail -n 20 /home/user/cua-driver.log 2>/dev/null || true",
            ],
            timeout=60,
        ).get("output", "")
        detail = " ".join(str(log_tail).split())
        raise PilotError(
            "driver daemon did not start"
            + (f": {detail[-1000:]}" if detail else "")
        ) from exc
    status = guest_exec(
        [
            "bash",
            "-lc",
            (
                "set -e; "
                "daemon_pid=$(pgrep -n -f "
                f"'/usr/local/bin/cua-driver serve --socket {GUEST_DRIVER_SOCKET}'"
                "); "
                f"test -S {GUEST_DRIVER_SOCKET}; "
                "kill -0 \"$daemon_pid\"; "
                "printf 'daemon_pid=%s\\nsocket=%s\\n' \"$daemon_pid\" "
                f"{GUEST_DRIVER_SOCKET}"
            ),
        ],
        timeout=60,
    )
    emit("driver_ready", source_sha=source_sha)
    return {
        "source_sha": source_sha,
        "binary_sha256": expected_hash,
        "version": installed.get("output", "").strip(),
        "start": "OSWorld setup launch endpoint",
        "status": status.get("output", "").strip(),
    }


def start_image_driver() -> dict[str, Any]:
    """Start and attest the release-pinned Driver already baked into the image."""

    manifest = read_json(ROOT / "manifest.json")
    expected_version = str(manifest["cua_driver"]["version"])
    expected_release = str(manifest["cua_driver"]["release"])
    expected_archive_sha256 = str(
        manifest["cua_driver"]["linux_x86_64_archive_sha256"]
    )
    installed = guest_exec(
        ["/usr/local/bin/cua-driver", "--version"],
        timeout=60,
    ).get("output", "").strip()
    if expected_version not in installed:
        raise PilotError(
            f"image Driver version mismatch: expected {expected_version}, "
            f"found {installed!r}"
        )

    metadata_result = guest_exec(
        ["cat", "/etc/cua-driver-osworld2-build.json"],
        timeout=60,
    )
    try:
        metadata = json.loads(metadata_result.get("output", ""))
    except json.JSONDecodeError as exc:
        raise PilotError("image Driver metadata is not valid JSON") from exc
    expected_metadata = {
        "benchmark_release": manifest["benchmark_release"],
        "cua_driver_tag": expected_release,
        "cua_driver_archive_sha256": expected_archive_sha256,
    }
    if metadata != expected_metadata:
        raise PilotError("image Driver metadata did not match the pinned manifest")

    guest_launch(
        [
            "bash",
            "-lc",
            (
                "export DISPLAY=\"${DISPLAY:-:0}\"; "
                "exec env CUA_DRIVER_RS_PERMISSIONS_GATE=0 "
                "/usr/local/bin/cua-driver serve "
                f"--socket {GUEST_DRIVER_SOCKET} "
                "--dangerously-bypass-approvals "
                ">/home/user/cua-driver.log 2>&1"
            ),
        ]
    )
    wait_for(
        description="release-pinned Cua Driver socket",
        timeout=180,
        poll=3,
        probe=lambda: guest_exec(
            [
                "bash",
                "-lc",
                (
                    f"if test -S {GUEST_DRIVER_SOCKET}; "
                    "then printf ready; else printf waiting; fi"
                ),
            ],
            timeout=20,
        ).get("output", "").strip(),
        ready=lambda value: value == "ready",
    )
    status = guest_exec(
        ["/usr/local/bin/cua-driver", "status", "--socket", GUEST_DRIVER_SOCKET],
        timeout=60,
    ).get("output", "").strip()
    doctor_path = "/tmp/cua-driver-osworld2-doctor.json"
    guest_launch(
        [
            "bash",
            "-lc",
            (
                "export DISPLAY=\"${DISPLAY:-:0}\"; "
                "runtime_dir=\"/run/user/$(id -u)\"; "
                "if test -d \"$runtime_dir\"; then "
                "export XDG_RUNTIME_DIR=\"$runtime_dir\"; fi; "
                "if test -S \"$runtime_dir/bus\"; then "
                "export DBUS_SESSION_BUS_ADDRESS="
                "\"unix:path=$runtime_dir/bus\"; fi; "
                "/usr/local/bin/cua-driver doctor --json "
                f">{doctor_path}.pending "
                "2>/tmp/cua-driver-osworld2-doctor.log && "
                f"mv {doctor_path}.pending {doctor_path}"
            ),
        ]
    )
    wait_for(
        description="Cua Driver Linux doctor report",
        timeout=120,
        poll=3,
        probe=lambda: guest_exec(
            [
                "bash",
                "-lc",
                f"if test -s {doctor_path}; then printf ready; else printf waiting; fi",
            ],
            timeout=20,
        ).get("output", "").strip(),
        ready=lambda value: value == "ready",
    )
    try:
        doctor = json.loads(
            guest_exec(["cat", doctor_path], timeout=60).get("output", "")
        )
    except json.JSONDecodeError as exc:
        raise PilotError("image Driver doctor report was not valid JSON") from exc
    sdk = install_image_driver_sdk(manifest)
    emit("driver_ready", release=expected_release, source="container_image")
    return {
        "release": expected_release,
        "version": installed,
        "archive_sha256": expected_archive_sha256,
        "metadata": metadata,
        "source": "container_image",
        "status": status,
        "doctor": doctor,
        "python_sdk": sdk,
    }


def install_image_driver_sdk(manifest: dict[str, Any]) -> dict[str, Any]:
    """Install and attest the release-matched Python SDK in the disposable guest."""

    driver = manifest["cua_driver"]
    release = str(driver["release"])
    archive = str(driver["python_sdk_linux_x86_64_archive"])
    archive_sha256 = str(driver["python_sdk_linux_x86_64_archive_sha256"])
    source_hashes = {
        str(name): str(digest)
        for name, digest in driver["python_sdk_source_sha256"].items()
    }
    expected_sources = {
        "__init__.py",
        "_native.py",
        "_native_contract.py",
        "wrapper.py",
    }
    if set(source_hashes) != expected_sources:
        raise PilotError("Python SDK source manifest is incomplete")
    if any(len(digest) != 64 for digest in source_hashes.values()):
        raise PilotError("Python SDK source manifest contains an invalid checksum")

    caller = ROOT / "driver_sdk_call.py"
    caller_bytes = caller.read_bytes()
    caller_sha256 = hashlib.sha256(caller_bytes).hexdigest()
    archive_path = f"{GUEST_DRIVER_SDK_ROOT}/{archive}"
    release_url = (
        f"https://github.com/trycua/cua/releases/download/{release}/{archive}"
    )
    guest_exec(
        ["mkdir", "-p", GUEST_DRIVER_SDK_PACKAGE],
        timeout=60,
    )
    guest_exec(
        [
            "curl",
            "-fL",
            "--retry",
            "4",
            "--retry-all-errors",
            "-o",
            archive_path,
            release_url,
        ],
        timeout=600,
    )
    archive_check = guest_exec(
        ["sha256sum", archive_path],
        timeout=120,
    ).get("output", "").split()
    if not archive_check or archive_check[0] != archive_sha256:
        raise PilotError("Python SDK release archive checksum mismatch")

    archive_member = (
        f"cua-driver-rs-{driver['version']}-linux-x86_64/"
        "libcua_driver_sdk.so"
    )
    guest_exec(
        [
            "tar",
            "-xzf",
            archive_path,
            "-C",
            GUEST_DRIVER_SDK_PACKAGE,
            "--strip-components=1",
            archive_member,
        ],
        timeout=180,
    )
    source_base = (
        f"https://raw.githubusercontent.com/trycua/cua/{release}/"
        "libs/cua-driver/python/src/cua_driver"
    )
    for name, expected_sha256 in sorted(source_hashes.items()):
        destination = f"{GUEST_DRIVER_SDK_PACKAGE}/{name}"
        guest_exec(
            [
                "curl",
                "-fL",
                "--retry",
                "4",
                "--retry-all-errors",
                "-o",
                destination,
                f"{source_base}/{name}",
            ],
            timeout=300,
        )
        source_check = guest_exec(
            ["sha256sum", destination],
            timeout=60,
        ).get("output", "").split()
        if not source_check or source_check[0] != expected_sha256:
            raise PilotError(f"Python SDK source checksum mismatch for {name}")

    encoded_caller = base64.b64encode(caller_bytes).decode("ascii")
    guest_exec(
        [
            "python3",
            "-c",
            (
                "import base64,pathlib,sys;"
                "pathlib.Path(sys.argv[2]).write_bytes(base64.b64decode(sys.argv[1]))"
            ),
            encoded_caller,
            GUEST_DRIVER_SDK_CALLER,
        ],
        timeout=60,
    )
    caller_check = guest_exec(
        ["sha256sum", GUEST_DRIVER_SDK_CALLER],
        timeout=60,
    ).get("output", "").split()
    if not caller_check or caller_check[0] != caller_sha256:
        raise PilotError("Python SDK caller checksum mismatch")

    smoke = guest_exec(
        [
            "env",
            f"PYTHONPATH={GUEST_DRIVER_SDK_ROOT}",
            "python3",
            GUEST_DRIVER_SDK_CALLER,
            "--socket",
            GUEST_DRIVER_SOCKET,
            "list_windows",
            "{}",
        ],
        timeout=120,
    )
    if int(smoke.get("returncode", 1)) != 0:
        raise PilotError("release-matched Python SDK smoke call failed")
    try:
        smoke_value = json.loads(str(smoke.get("output") or ""))
    except json.JSONDecodeError as exc:
        raise PilotError("Python SDK smoke call did not return JSON") from exc
    if smoke_value.get("is_error") is True:
        raise PilotError("Python SDK smoke call returned a Driver error")

    return {
        "release": release,
        "archive": archive,
        "archive_sha256": archive_sha256,
        "source_sha256": source_hashes,
        "caller_sha256": caller_sha256,
        "library_path": f"{GUEST_DRIVER_SDK_PACKAGE}/libcua_driver_sdk.so",
        "transport": "CuaDriver.connect(...).call_tool(...)",
        "smoke_verified": True,
    }


def delete_and_wait(
    *,
    http: Callable[[], httpx.Client],
    path: str,
    description: str,
    timeout: float = 300,
) -> bool:
    response = http().delete(path)
    if response.status_code not in (200, 202, 204, 404):
        emit("cleanup_delete_failed", resource=description, http=response.status_code)
        return False

    def probe() -> int:
        return http().get(path).status_code

    try:
        wait_for(
            description=f"{description} deletion",
            timeout=timeout,
            poll=5,
            probe=probe,
            ready=lambda status: status == 404,
        )
    except PilotError:
        emit("cleanup_verify_failed", resource=description)
        return False
    emit("cleanup_verified", resource=description)
    return True


def namespace_absent(http: Callable[[], httpx.Client], namespace: str) -> bool:
    def probe() -> bool:
        response = http().get("/api/namespaces")
        response.raise_for_status()
        payload = response.json()
        items = payload if isinstance(payload, list) else payload.get("items", [])
        names = {
            item.get("name") or (item.get("metadata") or {}).get("name")
            for item in items
            if isinstance(item, dict)
        }
        return namespace not in names

    try:
        wait_for(
            description="namespace deletion",
            timeout=420,
            poll=5,
            probe=probe,
            ready=bool,
        )
    except PilotError:
        emit("cleanup_verify_failed", resource="namespace")
        return False
    emit("cleanup_verified", resource="namespace")
    return True


def main() -> int:
    global STOP_REQUESTED
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--container-disk-image")
    parser.add_argument("--aws-region", default=DEFAULT_AWS_REGION)
    parser.add_argument("--aws-mock-port", type=int, default=DEFAULT_AWS_MOCK_PORT)
    parser.add_argument("--setup-task-082", action="store_true")
    parser.add_argument("--task-id", choices=["070", "073"])
    parser.add_argument("--build-driver", action="store_true")
    parser.add_argument("--driver-artifact-dir", type=Path)
    parser.add_argument("--start-image-driver", action="store_true")
    parser.add_argument("--source-sha")
    parser.add_argument("--poll", type=float, default=5)
    args = parser.parse_args()

    local = read_json(args.config)
    secret_name = local.get("fleet_secret_name")
    image = args.container_disk_image or local.get("container_disk_image")
    if not isinstance(secret_name, str) or not secret_name:
        raise PilotError("local config is missing fleet_secret_name")
    if not isinstance(image, str) or "@sha256:" not in image:
        raise PilotError(
            "local config or --container-disk-image must pin the image by digest"
        )
    driver_strategies = sum(
        bool(value)
        for value in (
            args.build_driver,
            args.driver_artifact_dir,
            args.start_image_driver,
        )
    )
    if driver_strategies > 1:
        raise PilotError(
            "choose one Driver setup strategy"
        )
    if (args.build_driver or args.driver_artifact_dir) and not args.source_sha:
        raise PilotError("driver setup requires --source-sha")
    if args.setup_task_082 and args.task_id:
        raise PilotError("choose either --setup-task-082 or --task-id")
    task_id = "082" if args.setup_task_082 else args.task_id

    credentials = load_aws_secret(secret_name, args.aws_region)
    client_id = credentials.get("client_id")
    client_secret = credentials.get("client_secret")
    token_url = credentials.get("token_url")
    base_url = credentials.get("base_url", DEFAULT_FLEET_BASE_URL)
    if not all(isinstance(value, str) and value for value in (
        client_id,
        client_secret,
        token_url,
        base_url,
    )):
        raise PilotError("configured Fleet credential is missing required fields")
    if not client_id.startswith("ukey-"):
        raise PilotError("Fleet lifecycle requires a per-user credential")

    sdk_path = ROOT.parents[2] / "fleet" / "python-sdk"
    sys.path.insert(0, str(sdk_path))
    from cua_train import TrainClient

    client = TrainClient.from_key(
        token_url=token_url,
        client_id=client_id,
        client_secret=client_secret,
        base_url=base_url,
    )
    http = client.get_httpx_client

    suffix = uuid.uuid4().hex[:8]
    namespace = f"osworld2-cua-pilot-{suffix}"
    claim = f"claim-{suffix}"
    services = (
        Service("control", CONTROL_PORT, CONTROL_PORT),
        Service("cdp", GUEST_CDP_PORT, CDP_PORT),
        Service("novnc", NOVNC_PORT, NOVNC_PORT),
        Service("vlc", VLC_PORT, VLC_PORT),
    )
    bridge: FleetBridge | None = None
    sandbox: str | None = None
    namespace_created = False
    pool_created = False
    claim_created = False
    cleanup_ok = False
    stop_requested = threading.Event()
    STOP_REQUESTED = stop_requested

    def request_stop(_signum: int, _frame: Any) -> None:
        stop_requested.set()

    signal.signal(signal.SIGINT, request_stop)
    signal.signal(signal.SIGTERM, request_stop)

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    state_path = RESULTS_DIR / "fleet-pilot-live.json"
    result_path = RESULTS_DIR / f"fleet-pilot-{suffix}.json"
    image_fingerprint = hashlib.sha256(image.encode("utf-8")).hexdigest()

    try:
        emit("provision_started", namespace=namespace, replicas=1)
        create_namespace(http(), namespace)
        namespace_created = True
        create_pool(http(), namespace=namespace, image=image, services=services)
        pool_created = True
        wait_template(http, namespace)
        status = wait_pool(http, namespace)
        if int(status.get("totalCount", 0)) != 1:
            raise PilotError("pool violated the exactly-one-VM invariant")
        emit("pool_ready", total=1, available=1)

        create_claim(http(), namespace, claim)
        claim_created = True
        sandbox = wait_claim(http, namespace, claim)
        emit("claim_bound", namespace=namespace)

        token_source = TokenSource(token_url, client_id, client_secret)
        bridge = FleetBridge(
            base_url=base_url,
            namespace=namespace,
            sandbox=sandbox,
            services=services,
            token_source=token_source,
        )
        bridge.start()
        control_status = wait_http_ready(
            f"http://127.0.0.1:{CONTROL_PORT}/terminal", timeout=420
        )
        emit("control_ready", http=control_status)

        driver_result: dict[str, Any] | None = None
        if args.build_driver:
            driver_result = build_and_start_driver(args.source_sha)
        elif args.driver_artifact_dir:
            driver_result = install_prebuilt_driver(
                args.driver_artifact_dir, args.source_sha
            )
        elif args.start_image_driver:
            driver_result = start_image_driver()

        task_cache = RESULTS_DIR / f"task-{task_id or 'none'}-{suffix}-cache"
        if task_id == "082":
            prepare_task_082(args.aws_mock_port, task_cache)
        elif task_id:
            prepare_browser_task(task_id, task_cache)

        cdp_status = None
        if task_id:
            wait_for(
                description="guest-local Chrome CDP",
                timeout=180,
                poll=3,
                probe=lambda: guest_exec(
                    [
                        "bash",
                        "-lc",
                        (
                            "if curl -fsS --max-time 10 "
                            "http://127.0.0.1:1337/json/version >/dev/null; "
                            "then printf ready; else printf waiting; fi"
                        ),
                    ],
                    timeout=20,
                ).get("output", "").strip(),
                ready=lambda value: value == "ready",
            )
            cdp_status = 200
        try:
            novnc_status = httpx.get(
                f"http://127.0.0.1:{NOVNC_PORT}/", timeout=15
            ).status_code
        except httpx.HTTPError:
            novnc_status = 0
        if novnc_status in (0, 502, 503, 504):
            emit("novnc_unavailable", continuing=True)
        live = {
            "schema_version": 1,
            "namespace": namespace,
            "claim": claim,
            "sandbox": sandbox,
            "replicas": 1,
            "image_identifier_sha256": image_fingerprint,
            "task_id": task_id,
            "source_sha": (
                args.source_sha
                if args.build_driver or args.driver_artifact_dir
                else None
            ),
            "loopback_services": {
                service.name: {
                    "host": "127.0.0.1",
                    "port": service.local_port,
                }
                for service in services
            },
            "probes": {
                "control_http": control_status,
                "cdp_http": cdp_status,
                "novnc_http": novnc_status,
            },
            "driver": driver_result,
        }
        state_path.write_text(json.dumps(live, indent=2) + "\n", encoding="utf-8")
        emit(
            "pilot_ready",
            task_id=live["task_id"],
            driver=bool(driver_result),
            state_file=work_relative_path(state_path),
        )
        while not stop_requested.wait(timeout=1):
            pass
    finally:
        # A stop request interrupts provisioning waits, but cleanup has its
        # own bounded verification loops and must be allowed to complete.
        stop_requested.clear()
        emit("cleanup_started")
        if bridge:
            bridge.stop()

        checks: list[bool] = []
        if claim_created:
            checks.append(
                delete_and_wait(
                    http=http,
                    path=claim_url(namespace, claim),
                    description="claim",
                )
            )
        if pool_created:
            checks.append(
                delete_and_wait(
                    http=http,
                    path=pool_url(namespace, namespace),
                    description="pool",
                    timeout=420,
                )
            )
        if namespace_created:
            response = http().delete(f"/api/namespaces/{namespace}")
            checks.append(response.status_code in (200, 202, 204, 404))
            checks.append(namespace_absent(http, namespace))
        cleanup_ok = bool(checks) and all(checks)

        result = {
            "schema_version": 1,
            "namespace": namespace,
            "replicas": 1,
            "image_identifier_sha256": image_fingerprint,
            "task_id": task_id,
            "source_sha": (
                args.source_sha
                if args.build_driver or args.driver_artifact_dir
                else None
            ),
            "claim_bound": sandbox is not None,
            "cleanup_verified": cleanup_ok,
        }
        result_path.write_text(
            json.dumps(result, indent=2) + "\n", encoding="utf-8"
        )
        with contextlib.suppress(FileNotFoundError):
            state_path.unlink()
        emit("cleanup_complete", verified=cleanup_ok)
    return 0 if cleanup_ok else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PilotError as exc:
        emit("pilot_failed", reason=str(exc))
        raise SystemExit(1)
