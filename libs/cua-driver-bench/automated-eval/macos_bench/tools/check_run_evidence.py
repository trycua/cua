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
    "pi-cua-driver",
    "pi-cua-driver-main",
    "pi-cua-driver-main-skill",
    "pi-cua-driver-script",
)


def _zombie(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text("utf-8").rsplit(")", 1)[1].split()[0] == "Z"
    except (OSError, IndexError):
        return False


def _group_has_live_member(pgid: int) -> bool:
    """True when some non-zombie process is in the group; falls back to the signal-0 answer."""
    found = False
    try:
        entries = list(Path("/proc").iterdir())
    except OSError:
        return True
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text("utf-8").rsplit(")", 1)[1].split()
            state, group = fields[0], int(fields[2])
        except (OSError, IndexError, ValueError):
            continue
        if group == pgid:
            found = True
            if state != "Z":
                return True
    return not found


def _start_ticks(pid: int) -> int | None:
    try:
        return int(Path(f"/proc/{pid}/stat").read_text("utf-8").rsplit(")", 1)[1].split()[19])
    except (OSError, IndexError, ValueError):
        return None


def _recycled(pid: int, start: object) -> bool:
    """True when a live process has this pid but is not the one the ledger recorded."""
    current = _start_ticks(pid)
    return isinstance(start, int) and current is not None and current != start


def _alive(pid: int, group: bool) -> bool:
    try:
        (os.killpg if group else os.kill)(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        pass
    return _group_has_live_member(pid) if group else not _zombie(pid)


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
        if _recycled(pid, entry.get("start")):
            continue  # the recorded process is gone; its pid now belongs to another one
        if _alive(pid, False):
            problems.append(f"process still alive: pid {pid} ({role})")
        elif pgid is not None and _alive(pgid, True):
            problems.append(f"process group still alive: pgid {pgid} ({role})")
    return problems


def _nonempty(path: Path) -> bool:
    return path.is_file() and path.stat().st_size > 0


def _valid_row(row: object) -> str | None:
    if not isinstance(row, dict):
        return "row is not an object"
    if not isinstance(row.get("trial_id"), str) or not row["trial_id"]:
        return "trial_id must be a non-empty string"
    attempt = row.get("attempt")
    if not isinstance(attempt, int) or isinstance(attempt, bool) or attempt < 1:
        return "attempt must be a positive integer"
    if not isinstance(row.get("arm"), str):
        return "arm must be a string"
    return None


def _video_problem(attempt: Path) -> str | None:
    video = attempt / "video"
    if not _nonempty(video / "video.mp4") or not _nonempty(video / "video-720p.mp4"):
        return "compressed video missing"
    try:
        meta = json.loads((video / "video.json").read_text("utf-8"))
    except (OSError, ValueError):
        return "video.json missing or invalid"
    if not isinstance(meta, dict):
        return "video.json is not an object"
    if meta.get("error"):
        return f"video compression failed: {meta['error']}"
    return None


RUN_MARKERS = ("run.lock", "results.jsonl", "manifest.json")  # a runner start writes run.lock first


def check_run(run: Path) -> list[str]:
    if not any((run / name).exists() for name in RUN_MARKERS):
        return [f"not a run directory: none of {', '.join(RUN_MARKERS)}"]
    problems: list[str] = []
    results = run / "results.jsonl"
    rows: list[dict] = []
    stopped = not (run / "DONE").exists()
    try:
        text = results.read_text("utf-8")
    except FileNotFoundError:
        text = ""  # checked below: only a stopped run may have no rows yet
    except OSError as error:
        return [f"results unreadable: {results}: {error}"]
    lines = text.splitlines()
    if lines and not text.endswith("\n") and stopped:
        lines.pop()  # a row cut off by the stop; resuming the run drops it and redoes that attempt
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
            invalid = _valid_row(row)
        except ValueError as error:
            invalid = f"invalid JSON: {error}"
        if invalid:
            problems.append(f"results.jsonl:{number} malformed row: {invalid}")
            continue
        rows.append(row)
    if not rows and not problems and not stopped:
        problems.append("results.jsonl has no rows")
    finished = (run / "DONE").exists()
    recorder, sentinel = "none", True
    try:
        manifest = json.loads((run / "manifest.json").read_text("utf-8"))
        if not isinstance(manifest, dict):
            raise ValueError("manifest is not an object")
        recorder, argv = manifest.get("recorder"), manifest.get("argv")
        if not isinstance(recorder, str) or not isinstance(argv, list):
            raise ValueError("manifest needs string recorder and list argv")
        sentinel = "--no-sentinel" not in argv
    except (OSError, ValueError) as error:
        if rows or not stopped:  # a run stopped before its first trial may have no manifest yet
            problems.append(f"manifest.json unreadable or invalid: {error}")
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
            if not row.get("recording_started"):
                problems.append(f"{name}: recording not started")
            else:
                video = _video_problem(attempt)
                # The raw file is kept until compression succeeds, and a stopped run's resume compresses it.
                pending = not finished and _nonempty(attempt / "rec" / "recording.mp4")
                if video and not pending:
                    problems.append(f"{name}: {video}")
    trials = run / "trials"
    final = {str(r["trial_id"]) for r in rows if r.get("final", True) and not r.get("smoke")}
    orphans, in_flight = [], []
    if trials.is_dir():
        for directory in sorted(trials.glob("*/a*")):
            if directory.is_dir():
                suffix = directory.name[1:]
                recorded = [k[1] for k in seen if str(k[0]) == directory.parent.name]
                if any(str(k) == suffix for k in recorded):
                    continue
                name = f"{directory.parent.name}/{directory.name}"
                nxt = suffix == str(max(recorded, default=0) + 1)
                (in_flight if nxt and directory.parent.name not in final else orphans).append(name)
    # The runner is serial: a stopped run that is not DONE has at most one attempt cut short, which a resume redoes.
    if finished or len(in_flight) > 1:
        orphans += in_flight
    problems += [f"orphan attempt directory {name}" for name in sorted(orphans)]
    for session in run.resolve().parents[:2]:
        ledger = session / "pids.jsonl"
        if ledger.is_file():
            problems += check_ledger(ledger)
            break
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
