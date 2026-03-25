"""
Demo 1: Fleet Throughput
Send tap+screenshot to 99 cloud VMs concurrently and report steps/hour.

Usage:
    CUA_API_KEY=sk_... python demo/1_fleet_throughput.py
"""

import asyncio
import time

from cua_sandbox import Sandbox

STATE_FILE = "/tmp/android_bench_sandboxes.txt"
CONCURRENCY = 20  # parallel VMs to drive at once
ROUNDS = 3  # tap+screenshot rounds per VM


# ── helpers ──────────────────────────────────────────────────────────────────


def load_names() -> list[str]:
    with open(STATE_FILE) as f:
        return [line.strip() for line in f if line.strip()]


async def connect_all(names: list[str]) -> list[tuple[str, Sandbox]]:
    print(f"Connecting to {len(names)} VMs  (concurrency={CONCURRENCY})...")

    sem = asyncio.Semaphore(CONCURRENCY)

    async def connect_one(name: str):
        async with sem:
            try:
                sb = await Sandbox.connect(name=name)
                print(f"  ✓  {name}")
                return (name, sb)
            except Exception as e:
                print(f"  ✗  {name}  {e}")
                return None

    results = await asyncio.gather(*[connect_one(n) for n in names])
    return [r for r in results if r is not None]


async def run_step(name: str, sb: Sandbox) -> float:
    """Tap center + screenshot. Returns elapsed seconds."""
    t0 = time.monotonic()
    w, h = await sb.screen.size()
    await sb.mouse.click(w // 2, h // 2)
    await sb.screenshot(format="jpeg")
    return time.monotonic() - t0


# ── main ─────────────────────────────────────────────────────────────────────


async def main():
    names = load_names()
    print(f"\n{'='*50}")
    print("  Fleet Throughput Demo")
    print(f"  {len(names)} VMs  |  {ROUNDS} rounds each")
    print(f"{'='*50}\n")

    sandboxes = await connect_all(names)
    print(f"\nConnected: {len(sandboxes)}/{len(names)} VMs\n")

    if not sandboxes:
        print("No VMs connected — exiting.")
        return

    # Run ROUNDS of tap+screenshot across all VMs concurrently
    sem = asyncio.Semaphore(CONCURRENCY)
    total_steps = 0
    latencies: list[float] = []

    print(f"Running {ROUNDS} rounds of tap+screenshot...\n")
    t_start = time.monotonic()

    for round_num in range(1, ROUNDS + 1):
        print(f"── Round {round_num}/{ROUNDS} ──")

        async def step(name: str, sb: Sandbox):
            async with sem:
                try:
                    elapsed = await run_step(name, sb)
                    print(f"  {name}  {elapsed*1000:.0f}ms")
                    return elapsed
                except Exception as e:
                    print(f"  {name}  ERROR: {e}")
                    return None

        round_results = await asyncio.gather(*[step(n, sb) for n, sb in sandboxes])
        ok = [r for r in round_results if r is not None]
        total_steps += len(ok)
        latencies.extend(ok)

    t_elapsed = time.monotonic() - t_start

    # Disconnect
    await asyncio.gather(*[sb.disconnect() for _, sb in sandboxes], return_exceptions=True)

    # Report
    steps_per_hour = total_steps / t_elapsed * 3600
    latencies.sort()
    p50 = latencies[len(latencies) // 2] * 1000
    p95 = latencies[int(len(latencies) * 0.95)] * 1000

    print(f"\n{'='*50}")
    print("  Results")
    print(f"{'='*50}")
    print(f"  VMs active     : {len(sandboxes)}")
    print(f"  Total steps    : {total_steps}")
    print(f"  Elapsed        : {t_elapsed:.1f}s")
    print(f"  Throughput     : {steps_per_hour:,.0f} steps/hour")
    print(f"  Latency p50    : {p50:.0f}ms")
    print(f"  Latency p95    : {p95:.0f}ms")
    print(f"{'='*50}\n")


if __name__ == "__main__":
    asyncio.run(main())
