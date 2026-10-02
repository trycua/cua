#!/usr/bin/env python3
"""The env-server conformance suite against a Fleet-claimed cua-spacesd,
through the Fleet gateway.

    CUA_E2E_FLEET_ENV_IMAGE=<rootfs ref> tests/e2e/cua-sdk/fleet_gateway_conformance.py --runtime gvisor

Applies a `cua-e2e-*` pool of the spacesd image, claims it with the cua SDK,
then runs `cargo test -p cua-spacesd-server --test conformance` with
CUA_ENV_TEST_TARGET = the claim's gateway service URL, the Fleet bearer in
`authorization` (CUA_ENV_TEST_GATEWAY_BEARER_FILE) and the env token in
`x-cua-env-authorization`. The pool is deleted in `finally`.

The Fleet bearer lives ~900 s and the suite can run far longer, so the bearer
is cached with its expiry and re-minted at 80% of its lifetime by a background
thread into a private temp file the harness re-reads
(CUA_ENV_TEST_GATEWAY_BEARER_FILE). The bearer is never printed.

gVisor only for now: Fleet pools have no per-claim secret field yet, so the env
token reaches a gVisor pod through an entrypoint override (the image reads
/etc/cua/env-token); a KubeVirt containerDisk guest has no such hook.

Needs Fleet OAuth credentials (CUA_CLIENT_ID/SECRET, CUA_TOKEN_URL) and the
cua Python package with its native library on PYTHONPATH.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import secrets
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from pathlib import Path

import cua

REPO = Path(__file__).resolve().parents[3]
KUBEVIRT_ENV_SKIP = (
    "KubeVirt spacesd lane needs a per-claim secret field (cloud PR): no way to "
    "deliver the env token to a containerDisk guest yet"
)


def fleet_bearer() -> tuple[str, int]:
    """A client-credentials access token and its lifetime (s)."""
    url = (
        os.environ.get("CUA_TOKEN_URL")
        or "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
    )
    body = urllib.parse.urlencode(
        {
            "grant_type": "client_credentials",
            "client_id": os.environ["CUA_CLIENT_ID"],
            "client_secret": os.environ["CUA_CLIENT_SECRET"],
        }
    ).encode()
    with urllib.request.urlopen(urllib.request.Request(url, data=body), timeout=30) as r:
        doc = json.load(r)
    return doc["access_token"], int(doc.get("expires_in", 0))


class RefreshingBearer:
    """A Fleet bearer cached with its expiry and re-minted before it lapses.

    ``start()`` writes the current bearer to a 0600 temp file and keeps it
    fresh from a daemon thread (refresh at ``refresh_fraction`` of the
    lifetime); ``stop()`` ends the thread and deletes the file.
    """

    def __init__(self, mint=fleet_bearer, refresh_fraction: float = 0.8, clock=time.monotonic):
        self._mint = mint
        self._fraction = refresh_fraction
        self._clock = clock
        self._lock = threading.Lock()
        self._token = ""
        self._lifetime = 0
        self._refresh_at = 0.0
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self.path: Path | None = None

    @property
    def lifetime(self) -> int:
        return self._lifetime

    def _refresh_delay(self, lifetime: int) -> float:
        # Unknown lifetime: assume the documented 900 s.
        return max(1.0, (lifetime or 900) * self._fraction)

    def _renew(self) -> None:
        token, lifetime = self._mint()
        with self._lock:
            self._token, self._lifetime = token, lifetime
            self._refresh_at = self._clock() + self._refresh_delay(lifetime)
        if self.path is not None:
            tmp = self.path.with_suffix(".tmp")
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(token)
            os.replace(tmp, self.path)

    def get(self) -> str:
        """The current bearer, re-minted first when it is due."""
        with self._lock:
            due = not self._token or self._clock() >= self._refresh_at
        if due:
            self._renew()
        with self._lock:
            return self._token

    def _run(self) -> None:
        while True:
            with self._lock:
                wait = max(1.0, self._refresh_at - self._clock())
            if self._stop.wait(wait):
                return
            try:
                self._renew()
            except Exception as e:  # keep the old bearer; retry shortly
                print(
                    f"bearer refresh failed ({type(e).__name__}); retrying",
                    file=sys.stderr,
                    flush=True,
                )
                with self._lock:
                    self._refresh_at = self._clock() + 30

    def start(self) -> Path:
        directory = Path(tempfile.mkdtemp(prefix="cua-e2e-bearer-"))
        self.path = directory / "bearer"
        self._renew()
        self._thread = threading.Thread(target=self._run, name="bearer-refresh", daemon=True)
        self._thread.start()
        return self.path

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=5)
        if self.path is not None:
            for p in (self.path, self.path.with_suffix(".tmp")):
                p.unlink(missing_ok=True)
            try:
                self.path.parent.rmdir()
            except OSError:
                pass


def env_token_command(token: str) -> list[str]:
    return [
        "/bin/sh",
        "-c",
        f"mkdir -p /etc/cua && printf %s {token} >/etc/cua/env-token && "
        "exec /opt/cua/desktop/entrypoint.sh",
    ]


async def main(runtime: str, image: str, test_filter: list[str]) -> int:
    if runtime != "gvisor":
        print(f"skip: {KUBEVIRT_ENV_SKIP}")
        return 0
    c = cua.embedded(fleet_from_env=True)
    fleet = c.fleet()
    run = os.environ.get("CUA_E2E_RUN") or secrets.token_hex(3)
    pool = f"cua-e2e-{run}-conf-{runtime}"[:63]
    token = secrets.token_hex(16)
    sb = None
    bearer = RefreshingBearer()
    t0 = time.monotonic()
    try:
        await fleet.apply_pool(
            cua.FleetPoolSpec(
                name=pool,
                image=image,
                runtime=runtime,
                replicas=1,
                cpu=2,
                memory_mb=4096,
                services={"env": 3211},
                command=env_token_command(token),
                ttl_seconds_after_created=7200,
            )
        )
        sb = await c.sandboxes().create(
            cua.SandboxCreateOptions(
                on="cloud",
                pool=pool,
                name=f"{pool}-c",
                token=token,
                ready_timeout_ms=1_200_000,
            )
        )
        bound = await fleet.attach_claim(pool, f"{pool}-c")
        url = fleet.service_url(bound, "env")
        bearer_file = bearer.start()
        # Wait for the driver behind the gateway (plain /health, no SDK).
        for attempt in range(60):
            req = urllib.request.Request(
                url + "/health",
                headers={
                    "authorization": f"Bearer {bearer.get()}",
                    "x-cua-fleet-claim": bound.claim,
                },
            )
            try:
                with urllib.request.urlopen(req, timeout=20) as r:
                    if r.status < 300:
                        break
            except OSError:
                pass
            if attempt == 59:
                raise RuntimeError("spacesd /health never answered through the gateway")
            await asyncio.sleep(5)
        print(
            f"claimed {bound.claim} in {time.monotonic() - t0:.0f}s; gateway {url}; "
            f"bearer lifetime {bearer.lifetime}s (refreshed at 80%)",
            flush=True,
        )
        env = dict(
            os.environ,
            CUA_ENV_TEST_TARGET=url,
            CUA_ENV_TEST_TOKEN=token,
            CUA_ENV_TEST_GATEWAY_BEARER_FILE=str(bearer_file),
            CUA_ENV_TEST_GATEWAY_CLAIM=bound.claim,
            CUA_ENV_TEST_BIG_BYTES=os.environ.get("CUA_ENV_TEST_BIG_BYTES", str(64 << 20)),
            CUA_ENV_TEST_LONG_SECS=os.environ.get("CUA_ENV_TEST_LONG_SECS", "60"),
            CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "4"),
        )
        t1 = time.monotonic()
        code = subprocess.run(
            [
                "timeout",
                "2700",
                "cargo",
                "test",
                "-p",
                "cua-spacesd-server",
                "--test",
                "conformance",
                "--",
                "--test-threads=4",
                *test_filter,
            ],
            cwd=REPO / "libs" / "cua-spacesd",
            env=env,
        ).returncode
        print(f"conformance ({runtime}) exit {code} in {time.monotonic() - t1:.0f}s", flush=True)
        return code
    finally:
        bearer.stop()
        if sb is not None:
            try:
                await sb.delete()
            except cua.CuaError as e:
                print(f"sandbox delete: {e}", file=sys.stderr)
        try:
            await fleet.delete_pool(pool)
            print(f"deleted pool {pool}", flush=True)
        except cua.CuaError as e:
            print(f"pool delete: {e}", file=sys.stderr)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--runtime", choices=["gvisor", "kubevirt"], default="gvisor")
    ap.add_argument("--image", default=os.environ.get("CUA_E2E_FLEET_ENV_IMAGE"))
    ap.add_argument("filter", nargs="*", help="cargo test name filters")
    a = ap.parse_args()
    if not a.image:
        sys.exit("set CUA_E2E_FLEET_ENV_IMAGE (the rootfs ref) or pass --image")
    sys.exit(asyncio.run(main(a.runtime, a.image, a.filter)))
