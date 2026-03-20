"""Android VM fleet benchmark — validate cloud infra for large-scale RL training.

Provisions N Android sandboxes in parallel, then drives them at a target
aggregate RPS. Reports achieved throughput, latency percentiles, and a
PASS/FAIL verdict.

Capacity estimates (per cloud Android VM, quickboot snapshot ~6s cold start):
  screenshot only   :  2.5–7 RPS  (150–400ms ADB screencap + JPEG encode)
  RL step (tap+obs) :  1.7–3 RPS  (action + screenshot, sequential)

Fleet throughput scales linearly with sandbox count until hitting:
  - Incus host KVM slots / CPU cores
  - Memory per VM (~4 GB each)
  - Traefik reverse-proxy saturation
  - Network bandwidth (screenshots ~20–100 KB JPEG each)

Usage:
    # Validate 20 RPS across 8 sandboxes
    python tests/android_rps_benchmark.py --target-rps 20 --sandboxes 8 --duration 30

    # Full RL-step stress test at 100 RPS
    python tests/android_rps_benchmark.py --target-rps 100 --sandboxes 40 --duration 60 --action step

    # Quick smoke test
    python tests/android_rps_benchmark.py --target-rps 4 --sandboxes 2 --duration 15
"""

from __future__ import annotations

import argparse
import asyncio
import math
import statistics
import time
from dataclasses import dataclass, field
from typing import Optional

from cua_sandbox import Image, Sandbox

# ── Configurable pass/fail thresholds ─────────────────────────────────────────

DEFAULT_MIN_ACHIEVED_FRACTION = 0.95  # achieved RPS must be ≥ 95% of target
DEFAULT_MAX_ERROR_RATE = 0.05  # ≤ 5% errors
DEFAULT_MAX_P99_MS = 5_000  # p99 latency ≤ 5 s


# ── Data types ────────────────────────────────────────────────────────────────


@dataclass
class SandboxStats:
    name: str
    provision_time: float
    requests: int = 0
    errors: int = 0
    latencies: list[float] = field(default_factory=list)

    @property
    def rps(self) -> float:
        if not self.latencies:
            return 0.0
        return len(self.latencies) / sum(self.latencies)


@dataclass
class BenchmarkResult:
    target_rps: float
    achieved_rps: float
    sandboxes: int
    duration: float
    total_requests: int
    total_errors: int
    provision_times: list[float]
    all_latencies: list[float]
    passed: bool
    fail_reasons: list[str]

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


class _TokenBucket:
    """Async token bucket for aggregate rate limiting across workers."""

    def __init__(self, rate: float):
        self._rate = rate
        self._tokens = float(rate)
        self._last = time.monotonic()
        self._lock = asyncio.Lock()

    async def acquire(self) -> None:
        while True:
            async with self._lock:
                now = time.monotonic()
                self._tokens = min(
                    self._rate,
                    self._tokens + (now - self._last) * self._rate,
                )
                self._last = now
                if self._tokens >= 1.0:
                    self._tokens -= 1.0
                    return
                wait = (1.0 - self._tokens) / self._rate
            await asyncio.sleep(wait)


# ── Sandbox lifecycle ─────────────────────────────────────────────────────────


async def _provision(image: Image, idx: int) -> tuple[Sandbox, SandboxStats]:
    t0 = time.monotonic()
    sb = await Sandbox.create(image)
    elapsed = time.monotonic() - t0
    name = getattr(sb, "name", None) or f"sb-{idx}"
    print(f"  [{idx + 1:>3}] {name}  provisioned in {elapsed:.1f}s")
    return sb, SandboxStats(name=name, provision_time=elapsed)


async def _provision_fleet(
    image: Image, n: int
) -> tuple[list[Sandbox], list[SandboxStats], list[float]]:
    tasks = [_provision(image, i) for i in range(n)]
    raw = await asyncio.gather(*tasks, return_exceptions=True)

    sandboxes, stats, provision_times = [], [], []
    for i, res in enumerate(raw):
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
    data = await sb.screenshot()
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
    data = await sb.screenshot()
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
    bucket: _TokenBucket,
    stop_event: asyncio.Event,
    action: str,
) -> None:
    """Drive one sandbox at the rate-limited pace until stop_event is set."""
    fn = _ACTION_FNS[action]
    while not stop_event.is_set():
        await bucket.acquire()
        if stop_event.is_set():
            break
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
    target_rps: float,
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
        pct = rps / target_rps * 100
        remain = max(0, duration - elapsed)
        print(
            f"  t={elapsed:>5.1f}s  {rps:>6.1f}/{target_rps:.0f} RPS ({pct:.0f}%)  "
            f"reqs={total_reqs}  errs={total_errs}  remain={remain:.0f}s"
        )


# ── Benchmark orchestrator ────────────────────────────────────────────────────


async def run_benchmark(
    target_rps: float,
    n_sandboxes: int,
    duration: float,
    action: str,
    image: Image,
    min_achieved_fraction: float = DEFAULT_MIN_ACHIEVED_FRACTION,
    max_error_rate: float = DEFAULT_MAX_ERROR_RATE,
    max_p99_ms: float = DEFAULT_MAX_P99_MS,
) -> BenchmarkResult:

    # ── Provision ──────────────────────────────────────────────────────────
    print(f"\n── Provisioning {n_sandboxes} sandbox(es) ──")
    sandboxes, all_stats, provision_times = await _provision_fleet(image, n_sandboxes)

    if not sandboxes:
        raise RuntimeError("All sandboxes failed to provision — aborting benchmark.")

    actual = len(sandboxes)
    print(
        f"  {actual}/{n_sandboxes} provisioned | "
        f"mean={statistics.mean(provision_times):.1f}s  "
        f"max={max(provision_times):.1f}s"
    )

    # ── Load test ─────────────────────────────────────────────────────────
    bucket = _TokenBucket(rate=target_rps)
    stop_event = asyncio.Event()
    t_start = time.monotonic()

    global _t_bench_start
    _t_bench_start = t_start

    per_sb = target_rps / actual
    print(
        f"\n── Load test: {target_rps} RPS × {duration}s ──\n"
        f"  action={action}  sandboxes={actual}  {per_sb:.1f} RPS/sandbox"
    )

    workers = [
        asyncio.create_task(_worker(sb, st, bucket, stop_event, action))
        for sb, st in zip(sandboxes, all_stats)
    ]
    reporter = asyncio.create_task(
        _progress_reporter(all_stats, t_start, duration, target_rps, stop_event)
    )

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

    fail_reasons: list[str] = []
    if achieved_rps < target_rps * min_achieved_fraction:
        fail_reasons.append(
            f"achieved RPS {achieved_rps:.1f} < {target_rps * min_achieved_fraction:.1f} "
            f"({min_achieved_fraction*100:.0f}% of target)"
        )
    err_rate = total_errs / max(total_reqs, 1)
    if err_rate > max_error_rate:
        fail_reasons.append(f"error rate {err_rate*100:.1f}% > {max_error_rate*100:.0f}%")
    p99_ms = _percentile(all_latencies, 99) * 1000
    if p99_ms > max_p99_ms:
        fail_reasons.append(f"p99 latency {p99_ms:.0f}ms > {max_p99_ms:.0f}ms")

    return BenchmarkResult(
        target_rps=target_rps,
        achieved_rps=achieved_rps,
        sandboxes=actual,
        duration=elapsed,
        total_requests=total_reqs,
        total_errors=total_errs,
        provision_times=provision_times,
        all_latencies=all_latencies,
        passed=not fail_reasons,
        fail_reasons=fail_reasons,
    )


# ── Report ────────────────────────────────────────────────────────────────────


def print_report(result: BenchmarkResult) -> None:
    status = "PASS ✓" if result.passed else "FAIL ✗"
    bar = "─" * 58
    print(f"\n{'═'*58}")
    print(f"  Android VM Fleet Benchmark  [{status}]")
    print(f"{'═'*58}")
    print(f"  {bar}")
    print("  Throughput")
    print(f"  {'Target RPS':<24} {result.target_rps:.1f}")
    print(
        f"  {'Achieved RPS':<24} {result.achieved_rps:.2f}  "
        f"({result.achieved_rps / result.target_rps * 100:.0f}% of target)"
    )
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
    print(f"  {bar}")

    if result.passed:
        print(f"  Infrastructure sustains {result.target_rps:.0f} RPS. Ready for RL at scale.")
    else:
        print("  Failures:")
        for r in result.fail_reasons:
            print(f"    • {r}")
        if result.all_latencies:
            rps_per_sb = result.achieved_rps / max(result.sandboxes, 1)
            needed_sb = math.ceil(result.target_rps / rps_per_sb) if rps_per_sb > 0 else "∞"
            print(f"\n  Per-sandbox throughput : ~{rps_per_sb:.1f} RPS")
            print(f"  Sandboxes to hit target: ~{needed_sb}")

    print(f"{'═'*58}\n")


# ── CLI ───────────────────────────────────────────────────────────────────────


def _parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Validate Android VM cloud infra for large-scale RL training.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Capacity estimates (per cloud Android VM, quickboot snapshot):
  screenshot        :  2.5–7 RPS    (150–400ms per call)
  RL step (tap+obs) :  1.7–3 RPS    (action + screenshot, sequential)

Pass conditions (all must hold):
  • achieved RPS ≥ 95% of --target-rps
  • error rate   ≤ 5%  (override: --max-error-rate)
  • p99 latency  ≤ 5s  (override: --max-p99-ms)

Examples:
  python tests/android_rps_benchmark.py --target-rps 20 --sandboxes 8 --duration 30
  python tests/android_rps_benchmark.py --target-rps 100 --sandboxes 40 --duration 60 --action step
  python tests/android_rps_benchmark.py --target-rps 5 --sandboxes 2 --duration 15
        """,
    )
    p.add_argument(
        "--target-rps",
        type=float,
        required=True,
        help="Target aggregate requests/steps per second across the whole fleet.",
    )
    p.add_argument(
        "--sandboxes",
        type=int,
        default=None,
        help=(
            "Number of Android sandboxes to provision. "
            "Defaults to ceil(target_rps / 3) — assuming ~3 RPS per sandbox."
        ),
    )
    p.add_argument(
        "--duration",
        type=float,
        default=30.0,
        help="Load-test duration in seconds (default: 30).",
    )
    p.add_argument(
        "--action",
        choices=list(_ACTION_FNS),
        default="screenshot",
        help=(
            "Action to benchmark. "
            "'screenshot' = observation only; "
            "'step' = tap + screenshot (full RL step). "
            "Default: screenshot."
        ),
    )
    p.add_argument(
        "--android-version",
        default="14",
        help="Android version to use (default: 14).",
    )
    p.add_argument(
        "--max-error-rate",
        type=float,
        default=DEFAULT_MAX_ERROR_RATE,
        help=f"Max acceptable error rate 0–1 (default: {DEFAULT_MAX_ERROR_RATE}).",
    )
    p.add_argument(
        "--max-p99-ms",
        type=float,
        default=DEFAULT_MAX_P99_MS,
        help=f"Max acceptable p99 latency in ms (default: {DEFAULT_MAX_P99_MS}).",
    )
    return p.parse_args()


async def _main() -> bool:
    args = _parse_args()
    n_sandboxes = args.sandboxes or math.ceil(args.target_rps / 3)
    image = Image.android(args.android_version)

    print("Android RL Fleet Benchmark")
    print(
        f"  target_rps={args.target_rps}  sandboxes={n_sandboxes}  "
        f"duration={args.duration}s  action={args.action}  android={args.android_version}"
    )

    result = await run_benchmark(
        target_rps=args.target_rps,
        n_sandboxes=n_sandboxes,
        duration=args.duration,
        action=args.action,
        image=image,
        max_error_rate=args.max_error_rate,
        max_p99_ms=args.max_p99_ms,
    )
    print_report(result)
    return result.passed


if __name__ == "__main__":
    passed = asyncio.run(_main())
    raise SystemExit(0 if passed else 1)
