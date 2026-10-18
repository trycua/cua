from __future__ import annotations

import asyncio
import json
import os
import re
import time
from pathlib import Path
from typing import Any

import httpx
from fleet_sdk import (
    CyclopsClient,
    CyclopsConfiguration,
    CyclopsCredentials,
    HttpClient,
    HttpError,
    HttpHeader,
    HttpRequest,
    HttpResponse,
    SdkError,
)

#: A containerDisk that ships cua-spacesd (for example a pushed
#: libs/images/linux build). When set, the live runners use it with
#: the default `env` service and drive the computer interfaces; otherwise they
#: use the pinned image and prove only lifecycle + service reachability, which
#: is all a daemon-agnostic sandbox promises.
ENV_IMAGE_VARIABLE = "CUA_LIVE_E2E_ENV_IMAGE"

DEFAULT_BASE_URL = "https://run.cua.ai"
DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
SENSITIVE_SUMMARY_KEY_NAMES = {"api_key", "apikey"}
SENSITIVE_SUMMARY_KEY_PARTS = {"authorization", "password", "secret", "token"}


class HttpxFleetClient(HttpClient):
    def __init__(self) -> None:
        self._client = httpx.AsyncClient(timeout=60.0)

    async def execute(self, request: HttpRequest) -> HttpResponse:
        try:
            response = await self._client.request(
                request.method,
                request.url,
                headers={header.name: header.value for header in request.headers},
                content=request.body,
            )
        except httpx.TransportError as error:
            raise HttpError.Transport(str(error)) from error
        return HttpResponse(
            status=response.status_code,
            headers=[
                HttpHeader(name=name, value=value) for name, value in response.headers.multi_items()
            ],
            body=response.content,
        )

    async def aclose(self) -> None:
        await self._client.aclose()


def has_oauth_credentials() -> bool:
    return bool(os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET"))


def _event_class(event_name: str) -> str:
    return {
        "schedule": "schedule",
        "push": "push",
        "workflow_dispatch": "manual",
    }.get(event_name, "manual")


def _normalize_namespace_name(raw: str) -> str:
    normalized = re.sub(r"[^a-z0-9-]+", "-", raw.lower()).strip("-")
    return normalized[:63].rstrip("-")


def build_namespace_name(lane: str, event_name: str) -> str:
    return _normalize_namespace_name(f"cua-live-{lane}-{_event_class(event_name)}")


def build_pool_namespace_name(mode: str, lane: str, event_name: str) -> str:
    return _normalize_namespace_name(f"cua-live-pool-{mode}-{lane}-{_event_class(event_name)}")


def build_fleet_client() -> tuple[CyclopsClient, HttpxFleetClient]:
    client_id = os.environ["CUA_CLIENT_ID"]
    client_secret = os.environ["CUA_CLIENT_SECRET"]
    http_client = HttpxFleetClient()
    configuration = CyclopsConfiguration(
        base_url=os.environ.get("CUA_FLEET_BASE_URL", DEFAULT_BASE_URL),
        token_url=os.environ.get("CUA_TOKEN_URL", DEFAULT_TOKEN_URL),
        credentials=CyclopsCredentials(client_id, client_secret),
        pool_poll_interval_ms=2000,
        pool_poll_limit=300,
        claim_poll_interval_ms=2000,
        claim_poll_limit=300,
    )
    return CyclopsClient.connect(configuration, http_client), http_client


def is_not_found_error(error: BaseException) -> bool:
    return isinstance(error, SdkError.Status) and error.status == 404


def is_pool_missing_error(error: BaseException) -> bool:
    # Fleet evaluates RBAC before existence, so reading a pool in a namespace
    # that has not been created yet returns 403 rather than 404. Mirror the
    # SDK's reconcile semantics, which create the pool on either status.
    return isinstance(error, SdkError.Status) and error.status in (403, 404)


async def wait_claims_absent(
    client: CyclopsClient,
    name: str,
    *,
    timeout: float = 180.0,
    interval: float = 5.0,
) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if not await client.list_claims(name):
                return True
        except Exception as error:
            if is_not_found_error(error):
                return True
            raise
        await asyncio.sleep(interval)
    try:
        return not await client.list_claims(name)
    except Exception as error:
        if is_not_found_error(error):
            return True
        raise


async def collect_resource_inventory(client: CyclopsClient, name: str) -> dict[str, list[str]]:
    try:
        templates = await client.list_templates(name)
        pools = await client.list_pools(name)
        claims = await client.list_claims(name)
    except Exception as error:
        # Deleting a pool deletes its namespace; once that finishes, RBAC
        # answers 403 for the namespace (checked before existence).
        if is_not_found_error(error) or is_pool_missing_error(error):
            return {"templates": [], "pools": [], "claims": []}
        raise
    return {
        "templates": [item.metadata.name for item in templates],
        "pools": [item.metadata.name for item in pools],
        "claims": [item.metadata.name for item in claims],
    }


def env_image() -> str | None:
    return os.environ.get(ENV_IMAGE_VARIABLE) or None


#: cua-sandbox's Fleet pools are KubeVirt containerDisk VMs, and Fleet has no
#: per-claim secret field yet, so the spacesd token cannot reach the guest.
KUBEVIRT_ENV_SKIP = (
    "KubeVirt spacesd lane needs a per-claim secret field (cloud PR): no way to "
    "deliver the env token to a containerDisk guest yet"
)


def namespace_prefix_ok(namespace: str, *prefixes: str) -> bool:
    """Live namespaces are `cua-live-*` (CI lanes) or `cua-e2e-*` (ad-hoc runs)."""
    return any(namespace.startswith(prefix) for prefix in prefixes)


async def check_service_reachable(sandbox: Any, service: str, summary: dict[str, Any]) -> None:
    """Daemon-agnostic guest check: the gateway routes to the named service."""
    response = await sandbox.services.request(service, method="GET", path="/")
    summary["service"] = {"name": service, "status": response.status_code}
    assert response.status_code < 500, f"service {service!r} answered HTTP {response.status_code}"


async def check_spacesd(
    sandbox: Any, summary: dict[str, Any], artifact_dir: Path, label: str
) -> None:
    """cua-spacesd guest check: every interface family answers."""
    width, height = await sandbox.screen.size()
    summary["screen"] = {"width": width, "height": height}
    assert width > 0 and height > 0

    screenshot = await sandbox.screenshot()
    artifact_dir.mkdir(parents=True, exist_ok=True)
    (artifact_dir / f"screen-{label}.png").write_bytes(screenshot)
    assert screenshot.startswith(b"\x89PNG\r\n\x1a\n")
    assert len(screenshot) > 1000

    result = await sandbox.shell.run("uname -s")
    summary["shell"] = {
        "success": result.success,
        "stdout": result.stdout.strip(),
        "stderr": result.stderr.strip(),
    }
    assert result.success
    assert result.stdout.strip() == "Linux"

    payload = os.urandom(65536)
    await sandbox.files.write_bytes("/tmp/cua-e2e-live.bin", payload)
    assert await sandbox.files.read_bytes("/tmp/cua-e2e-live.bin") == payload
    summary["files_round_trip"] = True


def assert_env_template_contract(template: Any) -> None:
    """Default (daemon-agnostic) template: `env` on 3211 and no guest probe."""
    vm_template = template.spec.vm_template
    services = {service.name: service.target_port for service in vm_template.services}
    assert services.get("env") == 3211, f"env service must target 3211, got {services}"
    assert vm_template.probes is None, "the default template must not probe a guest agent"


def assert_template_contract(template: Any, expected_port: int) -> None:
    vm_template = template.spec.vm_template
    server = next((service for service in vm_template.services if service.name == "server"), None)
    assert server is not None, "server service is required"
    assert (
        server.target_port == expected_port
    ), f"server target_port={server.target_port}, expected {expected_port}"
    # Readiness is client-side for managed pools (daemon-agnostic), so the
    # template may carry no probe; when it does, it must target the server port.
    if vm_template.probes is not None:
        probes = json.loads(vm_template.probes.to_json())
        observed = probes["readinessProbe"]["tcpSocket"]["port"]
        assert (
            observed == expected_port
        ), f"readiness probe port={observed}, expected {expected_port}"


def _is_sensitive_summary_key(key: object) -> bool:
    separated = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", str(key))
    normalized = re.sub(r"[^a-z0-9]+", "_", separated.lower()).strip("_")
    return (
        normalized in SENSITIVE_SUMMARY_KEY_NAMES
        or normalized.endswith("_api_key")
        or any(part in SENSITIVE_SUMMARY_KEY_PARTS for part in normalized.split("_"))
    )


def _redact_summary(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            key: "<redacted>" if _is_sensitive_summary_key(key) else _redact_summary(item)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_redact_summary(item) for item in value]
    if isinstance(value, tuple):
        return tuple(_redact_summary(item) for item in value)
    return value


def write_summary(path: Path, summary: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(_redact_summary(summary), indent=2, sort_keys=True) + "\n")
