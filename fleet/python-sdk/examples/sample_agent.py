#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "boto3",
#     "cua-train",
#     "httpx",
#     "pyyaml",
# ]
#
# [tool.uv]
# extra-index-url = ["https://wheels.cua.ai/simple/"]
# ///
"""Run **mini-swe-agent over TWO MCP servers** on a **Windows** CUA sandbox.

Self-contained, single-file — run it straight with ``uv`` (no venv, no
``pip install``); the inline script metadata pulls ``cua-train`` from the wheels
index automatically.

the sandbox runs **two** MCP servers at once:

  * CUA **computer-server** — Service ``<sandbox>-server`` (VM :8000), MCP /mcp
  * **cua-driver**          — Service ``<sandbox>-mcp``    (VM :3000), MCP /mcp

Lifecycle (every step narrated in the console):

  1. exchange the client_credentials for a bearer token (``TrainClient.from_key``)
  2. create the per-user namespace (``POST /api/namespaces`` — Capsule tenant
     RBAC; pool name == namespace)
  3. create an ``OSGymWorkspacePool`` (``cua.ai/v1``) with **one Service per MCP
     server**. The pool-operator compat shim projects it into an
     ``OSGymSandboxTemplate`` (``<pool>-template``) + ``OSGymSandboxWarmPool`` pair.
  4. wait for a warm VM, then claim a sandbox (``OSGymSandboxClaim``)
  5. open one MCP session per server, run the agent, release the claim

Auth needs a per-USER key (``ukey-…``, from POST /api/user-keys); the LLM runs
on **AWS Bedrock** via a direct ``boto3`` ``bedrock-runtime`` Converse call. AWS
credentials come straight from the standard environment / default chain (env
vars, a shared profile, SSO, or an instance role) — there is no LiteLLM layer.
Set AWS_REGION (or pass ``--aws-region``); the model id is a Bedrock id such as
``us.anthropic.claude-sonnet-4-6`` (a leading ``bedrock/`` is stripped).

    export CUA_CLIENT_ID=ukey-xxxxxxxx
    export CUA_CLIENT_SECRET=...
    export AWS_REGION=us-east-1
    export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...   # or AWS_PROFILE / role
    uv run sample_agent.py --name windows-agent-demo \
        --model us.anthropic.claude-sonnet-4-6 \
        --task "Open Microsoft Edge and browse to https://news.yahoo.com/."

…or, if your uv is too old to read the inline extra-index-url::

    uv run --with cua-train --with httpx --with boto3 \
        --extra-index-url https://wheels.cua.ai/simple/ \
        sample_agent.py --name windows-agent-demo --task "…"

The sandbox claim is ALWAYS released on exit — success, error, or Ctrl-C. Pass
``--keep`` to leave the *pool/namespace* up (the claim is still released),
``--delete-pool`` to tear everything down.
Env knobs: CUA_CLIENT_ID, CUA_CLIENT_SECRET, CUA_TOKEN_URL, CUA_BASE_URL, AGENT_MODEL,
plus the standard AWS_* variables (AWS_REGION, AWS_ACCESS_KEY_ID, …) read by boto3.
"""

from __future__ import annotations

import argparse
import base64
import dataclasses
import json
import os
import re
import signal
import sys
import time
import uuid
from collections.abc import Callable

import boto3
import httpx
import yaml

from cua_train import TrainClient

DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
DEFAULT_BASE_URL = "https://run.cua.ai"

# OSGymWorkspacePool — the legacy single-object pool CR the cyclops-cs SPA
# creates (cua.ai/v1). The pool-operator compat shim translates it into the
# native OSGymSandboxTemplate (<pool>-template) + OSGymSandboxWarmPool pair.
POOL_GROUP, POOL_VERSION, POOL_PLURAL = "cua.ai", "v1", "osgymworkspacepools"
# OSGymSandboxTemplate / OSGymSandboxClaim live in the osgym.cua.ai group.
EXT_GROUP, EXT_VERSION = "osgym.cua.ai", "v1alpha1"
TEMPLATE_PLURAL, CLAIM_PLURAL = "osgymsandboxtemplates", "osgymsandboxclaims"

SYSTEM = (
    "You operate a computer through the provided tools. You have access to "
    "MULTIPLE tool servers running inside the same machine — a CUA "
    "computer-server and a cua-driver — and their tool names are prefixed with "
    "the server they belong to (e.g. 'computer-server__screenshot', "
    "'cua-driver__click'). Pick whichever tool fits; they act on the same "
    "desktop. Inspect state (take a screenshot, read a file, run a command), "
    "then take ONE action at a time and observe the result before the next. "
    "When the task is fully complete, reply with a final message and no tool call."
)


def default_run_manifest() -> dict:
    return {
        "apiVersion": "cua.ai/v1alpha1",
        "kind": "AgentWindowsDoubleMCPRun",
        "metadata": {"name": None},
        "spec": {
            "pool": {
                "image": DEFAULT_IMAGE,
                "cpu": DEFAULT_CPU,
                "ram": DEFAULT_RAM,
                "firmware": DEFAULT_FIRMWARE,
                "replicas": 1,
                "minAvailable": 1,
                "readinessPort": None,
                "services": [],
            },
            "claim": {
                "name": None,
                "bindDeadline": 600,
            },
            "agent": {
                "model": None,
                "task": None,
                "maxSteps": 40,
                "keepImages": 2,
            },
            "rollout": {
                "templateTimeout": 120,
                "warmTimeout": 0,
                "bindTimeout": 0,
                "readyTimeout": 0,
                "pollInterval": 5,
            },
            "teardown": {
                "keepPool": False,
                "deletePool": False,
            },
        },
    }


def deep_merge(base: dict, override: dict) -> dict:
    merged = dict(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(base.get(key), dict):
            merged[key] = deep_merge(base[key], value)
        else:
            merged[key] = value
    return merged


def load_run_manifest(path: str | None) -> dict:
    manifest = default_run_manifest()
    if not path:
        return manifest
    with open(path, "r", encoding="utf-8") as f:
        raw = yaml.safe_load(f) or {}
    if not isinstance(raw, dict):
        die(f"config {path!r} must be a YAML mapping/object")
    manifest = deep_merge(manifest, raw)
    kind = manifest.get("kind")
    if kind != "AgentWindowsDoubleMCPRun":
        die(f"config {path!r} must have kind: AgentWindowsDoubleMCPRun (got {kind!r})")
    return manifest


def manifest_get(manifest: dict, *path: str, default=None):
    cur = manifest
    for part in path:
        if not isinstance(cur, dict):
            return default
        cur = cur.get(part)
        if cur is None:
            return default
    return cur


def resolve_name_template(name: str | None) -> str | None:
    if not name:
        return name
    suffix = uuid.uuid4().hex[:6]
    if "{randhex6}" in name:
        return name.replace("{randhex6}", suffix)
    if name.endswith("-"):
        return f"{name}{suffix}"
    return name


# Default LLM: Claude on AWS Bedrock (cross-region inference profile). Override
# with --model or AGENT_MODEL; whatever your AWS account/region has enabled.
DEFAULT_BEDROCK_MODEL = "us.anthropic.claude-sonnet-4-6"
MAX_OUTPUT_TOKENS = 4096


def default_model() -> str:
    """Bedrock model id default (AGENT_MODEL overrides)."""
    return os.environ.get("AGENT_MODEL") or DEFAULT_BEDROCK_MODEL


def env_first(name: str, *aliases: str) -> str | None:
    for key in (name, *aliases):
        value = os.environ.get(key)
        if value:
            return value
    return None


def bedrock_model_id(model: str) -> str:
    """Strip an optional ``bedrock/`` prefix; the Converse API wants the bare id."""
    return model[len("bedrock/"):] if model.startswith("bedrock/") else model


def make_bedrock_client(args: argparse.Namespace):
    """A boto3 bedrock-runtime client. Credentials and region come from the
    standard AWS environment / default chain (env vars, shared profile, SSO,
    instance role) — nothing LLM-specific to configure. --aws-region overrides
    the region; AWS_REGION/AWS_DEFAULT_REGION are honoured otherwise."""
    region = args.aws_region or env_first("AWS_REGION", "AWS_DEFAULT_REGION")
    client = boto3.client("bedrock-runtime", region_name=region) if region else boto3.client("bedrock-runtime")
    info(f"Bedrock client ready (region {client.meta.region_name})")
    return client


# OS profile (Windows)


@dataclasses.dataclass(frozen=True)
class McpServerSpec:
    """One MCP server exposed by the sandbox VM.

    ``label``    — short name; tool names are namespaced ``<label>__<tool>``.
    ``service``  — per-sandbox Service suffix: ``/api/svc/{ns}/{sandbox}-{service}``.
    ``port``     — targetPort on the VM the Service maps :80 -> .
    ``mcp_path`` — path the MCP server is mounted at (Streamable HTTP).
    ``health``   — a cheap GET that returns non-5xx once the guest service is up.
    """

    label: str
    service: str
    port: int
    mcp_path: str = "/mcp"
    health: str = "/healthz"


@dataclasses.dataclass(frozen=True)
class ServiceSpec:
    """One non-MCP service exposed by the sandbox VM."""

    label: str
    service: str
    port: int


OS_NAME = "windows"
DEFAULT_IMAGE = "296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-server-windows:latest"
# KubeVirt VM firmware (OSGymWorkspacePool template field; CRD enum [bios, efi]).
#   efi  — GPT/UEFI-only guest images (the dockur-built Windows desktop-workspace);
#          the operator also flips on Hyper-V enlightenments + clock config for it.
#   bios — KubeVirt's default; what the Linux workspace images boot with.
# The SPA/operator only send firmware when it differs from bios, so we omit it for
# bios and let the operator default apply. Default here is efi (this script's
# Windows lineage); per-run configs set spec.pool.firmware for non-Windows images.
DEFAULT_FIRMWARE = "efi"
FIRMWARE_CHOICES = ("bios", "efi")
DEFAULT_RAM = "4Gi"
DEFAULT_CPU = 4
DEFAULT_WARM_TIMEOUT = 900     # cold image pull + Windows boot


# URL builders (relative to base_url, e.g. https://run.cua.ai)


def pool_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{POOL_GROUP}/{POOL_VERSION}/namespaces/{pool}/{POOL_PLURAL}"
    return f"{base}/{name}" if name else base


def claims_url(pool: str, name: str | None = None) -> str:
    base = f"/api/k8s/apis/{EXT_GROUP}/{EXT_VERSION}/namespaces/{pool}/{CLAIM_PLURAL}"
    return f"{base}/{name}" if name else base


def load_pool_services(manifest: dict) -> tuple[tuple[McpServerSpec, ...], tuple[ServiceSpec, ...]]:
    raw_services = manifest_get(manifest, "spec", "pool", "services")
    if not isinstance(raw_services, list) or not raw_services:
        die("config must define spec.pool.services as a non-empty list")

    mcp_servers: list[McpServerSpec] = []
    extra_services: list[ServiceSpec] = []
    labels: set[str] = set()
    names: set[str] = set()

    for idx, raw in enumerate(raw_services):
        if not isinstance(raw, dict):
            die(f"spec.pool.services[{idx}] must be a YAML mapping/object")
        kind = raw.get("type")
        label = raw.get("label")
        service = raw.get("service")
        port = raw.get("port")
        if kind not in {"mcp", "auxiliary"}:
            die(f"spec.pool.services[{idx}].type must be 'mcp' or 'auxiliary'")
        if not isinstance(label, str) or not label:
            die(f"spec.pool.services[{idx}].label must be a non-empty string")
        if not isinstance(service, str) or not service:
            die(f"spec.pool.services[{idx}].service must be a non-empty string")
        if not isinstance(port, int) or port <= 0:
            die(f"spec.pool.services[{idx}].port must be a positive integer")
        if label in labels:
            die(f"duplicate spec.pool.services label: {label!r}")
        if service in names:
            die(f"duplicate spec.pool.services service: {service!r}")
        labels.add(label)
        names.add(service)
        if kind == "mcp":
            mcp_path = raw.get("mcpPath", "/mcp")
            health = raw.get("health", "/healthz")
            if not isinstance(mcp_path, str) or not mcp_path.startswith("/"):
                die(f"spec.pool.services[{idx}].mcpPath must be an absolute path string")
            if not isinstance(health, str) or not health.startswith("/"):
                die(f"spec.pool.services[{idx}].health must be an absolute path string")
            mcp_servers.append(
                McpServerSpec(label=label, service=service, port=port, mcp_path=mcp_path, health=health)
            )
        else:
            extra_services.append(ServiceSpec(label=label, service=service, port=port))

    if not mcp_servers:
        die("spec.pool.services must include at least one MCP service")
    return tuple(mcp_servers), tuple(extra_services)


def pool_services(
    mcp_servers: tuple[McpServerSpec, ...],
    extra_services: tuple[ServiceSpec, ...],
) -> tuple[ServiceSpec, ...]:
    base = tuple(ServiceSpec(label=s.label, service=s.service, port=s.port) for s in mcp_servers)
    return base + extra_services


# Status formatting helpers


def _fmt_conditions(conditions: list[dict] | None) -> str:
    if not conditions:
        return "none"
    parts = []
    for cond in conditions:
        ctype = cond.get("type", "?")
        status = cond.get("status", "?")
        reason = cond.get("reason", "?")
        msg = " ".join(str(cond.get("message", "")).split())
        chunk = f"{ctype}={status}/{reason}"
        if msg:
            chunk += f" ({msg})"
        parts.append(chunk)
    return "; ".join(parts)


def pool_status(http: httpx.Client, pool: str) -> dict:
    r = http.get(pool_url(pool, pool))
    r.raise_for_status()
    return r.json().get("status") or {}


def claim_status(http: httpx.Client, pool: str, name: str) -> dict:
    r = http.get(claims_url(pool, name))
    r.raise_for_status()
    return r.json().get("status") or {}


def claim_obj(http: httpx.Client, pool: str, name: str) -> dict:
    r = http.get(claims_url(pool, name))
    r.raise_for_status()
    return r.json()


def pool_summary(status: dict) -> str:
    return (
        f"phase={status.get('phase', 'Unknown')} "
        f"ready={int(status.get('availableCount', 0))}/{int(status.get('totalCount', 0))} "
        f"claimed={int(status.get('claimedCount', 0))}"
    )


def claim_summary(status: dict) -> str:
    sandbox = (status.get("sandbox") or {}).get("name")
    summary = f"phase={status.get('phase', 'Pending')}"
    if sandbox:
        summary += f" sandbox={sandbox}"
    conds = _fmt_conditions(status.get("conditions"))
    if conds != "none":
        summary += f" conditions=[{conds}]"
    return summary


def claim_operator_progress(obj: dict) -> str | None:
    raw = (obj.get("metadata") or {}).get("annotations", {}).get("kopf.zalando.org/on_claim_create")
    if not raw:
        return None
    try:
        data = json.loads(raw)
    except Exception:
        return raw
    retries = data.get("retries")
    message = " ".join(str(data.get("message", "")).split())
    parts = []
    if retries is not None:
        parts.append(f"retries={retries}")
    if message:
        parts.append(message)
    return "; ".join(parts) or None


# Console helpers


def step(msg: str) -> None:
    print(f"\n==> {msg}", flush=True)


def info(msg: str) -> None:
    print(f"    {msg}", flush=True)


def die(msg: str) -> None:
    sys.exit(f"\nERROR: {msg}")


class TeeWriter:
    def __init__(self, *streams) -> None:
        self.streams = streams

    def write(self, data: str) -> int:
        for stream in self.streams:
            stream.write(data)
        return len(data)

    def flush(self) -> None:
        for stream in self.streams:
            stream.flush()

    def isatty(self) -> bool:
        return any(getattr(stream, "isatty", lambda: False)() for stream in self.streams)


def configure_logging(log_file: str | None) -> None:
    if not log_file:
        return
    log_path = os.path.abspath(log_file)
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    log_stream = open(log_path, "a", encoding="utf-8", buffering=1)
    sys.stdout = TeeWriter(sys.stdout, log_stream)
    sys.stderr = TeeWriter(sys.stderr, log_stream)
    info(f"logging to {log_path}")


# Pool / claim lifecycle (mirrors create_pool_and_list_tools.py)


def create_namespace(http: httpx.Client, name: str) -> None:
    """Create the per-user namespace via the Capsule-backed SPA endpoint.

    Pool name == namespace name (1:1). Going through /api/namespaces is what
    makes Capsule stamp tenant ownership + RBAC, so the impersonated pool create
    below is allowed. 409 just means we are reusing an earlier run's namespace.
    """
    r = http.post("/api/namespaces", json={"name": name})
    if r.status_code == 409:
        info(f"namespace {name!r} already exists — reusing it")
        return
    if r.status_code == 403:
        die("403 creating namespace — namespaces/pools/claims need a per-USER key "
            "(client id 'ukey-…', from POST /api/user-keys).")
    r.raise_for_status()
    info(f"namespace {name!r} created")


def delete_namespace(http: httpx.Client, name: str) -> None:
    r = http.delete(f"/api/namespaces/{name}")
    if r.status_code in (200, 202, 204, 404):
        info(f"namespace {name!r} delete requested (HTTP {r.status_code})")
        return
    if r.status_code == 403:
        die("403 deleting namespace — the user key cannot replace this pool namespace.")
    r.raise_for_status()


def recreate_namespace(get_http: Callable[[], httpx.Client], name: str, poll: float) -> None:
    delete_namespace(get_http(), name)
    while True:
        r = get_http().post("/api/namespaces", json={"name": name})
        if r.status_code == 409:
            info(f"namespace {name!r} still terminating — waiting to recreate …")
            time.sleep(poll)
            continue
        if r.status_code == 403:
            die("403 creating namespace during replacement — namespaces/pools/claims need a per-USER key.")
        r.raise_for_status()
        info(f"namespace {name!r} recreated")
        return


def desired_pool_spec(
    *,
    image: str,
    cpu: int,
    ram: str,
    firmware: str,
    replicas: int,
    readiness_port: int | None,
    services: tuple[ServiceSpec, ...],
) -> dict:
    template: dict = {
        "containerDiskImage": image,
        "imagePullSecret": "ecr-credentials",
        "cpuCores": cpu,
        "memory": ram,
    }
    # Match the SPA/operator: only send firmware when it differs from the bios default.
    if firmware != "bios":
        template["firmware"] = firmware
    if readiness_port:
        template["probes"] = {"readinessProbe": {"tcpSocket": {"port": readiness_port}}}
    rendered_services = [{"name": s.service, "targetPort": s.port, "protocol": "TCP"} for s in services]
    return {"replicas": replicas, "template": template, "services": rendered_services}


def _json_canonical(value: dict) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def _flatten_json(value, prefix: str = "") -> dict[str, str]:
    flat: dict[str, str] = {}
    if isinstance(value, dict):
        for key in sorted(value):
            child = f"{prefix}.{key}" if prefix else str(key)
            flat.update(_flatten_json(value[key], child))
        if not value and prefix:
            flat[prefix] = "{}"
    elif isinstance(value, list):
        for idx, item in enumerate(value):
            child = f"{prefix}[{idx}]"
            flat.update(_flatten_json(item, child))
        if not value and prefix:
            flat[prefix] = "[]"
    else:
        flat[prefix] = json.dumps(value, sort_keys=True)
    return flat


def log_pool_spec_diff(current_spec: dict, desired_spec: dict) -> None:
    current_flat = _flatten_json(current_spec, "spec")
    desired_flat = _flatten_json(desired_spec, "spec")
    keys = sorted(set(current_flat) | set(desired_flat))
    diffs = []
    for key in keys:
        current_value = current_flat.get(key, "<missing>")
        desired_value = desired_flat.get(key, "<missing>")
        if current_value == desired_value:
            continue
        diffs.append((key, current_value, desired_value))
    if not diffs:
        info("pool spec diff: none")
        return
    info(f"pool spec diff: {len(diffs)} field(s) differ")
    for key, current_value, desired_value in diffs:
        info(f"  {key}: current={current_value} desired={desired_value}")


def pool_reconcile_action(current_spec: dict, desired_spec: dict) -> str:
    if current_spec == desired_spec:
        return "noop"
    # always replace because thats simpler and more robust
    return "replace"


def create_pool(
    get_http: Callable[[], httpx.Client],
    *,
    pool: str,
    image: str,
    cpu: int,
    ram: str,
    firmware: str,
    replicas: int,
    readiness_port: int | None,
    mcp_servers: tuple[McpServerSpec, ...],
    extra_services: tuple[ServiceSpec, ...],
    poll: float,
    allow_replace: bool = True,
) -> None:
    """Create the OSGymWorkspacePool with MCP and auxiliary services.

    Each service becomes a per-sandbox Service ``<sandbox>-<name>`` (port 80 ->
    targetPort). The MCP services are used by the agent; auxiliary services such
    as noVNC are exposed for debugging but are not opened as MCP sessions.
    """
    spec = desired_pool_spec(
        image=image,
        cpu=cpu,
        ram=ram,
        firmware=firmware,
        replicas=replicas,
        readiness_port=readiness_port,
        services=pool_services(mcp_servers, extra_services),
    )
    body = {
        "apiVersion": f"{POOL_GROUP}/{POOL_VERSION}",
        "kind": "OSGymWorkspacePool",
        "metadata": {"name": pool, "labels": {"cua.ai/pool": pool}},
        "spec": spec,
    }

    r = get_http().post(pool_url(pool), json=body)
    if r.status_code == 409:
        current = get_http().get(pool_url(pool, pool))
        current.raise_for_status()
        current_spec = current.json().get("spec") or {}
        log_pool_spec_diff(current_spec, spec)
        action = pool_reconcile_action(current_spec, spec)
        if action == "noop":
            info(f"pool {pool!r} already exists — reusing it (spec unchanged)")
        elif action == "scale":
            info(f"pool {pool!r} already exists — scaling replicas in place")
            rp = get_http().patch(
                pool_url(pool, pool),
                headers={"Content-Type": "application/merge-patch+json"},
                json={"spec": {"replicas": replicas}},
            )
            if rp.status_code == 403:
                die("403 updating pool replicas — the user key can create pools but cannot patch this pool spec.")
            rp.raise_for_status()
            info(f"pool {pool!r} scaled to replicas={replicas}")
        else:
            if not allow_replace:
                die(f"pool {pool!r} still differs from desired state after replacement attempt")
            info(f"pool {pool!r} already exists — desired template differs, replacing pool namespace")
            recreate_namespace(get_http, pool, poll)
            return create_pool(
                get_http,
                pool=pool,
                image=image,
                cpu=cpu,
                ram=ram,
                firmware=firmware,
                replicas=replicas,
                readiness_port=readiness_port,
                mcp_servers=mcp_servers,
                extra_services=extra_services,
                poll=poll,
                allow_replace=False,
            )
        info(f"image:     {image}")
        info(f"size:      {cpu} vCPU / {ram} / replicas={replicas} / firmware={firmware}")
        for s in mcp_servers:
            info(f"service:   {s.service:<14} -> targetPort {s.port}  (MCP at {s.mcp_path}, health {s.health})")
        for s in extra_services:
            info(f"service:   {s.service:<14} -> targetPort {s.port}  (auxiliary {s.label})")
        info(f"readiness: {'disabled' if not readiness_port else f'tcpSocket :{readiness_port}'}")
        return
    if r.status_code == 403:
        die("403 creating pool — /api/k8s needs a per-USER key ('ukey-…', POST /api/user-keys).")
    r.raise_for_status()
    info(f"pool {pool!r} created")
    info(f"image:     {image}")
    info(f"size:      {cpu} vCPU / {ram} / replicas={replicas} / firmware={firmware}")
    for s in mcp_servers:
        info(f"service:   {s.service:<14} -> targetPort {s.port}  (MCP at {s.mcp_path}, health {s.health})")
    for s in extra_services:
        info(f"service:   {s.service:<14} -> targetPort {s.port}  (auxiliary {s.label})")
    info(f"readiness: {'disabled' if not readiness_port else f'tcpSocket :{readiness_port}'}")


def wait_template(get_http: Callable[[], httpx.Client], pool: str, timeout: float, poll: float) -> None:
    """Wait for the compat shim to project the pool into <pool>-template."""
    tmpl = f"{pool}-template"
    deadline = time.monotonic() + timeout
    while True:
        r = get_http().get(template_url(pool, tmpl))
        if r.status_code == 200:
            info(f"pool-operator projected template {tmpl!r} + warm pool {pool!r}")
            return
        if r.status_code not in (404, 403):
            r.raise_for_status()
        if timeout > 0 and time.monotonic() > deadline:
            info(f"template {tmpl!r} not visible after {timeout:.0f}s — claiming anyway")
            try:
                info(f"pool status: {pool_summary(pool_status(get_http(), pool))}")
            except Exception:
                pass
            return
        info(f"waiting for pool-operator to project template {tmpl!r} …")
        time.sleep(poll)


def pool_counts(get_http: Callable[[], httpx.Client], pool: str) -> tuple[int, int, str]:
    """(totalCount, availableCount, phase) from the pool status, best-effort."""
    try:
        st = pool_status(get_http(), pool)
        return int(st.get("totalCount", 0)), int(st.get("availableCount", 0)), st.get("phase", "Unknown")
    except Exception:
        return 0, 0, "Unknown"


def wait_pool_warm(
    get_http: Callable[[], httpx.Client],
    pool: str,
    timeout: float,
    poll: float,
    *,
    replicas: int,
    min_available: int,
) -> None:
    """Wait until the pool reports a warm (Ready) VM, so the first claim binds fast.

    A cold pool has to pull the multi-GB containerDisk, boot the guest, and bring
    the readiness-port service up before a Sandbox is adoptable. Letting the pool
    warm first keeps the claim's bindDeadline from burning down during the pull.
    """
    start = time.monotonic()
    deadline = start + timeout
    last_summary = ""
    while True:
        try:
            st = pool_status(get_http(), pool)
            total = int(st.get("totalCount", 0))
            avail = int(st.get("availableCount", 0))
            phase = st.get("phase", "Unknown")
            summary = pool_summary(st)
        except Exception:
            total, avail, phase = 0, 0, "Unknown"
            summary = "phase=Unknown ready=0/0 claimed=0"
        if total >= replicas and avail >= min_available:
            info(f"pool warm: {avail}/{total} VM(s) ready (phase {phase})")
            return
        if summary != last_summary:
            info(f"pool detail: {summary}")
            last_summary = summary
        if timeout > 0 and time.monotonic() > deadline:
            info(f"timed out after {timeout:.0f}s waiting for a warm VM (pool {phase}, {avail}/{total})")
            info("continuing anyway — the claim may still bind once the VM finishes pulling/booting")
            return
        elapsed = int(time.monotonic() - start)
        info(f"[{elapsed:4d}s] pool={phase} ({avail}/{total} VMs ready, target total={replicas}, "
             f"target available={min_available}) — warming (image pull + guest boot + service on the readiness port) …")
        time.sleep(poll)


def create_claim(http: httpx.Client, pool: str, name: str, bind_deadline: int) -> None:
    spec: dict = {"sandboxTemplateRef": {"name": f"{pool}-template"}}
    if bind_deadline:
        spec["bindDeadline"] = bind_deadline
    r = http.post(claims_url(pool), json={
        "apiVersion": f"{EXT_GROUP}/{EXT_VERSION}", "kind": "OSGymSandboxClaim",
        "metadata": {"name": name}, "spec": spec})
    if r.status_code == 403:
        die("403 creating claim — per-pool keys have no K8s identity. Use a per-USER key ('ukey-…').")
    r.raise_for_status()
    info(f"claim {name!r} created")


def wait_bound(get_http: Callable[[], httpx.Client], pool: str, name: str, timeout: float, poll: float) -> str:
    """Poll the claim until status.phase == Bound; return the sandbox name."""
    start = time.monotonic()
    deadline = start + timeout
    last_claim = ""
    last_pool = ""
    last_operator = ""
    while True:
        claim = claim_obj(get_http(), pool, name)
        status = claim.get("status") or {}
        phase = status.get("phase", "Pending")
        sandbox = (status.get("sandbox") or {}).get("name")
        if phase == "Bound" and sandbox:
            return sandbox
        if phase == "Failed":
            die(f"claim {name} failed: {status}")
        if timeout > 0 and time.monotonic() > deadline:
            die(f"timed out after {timeout:.0f}s waiting for claim {name} to bind (phase={phase})")
        claim_line = claim_summary(status)
        if claim_line != last_claim:
            info(f"claim detail: {claim_line}")
            last_claim = claim_line
        operator_line = claim_operator_progress(claim)
        if operator_line and operator_line != last_operator:
            info(f"claim operator: {operator_line}")
            last_operator = operator_line
        try:
            pst = pool_status(get_http(), pool)
            total = int(pst.get("totalCount", 0))
            avail = int(pst.get("availableCount", 0))
            pphase = pst.get("phase", "Unknown")
            pool_line = pool_summary(pst)
        except Exception:
            total, avail, pphase = 0, 0, "Unknown"
            pool_line = "phase=Unknown ready=0/0 claimed=0"
        if pool_line != last_pool:
            info(f"pool detail: {pool_line}")
            last_pool = pool_line
        elapsed = int(time.monotonic() - start)
        info(f"[{elapsed:4d}s] claim={phase}  pool={pphase} ({avail}/{total} VMs ready) …")
        time.sleep(poll)


def wait_service_ready(get_http: Callable[[], httpx.Client], url: str, timeout: float, poll: float) -> None:
    """GET url until the in-guest service answers (not a proxy 502/503/504)."""
    start = time.monotonic()
    deadline = start + timeout
    while True:
        try:
            resp = get_http().get(url, timeout=15.0)
        except httpx.HTTPError as e:
            resp, reason = None, type(e).__name__
        else:
            reason = f"HTTP {resp.status_code}"
        if resp is not None and resp.status_code not in (502, 503, 504):
            info(f"service responding ({reason})")
            return
        if timeout > 0 and time.monotonic() > deadline:
            die(f"timed out after {timeout:.0f}s waiting for the in-guest service (last: {reason})")
        elapsed = int(time.monotonic() - start)
        info(f"[{elapsed:4d}s] {reason} — guest still booting …")
        time.sleep(poll)


# MCP (Streamable HTTP)  multi-server


def _sse_json(resp: httpx.Response) -> dict:
    """Streamable HTTP replies are SSE ('event: message\\ndata: {...}'); the JSON
    is on the data: line. Fall back to the raw body for plain JSON."""
    line = next((ln[6:] for ln in resp.text.splitlines() if ln.startswith("data: ")), resp.text)
    return json.loads(line)


@dataclasses.dataclass
class McpConn:
    """A live MCP session against one server of the bound sandbox."""

    spec: McpServerSpec
    svc_base: str          # /api/svc/{ns}/{sandbox}-{service}
    session_id: str | None
    tools: list[dict]      # this server's raw tools/list

    @property
    def url(self) -> str:
        return f"{self.svc_base}{self.spec.mcp_path}"

    def _headers(self) -> dict:
        h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        if self.session_id:
            h["Mcp-Session-Id"] = self.session_id
        return h


def mcp_open(http: httpx.Client, spec: McpServerSpec, svc_base: str, client_name: str) -> McpConn:
    """initialize -> notifications/initialized -> tools/list for one server."""
    url = f"{svc_base}{spec.mcp_path}"
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    init = http.post(url, headers=headers, timeout=30.0, json={
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                   "clientInfo": {"name": client_name, "version": "0.1.0"}}})
    init.raise_for_status()
    sid = init.headers.get("mcp-session-id")
    if sid:
        headers["Mcp-Session-Id"] = sid
    http.post(url, headers=headers, timeout=30.0, json={"jsonrpc": "2.0", "method": "notifications/initialized"})
    r = http.post(url, headers=headers, timeout=30.0, json={"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    r.raise_for_status()
    tools = _sse_json(r).get("result", {}).get("tools", [])
    return McpConn(spec=spec, svc_base=svc_base, session_id=sid, tools=tools)


_BEDROCK_IMAGE_FORMATS = {"png", "jpeg", "gif", "webp"}


def mcp_call(http: httpx.Client, conn: McpConn, name: str, arguments: dict) -> dict:
    """Dispatch one tools/call on a specific server. Returns text + inline images
    (decoded to raw bytes + Bedrock image format) + isError. Raises on a
    transport/proxy error so the caller can surface it."""
    r = http.post(conn.url, headers=conn._headers(), timeout=600.0, json={
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}})
    if r.status_code in (502, 503, 504):
        raise httpx.HTTPError(f"proxy {r.status_code}")
    r.raise_for_status()
    result = _sse_json(r).get("result", {})
    text_parts, images = [], []
    for block in result.get("content", []):
        if block.get("type") == "text":
            text_parts.append(block.get("text", ""))
        elif block.get("type") == "image":
            mime = block.get("mimeType", "image/png")
            fmt = mime.split("/")[-1].lower()
            fmt = "jpeg" if fmt == "jpg" else fmt
            if fmt not in _BEDROCK_IMAGE_FORMATS:
                fmt = "png"
            images.append({"format": fmt, "bytes": base64.b64decode(block["data"])})
    return {"text": "\n".join(text_parts), "images": images, "is_error": bool(result.get("isError"))}


def _sanitize(label: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_-]", "-", label)


def build_tool_registry(conns: list[McpConn]) -> tuple[list[dict], dict[str, tuple[McpConn, str]]]:
    """Union every server's tools into one Bedrock Converse toolConfig tool list,
    namespacing names as ``<server>__<tool>`` so two servers can expose the same
    tool without colliding. Returns (llm_tools, registry) where registry maps the
    namespaced name back to (connection, original tool name) for dispatch."""
    llm_tools: list[dict] = []
    registry: dict[str, tuple[McpConn, str]] = {}
    for conn in conns:
        prefix = _sanitize(conn.spec.label)
        for t in conn.tools:
            orig = t["name"]
            llm_name = f"{prefix}__{orig}"[:64]   # Bedrock caps tool names at 64
            # Extremely unlikely, but keep names unique if truncation collides.
            while llm_name in registry:
                llm_name = f"{llm_name[:60]}_{len(registry)}"
            registry[llm_name] = (conn, orig)
            desc = f"[{conn.spec.label}] " + " ".join((t.get("description") or "").split())
            schema = t.get("inputSchema") or {"type": "object", "properties": {}}
            llm_tools.append({"toolSpec": {
                "name": llm_name,
                "description": desc or llm_name,
                "inputSchema": {"json": schema}}})
    return llm_tools, registry


def _msg_has_image(m: dict) -> bool:
    return isinstance(m.get("content"), list) and any(
        "toolResult" in b and any("image" in c for c in b["toolResult"].get("content", []))
        for b in m["content"])


def _prune_images(messages: list[dict], keep: int) -> None:
    """Drop all but the last `keep` screenshots — image tokens dominate a
    computer-use trajectory's cost and stale frames mislead the model. Images live
    inside toolResult content blocks; replace each with a text placeholder so the
    toolResult stays valid (Converse requires non-empty toolResult content)."""
    idx = [i for i, m in enumerate(messages) if _msg_has_image(m)]
    for i in (idx[:-keep] if keep else idx):
        for b in messages[i]["content"]:
            tr = b.get("toolResult")
            if not tr:
                continue
            tr["content"] = [
                {"text": "[older screenshot dropped from context]"} if "image" in c else c
                for c in tr.get("content", [])]


# The agent loop (mini-swe-agent, multi-MCP)


def _converse_text(content: list[dict]) -> str:
    return "\n".join(b["text"] for b in content if "text" in b).strip()


def run_agent(http_get: Callable[[], httpx.Client], conns: list[McpConn], bedrock, *,
              task: str, model_id: str, max_steps: int, keep_images: int) -> str:
    """Canonical mini-swe-agent loop over the UNION of every server's tools, driven
    by the Bedrock Converse API (one tool turn per step)."""
    llm_tools, registry = build_tool_registry(conns)
    info(f"action space: {len(llm_tools)} tools across {len(conns)} MCP server(s)")
    for conn in conns:
        names = ", ".join(sorted(t["name"] for t in conn.tools)) or "(none)"
        info(f"  {conn.spec.label}: {names}")

    system = [{"text": SYSTEM}]
    tool_config = {"tools": llm_tools}
    messages: list[dict] = [{"role": "user", "content": [{"text": task}]}]
    for n in range(1, max_steps + 1):
        resp = bedrock.converse(
            modelId=model_id,
            system=system,
            messages=messages,
            toolConfig=tool_config,
            inferenceConfig={"temperature": 0.0, "maxTokens": MAX_OUTPUT_TOKENS},
        )
        out = resp["output"]["message"]
        messages.append(out)
        tool_uses = [b["toolUse"] for b in out["content"] if "toolUse" in b]
        if not tool_uses:
            step(f"agent finished after {n - 1} action(s)")
            final = _converse_text(out["content"])
            print(final or "(no final message)", flush=True)
            return final or "done"

        result_blocks: list[dict] = []
        for tu in tool_uses:
            llm_name, tuid = tu["name"], tu["toolUseId"]
            tool_args = tu.get("input") or {}
            entry = registry.get(llm_name)
            if entry is None:
                result_blocks.append({"toolResult": {"toolUseId": tuid,
                    "content": [{"text": f"unknown tool {llm_name!r}"}], "status": "error"}})
                continue
            conn, orig = entry
            info(f"[step {n}] {conn.spec.label} -> {orig}({json.dumps(tool_args)[:120]})")
            try:
                res = mcp_call(http_get(), conn, orig, tool_args)
            except httpx.HTTPError as e:
                result_blocks.append({"toolResult": {"toolUseId": tuid,
                    "content": [{"text": f"tool transport error: {e}"}], "status": "error"}})
                continue
            content: list[dict] = []
            if res["text"]:
                content.append({"text": res["text"]})
            for img in res["images"]:
                content.append({"image": {"format": img["format"], "source": {"bytes": img["bytes"]}}})
            if not content:
                content.append({"text": "(no output)"})
            result_blocks.append({"toolResult": {"toolUseId": tuid, "content": content,
                                                 "status": "error" if res["is_error"] else "success"}})
        messages.append({"role": "user", "content": result_blocks})
        _prune_images(messages, keep_images)
    return "max_steps_exceeded"


# Main


def main() -> None:
    pre = argparse.ArgumentParser(add_help=False)
    pre.add_argument("--config")
    pre_args, remaining = pre.parse_known_args()
    manifest = load_run_manifest(pre_args.config)
    mcp_servers, extra_services = load_pool_services(manifest)

    p = argparse.ArgumentParser(
        description=f"Provision a {OS_NAME} double-MCP pool on run.cua.ai, claim a sandbox, "
                    "and run mini-swe-agent against BOTH its MCP servers.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
        parents=[pre],
    )
    cfg_name = manifest_get(manifest, "metadata", "name")
    cfg_task = manifest_get(manifest, "spec", "agent", "task")
    p.add_argument("--name", required=cfg_name is None, default=cfg_name,
                   help="pool name (also the namespace; lowercase/dashes). Reuse the same name across retries "
                        "to keep the same provisioning pool warm.")
    p.add_argument("--task", required=cfg_task is None, default=cfg_task, help="the task prompt for the agent")
    p.add_argument("--client-id", default=os.environ.get("CUA_CLIENT_ID"),
                   help="OAuth client id (per-user key 'ukey-…'); env CUA_CLIENT_ID")
    p.add_argument("--client-secret", default=os.environ.get("CUA_CLIENT_SECRET"),
                   help="OAuth client secret; env CUA_CLIENT_SECRET")
    p.add_argument("--model", default=manifest_get(manifest, "spec", "agent", "model", default=default_model()) or default_model(),
                   help="Bedrock model id (a leading 'bedrock/' is stripped). Uses AGENT_MODEL if set, "
                        f"else {DEFAULT_BEDROCK_MODEL}.")
    p.add_argument("--aws-region", default=None,
                   help="AWS region for the Bedrock client. Defaults to AWS_REGION/AWS_DEFAULT_REGION "
                        "(or your ~/.aws config). Credentials always come from the AWS env / default chain.")
    p.add_argument("--image", default=manifest_get(manifest, "spec", "pool", "image"), help="workspace containerDisk image")
    p.add_argument("--cpu", type=int, default=manifest_get(manifest, "spec", "pool", "cpu"), help="vCPU cores per VM")
    p.add_argument("--ram", default=manifest_get(manifest, "spec", "pool", "ram"), help="memory per VM (K8s quantity)")
    p.add_argument("--firmware", choices=FIRMWARE_CHOICES,
                   default=manifest_get(manifest, "spec", "pool", "firmware", default=DEFAULT_FIRMWARE) or DEFAULT_FIRMWARE,
                   help="VM firmware. 'efi' for GPT/UEFI-only images (Windows desktop-workspace); "
                        "'bios' (KubeVirt default) for the Linux workspace images.")
    p.add_argument("--replicas", type=int, default=manifest_get(manifest, "spec", "pool", "replicas"), help="pre-warmed VMs the pool keeps")
    p.add_argument("--min-available", type=int, default=manifest_get(manifest, "spec", "pool", "minAvailable"),
                   help="minimum warm/available VMs to wait for before claiming. Set to 2+ to keep a small surplus warm.")
    p.add_argument("--readiness-port", type=int, default=manifest_get(manifest, "spec", "pool", "readinessPort"),
                   help="TCP port used for readiness probing. Set to 0 to disable readiness probes in the pool template.")
    p.add_argument("--max-steps", type=int, default=manifest_get(manifest, "spec", "agent", "maxSteps"), help="agent step budget")
    p.add_argument("--keep-images", type=int, default=manifest_get(manifest, "spec", "agent", "keepImages"), help="screenshots kept in LLM context")
    p.add_argument("--token-url", default=os.environ.get("CUA_TOKEN_URL", DEFAULT_TOKEN_URL))
    p.add_argument("--base-url", default=os.environ.get("CUA_BASE_URL", DEFAULT_BASE_URL))
    p.add_argument("--claim-name", default=manifest_get(manifest, "spec", "claim", "name"), help="claim name (default: claim-<random>)")
    p.add_argument("--template-timeout", type=float, default=manifest_get(manifest, "spec", "rollout", "templateTimeout"))
    p.add_argument("--warm-timeout", type=float, default=manifest_get(manifest, "spec", "rollout", "warmTimeout"),
                   help="seconds to wait for warm pool capacity. 0 waits indefinitely.")
    p.add_argument("--bind-deadline", type=int, default=manifest_get(manifest, "spec", "claim", "bindDeadline"))
    p.add_argument("--bind-timeout", type=float, default=manifest_get(manifest, "spec", "rollout", "bindTimeout"),
                   help="seconds to wait for claim binding. 0 waits indefinitely.")
    p.add_argument("--ready-timeout", type=float, default=manifest_get(manifest, "spec", "rollout", "readyTimeout"),
                   help="seconds to wait for each MCP server to answer after binding. 0 waits indefinitely.")
    p.add_argument("--poll-interval", type=float, default=manifest_get(manifest, "spec", "rollout", "pollInterval"))
    p.add_argument("--log-file", default=None,
                   help="append all console output to this file while the run is active")
    p.add_argument("--keep", action="store_true", default=manifest_get(manifest, "spec", "teardown", "keepPool"),
                   help="skip teardown — leave the pool/namespace/claim up for debugging or for reuse on the next retry")
    p.add_argument("--delete-pool", action="store_true", default=manifest_get(manifest, "spec", "teardown", "deletePool"),
                   help="on exit also delete the pool AND namespace (full cleanup)")
    args = p.parse_args(remaining)
    args.name = resolve_name_template(args.name)
    args.claim_name = resolve_name_template(args.claim_name)
    configure_logging(args.log_file)

    if not args.client_id or not args.client_secret:
        die("set --client-id/--client-secret or CUA_CLIENT_ID/CUA_CLIENT_SECRET "
            "(per-user key from POST /api/user-keys)")
    if args.client_id.startswith("key-"):
        die("that looks like a per-POOL key ('key-…'), which has no K8s identity. "
            "Creating namespaces/pools/claims needs a per-USER key ('ukey-…').")

    model_id = bedrock_model_id(args.model)
    bedrock = make_bedrock_client(args)

    pool = args.name
    claim_name = args.claim_name or f"claim-{uuid.uuid4().hex[:8]}"

    step(f"Authenticating to {args.base_url} (client {args.client_id})")
    client = TrainClient.from_key(token_url=args.token_url, client_id=args.client_id,
                                  client_secret=args.client_secret, base_url=args.base_url)
    http = client.get_httpx_client   # call per request — re-mints the bearer near expiry
    info("token acquired")

    step(f"Creating namespace {pool!r}")
    create_namespace(http(), pool)

    step(f"Creating {OS_NAME} double-MCP pool {pool!r}")
    readiness_port = args.readiness_port or None
    create_pool(
        http,
        pool=pool,
        image=args.image,
        cpu=args.cpu,
        ram=args.ram,
        firmware=args.firmware,
        replicas=args.replicas,
        readiness_port=readiness_port,
        mcp_servers=mcp_servers,
        extra_services=extra_services,
        poll=args.poll_interval,
    )

    step(f"Claiming a sandbox from {pool!r} (claim {claim_name!r})")

    # The claim is ALWAYS released — on success, on error, or on Ctrl-C — so an
    # interrupted or failed run never leaks a claimed VM. --keep only preserves the
    # pool/namespace, NOT the claim. The `released` flag makes release idempotent
    # across the SIGINT handler and the finally block.
    released = False

    def release_claim() -> None:
        nonlocal released
        if released:
            return
        released = True
        step(f"Releasing claim {claim_name!r}")
        try:
            r = http().delete(claims_url(pool, claim_name))
            info(f"released (HTTP {r.status_code}) — the VM hard-resets and rejoins the pool")
        except httpx.HTTPError as e:
            info(f"could not release claim: {type(e).__name__}: {e}")

    def _sigint_handler(signum, frame):
        print(flush=True)
        step("Interrupted (Ctrl-C) — releasing claim before exit")
        release_claim()
        sys.exit(130)

    signal.signal(signal.SIGINT, _sigint_handler)

    try:
        # Inside the try so the finally still releases even if create/bind fails midway.
        create_claim(http(), pool, claim_name, args.bind_deadline)
        step("Waiting for the claim to bind to the warm sandbox")
        sandbox = wait_bound(http, pool, claim_name, args.bind_timeout, args.poll_interval)
        info(f"bound to sandbox {sandbox!r}")

        # Connect to EVERY MCP server the double image exposes. Best-effort for
        # dev: a server that fails to connect is logged and skipped rather than
        # aborting the whole run, so a partial action space is still usable.
        conns: list[McpConn] = []
        for spec in mcp_servers:
            svc_base = f"/api/svc/{pool}/{sandbox}-{spec.service}"
            step(f"Waiting for the {spec.label} server (svc {sandbox}-{spec.service}, health {spec.health})")
            # wait_service_ready(http, f"{svc_base}{spec.health}", args.ready_timeout, args.poll_interval)
            step(f"Opening MCP session: {spec.label} at {svc_base}{spec.mcp_path}")
            try:
                conn = mcp_open(http(), spec, svc_base, client_name=f"mini-swe-multi-mcp-{OS_NAME}")
            except Exception as e:
                info(f"could not connect to {spec.label} MCP server: {type(e).__name__}: {e} — skipping (best effort)")
                continue
            info(f"session {conn.session_id or '(stateless)'} — {len(conn.tools)} tool(s)")
            conns.append(conn)

        if not conns:
            die("no MCP servers connected — cannot run the agent")

        step(f"Running mini-swe-agent (bedrock {model_id}) against {len(conns)} MCP server(s)")
        info(f"task: {args.task}")
        result = run_agent(http, conns, bedrock, task=args.task, model_id=model_id,
                           max_steps=args.max_steps, keep_images=args.keep_images)
        step(f"Agent result: {result[:200]}")
    finally:
        print(flush=True)
        # Always release the claim — success, error, or interrupt — no matter what.
        release_claim()
        if args.keep:
            step(f"Keeping pool/namespace {pool!r} — skipping pool teardown (--keep)")
            info(f"claim again: POST {claims_url(pool)} (spec.sandboxTemplateRef.name={pool}-template)")
            info(f"delete it:   DELETE {pool_url(pool, pool)} (and DELETE /api/namespaces/{pool})")
        elif args.delete_pool:
            step(f"Tearing down pool {pool!r} (pool + namespace)")
            for what, url in ((f"pool {pool!r}", pool_url(pool, pool)),
                              (f"namespace {pool!r}", f"/api/namespaces/{pool}")):
                try:
                    r = http().delete(url)
                    info(f"deleted {what} (HTTP {r.status_code})")
                except httpx.HTTPError as e:
                    info(f"could not delete {what}: {type(e).__name__}: {e}")
        else:
            step(f"Pool {pool!r} is live")
            info(f"claim again: POST {claims_url(pool)} (spec.sandboxTemplateRef.name={pool}-template)")
            info(f"delete it:   DELETE {pool_url(pool, pool)} (and DELETE /api/namespaces/{pool})")


if __name__ == "__main__":
    main()
