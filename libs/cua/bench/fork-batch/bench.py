#!/usr/bin/env python3
"""Batched-fork benchmark: `Sandboxes.fork_many` through the cua SDK and a
running `cua daemon` (the path a user or an SDK takes).

It forks a checkpoint N times (optionally baking one first from a VM image),
streams every event, and writes what happened and how fast:

    total_ms            batch start to its summary
    first_ready_ms      batch start to the first ready fork
    fork_ms_p50 / p95   each fork's own time on its machine
    per_host            ready forks per machine
    peak_parallel       most forks starting at once per machine
    groups_split        groups whose forks went to more than one machine
    sync                checkpoint copies: bytes, source, verified
    failed              forks that failed (with codes)

Usage:
    python3 bench.py --source <checkpoint> --count 64 --group-size 8 --on auto \\
        --parallel 4 --out results/run.json
    python3 bench.py --bake vm:/path/disk.img --count 8 --on local --out r.json
    python3 bench.py --worlds w0,w1,w2 --per-world 8 --on auto --out r.json

`--bake IMAGE` creates a VM Space from IMAGE, waits for its cua-spacesd,
takes a memory checkpoint and deletes the Space (the checkpoint stays
until the end of the run). Every fork the run makes is deleted at the end
(unless --keep). Then `check.py` compares a result with baseline.json.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import secrets
import statistics
import sys
import time

import cua


def pct(values, p):
    if not values:
        return None
    s = sorted(values)
    k = max(0, min(len(s) - 1, int(round(p / 100 * (len(s) - 1)))))
    return s[k]


async def bake(c, image: str, cpus: int, memory_mb: int, kind=None, runtime=None) -> str:
    sbx = c.sandboxes()
    name = f"bench-fb-{secrets.token_hex(3)}"
    token = secrets.token_hex(16)
    t0 = time.monotonic()
    await sbx.create(
        cua.SandboxCreateOptions(
            on="local",
            image=image,
            name=name,
            token=token,
            env={"CUA_ENV_TOKEN": token},
            cpus=cpus,
            memory_mb=memory_mb,
            ports=[3211],
            wait_for=[cua.ReadinessProbe(port=3211, http_path="/viewer/")],
            ready_timeout_ms=900_000,
            kind=kind,
            runtime=runtime,
            forkable=True,
        )
    )
    boot = time.monotonic() - t0
    ck = f"{name}-ck"
    t1 = time.monotonic()
    await sbx.checkpoint(name, ck, True)
    print(f"baked {ck}: boot {boot:.1f}s, checkpoint {time.monotonic() - t1:.1f}s", file=sys.stderr)
    await sbx.delete(name)
    return ck


async def run(args) -> dict:
    c = cua.connect() if not args.embedded else cua.embedded()
    sbx = c.sandboxes()
    baked = None
    if args.bake:
        baked = await bake(c, args.bake, args.cpus, args.memory_mb, args.kind, args.runtime)
    fallback = None
    if args.fallback_dir or args.fallback_bucket:
        fallback = cua.SnapshotFallbackOptions(
            dir=args.fallback_dir or "",
            s3_bucket=args.fallback_bucket or "",
            s3_prefix=args.fallback_prefix or "",
            s3_region=args.fallback_region or "",
        )
    opts = cua.ForkManyOptions(
        on=args.on,
        group_size=args.group_size,
        max_parallel_per_host=args.parallel,
        name_prefix=f"bench-fb-{secrets.token_hex(2)}",
        cpus=args.fork_cpus,
        fallback=fallback,
    )
    t0 = time.monotonic()
    if args.worlds:
        groups = [cua.ForkGroup(source=w, count=args.per_world) for w in args.worlds.split(",")]
        batch = await sbx.fork_groups(groups, opts)
        count = args.per_world * len(groups)
    else:
        source = baked or args.source
        batch = await sbx.fork_many(source, args.count, opts)
        count = args.count
    items, placements, syncs, summary = [], [], [], None
    first_ready = None
    while (ev := await batch.next()) is not None:
        now = (time.monotonic() - t0) * 1000
        if ev.is_placement():
            p = ev.placement
            placements.append(
                {
                    "group": p.group,
                    "source": p.source,
                    "host": p.host,
                    "count": p.count,
                    "attempt": p.attempt,
                    "needs_sync": p.needs_sync,
                    "reason": p.reason,
                }
            )
            print(f"[{now:8.0f} ms] place group {p.group} ({p.source}) x{p.count} -> {p.host}: {p.reason}", file=sys.stderr)
        elif ev.is_sync():
            s = ev.progress
            if s.done:
                src = getattr(s, "from_", None) or getattr(s, "from", "")
                syncs.append(
                    {
                        "name": s.name,
                        "digest": s.digest,
                        "from": src,
                        "bytes_done": s.bytes_done,
                        "bytes_total": s.bytes_total,
                        "bytes_reused": s.bytes_reused,
                        "verified": s.verified,
                    }
                )
                print(f"[{now:8.0f} ms] copied {s.name} ({src}): {s.bytes_done} bytes, reused {s.bytes_reused}, verified={s.verified}", file=sys.stderr)
        elif ev.is_item():
            i = ev.item
            if not i.error and first_ready is None:
                first_ready = i.elapsed_ms
            items.append(
                {
                    "index": i.index,
                    "group": i.group,
                    "source": i.source,
                    "name": i.name,
                    "host": i.host,
                    "space": i.space,
                    "error": i.error,
                    "error_code": i.error_code,
                    "attempt": i.attempt,
                    "elapsed_ms": i.elapsed_ms,
                    "fork_ms": i.fork_ms,
                }
            )
            what = f"ready on {i.host}" if not i.error else f"FAILED ({i.error_code}: {i.error})"
            print(f"[{now:8.0f} ms] fork {i.index:3d} {i.name}: {what} fork {i.fork_ms} ms (attempt {i.attempt})", file=sys.stderr)
        elif ev.is_done():
            summary = ev.summary
    total_ms = (time.monotonic() - t0) * 1000
    ok = [i for i in items if not i["error"]]
    hosts_per_group = {}
    for i in ok:
        hosts_per_group.setdefault((i["source"], i["group"]), set()).add(i["host"])
    out = {
        "on": args.on,
        "count": count,
        "group_size": args.group_size,
        "parallel": args.parallel,
        "ok": len(ok),
        "failed": len(items) - len(ok),
        "failures": sorted({i["error_code"] for i in items if i["error"]}),
        "total_ms": round(total_ms),
        "first_ready_ms": first_ready,
        "fork_ms_p50": pct([i["fork_ms"] for i in ok], 50),
        "fork_ms_p95": pct([i["fork_ms"] for i in ok], 95),
        "fork_ms_mean": round(statistics.mean([i["fork_ms"] for i in ok])) if ok else None,
        "per_host": dict(summary.per_host) if summary else {},
        "peak_parallel": dict(summary.peak_parallel) if summary else {},
        "groups_split": sorted(f"{s}#{g}" for (s, g), h in hosts_per_group.items() if len(h) > 1),
        "replaced": sum(1 for i in ok if i["attempt"] > 1),
        "sync": syncs,
        "placements": placements,
        "items": sorted(items, key=lambda i: i["index"]),
    }
    if not args.keep:
        spaces = c.spaces()
        for i in ok:
            try:
                await spaces.delete(i["space"])
            except cua.CuaError as e:  # noqa: PERF203 - report and go on
                print(f"could not delete {i['space']}: {e}", file=sys.stderr)
        if baked:
            await sbx.pin_checkpoint(baked, False)
            await sbx.delete_checkpoint(baked)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--source", help="checkpoint to fork")
    src.add_argument("--bake", help="VM image to bake a checkpoint from (vm:<disk> or a reference)")
    src.add_argument("--worlds", help="comma list of checkpoints (with --per-world)")
    ap.add_argument("--count", type=int, default=8)
    ap.add_argument("--per-world", type=int, default=8)
    ap.add_argument("--group-size", type=int, default=0)
    ap.add_argument("--on", default="local")
    ap.add_argument("--parallel", type=int, default=4)
    ap.add_argument("--fork-cpus", type=int, default=1, help="vCPUs the placement books per fork (rollouts mostly idle)")
    ap.add_argument("--kind", help="kind of the bake VM (vm)")
    ap.add_argument("--runtime", help="runtime of the bake VM (qemu)")
    ap.add_argument("--cpus", type=int, default=2)
    ap.add_argument("--memory-mb", type=int, default=1536)
    ap.add_argument("--fallback-dir")
    ap.add_argument("--fallback-bucket")
    ap.add_argument("--fallback-prefix")
    ap.add_argument("--fallback-region")
    ap.add_argument("--embedded", action="store_true", help="run the batch in this process instead of the daemon")
    ap.add_argument("--keep", action="store_true", help="keep the forks")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.environ.setdefault("CUA_TELEMETRY", "0")
    res = asyncio.run(run(args))
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(res, f, indent=2)
    brief = {k: res[k] for k in ("ok", "failed", "total_ms", "first_ready_ms", "fork_ms_p50", "fork_ms_p95", "per_host", "peak_parallel", "groups_split", "replaced")}
    print(json.dumps(brief, indent=2))
    return 0 if res["failed"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
