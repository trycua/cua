#!/usr/bin/env python3
"""Smoke one variant of a benchmark image through the cua SDK (the path
``cb run`` uses), locally or on Fleet.

    smoke.py ID --variant rootfs|containerdisk [--image REF | --disk PATH]
             [--cloud] [--evidence DIR] [--memory-mb N]

Local: rootfs defaults to ``cua-e2e-local/bench-ID:docker-local-<host arch>``
(the SDK runs it under gVisor); containerdisk defaults to
``$CUA_BENCH_WORK/ID/out/<arch>/disk.img`` under QEMU (one VM, at most
4 GiB). ``--cloud`` applies a pool ``cua-e2e-bench-ID-<rand>`` (gVisor for
rootfs, KubeVirt for containerdisk) from ``--image`` (a registry ref),
claims once and deletes the pool in ``finally``.

Checks, recorded in ``EVIDENCE/<variant>-<where>.json`` with screenshots:
  server       the bench server's health probe (bench.json server.health)
  spacesd       cua-spacesd GetCapabilities
  screen       screenshot size matches the desktop and is not blank
  input        a cua-driver click changes the screen (benchmark hook)
  bench        the benchmark's own checks (libs/images/bench/ID/smoke.py)
  doctor       `cua-spacesd doctor --json` when the image's spacesd has it
Exit 0 only when every check passes (the doctor is skipped, not failed, on
spacesd builds that predate it).
"""

from __future__ import annotations

import argparse
import asyncio
import importlib.util
import io
import json
import os
import platform
import sys
import time
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import bench as benchlib  # noqa: E402


def host_arch() -> str:
    return "arm64" if platform.machine().lower() in ("arm64", "aarch64") else "amd64"


class Report:
    def __init__(self) -> None:
        self.checks: list[dict] = []

    def add(self, name: str, ok: bool | None, detail: object = "", t0: float | None = None) -> None:
        row = {"check": name, "status": "pass" if ok else ("skip" if ok is None else "fail"),
               "detail": detail}
        if t0 is not None:
            row["seconds"] = round(time.monotonic() - t0, 2)
        self.checks.append(row)
        print(f"  {row['status']:4s} {name}: {str(detail)[:300]}", flush=True)

    @property
    def ok(self) -> bool:
        return all(c["status"] != "fail" for c in self.checks)


def png_info(data: bytes) -> tuple[int, int, float]:
    """(width, height, fraction of distinct 8x8-sampled colors) of a PNG."""
    from PIL import Image

    im = Image.open(io.BytesIO(data)).convert("RGB")
    w, h = im.size
    small = im.resize((64, 64))
    colors = len(set(small.get_flattened_data() if hasattr(small, 'get_flattened_data') else small.getdata()))
    return w, h, colors / 4096.0


def screen_diff(a: bytes, b: bytes) -> float:
    """Fraction of pixels that differ between two screenshots (downscaled)."""
    from PIL import Image, ImageChops

    ia = Image.open(io.BytesIO(a)).convert("L").resize((160, 90))
    ib = Image.open(io.BytesIO(b)).convert("L").resize((160, 90))
    diff = ImageChops.difference(ia, ib)
    px = diff.get_flattened_data() if hasattr(diff, "get_flattened_data") else diff.getdata()
    return sum(1 for p in px if p > 16) / (160 * 90)


def load_hooks(bench_id: str):
    path = benchlib.BENCH_ROOT / bench_id / "smoke.py"
    if not path.is_file():
        return None
    spec = importlib.util.spec_from_file_location(f"bench_smoke_{bench_id}", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


async def run_checks(sb, b: dict, hooks, report: Report, evidence: Path, tag: str) -> None:
    server = b.get("server") or {}
    method, _, path = (server.get("health") or "GET /").partition(" ")
    t0 = time.monotonic()
    last = None
    for _ in range(90):  # bounded: ~3 minutes for the server to come up after readiness
        try:
            r = await sb.service("server").request(method, path or "/", timeout=20)
            last = r.status_code
            if r.status_code == 200:
                break
        except Exception as e:  # noqa: BLE001 - still booting
            last = repr(e)[:200]
        await asyncio.sleep(2)
    report.add("server", last == 200, f"{method} {path} on :{server.get('port')} -> {last}", t0)

    t0 = time.monotonic()
    try:
        g = await sb.spacesd()
        caps = json.loads(await g.call_json("SystemService/GetCapabilities", "{}"))
        (evidence / f"{tag}-capabilities.json").write_text(json.dumps(caps, indent=2))
        report.add("spacesd", True, f"GetCapabilities: {', '.join(sorted(caps))[:200]}", t0)
    except Exception as e:  # noqa: BLE001
        report.add("spacesd", False, repr(e)[:300], t0)

    t0 = time.monotonic()
    shot = b""
    try:
        # Readiness means spacesd answers; the desktop may still be painting.
        # A VM's spacesd may still await its token (cloud-init) when the
        # bench server already answers, so early calls are retried too.
        err = None
        # bounded: ~6 min (a GNOME session on a 2-vCPU CI runner paints after ~2-3 min)
        for _ in range(180):
            try:
                shot = await sb.screenshot()
            except Exception as e:  # noqa: BLE001
                err = e
                await asyncio.sleep(2)
                continue
            w, h, variety = png_info(shot)
            if variety > 0.002:
                break
            await asyncio.sleep(2)
        if not shot:
            raise RuntimeError(f"no screenshot: {err!r}")
        (evidence / f"{tag}-screen.png").write_bytes(shot)
        want = (b.get("desktop") or {}).get("resolution")
        size_ok = want is None or f"{w}x{h}" == want
        report.add("screen", size_ok and variety > 0.002,
                   f"{w}x{h} (want {want}), color variety {variety:.3f}", t0)
    except Exception as e:  # noqa: BLE001
        report.add("screen", False, repr(e)[:300], t0)

    if hooks is not None and hasattr(hooks, "input_probe"):
        t0 = time.monotonic()
        try:
            detail = await hooks.input_probe(sb, evidence, tag, screen_diff)
            report.add("input", bool(detail.get("ok")), detail, t0)
        except Exception as e:  # noqa: BLE001
            report.add("input", False, repr(e)[:300], t0)
    if hooks is not None and hasattr(hooks, "bench_checks"):
        t0 = time.monotonic()
        try:
            for name, ok, detail in await hooks.bench_checks(sb, evidence, tag):
                report.add(f"bench.{name}", ok, detail)
        except Exception as e:  # noqa: BLE001
            report.add("bench", False, repr(e)[:300], t0)

    t0 = time.monotonic()
    try:
        # The image's own claims (/etc/cua-image/manifest.json) are the bar:
        # --strict fails on warnings too. Publishing is gated on this check.
        # --effects virtual: the doctor may drive the image's own fixture
        # windows (never anything a task left open; the sandbox is fresh).
        # As root when the guest allows it (passwordless sudo), like a pod
        # probe: the claim token directory is root-only (0700).
        r = await sb.shell.run("S=; [ \"$(id -u)\" = 0 ] || { sudo -n true 2>/dev/null && S='sudo -n'; }; "
                               # Images staged before the cua-guestd -> cua-spacesd rename ship the
                               # same daemon (and doctor flags) under the old name.
                               "D=$(command -v cua-spacesd || command -v cua-guestd || echo cua-spacesd); "
                               "$S \"$D\" doctor --strict --effects virtual --json >/tmp/cua-doctor.json 2>/tmp/cua-doctor.err; "
                               "echo \"rc=$?\"; cat /tmp/cua-doctor.json", timeout=600)
        out = getattr(r, "stdout", "") or ""
        rc = out.split("rc=", 1)[1].split()[0] if "rc=" in out else "?"
        report_json = out.split("\n", 1)[1] if "\n" in out else ""
        (evidence / f"{tag}-doctor.json").write_text(report_json)
        detail = f"rc={rc} (report in {tag}-doctor.json)"
        try:
            doc = json.loads(report_json)
            fails = [c.get("id") for c in doc.get("checks", []) if c.get("status") in ("fail", "warn")]
            detail += f"; not passing: {fails[:12]}" if fails else "; all checks pass"
        except ValueError:
            err = await sb.shell.run("tail -c 600 /tmp/cua-doctor.err", timeout=30)
            detail += f"; stderr: {(getattr(err, 'stdout', '') or '').strip()[:300]}"
        report.add("doctor", rc == "0", detail, t0)
    except Exception as e:  # noqa: BLE001
        report.add("doctor", False, f"not run: {e!r}"[:300], t0)


def child_for(index_ref: str, variant: str, arch: str) -> tuple[str, str]:
    """(index that holds the variant, repo@child digest for arch), read with crane."""
    import subprocess

    def manifest(ref: str) -> dict:
        env = dict(os.environ)
        if os.environ.get("CUA_BENCH_REGISTRY_CONFIG"):  # crane only, never the docker CLI
            env["DOCKER_CONFIG"] = os.environ["CUA_BENCH_REGISTRY_CONFIG"]
        return json.loads(subprocess.run(["crane", "manifest", ref], check=True,
                                         capture_output=True, text=True, env=env).stdout)

    repo = index_ref.split("@", 1)[0]
    idx = manifest(index_ref)
    if variant == "containerdisk" and (idx.get("annotations") or {}).get("ai.cua.image.variant") != "containerdisk":
        link = json.loads(idx["annotations"]["ai.cua.image.variants"])["containerdisk"]
        index_ref, idx = link, manifest(link)
    for m in idx["manifests"]:
        if (m.get("platform") or {}).get("architecture") == arch:
            return index_ref, f"{repo}@{m['digest']}"
    raise SystemExit(f"{index_ref} has no {arch} child")


async def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("id")
    ap.add_argument("--variant", required=True, choices=["rootfs", "containerdisk"])
    ap.add_argument("--image", help="registry ref (default: the local build)")
    ap.add_argument("--disk", help="local disk.img for the containerdisk variant")
    ap.add_argument("--cloud", action="store_true", help="run on Fleet (needs --image)")
    ap.add_argument("--memory-mb", type=int, default=None)
    ap.add_argument("--cpu", type=int, default=None,
                    help="vCPUs (default 4; locally capped at the host's CPU count, e.g. 2 on private-repo CI runners)")
    ap.add_argument("--evidence", default=None)
    ap.add_argument("--arch", choices=["amd64", "arm64"],
                    help="local: run this arch (a foreign one is emulated; rootfs then needs --runc)")
    ap.add_argument("--runc", action="store_true",
                    help="rootfs under runc instead of gVisor (a foreign arch under emulation: "
                         "runsc cannot run an emulated amd64 image on an arm64 host)")
    args = ap.parse_args()
    if args.cpu is None:
        args.cpu = 4 if args.cloud else max(1, min(4, os.cpu_count() or 1))

    from cua_sandbox import Image, Sandbox

    b = benchlib.load(args.id)
    hooks = load_hooks(args.id)
    where = "fleet" if args.cloud else "local"
    evidence = Path(args.evidence or (benchlib.BENCH_ROOT / args.id / "evidence"))
    evidence.mkdir(parents=True, exist_ok=True)
    server = b.get("server") or {}
    ports = sorted(set((b.get("extra_ports") or {}).values()) | ({server["port"]} if server else set()))
    kind = "container" if args.variant == "rootfs" else "vm"
    # Fleet runs amd64; locally the host arch when the benchmark has it, else
    # its only arch under emulation (OSWorld is amd64-only).
    arch = "amd64" if args.cloud else (args.arch or (host_arch() if host_arch() in b["arch"] else b["arch"][0]))
    tag = f"{args.variant}-{where}-{arch}"
    work = Path(os.environ.get("CUA_BENCH_WORK", "~/.cache/cua-bench-images")).expanduser()

    if args.cloud and not args.image:
        ap.error("--cloud needs --image (a pushed registry ref)")
    child = None
    index_ref = args.image
    if args.image and args.arch and not args.cloud and "@sha256:" in args.image:
        # The SDK runs the host arch of an index; for another arch, run that
        # arch's child by digest (for the disk: in the index its
        # ai.cua.image.variants names) and record which index it belongs to.
        index_ref, child = child_for(args.image, args.variant, args.arch)
        print(f"   {args.arch} {args.variant} child of {index_ref}: {child}", flush=True)
    if args.variant == "rootfs" or args.image:
        ref = child or args.image or f"cua-e2e-local/bench-{args.id}:docker-local-{arch}"
        # A private package pulled locally: CUA_REGISTRY_USERNAME/PASSWORD
        # (RegistrySecret.from_env) go to the local pull only. Never attached
        # to a Fleet pool here (that would store them in the cloud).
        secret = None
        if not args.cloud and os.environ.get("CUA_REGISTRY_PASSWORD"):
            from cua_sandbox import RegistrySecret
            secret = RegistrySecret.from_env()
        image = Image.from_registry(ref, os_type=b["os"], kind=kind, secret=secret)
    else:
        disk = Path(args.disk or work / args.id / "out" / arch / "disk.img").expanduser()
        ref = str(disk)
        image = Image.from_file(str(disk), os_type=b["os"], kind="vm")
    for p in ports:
        image = image.expose(p)
    mem = args.memory_mb or (8192 if args.cloud else 4096)
    if not args.cloud and mem > 4096:
        ap.error("local smokes stay at or under 4096 MiB (AGENT_BRIEF memory rule)")
    report = Report()
    pinned = None
    print(f"== smoke {args.id} {args.variant} {where}: {ref}", flush=True)
    started = time.monotonic()
    if args.cloud:
        from cua_sandbox import Pool, PoolOptions, SandboxSpec, generate_claim_token

        name = f"cua-e2e-bench-{args.id}-{uuid.uuid4().hex[:6]}"
        runtime = "gvisor" if args.variant == "rootfs" else "kubevirt"
        services = {"env": 3211, "server": server["port"]}
        services.update({f"port-{p}": p for p in ports if p != server["port"]})
        print(f"   pool {name} ({runtime})", flush=True)
        pool = await Pool.apply(
            name,
            SandboxSpec(image=image, cpu=args.cpu, memory_mb=mem, services=services, claim_secrets=True),
            PoolOptions(runtime=runtime, replicas=0, max_pool_size=1),
        )
        try:
            async with pool.claim(name=f"{name}-c1", claim_token=generate_claim_token(),
                                  time_to_start=1500, ttl_seconds_after_created=3600) as sb:
                report.add("claim", True, f"bound after {time.monotonic() - started:.0f}s")
                info = getattr(sb, "image_info", None)
                pinned = getattr(info, "pinned_ref", None)
                report.add("image_info", "sha256:" in str(pinned or ""), pinned)
                await run_checks(sb, b, hooks, report, evidence, tag)
        finally:
            print(f"   deleting pool {name}", flush=True)
            await pool.delete()
    else:
        extra = {"runtime": "runc"} if args.runc else {}
        async with Sandbox.ephemeral(image, local=True, name=f"cua-e2e-bench-{args.id}-{uuid.uuid4().hex[:6]}", **extra,
                                     server_port=server.get("port"), cpu=args.cpu, memory_mb=mem,
                                     time_to_start=1500, telemetry_enabled=False) as sb:
            report.add("ready", True, f"after {time.monotonic() - started:.0f}s")
            info = getattr(sb, "image_info", None)
            pinned = getattr(info, "pinned_ref", None)
            await run_checks(sb, b, hooks, report, evidence, tag)
    if child:
        # Evidence is keyed by the index the child was read from.
        pinned = index_ref
    out = {"id": args.id, "variant": args.variant, "where": where, "arch": arch, "image": ref,
           "child": child,
           "container_runtime": ("runc" if args.runc else "auto (gVisor when available)") if args.variant == "rootfs" else None,
           "pinned_ref": pinned, "doctor": next((c["status"] for c in report.checks if c["check"] == "doctor"), None),
           "seconds": round(time.monotonic() - started, 1), "ok": report.ok, "checks": report.checks,
           "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    (evidence / f"{tag}.json").write_text(json.dumps(out, indent=2) + "\n")
    print(f"== {'PASS' if report.ok else 'FAIL'} {args.id} {tag} ({out['seconds']}s) -> {evidence}/{tag}.json")
    return 0 if report.ok else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
