#!/usr/bin/env python3
"""Check a guard.json from guard.sh.

Invariants (any host):
  - no create failed and no rollout step failed;
  - a headless create is at most half a desktop create (p50);
  - an idle headless Space's cua-spacesd uses < 2% of a core;
  - a warm-pool claim (p50, 16 at once) is under a third of the wall time of 16 cold headless creates at once.
With --baseline (the same host's committed numbers): no time metric above
1.5x its baseline and no throughput below baseline / 1.5.
"""
import argparse, json, sys

TIMES = ["headless_create_p50_ms", "headless_b16_wall_ms", "pool_claim_batch_p50_ms"]
RATES = ["rollout_steps_per_sec"]
FACTOR = 1.5


def check(m, base=None):
    errs = []
    if m.get("create_failures"):
        errs.append(f"{m['create_failures']} creates failed")
    if m.get("rollout_errors"):
        errs.append(f"{m['rollout_errors']} rollout steps failed")
    d, h = m.get("desktop_create_p50_ms"), m.get("headless_create_p50_ms")
    if d and h and h > d / 2:
        errs.append(f"headless create p50 {h} ms is not under half the desktop's {d} ms")
    cpu = m.get("idle_spacesd_cpu_pct")
    if cpu is None or cpu < 0 or cpu >= 2:
        errs.append(f"idle headless cua-spacesd uses {cpu}% of a core (want < 2)")
    p, w = m.get("pool_claim_batch_p50_ms"), m.get("headless_b16_wall_ms")
    if p and w and p > w / 3:
        errs.append(f"pool claim p50 {p} ms is not under a third of 16 cold creates ({w} ms wall)")
    for k in ["headless_create_p50_ms", "rollout_steps_per_sec", "pool_claim_batch_p50_ms"]:
        if m.get(k) is None:
            errs.append(f"{k} missing")
    if base:
        for k in TIMES:
            if m.get(k) and base.get(k) and m[k] > base[k] * FACTOR:
                errs.append(f"{k} {m[k]} > {FACTOR}x baseline {base[k]}")
        for k in RATES:
            if m.get(k) and base.get(k) and m[k] < base[k] / FACTOR:
                errs.append(f"{k} {m[k]:.0f} < baseline {base[k]:.0f} / {FACTOR}")
    return errs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("guard")
    ap.add_argument("--baseline")
    a = ap.parse_args()
    m = json.load(open(a.guard))
    base = json.load(open(a.baseline)) if a.baseline else None
    errs = check(m, base)
    for e in errs:
        print(f"FAIL: {e}")
    print("PASS" if not errs else f"{len(errs)} check(s) failed")
    sys.exit(1 if errs else 0)


if __name__ == "__main__":
    main()
