"""
Demo 1: Fleet Throughput
Connect to 99 cloud VMs sequentially, then send tap+screenshot concurrently.
Reports total throughput as steps/hour at the end.

Usage:
    CUA_API_KEY=sk_... python demo/1_fleet_throughput.py
"""

import argparse
import asyncio
import time
from pathlib import Path

from cua_sandbox import Sandbox

STATE_FILE = "/tmp/android_bench_sandboxes.txt"
CONCURRENCY = 20  # parallel commands at once
ROUNDS = 3  # tap+screenshot rounds per VM
CONNECT_TIMEOUT = 30  # seconds per connect attempt

OUT_DIR = Path(__file__).parent / "out"


# ── helpers ──────────────────────────────────────────────────────────────────


def load_names(n: int | None = None) -> list[str]:
    with open(STATE_FILE) as f:
        names = [line.strip() for line in f if line.strip()]
    return names[:n] if n else names


async def connect_all(names: list[str]) -> list[tuple[str, Sandbox]]:
    """Connect to each VM sequentially so output is ordered and easy to read."""
    print(f"Connecting to {len(names)} VMs (sequential, timeout={CONNECT_TIMEOUT}s each)...\n")
    sandboxes = []
    for i, name in enumerate(names, 1):
        print(f"  [{i:>3}/{len(names)}] {name} ...", end=" ", flush=True)
        try:
            sb = await asyncio.wait_for(Sandbox.connect(name=name), timeout=CONNECT_TIMEOUT)
            print("✓", flush=True)
            sandboxes.append((name, sb))
        except asyncio.TimeoutError:
            print("✗ timeout", flush=True)
        except Exception as e:
            print(f"✗ {e}", flush=True)
    return sandboxes


async def run_step(name: str, sb: Sandbox) -> float:
    """Tap center + screenshot. Returns elapsed seconds."""
    t0 = time.monotonic()
    w, h = await sb.screen.size()
    await sb.mouse.click(w // 2, h // 2)
    await sb.screenshot(format="jpeg")
    return time.monotonic() - t0


# ── main ─────────────────────────────────────────────────────────────────────


async def main(n: int | None = None, rounds: int = ROUNDS):
    OUT_DIR.mkdir(exist_ok=True)

    names = load_names(n)
    print(f"\n{'='*50}")
    print("  Fleet Throughput Demo")
    print(f"  {len(names)} VMs  |  {rounds} rounds each  |  concurrency={CONCURRENCY}")
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

    print(f"Running {rounds} rounds of tap+screenshot (concurrency={CONCURRENCY})...\n")
    t_start = time.monotonic()

    for round_num in range(1, rounds + 1):
        print(f"── Round {round_num}/{rounds} ──")

        async def step(name: str, sb: Sandbox):
            async with sem:
                try:
                    elapsed = await run_step(name, sb)
                    print(f"  {name}  {elapsed * 1000:.0f}ms")
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

    # Save results
    steps_per_hour = total_steps / t_elapsed * 3600
    latencies.sort()
    p50 = latencies[len(latencies) // 2] * 1000
    p95 = latencies[int(len(latencies) * 0.95)] * 1000

    results = (
        f"VMs active     : {len(sandboxes)}\n"
        f"Total steps    : {total_steps}\n"
        f"Elapsed        : {t_elapsed:.1f}s\n"
        f"Throughput     : {steps_per_hour:,.0f} steps/hour\n"
        f"Latency p50    : {p50:.0f}ms\n"
        f"Latency p95    : {p95:.0f}ms\n"
    )
    (OUT_DIR / "fleet_results.txt").write_text(results)

    print(f"\n{'='*50}")
    print("  Results")
    print(f"{'='*50}")
    print(results, end="")
    print(f"  Saved to       : {OUT_DIR}/fleet_results.txt")
    print(f"{'='*50}\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--n", type=int, default=None, help="Number of VMs to use (default: all)")
    parser.add_argument(
        "--rounds", type=int, default=ROUNDS, help=f"Rounds per VM (default: {ROUNDS})"
    )
    args = parser.parse_args()
    asyncio.run(main(n=args.n, rounds=args.rounds))
