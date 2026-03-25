"""Android VM fleet benchmark — measure max achievable RPS for a given sandbox count.

Provisions N Android sandboxes in parallel, then drives them as fast as possible
(no rate limiting). Reports achieved throughput and latency percentiles.

Usage:
    python tests/android_rps_benchmark.py --sandboxes 2 --duration 30
    python tests/android_rps_benchmark.py --sandboxes 8 --duration 60 --action step
"""

from __future__ import annotations

import argparse
import asyncio
import builtins
import functools
import math
import os
import statistics
import time
from dataclasses import dataclass, field

from cua_sandbox import Image, Sandbox

print = functools.partial(builtins.print, flush=True)

# ── Data types ────────────────────────────────────────────────────────────────


@dataclass
class SandboxStats:
    name: str
    provision_time: float
    requests: int = 0
    errors: int = 0
    latencies: list[float] = field(default_factory=list)


@dataclass
class BenchmarkResult:
    achieved_rps: float
    sandboxes: int
    duration: float
    total_requests: int
    total_errors: int
    provision_times: list[float]
    all_latencies: list[float]

    @property
    def error_rate(self) -> float:
        return self.total_errors / max(self.total_requests, 1)

    @property
    def mean_provision(self) -> float:
        return statistics.mean(self.provision_times) if self.provision_times else 0.0

    @property
    def p50(self) -> float:
        return _percentile(self.all_latencies, 50)

    @property
    def p95(self) -> float:
        return _percentile(self.all_latencies, 95)

    @property
    def p99(self) -> float:
        return _percentile(self.all_latencies, 99)


# ── Helpers ───────────────────────────────────────────────────────────────────


def _percentile(data: list[float], p: float) -> float:
    if not data:
        return 0.0
    s = sorted(data)
    idx = (p / 100) * (len(s) - 1)
    lo = int(idx)
    hi = min(lo + 1, len(s) - 1)
    return s[lo] + (idx - lo) * (s[hi] - s[lo])


# ── Sandbox lifecycle ─────────────────────────────────────────────────────────


async def _provision(image: Image, idx: int) -> tuple[Sandbox, SandboxStats]:
    t0 = time.monotonic()
    sb = await Sandbox.create(image)
    elapsed = time.monotonic() - t0
    name = getattr(sb, "name", None) or f"sb-{idx}"
    print(f"  [{idx + 1:>3}] {name}  provisioned in {elapsed:.1f}s")
    return sb, SandboxStats(name=name, provision_time=elapsed)


async def _provision_fleet(
    image: Image, n: int, parallel: int = 2
) -> tuple[list[Sandbox], list[SandboxStats], list[float]]:
    sandboxes, stats, provision_times = [], [], []

    for batch_start in range(0, n, parallel):
        batch = range(batch_start, min(batch_start + parallel, n))
        tasks = [_provision(image, i) for i in batch]
        raw = await asyncio.gather(*tasks, return_exceptions=True)
        for i, res in zip(batch, raw):
            if isinstance(res, Exception):
                print(f"  [!] sandbox {i + 1} failed to provision: {res}")
            else:
                sb, st = res
                sandboxes.append(sb)
                stats.append(st)
                provision_times.append(st.provision_time)

    return sandboxes, stats, provision_times


async def _destroy_fleet(sandboxes: list[Sandbox]) -> None:
    results = await asyncio.gather(*[sb.destroy() for sb in sandboxes], return_exceptions=True)
    for i, r in enumerate(results):
        if isinstance(r, Exception):
            print(f"  [!] sandbox {i} destroy failed: {r}")


# ── Per-sandbox actions ───────────────────────────────────────────────────────

_t_bench_start: float = 0.0  # set when load test begins


def _ts() -> str:
    """Relative timestamp from load test start."""
    return f"{time.monotonic() - _t_bench_start:>7.3f}s"


async def _do_screenshot(sb: Sandbox, name: str) -> None:
    t0 = time.monotonic()
    data = await sb.screenshot(format="jpeg")
    elapsed = time.monotonic() - t0
    print(f"  {_ts()}  {name}  screenshot  {elapsed*1000:.0f}ms  {len(data)}B")
    if len(data) < 500:
        raise ValueError(f"screenshot too small ({len(data)} bytes)")


async def _do_step(sb: Sandbox, name: str) -> None:
    """Simulate one RL step: tap center of screen + screenshot."""
    t_size0 = time.monotonic()
    w, h = await sb.screen.size()
    t_size = time.monotonic() - t_size0
    print(f"  {_ts()}  {name}  screen_size {t_size*1000:.0f}ms  ({w}x{h})")

    t_tap0 = time.monotonic()
    await sb.mouse.click(w // 2, h // 2)
    t_tap = time.monotonic() - t_tap0
    print(f"  {_ts()}  {name}  tap         {t_tap*1000:.0f}ms")

    t_ss0 = time.monotonic()
    data = await sb.screenshot(format="jpeg")
    t_ss = time.monotonic() - t_ss0
    print(f"  {_ts()}  {name}  screenshot  {t_ss*1000:.0f}ms  {len(data)}B")

    if len(data) < 500:
        raise ValueError(f"screenshot too small ({len(data)} bytes)")


_ACTION_FNS = {
    "screenshot": _do_screenshot,
    "step": _do_step,
}
# action fn signature: (sb, name) -> None


# ── Worker loop ───────────────────────────────────────────────────────────────


async def _worker(
    sb: Sandbox,
    stats: SandboxStats,
    stop_event: asyncio.Event,
    action: str,
) -> None:
    """Drive one sandbox as fast as possible until stop_event is set."""
    fn = _ACTION_FNS[action]
    while not stop_event.is_set():
        t0 = time.monotonic()
        try:
            await fn(sb, stats.name)
            elapsed = time.monotonic() - t0
            stats.latencies.append(elapsed)
            stats.requests += 1
        except asyncio.CancelledError:
            raise
        except Exception as e:
            stats.errors += 1
            stats.requests += 1
            if not stop_event.is_set():
                print(f"  [!] {stats.name}: {e}")


# ── Progress reporter ─────────────────────────────────────────────────────────


async def _progress_reporter(
    all_stats: list[SandboxStats],
    t_start: float,
    duration: float,
    stop_event: asyncio.Event,
) -> None:
    interval = max(5.0, duration / 6)
    while not stop_event.is_set():
        await asyncio.sleep(interval)
        if stop_event.is_set():
            break
        elapsed = time.monotonic() - t_start
        total_reqs = sum(s.requests for s in all_stats)
        total_errs = sum(s.errors for s in all_stats)
        rps = total_reqs / elapsed if elapsed > 0 else 0.0
        remain = max(0, duration - elapsed)
        print(
            f"  t={elapsed:>5.1f}s  {rps:>6.1f} RPS  "
            f"reqs={total_reqs}  errs={total_errs}  remain={remain:.0f}s"
        )


# ── Benchmark orchestrator ────────────────────────────────────────────────────


async def run_benchmark(
    n_sandboxes: int,
    duration: float,
    action: str,
    image: Image,
    parallel: int = 2,
) -> BenchmarkResult:

    # ── Provision ──────────────────────────────────────────────────────────
    print(f"\n── Provisioning {n_sandboxes} sandbox(es) ──")
    sandboxes, all_stats, provision_times = await _provision_fleet(
        image, n_sandboxes, parallel=parallel
    )

    if not sandboxes:
        raise RuntimeError("All sandboxes failed to provision — aborting benchmark.")

    actual = len(sandboxes)
    print(
        f"  {actual}/{n_sandboxes} provisioned | "
        f"mean={statistics.mean(provision_times):.1f}s  "
        f"max={max(provision_times):.1f}s"
    )

    # ── Load test ─────────────────────────────────────────────────────────
    stop_event = asyncio.Event()
    t_start = time.monotonic()

    global _t_bench_start
    _t_bench_start = t_start

    print(f"\n── Load test: max RPS × {duration}s ──\n  action={action}  sandboxes={actual}")

    workers = [
        asyncio.create_task(_worker(sb, st, stop_event, action))
        for sb, st in zip(sandboxes, all_stats)
    ]
    reporter = asyncio.create_task(_progress_reporter(all_stats, t_start, duration, stop_event))

    await asyncio.sleep(duration)
    stop_event.set()

    for w in workers:
        w.cancel()
    reporter.cancel()
    await asyncio.gather(*workers, reporter, return_exceptions=True)

    elapsed = time.monotonic() - t_start

    # ── Cleanup ───────────────────────────────────────────────────────────
    print(f"\n── Destroying {actual} sandbox(es) ──")
    await _destroy_fleet(sandboxes)

    # ── Aggregate ─────────────────────────────────────────────────────────
    total_reqs = sum(s.requests for s in all_stats)
    total_errs = sum(s.errors for s in all_stats)
    all_latencies = [lat for s in all_stats for lat in s.latencies]
    achieved_rps = total_reqs / elapsed if elapsed > 0 else 0.0

    return BenchmarkResult(
        achieved_rps=achieved_rps,
        sandboxes=actual,
        duration=elapsed,
        total_requests=total_reqs,
        total_errors=total_errs,
        provision_times=provision_times,
        all_latencies=all_latencies,
    )


# ── Report ────────────────────────────────────────────────────────────────────


def print_report(result: BenchmarkResult) -> None:
    bar = "─" * 58
    print(f"\n{'═'*58}")
    print("  Android VM Fleet Benchmark")
    print(f"{'═'*58}")
    print(f"  {bar}")
    print("  Throughput")
    print(f"  {'Achieved RPS':<24} {result.achieved_rps:.2f}")
    print(f"  {'RPS / sandbox':<24} {result.achieved_rps / max(result.sandboxes, 1):.2f}")
    print(f"  {'Sandboxes':<24} {result.sandboxes}")
    print(f"  {'Duration':<24} {result.duration:.1f}s")
    print(f"  {'Total requests':<24} {result.total_requests}")
    print(f"  {'Error rate':<24} {result.error_rate * 100:.1f}%")
    print(f"  {bar}")
    print("  Provisioning")
    print(f"  {'Mean cold start':<24} {result.mean_provision:.1f}s")
    if result.provision_times:
        print(f"  {'Max cold start':<24} {max(result.provision_times):.1f}s")
    print(f"  {bar}")
    print("  Latency (successful requests)")
    if result.all_latencies:
        print(f"  {'p50':<24} {result.p50 * 1000:.0f}ms")
        print(f"  {'p95':<24} {result.p95 * 1000:.0f}ms")
        print(f"  {'p99':<24} {result.p99 * 1000:.0f}ms")
    else:
        print("  (no successful requests)")
    print(f"{'═'*58}\n")


# ── CLI ───────────────────────────────────────────────────────────────────────


_STATE_FILE = "/tmp/android_bench_sandboxes.txt"


def _parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Measure max RPS for a fleet of Android sandboxes.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Full run (provision + load test + delete):
  python tests/android_rps_benchmark.py --sandboxes 4 --duration 30

  # Step-by-step:
  python tests/android_rps_benchmark.py --sandboxes 4 --provision
  python tests/android_rps_benchmark.py --continue
  python tests/android_rps_benchmark.py --delete
        """,
    )
    p.add_argument(
        "--sandboxes", type=int, default=2, help="Number of Android sandboxes (default: 2)."
    )
    p.add_argument(
        "--duration", type=float, default=30.0, help="Load-test duration in seconds (default: 30)."
    )
    p.add_argument(
        "--action",
        choices=list(_ACTION_FNS),
        default="screenshot",
        help="'screenshot' = observation only; 'step' = tap + screenshot. Default: screenshot.",
    )
    p.add_argument("--android-version", default="14", help="Android version (default: 14).")
    p.add_argument(
        "--parallel",
        type=int,
        default=4,
        metavar="N",
        help="Number of sandboxes to provision concurrently (default: 4). Use 1 for fully sequential.",
    )
    mode = p.add_mutually_exclusive_group()
    mode.add_argument(
        "--provision",
        action="store_true",
        help=f"Provision sandboxes and save names to {_STATE_FILE}. No load test or delete.",
    )
    mode.add_argument(
        "--continue",
        dest="cont",
        action="store_true",
        help=f"Load sandbox names from {_STATE_FILE}, run load test, then delete.",
    )
    mode.add_argument(
        "--delete",
        action="store_true",
        help=f"Delete sandboxes listed in {_STATE_FILE}.",
    )
    return p.parse_args()


async def _main() -> None:
    args = _parse_args()
    image = Image.android(args.android_version)

    # ── --delete ──────────────────────────────────────────────────────────
    if args.delete:
        try:
            names = open(_STATE_FILE).read().split()
        except FileNotFoundError:
            print(f"No state file at {_STATE_FILE}")
            return
        print(f"── Deleting {len(names)} sandbox(es) from {_STATE_FILE} ──")
        sandboxes = [Sandbox(name=n) for n in names]
        await _destroy_fleet(sandboxes)
        os.unlink(_STATE_FILE)
        print("Done.")
        return

    # ── --continue ────────────────────────────────────────────────────────
    if args.cont:
        try:
            names = open(_STATE_FILE).read().split()
        except FileNotFoundError:
            print(f"No state file at {_STATE_FILE} — run --provision first.")
            return
        print(f"── Reconnecting to {len(names)} sandbox(es) from {_STATE_FILE} ──")
        sandboxes = []
        all_stats = []
        provision_times = []
        for name in names:
            sb = Sandbox(name=name)
            st = SandboxStats(name=name, provision_time=0.0)
            sandboxes.append(sb)
            all_stats.append(st)
            provision_times.append(0.0)
            print(f"  {name}")

        stop_event = asyncio.Event()
        t_start = time.monotonic()
        global _t_bench_start
        _t_bench_start = t_start

        print(
            f"\n── Load test: max RPS × {args.duration}s ──\n  action={args.action}  sandboxes={len(sandboxes)}"
        )
        workers = [
            asyncio.create_task(_worker(sb, st, stop_event, args.action))
            for sb, st in zip(sandboxes, all_stats)
        ]
        reporter = asyncio.create_task(
            _progress_reporter(all_stats, t_start, args.duration, stop_event)
        )
        await asyncio.sleep(args.duration)
        stop_event.set()
        for w in workers:
            w.cancel()
        reporter.cancel()
        await asyncio.gather(*workers, reporter, return_exceptions=True)
        elapsed = time.monotonic() - t_start

        print(f"\n── Destroying {len(sandboxes)} sandbox(es) ──")
        await _destroy_fleet(sandboxes)
        os.unlink(_STATE_FILE)

        total_reqs = sum(s.requests for s in all_stats)
        total_errs = sum(s.errors for s in all_stats)
        all_latencies = [lat for s in all_stats for lat in s.latencies]
        result = BenchmarkResult(
            achieved_rps=total_reqs / elapsed if elapsed > 0 else 0.0,
            sandboxes=len(sandboxes),
            duration=elapsed,
            total_requests=total_reqs,
            total_errors=total_errs,
            provision_times=provision_times,
            all_latencies=all_latencies,
        )
        print_report(result)
        return

    # ── --provision ───────────────────────────────────────────────────────
    if args.provision:
        # Resume: load existing names so we only provision what's missing
        existing_names: list[str] = []
        try:
            existing_names = open(_STATE_FILE).read().split()
            print(
                f"Resuming: {len(existing_names)} already provisioned, "
                f"targeting {args.sandboxes} total."
            )
        except FileNotFoundError:
            pass

        remaining = args.sandboxes - len(existing_names)
        print("Android RL Fleet Benchmark")
        print(
            f"  sandboxes={args.sandboxes}  android={args.android_version}  "
            f"mode=provision-only  remaining={remaining}"
        )

        if remaining <= 0:
            print(f"Already have {len(existing_names)} sandboxes — nothing to provision.")
            return

        print(f"\n── Provisioning {remaining} more sandbox(es) (parallel={args.parallel}) ──")
        new_sandboxes, _, _ = await _provision_fleet(image, remaining, parallel=args.parallel)
        new_names = [
            getattr(sb, "name", None) or f"sb-{len(existing_names) + i}"
            for i, sb in enumerate(new_sandboxes)
        ]
        all_names = existing_names + new_names
        with open(_STATE_FILE, "w") as f:
            f.write("\n".join(all_names) + "\n")
        print(f"\nSaved {len(all_names)} sandbox name(s) to {_STATE_FILE}")
        print("Run with --continue to start the load test.")
        return

    # ── normal (all-in-one) ───────────────────────────────────────────────
    print("Android RL Fleet Benchmark")
    print(
        f"  sandboxes={args.sandboxes}  duration={args.duration}s  action={args.action}  android={args.android_version}"
    )
    result = await run_benchmark(
        n_sandboxes=args.sandboxes,
        duration=args.duration,
        action=args.action,
        image=image,
        parallel=args.parallel,
    )
    print_report(result)


if __name__ == "__main__":
    asyncio.run(_main())
