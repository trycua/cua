#!/usr/bin/env python3
"""Compat overhead guard: the E2B SDK through the gateway against the same
operations done directly through the Cua SDK, on a real Space.

Measures (each side once per run, on its own fresh Space of the same kind):
  create     sandbox create to first usable (ms)
  command    command round trip, p50 of N `echo hi` runs (ms)
  write/read file throughput for a streamed file (MiB/s)

The committed baseline holds the ratio gateway/direct for each (>1 is
slower through the gateway; throughput ratios are direct/gateway). A ratio
above 1.5x its baseline fails.

    overhead.py --direct 'target/debug/examples/bench_direct --on local' \
                --env env.json [--check baseline.json | --write baseline.json] [--n 100] [--mib 64]

`env.json` is the gateway's client environment (`cua compat env --json`).
Needs the `e2b` Python package and a running Cua daemon.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shlex
import statistics
import subprocess
import sys
import time

THRESHOLD = 1.5


async def via_gateway(n: int, mib: int) -> dict:
    from e2b import AsyncSandbox

    t0 = time.perf_counter()
    sb = await AsyncSandbox.create(timeout=900, request_timeout=600)
    try:
        await sb.commands.run("true")
        create_ms = (time.perf_counter() - t0) * 1000
        ts = []
        for i in range(n + 5):
            t = time.perf_counter()
            r = await sb.commands.run("/bin/echo hi", user="root")
            assert r.stdout == "hi\n"
            ts.append((time.perf_counter() - t) * 1000)
        cmd = statistics.median(ts[5:])
        data = b"\x5a" * (1 << 20)

        def stream():
            for _ in range(mib):
                yield data

        class Reader:
            def __init__(self):
                self.it = stream()
                self.buf = b""

            def read(self, size=-1):
                if not self.buf:
                    self.buf = next(self.it, b"")
                out, self.buf = (
                    self.buf[: size if size > 0 else None],
                    self.buf[size if size > 0 else len(self.buf) :],
                )
                return out

        t = time.perf_counter()
        await sb.files.write("/tmp/bench-gateway.bin", Reader(), user="root", request_timeout=600)
        write_s = time.perf_counter() - t
        t = time.perf_counter()
        got = 0
        async with await sb.files.read(
            "/tmp/bench-gateway.bin", format="stream", user="root", request_timeout=600
        ) as s:
            async for chunk in s:
                got += len(chunk)
        read_s = time.perf_counter() - t
        assert got == mib << 20, got
        return {
            "create_ms": create_ms,
            "command_p50_ms": cmd,
            "write_mib_s": mib / write_s,
            "read_mib_s": mib / read_s,
        }
    finally:
        await sb.kill()


def ratios(gw: dict, direct: dict) -> dict:
    return {
        "create": round(gw["create_ms"] / direct["create_ms"], 3),
        "command": round(gw["command_p50_ms"] / direct["command_p50_ms"], 3),
        "write": round(direct["write_mib_s"] / gw["write_mib_s"], 3),
        "read": round(direct["read_mib_s"] / gw["read_mib_s"], 3),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--direct", required=True, help="command that prints the direct-side JSON (bench_direct)"
    )
    ap.add_argument("--env", required=True)
    ap.add_argument("--n", type=int, default=100)
    ap.add_argument("--mib", type=int, default=64)
    ap.add_argument("--check")
    ap.add_argument("--write")
    ap.add_argument("--rounds", type=int, default=3, help="repeat and take the median ratio")
    a = ap.parse_args()
    os.environ.update(json.load(open(a.env)))
    all_ratios = []
    last = {}
    for r in range(a.rounds):
        d = subprocess.run(
            shlex.split(a.direct) + ["--n", str(a.n), "--mib", str(a.mib)],
            capture_output=True,
            text=True,
            timeout=1800,
        )
        if d.returncode != 0:
            print(d.stderr[-2000:], file=sys.stderr)
            return 2
        direct = json.loads(d.stdout.strip().splitlines()[-1])
        gw = asyncio.run(via_gateway(a.n, a.mib))
        last = {"direct": direct, "gateway": gw}
        all_ratios.append(ratios(gw, direct))
        print(
            f"round {r + 1}: direct {direct} gateway {gw} ratios {all_ratios[-1]}", file=sys.stderr
        )
    med = {k: round(statistics.median(x[k] for x in all_ratios), 3) for k in all_ratios[0]}
    out = {
        "unit": "gateway/direct (throughput: direct/gateway); 1.0 is no overhead",
        "rounds": a.rounds,
        "ratios": med,
        "last": last,
    }
    print(json.dumps(out, indent=1))
    if a.write:
        json.dump(
            {"unit": out["unit"], "n": a.n, "mib": a.mib, "ratios": med},
            open(a.write, "w"),
            indent=1,
        )
        open(a.write, "a").write("\n")
        return 0
    if a.check:
        base = json.load(open(a.check))["ratios"]
        bad = {k: (med[k], base[k]) for k in base if med.get(k, 1e9) > THRESHOLD * base[k]}
        for k, (got, want) in bad.items():
            print(f"PERF {k}: ratio {got} > {THRESHOLD} x baseline {want}")
        return 1 if bad else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
