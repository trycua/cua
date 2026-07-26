#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "cua-train",
#     "httpx",
# ]
#
# [tool.uv]
# extra-index-url = ["https://wheels.cua.ai/simple/"]
# ///
"""Provision a **Windows computer-server** pool on cyclops-cs / run.cua.ai, claim
a sandbox from it, list the MCP tools it exposes — then tear the whole thing back
down. End to end, from nothing but a client id/secret.

This is the MCP companion to ``provision_windows_and_drive_with_cua_sdk.py``
(which claims the same kind of sandbox but drives it with the ``cua-computer``
SDK instead). Both target the Windows ``cua-server-windows`` image, which runs
``python -m computer_server`` on :8000 and exposes, on that one port:

  * the HTTP/WebSocket computer-use API (``/cmd``, ``/ws``, ``/status``)
  * an MCP server over Streamable HTTP at ``/mcp``

Here we only do the MCP side: the Streamable-HTTP handshake
(``initialize`` → ``notifications/initialized`` → ``tools/list``) and print the
advertised tools.

Lifecycle (every step is narrated so you can follow along in the console):

  1. exchange the client_credentials for a bearer token (``TrainClient.from_key``)
  2. create the per-user namespace (``POST /api/namespaces`` — same path the SPA
     uses, so Capsule sets up tenant ownership + RBAC; pool name == namespace)
  3. create an ``OSGymWorkspacePool`` (``cua.ai/v1``) via the ``/api/k8s``
     kubectl-proxy — the Windows image, **efi firmware**, and a service on :8000.
     The pool-operator's compat shim projects it into an ``OSGymSandboxTemplate``
     + ``OSGymSandboxWarmPool`` pair and warms the VM.
  4. claim a sandbox (``OSGymSandboxClaim``) and wait for it to bind
  5. reach the bound sandbox through ``/api/svc/{ns}/{sandbox}-{service}/`` and
     run the computer-server MCP handshake (initialize -> tools/list)
  6. **tear it all down** — delete the claim, the pool, and the namespace. This
     is a throwaway test scenario, so nothing is left billing. Pass ``--keep``
     to leave the pool up for poking at.

Auth — use a per-USER key (``ukey-...``), NOT a per-pool key:
``/api/k8s`` impersonates the token's owner so Capsule tenant RBAC applies.
Per-user keys (POST /api/user-keys) can create namespaces, pools, and claims;
per-pool keys (POST /api/keys, ``key-...``) have no K8s identity and are rejected.

Heads up: Windows is **slow to first-boot** (UEFI image pull + Windows Server
boot + computer-server scheduled task coming up). The defaults wait generously;
raise ``--warm-timeout`` if your pull is cold.

Setup (zero-install with uv)::

    export CUA_CLIENT_ID=ukey-xxxxxxxx      # from POST /api/user-keys
    export CUA_CLIENT_SECRET=...
    uv run provision_windows_and_list_mcp_tools.py --name win-mcp-demo

If your uv is too old to read the inline extra-index-url, run it the explicit
way::

    uv run --with cua-train --with httpx \
        --extra-index-url https://wheels.cua.ai/simple/ \
        provision_windows_and_list_mcp_tools.py --name win-mcp-demo

Env knobs: CUA_CLIENT_ID, CUA_CLIENT_SECRET, CUA_TOKEN_URL, CUA_BASE_URL.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import uuid
from collections.abc import Callable

import httpx

from cua_train import TrainClient

DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
DEFAULT_BASE_URL = "https://run.cua.ai"
# The Windows computer-server image: Windows Server 2022 + cua-computer-server on
# :8000 (HTTP/WS API at /cmd//ws, MCP at /mcp). It is a dockur-built GPT/UEFI
# image, so the pool MUST boot it with efi firmware (see --firmware below).
DEFAULT_IMAGE = "296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-server-windows:latest"

# OSGymWorkspacePool — the legacy single-object pool CR the cyclops-cs SPA
# creates (cua.ai/v1). The pool-operator compat shim translates it into the
# native OSGymSandboxTemplate + OSGymSandboxWarmPool pair.
POOL_GROUP = "cua.ai"
POOL_VERSION = "v1"
POOL_PLURAL = "osgymworkspacepools"

# OSGymSandboxTemplate / OSGymSandboxClaim live in the osgym.cua.ai group.
EXT_GROUP = "osgym.cua.ai"
EXT_VERSION = "v1alpha1"
TEMPLATE_PLURAL = "osgymsandboxtemplates"
CLAIM_PLURAL = "osgymsandboxclaims"


# ── URL builders (relative to base_url, e.g. https://run.cua.ai) ──────────


def pool_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{POOL_GROUP}/{POOL_VERSION}/namespaces/{pool}/{POOL_PLURAL}"
    return f"{base}/{name}" if name else base


def template_url(pool: str, name: str) -> str:
    return f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/{pool}/{TEMPLATE_PLURAL}/{name}"


def claims_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/{pool}/{CLAIM_PLURAL}"
    return f"{base}/{name}" if name else base


# ── Console helpers ───────────────────────────────────────────────────────


def step(msg: str) -> None:
    print(f"\n==> {msg}", flush=True)


def info(msg: str) -> None:
    print(f"    {msg}", flush=True)


def die(msg: str) -> None:
    sys.exit(f"\nERROR: {msg}")


# ── Lifecycle steps ───────────────────────────────────────────────────────


def create_namespace(http: httpx.Client, name: str) -> None:
    """Create the per-user namespace via the Capsule-backed SPA endpoint.

    Pool name == namespace name (1:1). Going through /api/namespaces (rather
    than a raw /api/k8s namespace create) is what makes Capsule stamp tenant
    ownership + RBAC, so the impersonated pool create below is allowed. A 409
    just means we are reusing a namespace from an earlier run.
    """
    r = http.post("/api/namespaces", json={"name": name})
    if r.status_code == 409:
        info(f"namespace {name!r} already exists — reusing it")
        return
    if r.status_code == 403:
        die(
            "403 creating namespace — is this a per-pool key? Namespaces/pools/"
            "claims need a per-USER key (client id starting with 'ukey-', from "
            "POST /api/user-keys)."
        )
    r.raise_for_status()
    info(f"namespace {name!r} created")


def create_pool(
    http: httpx.Client,
    *,
    pool: str,
    image: str,
    cpu: int,
    ram: str,
    replicas: int,
    firmware: str,
    service_name: str,
    port: int,
    readiness_port: int,
) -> None:
    """Create the OSGymWorkspacePool. Mirrors cyclops-cs api.createPool exactly."""
    template: dict = {
        "containerDiskImage": image,
        "imagePullSecret": "ecr-credentials",
        "cpuCores": cpu,
        "memory": ram,
    }
    # The SPA only sends firmware when it differs from KubeVirt's bios default.
    # Windows needs efi: the VM builder then auto-applies the Hyper-V
    # enlightenments + Windows-tuned clocks (vm_builders.vm_body, firmware=="efi").
    if firmware != "bios":
        template["firmware"] = firmware
    # Gate "Ready" (and therefore claim binding) on the computer-server port
    # being bound. Without a probe the VM is advertised ready ~30s before the
    # guest finishes booting (CUA-535) — doubly important for slow Windows boots.
    # readiness_port == 0 disables the probe (KubeVirt's launcher-up default).
    if readiness_port:
        template["probes"] = {"readinessProbe": {"tcpSocket": {"port": readiness_port}}}

    body = {
        "apiVersion": f"{POOL_GROUP}/{POOL_VERSION}",
        "kind": "OSGymWorkspacePool",
        "metadata": {"name": pool, "labels": {"cua.ai/pool": pool}},
        "spec": {
            "replicas": replicas,
            "template": template,
            # Each entry becomes a per-sandbox K8s Service "<sandbox>-<name>"
            # mapping port 80 -> targetPort on the VM.
            "services": [
                {"name": service_name, "targetPort": port, "protocol": "TCP"}
            ],
        },
    }

    r = http.post(pool_url(pool), json=body)
    if r.status_code == 409:
        info(f"pool {pool!r} already exists — reusing it (services/replicas unchanged)")
        return
    if r.status_code == 403:
        die(
            "403 creating pool — per-pool keys cannot reach /api/k8s. Use a "
            "per-USER key (client id starting with 'ukey-', from POST /api/user-keys)."
        )
    r.raise_for_status()
    info(f"pool {pool!r} created")
    info(f"image:     {image}")
    info(f"size:      {cpu} vCPU / {ram} / replicas={replicas} / firmware={firmware}")
    info(f"service:   {service_name} -> targetPort {port} (reachable on Service port 80)")
    info(f"readiness: {'tcpSocket :' + str(readiness_port) if readiness_port else 'launcher-up (no probe)'}")


def wait_template(get_http: Callable[[], httpx.Client], pool: str, timeout: float, poll: float) -> None:
    """Wait for the compat shim to project the pool into <pool>-template.

    The claim below references the template by name; waiting for it to exist
    keeps the claim from sitting Pending purely because the projection hasn't
    happened yet. Best-effort: if it never appears we still try the claim.

    Takes the ``http`` accessor (not a client) and re-fetches it each poll so
    the bearer token is re-minted as it nears expiry.
    """
    tmpl = f"{pool}-template"
    deadline = time.monotonic() + timeout
    while True:
        r = get_http().get(template_url(pool, tmpl))
        if r.status_code == 200:
            info(f"pool-operator projected template {tmpl!r} + warm pool {pool!r}")
            return
        if r.status_code not in (404, 403):
            r.raise_for_status()
        if time.monotonic() > deadline:
            info(f"template {tmpl!r} not visible after {timeout:.0f}s — claiming anyway")
            return
        info(f"waiting for pool-operator to project template {tmpl!r} …")
        time.sleep(poll)


def pool_counts(get_http: Callable[[], httpx.Client], pool: str) -> tuple[int, int, str]:
    """(totalCount, availableCount, phase) from the pool status, best-effort."""
    try:
        r = get_http().get(pool_url(pool, pool))
        r.raise_for_status()
        st = r.json().get("status") or {}
        return (
            int(st.get("totalCount", 0)),
            int(st.get("availableCount", 0)),
            st.get("phase", "Unknown"),
        )
    except Exception:
        return (0, 0, "Unknown")


def wait_pool_warm(get_http: Callable[[], httpx.Client], pool: str, timeout: float, poll: float) -> None:
    """Wait until the pool reports an available (warm, Ready) VM.

    This is the key to a reliable first claim: a brand-new pool has to pull the
    (multi-GB) containerDisk image, boot the guest, and bring the in-guest
    service up on the readiness port before a Sandbox becomes adoptable. For the
    Windows image that cold start is *minutes* (UEFI image pull + Windows Server
    boot + the computer-server scheduled task), and routinely exceeds the
    operator's 300s *claim* bind deadline (claim_handlers.BIND_DEADLINE_DEFAULT_S).
    If we claim first, the deadline burns down during the image pull and the
    claim Fails with ``BindDeadlineExceeded`` — exactly the failure mode this
    avoids.

    So we let the warm pool reach availableCount>=1 (phase "Ready") *before*
    claiming; the subsequent claim then binds in seconds. Re-fetches the
    ``http`` accessor each poll to keep the bearer token fresh over a long pull.
    """
    start = time.monotonic()
    deadline = start + timeout
    while True:
        total, avail, phase = pool_counts(get_http, pool)
        if avail >= 1:
            info(f"pool warm: {avail}/{total} VM(s) ready (phase {phase})")
            return
        if time.monotonic() > deadline:
            die(f"timed out after {timeout:.0f}s waiting for a warm VM "
                f"(pool {phase}, {avail}/{total} ready).\n"
                f"  Most common cause: the --image tag does not exist in the registry, "
                f"so the VM sits in ImagePullBackOff and never boots. List real tags with:\n"
                f"    aws ecr describe-images --repository-name cua-server-windows --region us-west-2\n"
                f"  Less commonly the cold Windows boot just needs longer — raise --warm-timeout.")
        elapsed = int(time.monotonic() - start)
        info(f"[{elapsed:4d}s] pool={phase} ({avail}/{total} VMs ready) — warming "
             f"(image pull + Windows boot + computer-server on the readiness port) …")
        time.sleep(poll)


def create_claim(http: httpx.Client, pool: str, name: str, bind_deadline: int = 0) -> None:
    spec: dict = {"sandboxTemplateRef": {"name": f"{pool}-template"}}
    # The pool-operator fails an unbound claim after spec.bindDeadline seconds
    # (default 300, claim_handlers.BIND_DEADLINE_DEFAULT_S). We claim only after
    # the pool is already warm, so binding is near-instant — but a non-zero
    # deadline is a safety margin in case the warm VM is mid hard-reset.
    if bind_deadline:
        spec["bindDeadline"] = bind_deadline
    r = http.post(
        claims_url(pool),
        json={
            "apiVersion": f"{EXT_GROUP}/{EXT_VERSION}",
            "kind": "OSGymSandboxClaim",
            "metadata": {"name": name},
            "spec": spec,
        },
    )
    if r.status_code == 403:
        die(
            "403 creating claim — per-pool keys have no K8s identity. Use a "
            "per-USER key (client id starting with 'ukey-', from POST /api/user-keys)."
        )
    r.raise_for_status()
    info(f"claim {name!r} created")


def wait_bound(get_http: Callable[[], httpx.Client], pool: str, name: str, timeout: float, poll: float) -> str:
    """Poll the claim until status.phase == Bound; return the sandbox name.

    While we wait we also surface the pool's warm-VM counts, so a cold start is
    visible rather than a silent hang. Re-fetches the ``http`` accessor each poll
    so the bearer token stays fresh across a long (cold-boot) wait.
    """
    start = time.monotonic()
    deadline = start + timeout
    while True:
        r = get_http().get(claims_url(pool, name))
        r.raise_for_status()
        status = r.json().get("status") or {}
        phase = status.get("phase", "Pending")
        sandbox = (status.get("sandbox") or {}).get("name")
        if phase == "Bound" and sandbox:
            return sandbox
        if phase == "Failed":
            die(f"claim {name} failed: {status}")
        if time.monotonic() > deadline:
            die(f"timed out after {timeout:.0f}s waiting for claim {name} to bind (phase={phase})")
        total, avail, pphase = pool_counts(get_http, pool)
        elapsed = int(time.monotonic() - start)
        info(f"[{elapsed:4d}s] claim={phase}  pool={pphase} ({avail}/{total} VMs ready) …")
        time.sleep(poll)


def wait_service_ready(get_http: Callable[[], httpx.Client], url: str, timeout: float, poll: float) -> None:
    """GET url until the in-guest service answers (not a proxy 502/503/504).

    Backstop for the slim window where the sandbox is Bound but the in-guest
    HTTP server hasn't finished accepting connections yet. For computer-server
    that endpoint is ``/status``. Re-fetches the ``http`` accessor each poll to
    keep the bearer token fresh.
    """
    start = time.monotonic()
    deadline = start + timeout
    while True:
        try:
            resp = get_http().get(url, timeout=15.0)
        except httpx.HTTPError as e:
            resp = None
            reason = type(e).__name__
        else:
            reason = f"HTTP {resp.status_code}"
        if resp is not None and resp.status_code not in (502, 503, 504):
            info(f"service responding ({reason})")
            return
        if time.monotonic() > deadline:
            die(f"timed out after {timeout:.0f}s waiting for the in-guest service (last: {reason})")
        elapsed = int(time.monotonic() - start)
        info(f"[{elapsed:4d}s] {reason} — guest still booting …")
        time.sleep(poll)


def list_mcp_tools(http: httpx.Client, base: str, mcp_path: str) -> None:
    """computer-server MCP handshake over Streamable HTTP:
    initialize -> notifications/initialized -> tools/list, then print the tools."""
    url = f"{base}/{mcp_path.lstrip('/')}"
    headers = {
        "Content-Type": "application/json",
        # Streamable HTTP requires advertising both content types.
        "Accept": "application/json, text/event-stream",
    }

    r = http.post(
        url,
        headers=headers,
        timeout=30.0,
        json={
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "provision_windows_and_list_mcp_tools", "version": "0.1.0"},
            },
        },
    )
    r.raise_for_status()
    session = r.headers.get("mcp-session-id")
    info(f"MCP initialize: HTTP {r.status_code}" + (f", session {session}" if session else ""))
    if session:
        headers["Mcp-Session-Id"] = session

    # Notify the server we're ready (no response expected).
    http.post(
        url,
        headers=headers,
        timeout=30.0,
        json={"jsonrpc": "2.0", "method": "notifications/initialized"},
    )

    r = http.post(
        url,
        headers=headers,
        timeout=30.0,
        json={"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    )
    r.raise_for_status()
    # Streamable HTTP returns SSE ("event: message\ndata: {...}"); the JSON
    # payload is on the data: line. Fall back to the raw body for plain JSON.
    data = next((ln[6:] for ln in r.text.splitlines() if ln.startswith("data: ")), r.text)
    tools = json.loads(data).get("result", {}).get("tools", [])

    print(f"\nMCP tools ({len(tools)}):", flush=True)
    for i, t in enumerate(sorted(tools, key=lambda t: t.get("name", "")), 1):
        name = t.get("name", "?")
        desc = " ".join((t.get("description") or "").split())
        if len(desc) > 70:
            desc = desc[:67] + "…"
        print(f"  {i:3d}. {name:<24} {desc}", flush=True)
    if not tools:
        print("  (the server returned an empty tool list)", flush=True)


def teardown(get_http: Callable[[], httpx.Client], pool: str, claim_name: str) -> None:
    """Best-effort full cleanup: delete the claim, the pool, then the namespace.

    Idempotent — a 404 just means something was already gone. This is a test
    scenario, so we remove everything we created and leave nothing billing.
    """
    for what, url in (
        (f"claim {claim_name!r}", claims_url(pool, claim_name)),
        (f"pool {pool!r}", pool_url(pool, pool)),
        (f"namespace {pool!r}", f"/api/namespaces/{pool}"),
    ):
        try:
            r = get_http().delete(url)
            info(f"deleted {what} (HTTP {r.status_code})")
        except httpx.HTTPError as e:
            info(f"could not delete {what}: {type(e).__name__}: {e}")


# ── Main ──────────────────────────────────────────────────────────────────


def main() -> None:
    p = argparse.ArgumentParser(
        description="Provision a Windows computer-server pool on run.cua.ai, claim a sandbox, "
                    "list its MCP tools, then tear it all down.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("--name", required=True, help="pool name (also the namespace; lowercase/dashes)")
    p.add_argument("--client-id", default=os.environ.get("CUA_CLIENT_ID"),
                   help="OAuth client id (per-user key 'ukey-…'); env CUA_CLIENT_ID")
    p.add_argument("--client-secret", default=os.environ.get("CUA_CLIENT_SECRET"),
                   help="OAuth client secret; env CUA_CLIENT_SECRET")
    p.add_argument("--image", default=DEFAULT_IMAGE, help="Windows workspace containerDisk image")
    p.add_argument("--port", type=int, default=8000, help="computer-server port on the VM")
    p.add_argument("--service-name", default="api", help="name for the per-sandbox Service")
    p.add_argument("--mcp-path", default="/mcp", help="path the MCP server is mounted at")
    p.add_argument("--replicas", type=int, default=1, help="pre-warmed VMs the pool keeps")
    p.add_argument("--cpu", type=int, default=4, help="vCPU cores per VM")
    p.add_argument("--ram", default="8Gi", help="memory per VM (K8s quantity; Windows wants >=8Gi)")
    p.add_argument("--firmware", choices=["bios", "efi"], default="efi",
                   help="VM firmware (efi for the GPT/UEFI-only Windows image)")
    p.add_argument("--readiness-port", type=int, default=None,
                   help="TCP readiness port (default: --port; 0 disables the probe)")
    p.add_argument("--token-url", default=os.environ.get("CUA_TOKEN_URL", DEFAULT_TOKEN_URL),
                   help="OIDC token endpoint")
    p.add_argument("--base-url", default=os.environ.get("CUA_BASE_URL", DEFAULT_BASE_URL),
                   help="cyclops-cs base URL")
    p.add_argument("--claim-name", help="claim name (default: claim-<random>)")
    p.add_argument("--template-timeout", type=float, default=120,
                   help="seconds to wait for the pool->template projection")
    p.add_argument("--warm-timeout", type=float, default=1200,
                   help="seconds to wait for the pool's first warm VM before claiming "
                        "(covers the cold image pull + slow Windows boot)")
    p.add_argument("--bind-deadline", type=int, default=600,
                   help="claim spec.bindDeadline (operator fails an unbound claim after "
                        "this; 0 uses the operator default of 300s)")
    p.add_argument("--bind-timeout", type=float, default=300,
                   help="seconds the script waits for the claim to bind (fast once warm)")
    p.add_argument("--ready-timeout", type=float, default=420,
                   help="seconds to wait for the in-guest service after binding")
    p.add_argument("--poll-interval", type=float, default=5,
                   help="seconds between status polls")
    p.add_argument("--keep", action="store_true",
                   help="skip teardown — leave the pool, namespace, and claim up for debugging")
    args = p.parse_args()

    if not args.client_id or not args.client_secret:
        die("set --client-id/--client-secret or CUA_CLIENT_ID/CUA_CLIENT_SECRET "
            "(per-user key from POST /api/user-keys)")
    if args.client_id.startswith("key-"):
        die("that looks like a per-POOL key ('key-…'), which has no K8s identity. "
            "Creating namespaces/pools/claims needs a per-USER key ('ukey-…', "
            "from POST /api/user-keys).")

    readiness_port = args.port if args.readiness_port is None else args.readiness_port
    pool = args.name
    claim_name = args.claim_name or f"claim-{uuid.uuid4().hex[:8]}"

    step(f"Authenticating to {args.base_url} (client {args.client_id})")
    client = TrainClient.from_key(
        token_url=args.token_url,
        client_id=args.client_id,
        client_secret=args.client_secret,
        base_url=args.base_url,
    )
    # get_httpx_client() re-mints the bearer token near expiry, so fetch it
    # per request rather than holding one reference.
    http = client.get_httpx_client
    info("token acquired")

    step(f"Creating namespace {pool!r}")
    create_namespace(http(), pool)

    step(f"Creating pool {pool!r}")
    create_pool(
        http(),
        pool=pool,
        image=args.image,
        cpu=args.cpu,
        ram=args.ram,
        replicas=args.replicas,
        firmware=args.firmware,
        service_name=args.service_name,
        port=args.port,
        readiness_port=readiness_port,
    )

    step("Waiting for the pool-operator to project the template + warm pool")
    wait_template(http, pool, args.template_timeout, args.poll_interval)

    step("Waiting for the pool's first VM to warm up (so the claim binds fast)")
    info("(cold start = containerDisk image pull + Windows boot + computer-server on the readiness port)")
    wait_pool_warm(http, pool, args.warm_timeout, args.poll_interval)

    step(f"Claiming a sandbox from {pool!r} (claim {claim_name!r})")
    create_claim(http(), pool, claim_name, args.bind_deadline)

    try:
        step("Waiting for the claim to bind to the warm sandbox")
        sandbox = wait_bound(http, pool, claim_name, args.bind_timeout, args.poll_interval)
        info(f"bound to sandbox {sandbox!r}")

        # Per-sandbox Service is "<sandbox>-<service>" on port 80 -> targetPort.
        svc_base = f"/api/svc/{pool}/{sandbox}-{args.service_name}"

        step("Waiting for the in-guest computer-server to answer")
        wait_service_ready(http, f"{svc_base}/status", args.ready_timeout, args.poll_interval)

        step(f"Listing MCP tools at {svc_base}{args.mcp_path}")
        list_mcp_tools(http(), svc_base, args.mcp_path)
    finally:
        print(flush=True)
        if args.keep:
            step(f"Keeping infra ({pool!r}) — skipping teardown (--keep)")
            info(f"claim:     {claims_url(pool, claim_name)}")
            info(f"pool:      {pool_url(pool, pool)}")
            info(f"namespace: /api/namespaces/{pool}")
        else:
            step(f"Tearing down {pool!r} (claim + pool + namespace)")
            teardown(http, pool, claim_name)
            info("done — nothing left billing")


if __name__ == "__main__":
    main()
