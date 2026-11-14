#!/usr/bin/env python3
"""
Windows Arc PAYG license probe against a run.cua.ai Windows workspace pool.

End-to-end operational probe against the cua cloud, driven entirely through the
run.cua.ai backend with an OAuth client id/secret (no kubectl, no cluster VPN).

DEFAULT MODE — pre-baked licensed image (cua-server-windows-2025, a warm pool of
size 1). The image is already Windows Server 2025 full-edition + Arc-connected +
PAYG activated at build time, so the probe does no Azure work:

  1. provision the Windows workspace pool (replicas=1) if it does not exist
  2. claim a sandbox from the pool and wait for it to bind
  4. take a screenshot of the bound desktop (cua-computer SDK, through the proxy)
  5. upload the screenshot to S3 and mint a presigned URL
  6. POST the presigned URL to Alertmanager (am.cua.ai)
  7. return the claim to the pool (always runs)

RUNTIME-ONBOARDING MODE (the non-baked alternative) — set ARC_MACHINE +
AZURE_SUBSCRIPTION_ID and the probe also:

  3. attaches the Azure Arc PAYG Windows Server license (before the screenshot)
  7. deactivates the license (after), then returns the claim

Steps 1/2/4 talk to run.cua.ai. Authentication is the client_credentials
exchange handled by ``cua_train.TrainClient.from_key`` — set CUA_CLIENT_ID /
CUA_CLIENT_SECRET to a **per-user** key (``ukey-…``, from ``POST
/api/user-keys``); per-pool keys (``key-…``) have no K8s identity and are
rejected on the namespace/pool/claim steps.

Pool + claim CRs are created through the backend's ``/api/k8s`` kubectl-proxy:
the pool is an ``OSGymWorkspacePool`` (``cua.ai/v1``), the claim an
``OSGymSandboxClaim`` (``osgym.cua.ai/v1alpha1``). The bound sandbox is reached
through the authenticated ``/api/svc/{ns}/{sandbox}-{service}`` reverse proxy,
which the cua-computer SDK speaks to directly via ``api_base_url`` +
``api_headers`` (cua-computer>=0.5.19).

Steps 3/7 drive Azure Resource Manager through ``az rest`` against a Connected
Machine (``Microsoft.HybridCompute/machines/<machine>/licenseProfiles/default``).
The license toggle operates on a *pre-onboarded* Arc machine named by
``--arc-machine`` / ARC_MACHINE — that machine must already be Connected to Arc
and converted to a full (non-eval) Windows Server 2025 edition (one-time manual
setup, see the trycua/cloud onboarding runbook). If ARC_MACHINE is unset the
license steps are skipped and the probe just claims + screenshots.

Cleanup (step 7) runs in a finally block, so the license is disabled and the
claim returned even if an earlier step fails.

Quick start::

    pip install cua-train cua-computer httpx boto3 requests \
        --extra-index-url https://wheels.cua.ai/simple/
    export CUA_CLIENT_ID=ukey-xxxxxxxx CUA_CLIENT_SECRET=...
    export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
    export ARC_MACHINE=ws2025-trial AZURE_SUBSCRIPTION_ID=...   # optional license
    python scripts/windows_arc_license_probe.py --pool arc-windows
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import subprocess
import sys
import time
import uuid
from collections.abc import Callable
from datetime import datetime, timedelta, timezone

import boto3
import httpx
import requests
from cua_train import TrainClient

log = logging.getLogger("windows-arc-license-probe")

DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
DEFAULT_BASE_URL = "https://run.cua.ai"
# Windows computer-server image: Windows Server 2025 + computer_server on :8000
# (HTTP/WS API at /cmd, /ws, /status; MCP at /mcp), PRE-LICENSED via Azure Arc
# PAYG at build time (cua-server-windows-2025-workspace). dockur-built GPT/UEFI
# image, so the pool MUST boot it with efi firmware. Because the image is
# pre-licensed, the probe does NOT toggle licensing per run unless ARC_MACHINE
# is set (the runtime-onboarding alternative).
DEFAULT_IMAGE = "296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-server-windows-2025:latest"

# OSGymWorkspacePool — the single-object pool CR the run.cua.ai SPA creates
# (cua.ai/v1); the pool-operator compat shim projects it into the native
# OSGymSandboxTemplate + OSGymSandboxWarmPool pair.
POOL_GROUP, POOL_VERSION, POOL_PLURAL = "cua.ai", "v1", "osgymworkspacepools"
# OSGymSandboxTemplate / OSGymSandboxClaim live in the osgym.cua.ai group.
EXT_GROUP, EXT_VERSION = "osgym.cua.ai", "v1alpha1"
TEMPLATE_PLURAL, CLAIM_PLURAL = "osgymsandboxtemplates", "osgymsandboxclaims"


# --------------------------------------------------------------------------- #
# config
# --------------------------------------------------------------------------- #
def _env(name: str, default: str | None = None) -> str | None:
    return os.environ.get(name, default)


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.ArgumentDefaultsHelpFormatter
    )

    # run.cua.ai auth + pool/claim
    p.add_argument("--client-id", default=_env("CUA_CLIENT_ID"),
                   help="OAuth client id (per-user key 'ukey-…'); env CUA_CLIENT_ID")
    p.add_argument("--client-secret", default=_env("CUA_CLIENT_SECRET"),
                   help="OAuth client secret; env CUA_CLIENT_SECRET")
    p.add_argument("--token-url", default=_env("CUA_TOKEN_URL", DEFAULT_TOKEN_URL))
    p.add_argument("--base-url", default=_env("CUA_BASE_URL", DEFAULT_BASE_URL))
    p.add_argument("--pool", default=_env("CUA_POOL", "arc-windows"),
                   help="pool name (== namespace; lowercase/dashes)")
    p.add_argument("--image", default=_env("CUA_WINDOWS_IMAGE", DEFAULT_IMAGE))
    p.add_argument("--service-name", default=_env("CUA_SERVICE_NAME", "api"))
    p.add_argument("--port", type=int, default=int(_env("CUA_PORT", "8000")),
                   help="in-guest computer-server port (8000 for cua-server-windows)")
    p.add_argument("--firmware", choices=["bios", "efi"], default=_env("CUA_FIRMWARE", "efi"))
    p.add_argument("--cpu", type=int, default=int(_env("CUA_CPU", "4")))
    p.add_argument("--ram", default=_env("CUA_RAM", "8Gi"))
    p.add_argument("--replicas", type=int, default=int(_env("CUA_REPLICAS", "1")))
    p.add_argument("--warm-timeout", type=float, default=float(_env("CUA_WARM_TIMEOUT", "1200")),
                   help="seconds to wait for the pool's first warm VM (cold Windows boot is slow)")
    p.add_argument("--bind-deadline", type=int, default=int(_env("CUA_BIND_DEADLINE", "600")))
    p.add_argument("--bind-timeout", type=float, default=float(_env("CUA_BIND_TIMEOUT", "300")))
    p.add_argument("--ready-timeout", type=float, default=float(_env("CUA_READY_TIMEOUT", "420")))
    p.add_argument("--template-timeout", type=float, default=120)
    p.add_argument("--poll-interval", type=float, default=5)

    # Azure Arc licensing (optional — skipped unless ARC_MACHINE is set)
    p.add_argument("--subscription", default=_env("AZURE_SUBSCRIPTION_ID"))
    p.add_argument("--resource-group", default=_env("ARC_RESOURCE_GROUP", "arc-trial-rg"))
    p.add_argument("--region", default=_env("ARC_REGION", "eastus"))
    p.add_argument("--arc-machine", default=_env("ARC_MACHINE"),
                   help="pre-onboarded Arc Connected Machine to license (unset => skip)")
    p.add_argument("--arc-api-version", default=_env("ARC_API_VERSION", "2023-10-03-preview"))
    p.add_argument("--skip-license", action="store_true", default=_env("SKIP_LICENSE") == "1")

    # S3
    p.add_argument("--s3-bucket", default=_env("S3_BUCKET_NAME", "cua-agent-artifacts"))
    p.add_argument("--s3-region", default=_env("AWS_REGION", "us-west-2"))
    p.add_argument("--s3-prefix", default=_env("S3_PREFIX", "windows-arc-license-probe"))
    p.add_argument("--presign-ttl", type=int, default=int(_env("PRESIGN_TTL", "3600")))

    # Alertmanager
    p.add_argument("--alertmanager-url", default=_env("ALERTMANAGER_URL", "https://am.cua.ai"))
    p.add_argument("--severity", default=_env("ALERT_SEVERITY", "info"))

    p.add_argument("--claim-name", help="claim name (default: <pool>-claim-<random>)")
    p.add_argument("--delete-pool", action="store_true",
                   help="also delete the pool + namespace at the end (default: keep the pool)")
    p.add_argument("--keep-claim", action="store_true", help="do not delete the claim (debugging)")
    p.add_argument("-v", "--verbose", action="store_true")
    return p.parse_args(argv)


# --------------------------------------------------------------------------- #
# run.cua.ai /api/k8s URL builders (relative to base_url)
# --------------------------------------------------------------------------- #
def pool_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{POOL_GROUP}/{POOL_VERSION}/namespaces/{pool}/{POOL_PLURAL}"
    return f"{base}/{name}" if name else base


def template_url(pool: str, name: str) -> str:
    return f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/{pool}/{TEMPLATE_PLURAL}/{name}"


def claims_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/{pool}/{CLAIM_PLURAL}"
    return f"{base}/{name}" if name else base


# --------------------------------------------------------------------------- #
# step 1: provision pool (idempotent)
# --------------------------------------------------------------------------- #
def ensure_pool(http: httpx.Client, a: argparse.Namespace) -> None:
    # Namespace via the Capsule-backed SPA endpoint so tenant RBAC is stamped.
    r = http.post("/api/namespaces", json={"name": a.pool})
    if r.status_code == 403:
        sys.exit("403 creating namespace — use a per-USER key (ukey-…, POST /api/user-keys)")
    if r.status_code == 409:
        log.info("namespace %s already exists", a.pool)
    else:
        r.raise_for_status()
        log.info("namespace %s created", a.pool)

    template: dict = {
        "containerDiskImage": a.image,
        "imagePullSecret": "ecr-credentials",
        "cpuCores": a.cpu,
        "memory": a.ram,
    }
    if a.firmware != "bios":
        template["firmware"] = a.firmware
    # Gate Ready (and claim binding) on the computer-server port being bound.
    template["probes"] = {"readinessProbe": {"tcpSocket": {"port": a.port}}}

    body = {
        "apiVersion": f"{POOL_GROUP}/{POOL_VERSION}",
        "kind": "OSGymWorkspacePool",
        "metadata": {"name": a.pool, "labels": {"cua.ai/pool": a.pool}},
        "spec": {
            "replicas": a.replicas,
            "template": template,
            # Each entry becomes a per-sandbox Service "<sandbox>-<name>" mapping
            # Service port 80 -> targetPort on the VM.
            "services": [{"name": a.service_name, "targetPort": a.port, "protocol": "TCP"}],
        },
    }
    r = http.post(pool_url(a.pool), json=body)
    if r.status_code == 409:
        # Reuse an existing pool, but don't silently validate the wrong VM:
        # if the existing pool boots a different image, fail loudly. (We compare
        # only the containerDisk image — the server normalizes/defaults other
        # spec fields, so a full-spec equality check would misfire.)
        existing = http.get(pool_url(a.pool, a.pool))
        existing.raise_for_status()
        cur_image = (((existing.json().get("spec") or {}).get("template") or {})
                     .get("containerDiskImage"))
        if cur_image and cur_image != a.image:
            sys.exit(f"pool {a.pool} already exists with a different image "
                     f"({cur_image} != {a.image}) — refusing to reuse it")
        log.info("pool %s already exists with the expected image — reusing it", a.pool)
        return
    if r.status_code == 403:
        sys.exit("403 creating pool — use a per-USER key (ukey-…, POST /api/user-keys)")
    r.raise_for_status()
    log.info("pool %s created (image=%s firmware=%s replicas=%d)",
             a.pool, a.image, a.firmware, a.replicas)


def wait_template(get_http: Callable[[], httpx.Client], a: argparse.Namespace) -> None:
    tmpl = f"{a.pool}-template"
    deadline = time.monotonic() + a.template_timeout
    while True:
        r = get_http().get(template_url(a.pool, tmpl))
        if r.status_code == 200:
            log.info("pool-operator projected template %s + warm pool", tmpl)
            return
        if r.status_code not in (404, 403):
            r.raise_for_status()
        if time.monotonic() > deadline:
            log.info("template %s not visible after %.0fs — claiming anyway", tmpl, a.template_timeout)
            return
        time.sleep(a.poll_interval)


def pool_counts(get_http: Callable[[], httpx.Client], pool: str) -> tuple[int, int, str]:
    try:
        r = get_http().get(pool_url(pool, pool))
        r.raise_for_status()
        st = r.json().get("status") or {}
        return int(st.get("totalCount", 0)), int(st.get("availableCount", 0)), st.get("phase", "Unknown")
    except (httpx.TimeoutException, httpx.TransportError) as e:
        # Only swallow transient transport errors (retried by the caller's poll
        # loop). Auth/RBAC/server/JSON errors propagate so the probe fails fast
        # instead of looping to a misleading warm-pool timeout.
        log.warning("transient error reading pool status: %s", e)
        return 0, 0, "Unknown"


def wait_pool_warm(get_http: Callable[[], httpx.Client], a: argparse.Namespace) -> None:
    """Wait for availableCount>=1 before claiming so the claim binds in seconds.

    A cold Windows pool's first VM (UEFI image pull + Windows Server boot +
    computer-server task) takes minutes and would otherwise burn the claim's
    bind deadline.
    """
    start = time.monotonic()
    deadline = start + a.warm_timeout
    while True:
        total, avail, phase = pool_counts(get_http, a.pool)
        if avail >= 1:
            log.info("pool warm: %d/%d VM(s) ready (phase %s)", avail, total, phase)
            return
        if time.monotonic() > deadline:
            sys.exit(
                f"timed out after {a.warm_timeout:.0f}s waiting for a warm VM "
                f"(pool {phase}, {avail}/{total} ready). Most common cause: the image "
                f"tag does not exist so the VM sits in ImagePullBackOff."
            )
        log.info("[%4ds] pool=%s (%d/%d ready) — warming...",
                 int(time.monotonic() - start), phase, avail, total)
        time.sleep(a.poll_interval)


# --------------------------------------------------------------------------- #
# step 2: claim + bind
# --------------------------------------------------------------------------- #
def create_claim(http: httpx.Client, a: argparse.Namespace, name: str) -> None:
    spec: dict = {"sandboxTemplateRef": {"name": f"{a.pool}-template"}}
    if a.bind_deadline:
        spec["bindDeadline"] = a.bind_deadline
    r = http.post(
        claims_url(a.pool),
        json={
            "apiVersion": f"{EXT_GROUP}/{EXT_VERSION}",
            "kind": "OSGymSandboxClaim",
            "metadata": {"name": name},
            "spec": spec,
        },
    )
    if r.status_code == 403:
        sys.exit("403 creating claim — use a per-USER key (ukey-…, POST /api/user-keys)")
    r.raise_for_status()
    log.info("claim %s created", name)


def wait_bound(get_http: Callable[[], httpx.Client], a: argparse.Namespace, name: str) -> str:
    start = time.monotonic()
    deadline = start + a.bind_timeout
    while True:
        r = get_http().get(claims_url(a.pool, name))
        r.raise_for_status()
        status = r.json().get("status") or {}
        phase = status.get("phase", "Pending")
        sandbox = (status.get("sandbox") or {}).get("name")
        if phase == "Bound" and sandbox:
            log.info("claim %s bound to sandbox %s", name, sandbox)
            return sandbox
        if phase == "Failed":
            sys.exit(f"claim {name} failed: {status}")
        if time.monotonic() > deadline:
            sys.exit(f"timed out after {a.bind_timeout:.0f}s waiting for claim {name} (phase={phase})")
        log.info("[%4ds] claim=%s ...", int(time.monotonic() - start), phase)
        time.sleep(a.poll_interval)


def wait_service_ready(get_http: Callable[[], httpx.Client], url: str, a: argparse.Namespace) -> None:
    """GET url until the in-guest service answers (not a proxy 502/503/504).

    For computer-server that endpoint is /status; the guest may still be
    booting for a few minutes after the claim binds.
    """
    start = time.monotonic()
    deadline = start + a.ready_timeout
    while True:
        try:
            resp = get_http().get(url, timeout=15.0)
            reason = f"HTTP {resp.status_code}"
            # Require a real 2xx from /status — a persistent 401/404/500 means
            # broken auth, a wrong service path, or a failed computer-server, and
            # should fail at readiness rather than feed a broken VM to the
            # screenshot step. 502/503/504 are the normal "guest still booting"
            # proxy codes and keep polling.
            ok = 200 <= resp.status_code < 300
            if not ok and resp.status_code not in (502, 503, 504):
                reason = f"HTTP {resp.status_code}: {resp.text[:200]}"
        except httpx.HTTPError as e:
            reason, ok = type(e).__name__, False
        if ok:
            log.info("service responding (%s)", reason)
            return
        if time.monotonic() > deadline:
            sys.exit(f"timed out after {a.ready_timeout:.0f}s waiting for the in-guest service ({reason})")
        log.info("[%4ds] %s — guest still booting...", int(time.monotonic() - start), reason)
        time.sleep(a.poll_interval)


# --------------------------------------------------------------------------- #
# step 3 / 7: Azure Arc PAYG license
# --------------------------------------------------------------------------- #
def _license_url(a: argparse.Namespace) -> str:
    return (
        f"https://management.azure.com/subscriptions/{a.subscription}"
        f"/resourceGroups/{a.resource_group}"
        f"/providers/Microsoft.HybridCompute/machines/{a.arc_machine}"
        f"/licenseProfiles/default?api-version={a.arc_api_version}"
    )


def _az_rest(method: str, url: str, body: dict | None = None) -> dict:
    cmd = ["az", "rest", "--method", method, "--url", url]
    if body is not None:
        cmd += ["--body", json.dumps(body)]
    log.debug("$ %s", " ".join(cmd))
    try:
        cp = subprocess.run(cmd, text=True, capture_output=True, check=True, timeout=120)
    except subprocess.TimeoutExpired as e:
        # An az CLI auth/network hang would otherwise stall the probe until the
        # workflow timeout and delay cleanup/alerting — fail fast instead.
        raise RuntimeError(f"az rest timed out after {e.timeout}s") from e
    out = (cp.stdout or "").strip()
    return json.loads(out) if out else {}


def set_license(a: argparse.Namespace, *, enabled: bool) -> None:
    status = "Enabled" if enabled else "Disabled"
    log.info("setting Arc PAYG license on %s -> %s", a.arc_machine, status)
    _az_rest("put", _license_url(a), {
        "location": a.region,
        "properties": {"productProfile": {"productType": "WindowsServer", "subscriptionStatus": status}},
    })


def license_status(a: argparse.Namespace) -> str:
    try:
        resp = _az_rest("get", _license_url(a))
        return resp.get("properties", {}).get("productProfile", {}).get("subscriptionStatus", "")
    except Exception as e:  # noqa: BLE001
        log.warning("could not read license status: %s", e)
        return ""


# --------------------------------------------------------------------------- #
# step 4: screenshot via the cua-computer SDK through the run.cua.ai proxy
# --------------------------------------------------------------------------- #
async def take_screenshot(api_base_url: str, auth: str) -> bytes:
    # Imported lazily so --help / arg errors don't pay the cua-computer import.
    from computer import Computer

    log.info("connecting cua-computer SDK to %s", api_base_url)
    computer = Computer(
        use_host_computer_server=True,  # connect to an existing server, don't spawn a VM
        os_type="windows",
        api_base_url=api_base_url,
        api_headers={"Authorization": auth},
    )
    async with computer:
        size = await computer.interface.get_screen_size()
        log.info("screen size: %sx%s", size["width"], size["height"])
        png = await computer.interface.screenshot()
    if png[:4] != b"\x89PNG":
        log.warning("screenshot is not a PNG (got %r)", png[:8])
    log.info("captured screenshot (%d bytes)", len(png))
    return png


# --------------------------------------------------------------------------- #
# step 5: S3 upload + presigned URL
# --------------------------------------------------------------------------- #
def upload_and_presign(a: argparse.Namespace, png: bytes, sandbox: str) -> str:
    ts = datetime.now(tz=timezone.utc).strftime("%Y-%m-%d/%H%M%SZ")
    key = f"{a.s3_prefix}/{ts}/{sandbox}.png"
    s3 = boto3.client("s3", region_name=a.s3_region)
    s3.put_object(Bucket=a.s3_bucket, Key=key, Body=png, ContentType="image/png")
    url = s3.generate_presigned_url(
        "get_object", Params={"Bucket": a.s3_bucket, "Key": key}, ExpiresIn=a.presign_ttl
    )
    log.info("uploaded s3://%s/%s and signed URL (ttl=%ds)", a.s3_bucket, key, a.presign_ttl)
    return url


# --------------------------------------------------------------------------- #
# step 6: post to Alertmanager (am.cua.ai)
# --------------------------------------------------------------------------- #
def post_alert(a: argparse.Namespace, *, signed_url: str, sandbox: str, lic: str) -> None:
    now = datetime.now(tz=timezone.utc)
    alert = {
        "labels": {
            "alertname": "WindowsArcLicenseProbe",
            "severity": a.severity,
            "service": "cua-sdk",
            "job": "windows-arc-license-probe",
            "pool": a.pool,
            "sandbox": sandbox,
        },
        "annotations": {
            "summary": f"Windows Arc license probe captured {sandbox}",
            "description": (
                f"Bound sandbox {sandbox} from pool {a.pool}.\n"
                f"Arc machine: {a.arc_machine or '(none)'} license={lic or 'n/a'}.\n"
                f"Screenshot (presigned, expires in {a.presign_ttl}s): {signed_url}"
            ),
            "screenshot_url": signed_url,
            "dashboard": "https://grafana.cua.ai",
        },
        "startsAt": now.isoformat(),
        "endsAt": (now + timedelta(minutes=30)).isoformat(),
    }
    resp = requests.post(
        f"{a.alertmanager_url}/api/v2/alerts",
        data=json.dumps([alert]),
        headers={"Content-Type": "application/json"},
        timeout=15,
    )
    if resp.status_code != 200:
        raise RuntimeError(f"alertmanager returned {resp.status_code}: {resp.text}")
    log.info("posted alert to %s/api/v2/alerts", a.alertmanager_url)


# --------------------------------------------------------------------------- #
# orchestration
# --------------------------------------------------------------------------- #
async def amain(a: argparse.Namespace) -> int:
    if not a.client_id or not a.client_secret:
        sys.exit("set CUA_CLIENT_ID / CUA_CLIENT_SECRET (per-user key, POST /api/user-keys)")
    if a.client_id.startswith("key-"):
        sys.exit("that is a per-POOL key ('key-…') with no K8s identity — use a per-USER key ('ukey-…')")

    do_license = not a.skip_license and bool(a.arc_machine) and bool(a.subscription)
    if not a.skip_license and a.arc_machine and not a.subscription:
        log.warning("ARC_MACHINE set but AZURE_SUBSCRIPTION_ID missing — skipping license steps")

    client = TrainClient.from_key(
        token_url=a.token_url, client_id=a.client_id,
        client_secret=a.client_secret, base_url=a.base_url,
    )
    # get_httpx_client() re-mints the bearer near expiry — fetch it per request.
    http = client.get_httpx_client
    log.info("authenticated to %s as %s", a.base_url, a.client_id)

    # Step 1
    ensure_pool(http(), a)
    wait_template(http, a)
    wait_pool_warm(http, a)

    # Step 2
    claim_name = a.claim_name or f"{a.pool}-claim-{uuid.uuid4().hex[:10]}"
    create_claim(http(), a, claim_name)

    licensed = False
    try:
        sandbox = wait_bound(http, a, claim_name)
        svc_path = f"/api/svc/{a.pool}/{sandbox}-{a.service_name}"
        wait_service_ready(http, f"{svc_path}/status", a)

        # Step 3
        if do_license:
            set_license(a, enabled=True)
            licensed = True

        # Step 4 — fresh bearer for the SDK (proxy re-checks it per request).
        client.get_httpx_client()  # forces a token refresh
        auth = f"Bearer {client.token}"
        png = await take_screenshot(f"{a.base_url}{svc_path}", auth)

        # Step 5 & 6
        signed_url = upload_and_presign(a, png, sandbox)
        post_alert(a, signed_url=signed_url, sandbox=sandbox,
                   lic=license_status(a) if do_license else "")
        log.info("probe succeeded: %s", signed_url)
        return 0
    finally:
        # Step 7 — always runs.
        if licensed:
            try:
                set_license(a, enabled=False)
            except Exception:  # noqa: BLE001
                log.exception("failed to disable license on %s", a.arc_machine)
        if not a.keep_claim:
            try:
                r = http().delete(claims_url(a.pool, claim_name))
                log.info("deleted claim %s (returned to pool, HTTP %s)", claim_name, r.status_code)
            except httpx.HTTPError as e:
                log.warning("failed to delete claim %s: %s", claim_name, e)
        if a.delete_pool:
            for what, url in ((f"pool {a.pool}", pool_url(a.pool, a.pool)),
                              (f"namespace {a.pool}", f"/api/namespaces/{a.pool}")):
                try:
                    r = http().delete(url)
                    log.info("deleted %s (HTTP %s)", what, r.status_code)
                except httpx.HTTPError as e:
                    log.warning("could not delete %s: %s", what, e)


def main(argv: list[str]) -> int:
    a = parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if a.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    return asyncio.run(amain(a))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
