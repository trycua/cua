#!/usr/bin/env python3
"""Run a command and enforce a memory budget on its whole process tree.

    python3 scripts/rss-budget.py --budget-mb 6144 -- pnpm build
    python3 scripts/rss-budget.py --budget-mb 4096 --report peak.json -- next dev -p 8190

Samples the resident set size of the command and every descendant every
second (`ps`, so it works on macOS and Linux). Prints the peak when the command
exits. When the tree exceeds the budget it is killed and the exit status is 99,
so CI fails instead of the runner (or a laptop) running out of memory.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time


def tree_rss_kb(root: int) -> int:
    out = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,rss="], capture_output=True, text=True).stdout
    children: dict[int, list[int]] = {}
    rss: dict[int, int] = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) != 3:
            continue
        pid, ppid, kb = (int(p) for p in parts)
        children.setdefault(ppid, []).append(pid)
        rss[pid] = kb
    total, stack, seen = 0, [root], set()
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        total += rss.get(pid, 0)
        stack.extend(children.get(pid, []))
    return total


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--budget-mb", type=int, required=True)
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("--report", help="write {peak_mb, budget_mb, exceeded, status} as JSON")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd
    if not cmd:
        ap.error("missing command")
    env = dict(os.environ)
    env.setdefault("NODE_OPTIONS", "--max-old-space-size=4096")
    proc = subprocess.Popen(cmd, env=env, start_new_session=True)
    peak, exceeded = 0, False
    while proc.poll() is None:
        kb = tree_rss_kb(proc.pid)
        peak = max(peak, kb)
        if kb > a.budget_mb * 1024:
            exceeded = True
            print(f"[rss-budget] {kb // 1024} MB > budget {a.budget_mb} MB: killing {cmd[0]}", file=sys.stderr)
            os.killpg(proc.pid, signal.SIGKILL)
            break
        time.sleep(a.interval)
    status = proc.wait()
    result = {"peak_mb": peak // 1024, "budget_mb": a.budget_mb, "exceeded": exceeded, "status": status}
    print(f"[rss-budget] peak {result['peak_mb']} MB (budget {a.budget_mb} MB), exit {status}", file=sys.stderr)
    if a.report:
        with open(a.report, "w") as f:
            json.dump(result, f)
    if exceeded:
        return 99
    return status


if __name__ == "__main__":
    sys.exit(main())
