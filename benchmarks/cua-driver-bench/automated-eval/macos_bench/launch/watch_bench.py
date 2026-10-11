#!/usr/bin/env python3
"""Print the live state of a benchmark run: heartbeat, flags, last results, pauses.
python3 watch_bench.py [RUN_ID] [-f]    (-f repeats every 30 s)"""

import json
import sys
import time
from pathlib import Path

import os

WORK = Path(
    os.environ.get("CDB_BENCH_WORK") or Path.home() / ".cache" / "cua-bench-h2h"
).expanduser()


def show(run: Path) -> None:
    beat = run / "heartbeat.json"
    if beat.exists():
        b = json.loads(beat.read_text())
        q = b.get("quota") or {}
        print(
            f"{b['ts']} state={b['state']} trial={b.get('trial_id')} a{b.get('attempt')} elapsed={b.get('trial_elapsed_s')}s "
            f"done={b['trials_done']} blocks={b.get('blocks_complete')}/{b.get('blocks_total')} 5h={q.get('five_hour')} 7d={q.get('seven_day')} "
            f"spend_run=${b['spend_run_usd']} free={b['free_gb']}GB pause={b.get('pause')}"
        )
    for flag in (
        sorted(run.glob("STOPPED_*"))
        + sorted(run.glob("DONE"))
        + sorted(run.glob("PREFLIGHT_FAILED"))
    ):
        print("FLAG", flag.name)
    results = run / "results.jsonl"
    if results.exists():
        rows = [json.loads(ln) for ln in results.read_text().splitlines() if ln.strip()]
        for r in rows[-6:]:
            print(
                f"  {r['trial_id']} a{r['attempt']} {r['status']} passed={r['passed']} wall={r['wall_s']}s turns={r.get('turns')} ${r['cost_usd']:.3f} 7d={r.get('quota_seven_day_after')} infra={r.get('infra_failure')}"
            )
        print(f"  {len(rows)} rows, {sum(1 for r in rows if r['passed'])} passed")
    log = run / "runner.log"
    if log.exists():
        print("  log tail:", *log.read_text().splitlines()[-3:], sep="\n    ")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "-f"]
    run = WORK / "runs" / (args[0] if args else "main")
    while True:
        show(run)
        if "-f" not in sys.argv:
            break
        print("-" * 60)
        time.sleep(30)
