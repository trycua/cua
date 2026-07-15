#!/usr/bin/env python3
"""Claim a sandbox from a pool, wait for it to bind, then call one of its
service endpoints — all through the cyclops-cs backend with a client id/secret.

What it illustrates:
  * ``TrainClient.from_key`` for the client_credentials exchange + transparent
    token refresh, reused as a plain authenticated httpx client for endpoints
    that are NOT in the generated SDK surface
  * creating an ``OSGymSandboxClaim`` CR via the ``/api/k8s`` kubectl-proxy
    (the same path the SPA uses — the pool-operator binds it to a warm sandbox)
  * reaching the claimed sandbox through ``/api/svc/{namespace}/{service}/``

Auth — use a per-USER key (``ukey-...``), not a per-pool key:
``/api/k8s`` impersonates the token's owner so Capsule tenant RBAC applies.
Per-user keys (POST /api/user-keys) act on behalf of their owner and can
create claims; per-pool keys (POST /api/keys, ``key-...``) have no K8s
identity and are rejected on the claim step.

Setup::

    pip install cua-train --extra-index-url https://wheels.cua.ai/simple/
    export CUA_CLIENT_ID=ukey-xxxxxxxx      # from POST /api/user-keys
    export CUA_CLIENT_SECRET=...
    python claim_and_connect.py --pool my-pool            # first pool service
    python claim_and_connect.py --pool my-pool --service vnc --path /healthz
    python claim_and_connect.py --pool win --service cua-driver --mcp

``--mcp`` runs the cua-driver MCP handshake (initialize → session id →
tools/list) instead of a plain GET — proof the desktop is drivable, not
just that something answers HTTP.

The claim is released (DELETE) on exit unless ``--keep`` is passed.

A sandbox released by an earlier claim is hard-reset in place and rejoins
the pool, but its guest OS reboots — the in-guest service answers 502
("upstream unavailable") for a few minutes after the claim binds. The
script polls the service until it responds (``--ready-timeout``).

Env knobs: CUA_CLIENT_ID, CUA_CLIENT_SECRET, CUA_TOKEN_URL, CUA_BASE_URL.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import uuid

import httpx

from cua_train import TrainClient

# OSGymSandboxClaim CRD coordinates (osgym.cua.ai group — the pool-operator
# watches these and binds each claim to a warm sandbox from the pool).
CLAIM_GROUP = "osgym.cua.ai"
CLAIM_VERSION = "v1alpha1"
CLAIM_PLURAL = "osgymsandboxclaims"

DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
DEFAULT_BASE_URL = "https://run.cua.ai"


def claims_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{CLAIM_GROUP}/{CLAIM_VERSION}/namespaces/{pool}/{CLAIM_PLURAL}"
    return f"{base}/{name}" if name else base


def first_pool_service(http: httpx.Client, pool: str) -> str:
    """Name of the pool's first declared service (spec.services[0].name)."""
    r = http.get(f"/api/k8s/apis/cua.ai/v1/namespaces/{pool}/osgymworkspacepools/{pool}")
    r.raise_for_status()
    services = r.json().get("spec", {}).get("services") or []
    if not services:
        sys.exit(f"pool {pool!r} declares no services — pass --service or add one to the pool spec")
    return services[0]["name"]


def create_claim(http: httpx.Client, pool: str, name: str) -> None:
    r = http.post(
        claims_url(pool),
        json={
            "apiVersion": f"{CLAIM_GROUP}/{CLAIM_VERSION}",
            "kind": "OSGymSandboxClaim",
            "metadata": {"name": name},
            # Pool name = namespace name (1:1), and each pool ships a template
            # named "<pool>-template" — same convention the SPA uses.
            "spec": {"sandboxTemplateRef": {"name": f"{pool}-template"}},
        },
    )
    if r.status_code == 403:
        sys.exit(
            "403 creating claim — is this a per-pool key? Claims need a per-USER "
            "key (client id starting with 'ukey-', from POST /api/user-keys)."
        )
    r.raise_for_status()


def wait_bound(http: httpx.Client, pool: str, name: str, timeout: float) -> str:
    """Poll the claim until status.phase == Bound; return the sandbox name."""
    deadline = time.monotonic() + timeout
    while True:
        r = http.get(claims_url(pool, name))
        r.raise_for_status()
        status = r.json().get("status") or {}
        phase = status.get("phase", "Pending")
        sandbox = (status.get("sandbox") or {}).get("name")
        if phase == "Bound" and sandbox:
            return sandbox
        if phase == "Failed":
            sys.exit(f"claim {name} failed: {status}")
        if time.monotonic() > deadline:
            sys.exit(f"timed out after {timeout:.0f}s waiting for claim {name} (phase={phase})")
        print(f"  claim {name}: {phase} …")
        time.sleep(2)


def wait_service_ready(http, url: str, timeout: float) -> httpx.Response:
    """GET url until the in-guest service answers with something other than
    a proxy 502/503/504 (the guest OS is still booting after a reset)."""
    deadline = time.monotonic() + timeout
    while True:
        resp = http().get(url)
        if resp.status_code not in (502, 503, 504):
            return resp
        if time.monotonic() > deadline:
            sys.exit(f"timed out after {timeout:.0f}s waiting for the service (last: HTTP {resp.status_code})")
        print(f"  HTTP {resp.status_code} — guest still booting …")
        time.sleep(15)


def mcp_handshake(http, base: str) -> None:
    """initialize → notifications/initialized → tools/list against the
    cua-driver Streamable HTTP MCP endpoint at {base}/mcp."""
    url = f"{base}/mcp"
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    r = http().post(url, headers=headers, json={
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "claim_and_connect", "version": "0.1.0"},
        },
    })
    r.raise_for_status()
    session = r.headers.get("mcp-session-id")
    print(f"MCP initialize: HTTP {r.status_code}, session {session}")
    if session:
        headers["Mcp-Session-Id"] = session
    http().post(url, headers=headers, json={"jsonrpc": "2.0", "method": "notifications/initialized"})
    r = http().post(url, headers=headers, json={"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    r.raise_for_status()
    # Streamable HTTP returns SSE ("event: message\ndata: {...}"); the JSON
    # payload is on the data: line.
    data = next((ln[6:] for ln in r.text.splitlines() if ln.startswith("data: ")), r.text)
    tools = [t.get("name") for t in json.loads(data).get("result", {}).get("tools", [])]
    print(f"MCP tools ({len(tools)}): {', '.join(sorted(tools)[:12])}{', …' if len(tools) > 12 else ''}")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--pool", required=True, help="pool name (= namespace)")
    p.add_argument("--service", help="pool service name (default: first service in the pool spec)")
    p.add_argument("--path", default="/", help="path to request on the service (default: /)")
    p.add_argument("--claim-name", help="claim name (default: claim-<random>)")
    p.add_argument("--timeout", type=float, default=300, help="seconds to wait for Bound (default: 300)")
    p.add_argument("--ready-timeout", type=float, default=420,
                   help="seconds to wait for the in-guest service after binding (default: 420)")
    p.add_argument("--mcp", action="store_true",
                   help="run the cua-driver MCP handshake (initialize + tools/list) instead of GET --path")
    p.add_argument("--keep", action="store_true", help="keep the claim instead of releasing it on exit")
    args = p.parse_args()

    client_id = os.environ.get("CUA_CLIENT_ID")
    client_secret = os.environ.get("CUA_CLIENT_SECRET")
    if not client_id or not client_secret:
        sys.exit("set CUA_CLIENT_ID and CUA_CLIENT_SECRET (per-user key, POST /api/user-keys)")

    client = TrainClient.from_key(
        token_url=os.environ.get("CUA_TOKEN_URL", DEFAULT_TOKEN_URL),
        client_id=client_id,
        client_secret=client_secret,
        base_url=os.environ.get("CUA_BASE_URL", DEFAULT_BASE_URL),
    )

    # get_httpx_client() re-mints the bearer token when it is close to expiry,
    # so fetch it per request rather than holding one reference.
    http = client.get_httpx_client
    claim_name = args.claim_name or f"claim-{uuid.uuid4().hex[:8]}"

    service = args.service or first_pool_service(http(), args.pool)

    print(f"creating claim {claim_name} on pool {args.pool} (template {args.pool}-template)")
    create_claim(http(), args.pool, claim_name)

    try:
        sandbox = wait_bound(http(), args.pool, claim_name, args.timeout)
        # Per-sandbox K8s Services are named "<sandbox>-<service>" and exposed
        # on port 80 by the operator; /api/svc proxies to them in-cluster.
        svc_base = f"/api/svc/{args.pool}/{sandbox}-{service}"
        url = f"{svc_base}/{args.path.lstrip('/')}"
        print(f"bound to sandbox {sandbox}; GET {url}")

        resp = wait_service_ready(http, url, args.ready_timeout)
        print(f"HTTP {resp.status_code} {resp.headers.get('content-type', '')}")
        body = resp.text
        print(body[:2000] + ("…" if len(body) > 2000 else ""))

        if args.mcp:
            mcp_handshake(http, svc_base)
    finally:
        if args.keep:
            print(f"keeping claim {claim_name} (release with: DELETE {claims_url(args.pool, claim_name)})")
        else:
            r = http().delete(claims_url(args.pool, claim_name))
            print(f"released claim {claim_name} (HTTP {r.status_code})")


if __name__ == "__main__":
    main()
