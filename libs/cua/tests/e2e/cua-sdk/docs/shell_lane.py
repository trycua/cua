#!/usr/bin/env python3
"""Shell lanes: run the docs shell blocks tagged for a host lane, on a
disposable CI machine of that kind (nightly, alert only).

    shell_lane.py --lane windows [--first PAGE]   # PowerShell blocks, pwsh
    shell_lane.py --lane lume [--first PAGE]      # bash blocks on macOS

Blocks run in page order (``--first`` pages before the others, e.g. the
install page), each in a fresh ``bash -euo pipefail`` or ``pwsh`` process,
so a later block sees what earlier ones installed on the machine but not
their shell state. These lanes install software: never run them on a
developer's machine. Results go to ``$CUA_E2E_RESULTS/docs-blocks.jsonl``.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path

import extract

HOST_OF_LANE = {"windows": "Windows", "lume": "Darwin"}


def runner(lang: str) -> list[str]:
    lang = extract.norm_lang(lang)
    if lang == "powershell":
        exe = shutil.which("pwsh") or shutil.which("powershell")
        if not exe:
            raise SystemExit("PowerShell is required for PowerShell blocks")
        return [exe, "-NoProfile", "-NonInteractive", "-Command"]
    if lang == "bash":
        return ["bash", "-euo", "pipefail", "-c"]
    raise SystemExit(f"no shell for {lang}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--lane", required=True, choices=sorted(HOST_OF_LANE))
    ap.add_argument("--first", action="append", default=[], help="pages to run first")
    ap.add_argument("--docs", type=Path, default=extract.DOCS)
    ap.add_argument("--timeout", type=int, default=900)
    a = ap.parse_args()
    want = HOST_OF_LANE[a.lane]
    if platform.system() != want or not os.environ.get("CI"):
        print(f"the {a.lane} lane runs on a disposable {want} CI machine only", file=sys.stderr)
        return 2
    blocks = [b for b in extract.all_blocks(a.docs) if a.lane in b.lanes]
    rank = {p: i for i, p in enumerate(a.first)}
    blocks.sort(key=lambda b: (rank.get(b.guide, len(rank)), b.guide, b.index))
    rows, failures = [], []
    for b in blocks:
        t0 = time.monotonic()
        try:
            out = subprocess.run(
                [*runner(b.lang), b.code], capture_output=True, text=True, timeout=a.timeout
            )
            ok, err = out.returncode == 0, out.stderr[-1500:] or out.stdout[-1500:]
        except subprocess.TimeoutExpired:
            ok, err = False, f"timed out after {a.timeout}s"
        print(f"{'pass' if ok else 'FAIL'}  {b.id}  ({time.monotonic() - t0:.0f}s)", flush=True)
        rows.append(
            {
                "block_id": b.id,
                "page": b.guide,
                "line": b.line,
                "lane": a.lane,
                "lang": b.lang,
                "status": "pass" if ok else "fail",
                "test": f"shell_lane[{a.lane}]",
                "reason": "" if ok else err.strip()[:400],
            }
        )
        if not ok:
            failures.append(f"{b.guide}:{b.line} ({b.id}):\n{err}")
    out_dir = os.environ.get("CUA_E2E_RESULTS")
    if out_dir:
        Path(out_dir).mkdir(parents=True, exist_ok=True)
        with open(Path(out_dir) / "docs-blocks.jsonl", "a", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    if not blocks:
        print(f'no block is tagged test="{a.lane}"', file=sys.stderr)
        return 1
    if failures:
        print(f"{a.lane} lane:", *failures, sep="\n\n", file=sys.stderr)
        return 1
    print(f"{a.lane} lane OK: {len(blocks)} block(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
