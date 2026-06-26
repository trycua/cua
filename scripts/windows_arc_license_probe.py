#!/usr/bin/env python3
"""
Windows Arc PAYG license probe against an OSGym warm pool.

End-to-end operational probe that exercises the full "license a Windows box,
prove it's alive, then unlicense it" loop against the cua cloud OSGym fleet:

  1. provision the Windows warm pool (idempotent — created only if absent)
  2. create an OSGymSandboxClaim and wait for it to bind to a VM
  3. attach the Azure Arc pay-as-you-go (PAYG) Windows Server license
  4. take a screenshot of the bound desktop
  5. upload the screenshot to S3 and mint a presigned URL
  6. POST the presigned URL to Alertmanager (am.cua.ai) as an annotation
  7. deactivate the license and return the claim to the pool (always runs)

Steps 1/2/4 talk to the K3s cluster that runs OSGym (group `osgym.cua.ai`)
through `kubectl`, so a working kubeconfig that can reach the cluster API
server is required (see the workflow for the Tailscale + KUBECONFIG wiring).
Steps 3/7 drive Azure Resource Manager through `az rest` against a Connected
Machine (`Microsoft.HybridCompute/machines/<machine>/licenseProfiles/default`).

The license toggle operates on a *pre-onboarded* Arc machine named by
`--arc-machine` / ARC_MACHINE. That machine must already be Connected to Arc
and converted to a full (non-eval) Windows Server 2025 edition — onboarding and
the eval->full DISM conversion are one-time manual setup, not part of this
recurring probe. See docs/ in trycua/cloud for the onboarding runbook.

Everything is configurable via flags or the matching UPPER_SNAKE env var.
The cleanup (step 7) runs in a finally block so the license is disabled and
the claim is returned even if an earlier step fails.
"""

from __future__ import annotations

import argparse
import base64
import json
import logging
import os
import socket
import subprocess
import sys
import time
import uuid
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone

import boto3
import requests

log = logging.getLogger("windows-arc-license-probe")

# OSGym CRD coordinates (group osgym.cua.ai/v1alpha1; see trycua/cloud).
API_VERSION = "osgym.cua.ai/v1alpha1"


# --------------------------------------------------------------------------- #
# config
# --------------------------------------------------------------------------- #
def _env(name: str, default: str | None = None) -> str | None:
    return os.environ.get(name, default)


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)

    # OSGym pool / claim
    p.add_argument("--namespace", default=_env("OSGYM_NAMESPACE", "osgym"))
    p.add_argument("--pool", default=_env("OSGYM_POOL", "arc-windows"))
    p.add_argument("--template", default=_env("OSGYM_TEMPLATE", "arc-windows-template"))
    p.add_argument(
        "--image",
        default=_env(
            "OSGYM_WINDOWS_IMAGE",
            "296062593712.dkr.ecr.us-west-2.amazonaws.com/cua-server-windows:latest",
        ),
        help="KubeVirt containerDisk image the pool template boots.",
    )
    p.add_argument("--image-pull-secret", default=_env("OSGYM_IMAGE_PULL_SECRET", "ecr-credentials"))
    p.add_argument("--cpu-cores", type=int, default=int(_env("OSGYM_CPU_CORES", "4")))
    p.add_argument("--memory", default=_env("OSGYM_MEMORY", "8Gi"))
    p.add_argument(
        "--api-port",
        type=int,
        default=int(_env("OSGYM_API_PORT", "8000")),
        help="In-guest computer-server port that serves POST /cmd (8000 for "
        "cua-server-windows; 5000 for desktop-workspace-windows).",
    )
    p.add_argument("--bind-timeout", type=int, default=int(_env("OSGYM_BIND_TIMEOUT", "900")),
                   help="Seconds to wait for the claim to reach phase=Bound.")
    p.add_argument("--claim-ttl", type=int, default=int(_env("OSGYM_CLAIM_TTL", "1200")),
                   help="Seconds before the claim's shutdownTime (reaper backstop).")

    # Azure Arc licensing
    p.add_argument("--subscription", default=_env("AZURE_SUBSCRIPTION_ID"))
    p.add_argument("--resource-group", default=_env("ARC_RESOURCE_GROUP", "arc-trial-rg"))
    p.add_argument("--region", default=_env("ARC_REGION", "eastus"))
    p.add_argument("--arc-machine", default=_env("ARC_MACHINE"),
                   help="Name of the pre-onboarded Arc Connected Machine to license.")
    p.add_argument("--arc-api-version", default=_env("ARC_API_VERSION", "2023-10-03-preview"))
    p.add_argument("--skip-license", action="store_true", default=_env("SKIP_LICENSE") == "1",
                   help="Skip steps 3 & 7 (useful when no Arc machine is wired up yet).")

    # S3
    p.add_argument("--s3-bucket", default=_env("S3_BUCKET_NAME", "cua-agent-artifacts"))
    p.add_argument("--s3-region", default=_env("AWS_REGION", "us-west-2"))
    p.add_argument("--s3-prefix", default=_env("S3_PREFIX", "windows-arc-license-probe"))
    p.add_argument("--presign-ttl", type=int, default=int(_env("PRESIGN_TTL", "3600")))

    # Alertmanager
    p.add_argument("--alertmanager-url", default=_env("ALERTMANAGER_URL", "https://am.cua.ai"))
    p.add_argument("--severity", default=_env("ALERT_SEVERITY", "info"))

    p.add_argument("--keep-claim", action="store_true",
                   help="Do not delete the claim at the end (debugging).")
    p.add_argument("-v", "--verbose", action="store_true")
    return p.parse_args(argv)


# --------------------------------------------------------------------------- #
# subprocess helpers
# --------------------------------------------------------------------------- #
def run(cmd: list[str], *, input_str: str | None = None, check: bool = True,
        capture: bool = True) -> subprocess.CompletedProcess:
    log.debug("$ %s", " ".join(cmd))
    return subprocess.run(
        cmd,
        input=input_str,
        text=True,
        capture_output=capture,
        check=check,
    )


def kubectl(args: list[str], *, namespace: str | None, input_str: str | None = None,
            check: bool = True) -> subprocess.CompletedProcess:
    cmd = ["kubectl"]
    if namespace:
        cmd += ["-n", namespace]
    cmd += args
    return run(cmd, input_str=input_str, check=check)


def kubectl_apply(manifest: dict, *, namespace: str) -> None:
    kubectl(["apply", "-f", "-"], namespace=namespace, input_str=json.dumps(manifest))


def az_rest(method: str, url: str, body: dict | None = None) -> dict:
    cmd = ["az", "rest", "--method", method, "--url", url]
    if body is not None:
        cmd += ["--body", json.dumps(body)]
    cp = run(cmd, check=True)
    out = (cp.stdout or "").strip()
    return json.loads(out) if out else {}


# --------------------------------------------------------------------------- #
# step 1: provision pool
# --------------------------------------------------------------------------- #
def ensure_pool(a: argparse.Namespace) -> None:
    exists = kubectl(["get", "oswp", a.pool], namespace=a.namespace, check=False)
    if exists.returncode == 0:
        log.info("warm pool %s/%s already exists", a.namespace, a.pool)
        return

    log.info("provisioning warm pool %s/%s (template=%s, image=%s)",
             a.namespace, a.pool, a.template, a.image)

    template = {
        "apiVersion": API_VERSION,
        "kind": "OSGymSandboxTemplate",
        "metadata": {"name": a.template, "namespace": a.namespace},
        "spec": {
            "vmTemplate": {
                "containerDiskImage": a.image,
                "imagePullSecret": a.image_pull_secret,
                "cpuCores": a.cpu_cores,
                "memory": a.memory,
                # cua-server-windows boots BIOS; flip to "efi" for dockur EFI images.
                "firmware": "bios",
                # computer-server HTTP/cmd API + cua-driver MCP + noVNC, each
                # fronted by a ClusterIP Service on :80 -> targetPort.
                "services": [
                    {"name": "api", "targetPort": a.api_port},
                    {"name": "mcp", "targetPort": 3000},
                    {"name": "novnc", "targetPort": 6080},
                ],
                # Gate readiness on computer-server actually serving.
                "probes": {
                    "readinessProbe": {
                        "tcpSocket": {"port": a.api_port},
                        "initialDelaySeconds": 60,
                        "periodSeconds": 5,
                        "failureThreshold": 120,
                    }
                },
            }
        },
    }
    warmpool = {
        "apiVersion": API_VERSION,
        "kind": "OSGymSandboxWarmPool",
        "metadata": {"name": a.pool, "namespace": a.namespace},
        "spec": {
            "replicas": 0,
            "sandboxTemplateRef": {"name": a.template},
            # Static floor of 0; a Pending claim is the demand signal that
            # scales the pool up on demand.
            "autoscaling": {"minPoolSize": 0, "initialPoolSize": 0, "maxPoolSize": 4},
        },
    }
    kubectl_apply(template, namespace=a.namespace)
    kubectl_apply(warmpool, namespace=a.namespace)
    log.info("warm pool %s provisioned", a.pool)


# --------------------------------------------------------------------------- #
# step 2: create & bind claim
# --------------------------------------------------------------------------- #
def create_claim(a: argparse.Namespace) -> str:
    claim_name = f"{a.pool}-claim-{uuid.uuid4().hex[:10]}"
    shutdown = datetime.now(tz=timezone.utc) + timedelta(seconds=a.claim_ttl)
    claim = {
        "apiVersion": API_VERSION,
        "kind": "OSGymSandboxClaim",
        "metadata": {"name": claim_name, "namespace": a.namespace},
        "spec": {
            "sandboxTemplateRef": {"name": a.template},
            "warmpool": a.pool,
            "bindDeadline": a.bind_timeout,
            "lifecycle": {
                "shutdownTime": shutdown.isoformat().replace("+00:00", "Z"),
                # Retain: returning the claim restarts the VM in place and
                # hands it back to the warm pool.
                "shutdownPolicy": "Retain",
            },
        },
    }
    kubectl_apply(claim, namespace=a.namespace)
    log.info("created claim %s", claim_name)
    return claim_name


def wait_for_bind(a: argparse.Namespace, claim_name: str) -> tuple[str, str]:
    deadline = time.monotonic() + a.bind_timeout
    while time.monotonic() < deadline:
        cp = kubectl(["get", "osbc", claim_name, "-o", "json"],
                     namespace=a.namespace, check=False)
        if cp.returncode == 0:
            status = json.loads(cp.stdout).get("status", {})
            phase = status.get("phase")
            if phase == "Bound":
                sb = status.get("sandbox", {})
                log.info("claim %s bound to sandbox %s (%s)",
                         claim_name, sb.get("name"), sb.get("service"))
                return sb["name"], sb.get("service", "")
            if phase == "Failed":
                raise RuntimeError(f"claim {claim_name} reached phase=Failed: {status}")
            log.info("claim %s phase=%s, waiting...", claim_name, phase)
        time.sleep(5)
    raise TimeoutError(f"claim {claim_name} did not bind within {a.bind_timeout}s")


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


def set_license(a: argparse.Namespace, *, enabled: bool) -> None:
    status = "Enabled" if enabled else "Disabled"
    log.info("setting Arc PAYG license on %s -> %s", a.arc_machine, status)
    body = {
        "location": a.region,
        "properties": {
            "productProfile": {"productType": "WindowsServer", "subscriptionStatus": status}
        },
    }
    az_rest("put", _license_url(a), body)


def license_status(a: argparse.Namespace) -> str:
    try:
        resp = az_rest("get", _license_url(a))
        return resp.get("properties", {}).get("productProfile", {}).get("subscriptionStatus", "")
    except Exception as e:  # noqa: BLE001
        log.warning("could not read license status: %s", e)
        return ""


# --------------------------------------------------------------------------- #
# step 4: screenshot via port-forward to the bound VM's computer-server
# --------------------------------------------------------------------------- #
def _free_local_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@contextmanager
def port_forward(a: argparse.Namespace, sandbox: str):
    """kubectl port-forward to the sandbox's `-api` Service (listens on :80)."""
    local = _free_local_port()
    svc = f"svc/{sandbox}-api"
    proc = subprocess.Popen(
        ["kubectl", "-n", a.namespace, "port-forward", svc, f"{local}:80"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    try:
        # Wait for the local listener to come up.
        for _ in range(60):
            try:
                with socket.create_connection(("127.0.0.1", local), timeout=1):
                    break
            except OSError:
                if proc.poll() is not None:
                    out = proc.stdout.read() if proc.stdout else ""
                    raise RuntimeError(f"port-forward to {svc} exited early:\n{out}")
                time.sleep(1)
        else:
            raise TimeoutError(f"port-forward to {svc} never became reachable")
        log.info("port-forward %s -> 127.0.0.1:%d ready", svc, local)
        yield local
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()


def take_screenshot(a: argparse.Namespace, sandbox: str) -> bytes:
    with port_forward(a, sandbox) as local:
        url = f"http://127.0.0.1:{local}/cmd"
        # computer-server may take a moment after readiness; retry briefly.
        last_err: Exception | None = None
        for attempt in range(12):
            try:
                resp = requests.post(
                    url,
                    json={"command": "screenshot", "params": {}},
                    timeout=30,
                )
                resp.raise_for_status()
                # Response is an SSE-style chunk: `data: {json}` (one line).
                payload = _parse_cmd_response(resp.text)
                if not payload.get("success"):
                    raise RuntimeError(f"screenshot command failed: {payload}")
                b64 = payload["image_data"]
                png = base64.b64decode(b64)
                if png[:4] != b"\x89PNG":
                    log.warning("screenshot is not a PNG (got %r)", png[:8])
                log.info("captured screenshot (%d bytes)", len(png))
                return png
            except Exception as e:  # noqa: BLE001
                last_err = e
                log.info("screenshot attempt %d failed: %s", attempt + 1, e)
                time.sleep(5)
        raise RuntimeError(f"screenshot failed after retries: {last_err}")


def _parse_cmd_response(text: str) -> dict:
    text = text.strip()
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("data:"):
            return json.loads(line[len("data:"):].strip())
    # Some builds return bare JSON.
    return json.loads(text)


# --------------------------------------------------------------------------- #
# step 5: S3 upload + presigned URL
# --------------------------------------------------------------------------- #
def upload_and_presign(a: argparse.Namespace, png: bytes, sandbox: str) -> str:
    ts = datetime.now(tz=timezone.utc).strftime("%Y-%m-%d/%H%M%SZ")
    key = f"{a.s3_prefix}/{ts}/{sandbox}.png"
    s3 = boto3.client("s3", region_name=a.s3_region)
    s3.put_object(Bucket=a.s3_bucket, Key=key, Body=png, ContentType="image/png")
    url = s3.generate_presigned_url(
        "get_object",
        Params={"Bucket": a.s3_bucket, "Key": key},
        ExpiresIn=a.presign_ttl,
    )
    log.info("uploaded s3://%s/%s and signed URL (ttl=%ds)", a.s3_bucket, key, a.presign_ttl)
    return url


# --------------------------------------------------------------------------- #
# step 6: post to Alertmanager
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
def main(argv: list[str]) -> int:
    a = parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if a.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    do_license = not a.skip_license and bool(a.arc_machine) and bool(a.subscription)
    if not a.skip_license and not do_license:
        log.warning(
            "license steps disabled: ARC_MACHINE/AZURE_SUBSCRIPTION_ID not set "
            "(pass --skip-license to silence)"
        )

    # Step 1 & 2
    ensure_pool(a)
    claim_name = create_claim(a)

    licensed = False
    try:
        sandbox, _service = wait_for_bind(a, claim_name)

        # Step 3
        if do_license:
            set_license(a, enabled=True)
            licensed = True

        # Step 4 & 5
        png = take_screenshot(a, sandbox)
        signed_url = upload_and_presign(a, png, sandbox)

        # Step 6
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
            cp = kubectl(["delete", "osbc", claim_name, "--ignore-not-found"],
                         namespace=a.namespace, check=False)
            if cp.returncode == 0:
                log.info("deleted claim %s (returned to pool)", claim_name)
            else:
                log.warning("failed to delete claim %s: %s", claim_name, cp.stderr)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
