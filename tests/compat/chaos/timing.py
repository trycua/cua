#!/usr/bin/env python3
"""64 parallel Sandbox.create through the E2B SDK: latency p50/p95/max, SDK timeouts, errors; deletes all."""

import collections
import statistics
import sys
import threading
import time

from e2b import Sandbox

N = int(sys.argv[1]) if len(sys.argv) > 1 else 64
res = []
lock = threading.Lock()
boxes = []


def one():
    t = time.monotonic()
    try:
        s = Sandbox.create(timeout=600)
        with lock:
            boxes.append(s)
            res.append(("ok", time.monotonic() - t, ""))
    except Exception as e:
        with lock:
            res.append(("err", time.monotonic() - t, f"{type(e).__name__}: {str(e)[:100]}"))


t0 = time.monotonic()
ts = [threading.Thread(target=one) for _ in range(N)]
[t.start() for t in ts]
[t.join() for t in ts]
wall = time.monotonic() - t0
ok = sorted(d for k, d, _ in res if k == "ok")
errs = collections.Counter(m for k, _, m in res if k == "err")


def pct(p):
    return ok[min(len(ok) - 1, int(len(ok) * p))] if ok else float("nan")


print(
    f"N={N} ok={len(ok)} err={N-len(ok)} wall={wall:.1f}s p50={pct(.5):.1f}s p95={pct(.95):.1f}s max={(ok[-1] if ok else float('nan')):.1f}s"
)
for m, c in errs.most_common(5):
    print(c, m)
t = time.monotonic()
ds = [threading.Thread(target=lambda b=b: b.kill()) for b in boxes]
[d.start() for d in ds]
[d.join() for d in ds]
print(f"deleted {len(boxes)} in {time.monotonic()-t:.1f}s")
