#!/usr/bin/env python3
"""Compat overhead guard: how long the Modal gateway itself takes.

Over the mock Spaces (so the guest costs nothing) it times the calls a Modal
client makes most, through the unmodified Python SDK on one connection, and
divides each p50 by the p50 of a call that does no work (`ClientHello`),
which factors out the machine and the SDK's own cost. `baseline.json` holds
the committed ratios; a ratio above 1.5x its baseline fails.

    overhead.py env.json [--check baseline.json | --write baseline.json] [--n 100]

Run with the pinned SDK's python (tests/compat/run_hermetic.sh does).
"""

from __future__ import annotations

import argparse
import io
import json
import os
import statistics
import sys
import time

THRESHOLD = 1.5


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("env", help="the gateway environment (JSON)")
    ap.add_argument("--n", type=int, default=100)
    ap.add_argument(
        "--rounds",
        type=int,
        default=3,
        help="rounds; --write keeps the slowest, --check the fastest",
    )
    ap.add_argument("--check")
    ap.add_argument("--write")
    a = ap.parse_args()
    os.environ.update(json.load(open(a.env)))
    import modal

    client = modal.Client.from_env()
    app = modal.App.lookup("perf", create_if_missing=True)
    img = modal.Image.from_registry("ghcr.io/trycua/linux:24.04")
    d = modal.Dict.from_name("perf-dict", create_if_missing=True)
    q = modal.Queue.from_name("perf-q", create_if_missing=True)
    vol = modal.Volume.from_name("perf-vol", create_if_missing=True)
    with vol.batch_upload(force=True) as b:
        b.put_file(io.BytesIO(os.urandom(4096)), "/f.bin")
    sb = modal.Sandbox.create("sleep", "600000", app=app, image=img, timeout=300)
    sb.filesystem.write_bytes(os.urandom(4096), "/tmp/perf.bin")
    n = [0]

    def secret():
        n[0] += 1
        modal.Secret.from_dict({"K": str(n[0])}).hydrate()

    ops = {
        "noop_hello": lambda: client.hello(),
        "dict_put": lambda: d.put("k", "v"),
        "dict_get": lambda: d.get("k"),
        "queue_put_get": lambda: (q.put(1), q.get()),
        "secret_create": secret,
        "volume_listdir": lambda: vol.listdir("/"),
        "volume_read_4k": lambda: b"".join(vol.read_file("f.bin")),
        "sandbox_get_tags": lambda: sb.get_tags(),
        "sandbox_poll": lambda: sb.poll(),
        "sandbox_exec_echo": lambda: sb.exec("echo", "hi").stdout.read(),
        "fs_read_4k": lambda: sb.filesystem.read_bytes("/tmp/perf.bin"),
        "fs_stat": lambda: sb.filesystem.stat("/tmp/perf.bin"),
        "fs_list": lambda: sb.filesystem.list_files("/tmp"),
    }
    rounds = []
    for _ in range(a.rounds):
        p50 = {}
        for name, fn in ops.items():
            for _ in range(5):
                fn()
            ts = []
            for _ in range(a.n):
                t0 = time.perf_counter()
                fn()
                ts.append((time.perf_counter() - t0) * 1000)
            p50[name] = statistics.median(ts)
        rounds.append(p50)
    sb.terminate()
    pick = max if a.write else min
    ratios = {
        k: round(pick(r[k] / r["noop_hello"] for r in rounds), 3) for k in ops if k != "noop_hello"
    }
    p50 = rounds[-1]
    ref = p50["noop_hello"]
    out = {
        "unit": "p50 ratio to ClientHello",
        "n": a.n,
        "ratios": ratios,
        "noop_p50_ms": round(ref, 3),
        "p50_ms": {k: round(v, 3) for k, v in p50.items()},
    }
    print(json.dumps(out, indent=1))
    if a.write:
        json.dump({"unit": out["unit"], "n": a.n, "ratios": ratios}, open(a.write, "w"), indent=1)
        open(a.write, "a").write("\n")
        return 0
    if a.check:
        base = json.load(open(a.check))["ratios"]
        bad = {
            k: (ratios[k], base[k]) for k in base if k in ratios and ratios[k] > THRESHOLD * base[k]
        }
        missing = [k for k in base if k not in ratios]
        for k, (got, want) in bad.items():
            print(f"PERF {k}: ratio {got} > {THRESHOLD} x baseline {want}")
        for k in missing:
            print(f"PERF {k}: in the baseline, not measured")
        return 1 if bad or missing else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
