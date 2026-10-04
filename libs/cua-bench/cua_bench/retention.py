"""Disk use of cua-bench results: a size warning and opt-in retention.

Every run keeps its results (``summary.json``, traces, trajectories and
screenshots) under ``$XDG_DATA_HOME/cua-bench/runs/<run_id>`` or
``--output-dir``. A run with screenshots takes tens of MB per 100 steps, so
the runs directory grows with every benchmark.

* After each run, cua-bench warns when the results directory is larger than
  ``CUA_BENCH_WARN_SIZE`` (default 10 GiB; ``0`` turns the warning off).
* Nothing is ever deleted unless retention is configured explicitly, with
  ``--keep-runs N`` / ``--max-age DAYS`` / ``--max-results-size SIZE`` on
  ``cb run``, the ``CUA_BENCH_KEEP_RUNS`` / ``CUA_BENCH_MAX_AGE_DAYS`` /
  ``CUA_BENCH_MAX_RESULTS_SIZE`` environment variables, or ``cb prune --runs
  --keep N`` / ``--older-than DAYS``. Retention removes whole runs, oldest
  first, and never the run that just finished or a run that is still going
  (its ``run.pid`` names a live process).
"""

from __future__ import annotations

import os
import re
import shutil
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Optional

#: Default size above which cua-bench warns about the results directory.
DEFAULT_WARN_BYTES = 10 * 1024**3

_UNITS = {"": 1, "b": 1, "k": 1024, "m": 1024**2, "g": 1024**3, "t": 1024**4}


def parse_size(text: str) -> int:
    """``500M``, ``20G``, ``20GiB``, ``1.5T`` or plain bytes (binary units)."""
    match = re.fullmatch(r"\s*([0-9]+(?:\.[0-9]+)?)\s*([kmgt]?)(?:i?b)?\s*", str(text), re.I)
    if not match:
        raise ValueError(f"not a size: {text!r} (examples: 500M, 20G)")
    return int(float(match.group(1)) * _UNITS[match.group(2).lower()])


def format_size(size: float) -> str:
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if size < 1024 or unit == "TiB":
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} TiB"


def dir_size(path: Path, limit_entries: int = 2_000_000) -> int:
    """Bytes under ``path`` (files only, symlinks not followed). Walks at most
    ``limit_entries`` entries so a huge tree cannot stall a run."""
    total = 0
    seen = 0
    stack = [Path(path)]
    while stack:
        current = stack.pop()
        try:
            entries = list(os.scandir(current))
        except OSError:
            continue
        for entry in entries:
            seen += 1
            if seen > limit_entries:
                return total
            try:
                if entry.is_dir(follow_symlinks=False):
                    stack.append(Path(entry.path))
                elif entry.is_file(follow_symlinks=False):
                    total += entry.stat(follow_symlinks=False).st_size
            except OSError:
                continue
    return total


def warn_bytes() -> int:
    """The warning threshold (``CUA_BENCH_WARN_SIZE``; ``0`` disables)."""
    raw = os.environ.get("CUA_BENCH_WARN_SIZE", "").strip()
    if not raw:
        return DEFAULT_WARN_BYTES
    try:
        return parse_size(raw)
    except ValueError:
        return DEFAULT_WARN_BYTES


def size_warning(results_dir: Path, threshold: Optional[int] = None) -> Optional[str]:
    """A warning when ``results_dir`` is larger than the threshold, else None."""
    threshold = warn_bytes() if threshold is None else threshold
    if threshold <= 0 or not Path(results_dir).exists():
        return None
    size = dir_size(Path(results_dir))
    if size <= threshold:
        return None
    return (
        f"cua-bench results in {results_dir} take {format_size(size)} "
        f"(over {format_size(threshold)}). Keep fewer runs with `cb prune --runs --keep N` "
        f"or `cb run ... --keep-runs N`; nothing is deleted automatically. "
        f"Set CUA_BENCH_WARN_SIZE to change this warning (0 turns it off)."
    )


@dataclass(frozen=True)
class Retention:
    """Which runs to keep. Every field ``None`` (the default) keeps all."""

    keep_runs: Optional[int] = None
    max_age_days: Optional[float] = None
    max_bytes: Optional[int] = None

    @property
    def enabled(self) -> bool:
        return any(v is not None for v in (self.keep_runs, self.max_age_days, self.max_bytes))

    @classmethod
    def from_env(cls, env: Optional[dict] = None) -> "Retention":
        env = os.environ if env is None else env

        def get(name: str) -> Optional[str]:
            value = env.get(name, "").strip()
            return value or None

        keep = get("CUA_BENCH_KEEP_RUNS")
        age = get("CUA_BENCH_MAX_AGE_DAYS")
        size = get("CUA_BENCH_MAX_RESULTS_SIZE")
        return cls(
            keep_runs=int(keep) if keep is not None else None,
            max_age_days=float(age) if age is not None else None,
            max_bytes=parse_size(size) if size is not None else None,
        )

    def merged(self, other: "Retention") -> "Retention":
        """``other``'s fields where set, else this one's (flags over env)."""
        return Retention(
            keep_runs=other.keep_runs if other.keep_runs is not None else self.keep_runs,
            max_age_days=(
                other.max_age_days if other.max_age_days is not None else self.max_age_days
            ),
            max_bytes=other.max_bytes if other.max_bytes is not None else self.max_bytes,
        )


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


def _running(run_dir: Path) -> bool:
    try:
        pid = int((run_dir / "run.pid").read_text().strip())
    except (OSError, ValueError):
        return False
    return _pid_alive(pid)


def run_dirs(runs_dir: Path) -> list[Path]:
    """Run directories, newest first (by modification time)."""
    try:
        dirs = [p for p in Path(runs_dir).iterdir() if p.is_dir() and not p.is_symlink()]
    except OSError:
        return []
    return sorted(dirs, key=lambda p: p.stat().st_mtime, reverse=True)


def plan(
    runs_dir: Path,
    policy: Retention,
    protect: Iterable[Path] = (),
    now: Optional[float] = None,
) -> list[Path]:
    """Runs ``policy`` would delete, oldest first. Never ``protect`` or a
    run whose process is alive."""
    if not policy.enabled:
        return []
    now = time.time() if now is None else now
    protected = {Path(p).resolve() for p in protect}
    runs = run_dirs(runs_dir)
    doomed: list[Path] = []
    for index, run in enumerate(runs):
        if run.resolve() in protected or _running(run):
            continue
        too_many = policy.keep_runs is not None and index >= policy.keep_runs
        too_old = (
            policy.max_age_days is not None
            and now - run.stat().st_mtime > policy.max_age_days * 86_400
        )
        if too_many or too_old:
            doomed.append(run)
    if policy.max_bytes is not None:
        sizes = {run: dir_size(run) for run in runs}
        total = sum(sizes[r] for r in runs if r not in doomed)
        for run in reversed(runs):  # oldest first
            if total <= policy.max_bytes:
                break
            if run in doomed or run.resolve() in protected or _running(run):
                continue
            doomed.append(run)
            total -= sizes[run]
    return sorted(doomed, key=lambda p: p.stat().st_mtime)


def apply(
    runs_dir: Path,
    policy: Retention,
    protect: Iterable[Path] = (),
    dry_run: bool = False,
) -> list[Path]:
    """Deletes what :func:`plan` returns (unless ``dry_run``); returns it."""
    doomed = plan(runs_dir, policy, protect)
    if not dry_run:
        for run in doomed:
            shutil.rmtree(run, ignore_errors=True)
    return doomed


#: ``cb interact --view`` traces kept (newest first) when no --trace-out is given.
INTERACT_TRACES_KEPT = 5
#: Leftover ``cua_trace_*`` directories older than this are removed from the
#: temp dir (cua-bench 0.3 and earlier wrote ``--view`` traces there).
STALE_TEMP_SECONDS = 24 * 3600


def interact_traces_dir() -> Path:
    """Where ``cb interact --view`` keeps its traces (XDG data)."""
    xdg = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    return Path(xdg) / "cua-bench" / "interact"


def new_interact_trace_dir() -> Path:
    """A fresh directory for one ``cb interact --view`` trace. Older traces
    beyond :data:`INTERACT_TRACES_KEPT` and stale temp-dir leftovers are
    removed first, so the traces never grow without bound."""
    root = interact_traces_dir()
    root.mkdir(parents=True, exist_ok=True)
    prune_newest(root, INTERACT_TRACES_KEPT - 1)
    prune_stale_temp("cua_trace_")
    import uuid

    path = root / f"{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
    path.mkdir()
    return path


def prune_newest(root: Path, keep: int) -> list[Path]:
    """Removes all but the ``keep`` newest directories under ``root``."""
    dirs = run_dirs(root)
    doomed = dirs[max(keep, 0) :]
    for d in doomed:
        shutil.rmtree(d, ignore_errors=True)
    return doomed


def prune_stale_temp(prefix: str, older_than: float = STALE_TEMP_SECONDS, now=None) -> list[Path]:
    """Removes ``<tmp>/<prefix>*`` directories older than ``older_than``."""
    import tempfile

    now = time.time() if now is None else now
    removed = []
    try:
        entries = list(Path(tempfile.gettempdir()).glob(f"{prefix}*"))
    except OSError:
        return removed
    for entry in entries:
        try:
            if entry.is_dir() and not entry.is_symlink() and now - entry.stat().st_mtime > older_than:
                shutil.rmtree(entry, ignore_errors=True)
                removed.append(entry)
        except OSError:
            continue
    return removed

