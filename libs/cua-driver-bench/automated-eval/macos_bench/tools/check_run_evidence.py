#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Check that a bench run left complete evidence and no stray processes.

Usage: check_run_evidence.py RUN_DIR | --ledger-only FILE
Exit 0 complete, 1 incomplete, 2 usage. Evidence completeness is separate from task success.
Only signal 0 is ever sent.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

CUA_ARMS = (
    "cc-cua-driver",
    "cc-cua-driver-main",
    "cc-cua-driver-main-skill",
    "cc-cua-driver-script",
)


def _zombie(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text("utf-8").rsplit(")", 1)[1].split()[0] == "Z"
    except (OSError, IndexError):
        return False


def _alive(pid: int, group: bool) -> bool:
    try:
        (os.killpg if group else os.kill)(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        pass
    return not (not group and _zombie(pid))


def check_ledger(path: Path) -> list[str]:
    problems: list[str] = []
    try:
        lines = path.read_text("utf-8").splitlines()
    except OSError as error:
        return [f"ledger unreadable: {path}: {error}"]
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            entry = json.loads(line)
            pid = entry["pid"]
            pgid = entry.get("pgid")
            if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
                raise ValueError("pid must be a positive integer")
            if pgid is not None and (
                not isinstance(pgid, int) or isinstance(pgid, bool) or pgid <= 0
            ):
                raise ValueError("pgid must be a positive integer or null")
        except (ValueError, KeyError, TypeError, AttributeError) as error:
            problems.append(f"{path.name}:{number} malformed ledger line: {error}")
            continue
        role = entry.get("role")
        if _alive(pid, False):
            problems.append(f"process still alive: pid {pid} ({role})")
        elif pgid is not None and (pgid != pid or not _zombie(pid)) and _alive(pgid, True):
            problems.append(f"process group still alive: pgid {pgid} ({role})")
    return problems


def _nonempty(path: Path) -> bool:
    return path.is_file() and path.stat().st_size > 0


def check_run(run: Path) -> list[str]:
    problems: list[str] = []
    results = run / "results.jsonl"
    rows: list[dict] = []
    try:
        lines = results.read_text("utf-8").splitlines()
    except OSError as error:
        return [f"results unreadable: {results}: {error}"]
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
            if not isinstance(row, dict):
                raise ValueError("row is not an object")
            row["trial_id"], row["attempt"]
        except (ValueError, KeyError) as error:
            problems.append(f"results.jsonl:{number} malformed row: {error!r}")
            continue
        rows.append(row)
    if not rows and not problems:
        problems.append("results.jsonl has no rows")
    recorder, sentinel = "none", True
    try:
        manifest = json.loads((run / "manifest.json").read_text("utf-8"))
        recorder = manifest.get("recorder", "none")
        sentinel = "--no-sentinel" not in (manifest.get("argv") or [])
    except (OSError, ValueError) as error:
        problems.append(f"manifest.json unreadable: {error}")
    seen: set[tuple] = set()
    for row in rows:
        key = (row["trial_id"], row["attempt"])
        if key in seen:
            problems.append(f"duplicate row {key[0]} a{key[1]}")
            continue
        seen.add(key)
        name = f"{key[0]} a{key[1]}"
        attempt = run / "trials" / str(key[0]) / f"a{key[1]}"
        if not attempt.is_dir():
            problems.append(f"{name}: no attempt directory")
            continue
        art = attempt / "artifacts"
        if not _nonempty(attempt / "trial.json"):
            problems.append(f"{name}: missing trial.json")
        if not (_nonempty(art / "final.png") or _nonempty(art / "failure.png")):
            problems.append(f"{name}: missing screenshot")
        if row.get("arm") in CUA_ARMS and not _nonempty(art / "driver.log"):
            problems.append(f"{name}: missing driver.log")
        if sentinel:
            events = 0
            try:
                for line in (art / "sentinel.jsonl").read_text("utf-8").splitlines():
                    if line.strip():
                        json.loads(line)
                        events += 1
            except (OSError, ValueError):
                events = 0
            if not events:
                problems.append(f"{name}: sentinel events missing or invalid")
        if recorder != "none":
            videos = [p for p in attempt.rglob("video*.mp4") if p.stat().st_size > 0]
            if not row.get("recording_started"):
                problems.append(f"{name}: recording not started")
            elif not videos:
                problems.append(f"{name}: compressed video missing")
    trials = run / "trials"
    if trials.is_dir():
        for directory in sorted(trials.glob("*/a*")):
            if directory.is_dir():
                suffix = directory.name[1:]
                if not any(
                    str(k[0]) == directory.parent.name and str(k[1]) == suffix for k in seen
                ):
                    problems.append(
                        f"orphan attempt directory {directory.parent.name}/{directory.name}"
                    )
    ledger = run.resolve().parent / "pids.jsonl"
    if ledger.is_file():
        problems += check_ledger(ledger)
    return problems


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "--ledger-only":
        problems = check_ledger(Path(argv[1]))
        target = None
    elif len(argv) == 1 and not argv[0].startswith("-"):
        target = Path(argv[0])
        if not target.is_dir():
            print(f"usage: not a directory: {target}", file=sys.stderr)
            return 2
        problems = check_run(target)
    else:
        print("usage: check_run_evidence.py RUN_DIR | --ledger-only FILE", file=sys.stderr)
        return 2
    for problem in problems:
        print(f"Invalid evidence: {problem}", file=sys.stderr)
    if target is not None:
        (target / "evidence-check.json").write_text(
            json.dumps({"ok": not problems, "problems": problems}, indent=2) + "\n", "utf-8"
        )
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
