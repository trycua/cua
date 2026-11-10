"""Translation overhead of cua-e2b: the time an e2b call spends in the shim
on top of the backend call it makes.

Hermetic mode (`python -m perf.overhead`): the backend is the in-memory
double, so what is measured is only the shim (argument translation, error
mapping, decoding, event pump). Each figure is the median over many calls,
divided by a CPU calibration loop so the number does not depend on the
machine; `baseline.json` holds the committed figures and a run fails when one
exceeds 1.5x its baseline.

Live mode (`python -m perf.overhead --live`): against a real Space, the same
operation through the shim and through the cua SDK directly; the shim must be
within 1.5x of the direct time plus a fixed allowance for the extra round
trips the e2b contract needs (`LIVE_ALLOWANCE_S`). Run on a runner that can
boot Linux VMs (the nightly lane, or by hand on a Mac Studio (M2 Max)).
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import statistics
import sys
import time
from pathlib import Path
from typing import Awaitable, Callable, Dict

HERE = Path(__file__).resolve().parent
BASELINE = HERE / "baseline.json"
THRESHOLD = 1.5
LIVE_ALLOWANCE_S = 0.15
NOISE_FLOOR = 0.004  # units; below this a figure is timer noise


def calibrate() -> float:
    """Seconds for a fixed amount of pure-Python work (the unit of overhead)."""
    best = float("inf")
    for _ in range(5):
        t = time.perf_counter()
        d = {}
        for i in range(20000):
            d[str(i)] = hashlib.md5(str(i).encode()).hexdigest()
        best = min(best, time.perf_counter() - t)
    return best


async def _median(op: Callable[[], Awaitable], n: int) -> float:
    for _ in range(max(3, n // 10)):
        await op()
    samples = []
    for _ in range(n):
        t = time.perf_counter()
        await op()
        samples.append(time.perf_counter() - t)
    return statistics.median(samples)


async def hermetic() -> Dict[str, float]:
    sys.path.insert(0, str(HERE.parent))
    from e2b import AsyncSandbox
    from e2b._backend import set_backend
    from fake_backend import FakeBackend

    backend = FakeBackend()
    set_backend(backend)
    sb = await AsyncSandbox.create(template="w")
    m = backend.machines[sb.sandbox_id]
    payload = b"x" * 65536
    await sb.files.write("/tmp/f", payload)
    unit = calibrate()
    ops = {
        "commands.run": (
            lambda: sb.commands.run("echo hi"),
            lambda: m.spawn(["echo hi"], env={}, cwd=None, user=None, timeout_ms=None, stdin=False),
        ),
        "files.write_64k": (
            lambda: sb.files.write("/tmp/f", payload),
            lambda: m.upload("/tmp/f", payload),
        ),
        "files.read_64k": (
            lambda: sb.files.read("/tmp/f", format="bytes"),
            lambda: m.download("/tmp/f"),
        ),
        "files.list": (lambda: sb.files.list("/tmp"), lambda: m.list_dir("/tmp", 1)),
        "files.exists": (lambda: sb.files.exists("/tmp/f"), lambda: m.stat("/tmp/f")),
    }
    out = {}
    for name, (via_shim, direct) in ops.items():
        if name == "commands.run":

            async def direct_run():
                p = await m.spawn(
                    ["echo hi"], env={}, cwd=None, user=None, timeout_ms=None, stdin=False
                )
                while (ev := await p.next_event()) is not None and ev.kind != "exit":
                    pass

            direct = direct_run
        shim_t = await _median(via_shim, 300)
        base_t = await _median(direct, 300)
        out[name] = round(max(shim_t - base_t, 0.0) / unit, 4)
    set_backend(None)
    return out


def best_of(n: int = 3) -> Dict[str, float]:
    """The least-disturbed of `n` runs, per figure (a loaded machine only adds time)."""
    runs = [asyncio.run(hermetic()) for _ in range(n)]
    return {k: min(r[k] for r in runs) for k in runs[0]}


def check(measured: Dict[str, float], baseline: Dict[str, float]) -> list:
    return [
        f"{k}: {v} units > {THRESHOLD}x baseline {baseline[k]}"
        for k, v in measured.items()
        if k in baseline and v > max(baseline[k] * THRESHOLD, NOISE_FLOOR)
    ]


async def live() -> Dict[str, dict]:
    import os

    from e2b import AsyncSandbox

    import cua

    sb = await AsyncSandbox.create(timeout=900)
    try:
        c = await sb._m._c.spaces().space(sb._m._space_id)
        env = c.spacesd()
        payload = b"y" * (4 << 20)
        await sb.files.write("/tmp/p", payload)

        async def direct_run():
            await env.run(
                cua.SpacesdCommand(
                    program="/bin/bash", args=["-l", "-c", "echo hi"], timeout_ms=60000
                )
            )

        ops = {
            "commands.run": (lambda: sb.commands.run("echo hi"), direct_run),
            "files.write_4m": (
                lambda: sb.files.write("/tmp/p", payload),
                lambda: env.upload("/tmp/p", payload, cua.UploadOptions(create_parents=True)),
            ),
            "files.read_4m": (
                lambda: sb.files.read("/tmp/p", format="bytes"),
                lambda: env.download("/tmp/p"),
            ),
        }
        out = {}
        for name, (via_shim, direct) in ops.items():
            s = await _median(via_shim, 20)
            d = await _median(direct, 20)
            out[name] = {"shim_s": round(s, 4), "direct_s": round(d, 4), "ratio": round(s / d, 3)}
        return out
    finally:
        await sb.kill()


def main(argv) -> int:
    if "--live" in argv:
        res = asyncio.run(live())
        print(json.dumps(res, indent=1))
        bad = [
            k for k, v in res.items() if v["shim_s"] > v["direct_s"] * THRESHOLD + LIVE_ALLOWANCE_S
        ]
        if bad:
            print("OVER THRESHOLD:", bad, file=sys.stderr)
            return 1
        return 0
    res = asyncio.run(hermetic()) if "--record" in argv else best_of()
    if "--record" in argv:
        runs = [res] + [asyncio.run(hermetic()) for _ in range(4)]
        res = {k: statistics.median(r[k] for r in runs) for k in res}  # typical of five
        BASELINE.write_text(json.dumps(res, indent=1, sort_keys=True) + "\n")
        print(json.dumps(res, indent=1))
        return 0
    problems = check(res, json.loads(BASELINE.read_text()))
    print(json.dumps(res, indent=1))
    for p in problems:
        print("OVER THRESHOLD:", p, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
