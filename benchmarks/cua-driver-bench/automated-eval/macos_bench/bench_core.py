"""Pure logic for the Claude Code benchmark runner: schedule, quota gate, spend ledger, backoff,
wall-clock cutoff and round bookkeeping. No GUI, no model calls, so it is unit-tested.

Pre-registered rules implemented here:

* Task-major blocks in a fixed priority order, never shuffled. Phase 1: for each task, runs 1-3, each
  run a pair of back-to-back trials (one per arm). Phase 2 (only after phase 1 is complete for every
  task and quota allows): for each task, runs 4-5 as a block of 2 pairs.
* For the task at 0-based position ``i`` and 0-based run ``r`` the first arm of the pair is
  ``arms[0]`` when ``(i + r)`` is even and ``arms[1]`` otherwise. Both arms share the run's seed.
* A block is the unit of completeness. Only complete blocks are analysed; rows of an interrupted
  block are kept and flagged ``block_incomplete``.
* No dollar cap by default (subscription). ``total_cost_usd`` is recorded as equivalent cost.
* Stop starting trials when the 7-day quota reaches 0.95 or a quota event reads "rejected" on
  the 7-day window. A rejected 5-hour window waits for its reset instead.
"""

from __future__ import annotations

import fcntl
import json
import re
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

SEVEN_DAY_STOP = 0.95
BACKOFF_BASE_S = 60.0
BACKOFF_CAP_S = 900.0
RESET_WAIT_MAX_S = 3 * 3600.0
OVERLOAD_RETRIES = 3


# ---------------------------------------------------------------- tasks and schedule


def priority_key(task_id: str) -> tuple[int, int, str]:
    """CDB-* first (primary result), then MB-NN probes in numeric order, anything else after, by name."""
    if task_id.startswith("CDB-"):
        return (0, 0, task_id)
    match = re.fullmatch(r"MB-(\d+)", task_id)
    if match:
        return (1, int(match.group(1)), task_id)
    return (2, 0, task_id)


def order_tasks(task_ids: Iterable[str], explicit: bool = False) -> list[str]:
    ids = list(task_ids)
    return ids if explicit else sorted(ids, key=priority_key)


def first_arm_index(task_pos: int, run_index: int, n_arms: int = 2) -> int:
    """Which arm runs first in the pair for task ``task_pos`` and run ``run_index`` (both 0-based):
    arms[0] when (task_pos + run_index) is even, else arms[1]."""
    if n_arms == 2:
        return 0 if (task_pos + run_index) % 2 == 0 else 1
    return (task_pos + run_index) % n_arms


def arm_order(task_pos: int, run_index: int, arms: list[str]) -> list[str]:
    start = first_arm_index(task_pos, run_index, len(arms))
    return arms[start:] + arms[:start]


def trial_id(task: str, run_index: int, arm: str) -> str:
    return f"{task}-r{run_index + 1}-{arm}"


def block_id(phase: int, task: str) -> str:
    return f"P{phase}-{task}"


def build_pair(
    task_ids: list[str], task: str, run_index: int, arms: list[str], schedule_seed: int, phase: int
) -> list[dict[str, Any]]:
    """The two trials (one per arm) of one run of one task, in execution order."""
    pos = task_ids.index(task)
    entries = []
    for slot, arm in enumerate(arm_order(pos, run_index, arms)):
        entries.append(
            {
                "trial_id": trial_id(task, run_index, arm),
                "phase": phase,
                "block": block_id(phase, task),
                "run_index": run_index,
                "run": run_index + 1,
                "task": task,
                "arm": arm,
                "task_pos": pos,
                "arm_slot": slot,
                "first_arm": slot == 0,
                "schedule_seed": schedule_seed,
            }
        )
    return entries


def build_blocks(
    task_ids: list[str], arms: list[str], phase1_runs: int, phase2_runs: int, schedule_seed: int
) -> list[dict[str, Any]]:
    """Task-major blocks. Phase 1: every task in priority order, runs 0..phase1_runs-1, each run a pair of
    back-to-back trials (one per arm). Phase 2: every task again, the next ``phase2_runs`` runs. A block is
    the unit of completeness. Order is deterministic; no randomness is used (the seed is only recorded)."""
    blocks: list[dict[str, Any]] = []
    for phase, first, count in ((1, 0, phase1_runs), (2, phase1_runs, phase2_runs)):
        if count <= 0:
            continue
        for task in task_ids:
            entries: list[dict[str, Any]] = []
            for run_index in range(first, first + count):
                entries.extend(build_pair(task_ids, task, run_index, arms, schedule_seed, phase))
            blocks.append(
                {
                    "id": block_id(phase, task),
                    "phase": phase,
                    "task": task,
                    "runs": list(range(first, first + count)),
                    "entries": entries,
                }
            )
    index = 0
    for block in blocks:
        for entry in block["entries"]:
            entry["order_index"] = index
            index += 1
    return blocks


def flat_entries(blocks: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [e for b in blocks for e in b["entries"]]


def probe_seed(task_id: str, run_index: int) -> int:
    """Task seed, identical for both arms: sha256(task:run_index) mod 10^6 (as in the pilot)."""
    import hashlib

    digest = hashlib.sha256(f"{task_id}:{run_index}".encode()).hexdigest()
    return int(digest[:8], 16) % 1_000_000


def phase2_allowed(blocks: list[dict[str, Any]], final_ids: set[str], quota_ok: bool) -> bool:
    """Phase 2 starts only when every phase-1 block is complete and quota allows."""
    if not quota_ok:
        return False
    return all(e["trial_id"] in final_ids for b in blocks if b["phase"] == 1 for e in b["entries"])


def block_status(
    rows: Iterable[dict[str, Any]], blocks: list[dict[str, Any]]
) -> dict[str, dict[str, Any]]:
    """Completeness per block: every (run, arm) trial has a final row. Smoke rows are ignored."""
    final: dict[str, dict[str, Any]] = {}
    for row in rows:
        if row.get("smoke") or not row.get("final", True):
            continue
        final[row["trial_id"]] = row
    out: dict[str, dict[str, Any]] = {}
    for block in blocks:
        want = [e["trial_id"] for e in block["entries"]]
        have = [t for t in want if t in final]
        out[block["id"]] = {
            "complete": len(have) == len(want),
            "n_trials": len(have),
            "n_expected": len(want),
            "n_excluded": sum(1 for t in have if final[t].get("excluded")),
        }
    return out


def flag_incomplete(
    rows: list[dict[str, Any]], blocks: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    status = block_status(rows, blocks)
    where = {e["trial_id"]: b["id"] for b in blocks for e in b["entries"]}
    for row in rows:
        if row.get("smoke"):
            row["block_incomplete"] = False
            continue
        bid = where.get(row["trial_id"])
        row["block"] = bid
        row["block_incomplete"] = not (bid and status[bid]["complete"])
    return rows


# ---------------------------------------------------------------- quota


def quota_gate(
    quota: dict[str, Any] | None, seven_day_stop: float = SEVEN_DAY_STOP
) -> dict[str, Any]:
    """Decide from the latest normalised quota event (see claude_events.quota_from_event).

    Returns ``{"action": "go" | "stop" | "wait", "reason": str, "wait_until": epoch | None}``.
    """
    if not quota:
        return {"action": "go", "reason": "no quota data yet", "wait_until": None}
    seven = quota.get("seven_day")
    five = quota.get("five_hour")
    status = quota.get("status")
    kind = quota.get("type")
    if seven is not None and seven >= seven_day_stop:
        return {
            "action": "stop",
            "reason": f"seven_day utilization {seven:.3f} >= {seven_day_stop}",
            "wait_until": None,
        }
    if status == "rejected":
        if kind == "five_hour" and (seven is None or seven < seven_day_stop):
            return {
                "action": "wait",
                "reason": "five_hour window rejected",
                "wait_until": quota.get("five_hour_resets_at") or quota.get("resets_at"),
            }
        return {"action": "stop", "reason": f"quota rejected ({kind})", "wait_until": None}
    if five is not None and five >= 1.0 and (seven is None or seven < seven_day_stop):
        return {
            "action": "wait",
            "reason": f"five_hour utilization {five:.3f}",
            "wait_until": quota.get("five_hour_resets_at"),
        }
    return {"action": "go", "reason": "ok", "wait_until": None}


# ---------------------------------------------------------------- backoff, cutoff


def backoff_seconds(
    attempt: int, base: float = BACKOFF_BASE_S, cap: float = BACKOFF_CAP_S
) -> float:
    """Exponential backoff: base, 2*base, 4*base ... capped. ``attempt`` is 0-based."""
    return min(cap, base * (2 ** max(0, attempt)))


def wait_seconds(
    attempt: int,
    reset_epoch: float | None,
    now: float | None = None,
    max_wait: float = RESET_WAIT_MAX_S,
) -> float:
    """Wait until the reset time when the message carries one (plus a small margin, at most
    ``max_wait``); otherwise exponential backoff."""
    now = time.time() if now is None else now
    if reset_epoch and reset_epoch > now:
        return min(max_wait, reset_epoch - now + 30.0)
    return backoff_seconds(attempt)


def parse_utc(text: str) -> float:
    """ISO-8601 -> epoch seconds. A value without a zone is read as UTC."""
    value = datetime.fromisoformat(text.strip().replace("Z", "+00:00"))
    if value.tzinfo is None:
        value = value.replace(tzinfo=timezone.utc)
    return value.timestamp()


def past_cutoff(cutoff_epoch: float | None, now: float | None = None) -> bool:
    return cutoff_epoch is not None and (time.time() if now is None else now) >= cutoff_epoch


def block_fits(
    cutoff_epoch: float | None,
    est_block_s: float | None,
    now: float | None = None,
    margin: float = 1.1,
) -> bool:
    """For phase-2 blocks: start only if a whole block is expected to finish before the cutoff so a
    block is not wasted as incomplete. Unknown estimate means do not start."""
    if cutoff_epoch is None:
        return True
    if not est_block_s:
        return False
    now = time.time() if now is None else now
    return now + est_block_s * margin <= cutoff_epoch


# ---------------------------------------------------------------- ledger


class Ledger:
    """Shared append-only JSONL ledger of model spend (equivalent cost), safe across processes."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)

    def append(
        self, who: str, purpose: str, model: str | None, cost_usd: float | None, **extra: Any
    ) -> dict[str, Any]:
        entry = {
            "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "who": who,
            "purpose": purpose,
            "model": model,
            "cost_usd": cost_usd,
            **extra,
        }
        with self.path.open("a", encoding="utf-8") as handle:
            fcntl.flock(handle, fcntl.LOCK_EX)
            try:
                handle.write(json.dumps(entry) + "\n")
                handle.flush()
            finally:
                fcntl.flock(handle, fcntl.LOCK_UN)
        return entry

    def entries(self) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        out = []
        for line in self.path.read_text("utf-8").splitlines():
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                continue
        return out

    def cumulative(self, who: str | None = None) -> float:
        return round(
            sum(
                float(e.get("cost_usd") or 0.0)
                for e in self.entries()
                if who is None or e.get("who") == who
            ),
            6,
        )


def budget_allows(cumulative: float, per_trial_cap: float, cap: float | None) -> bool:
    """Optional dollar gate. ``cap=None`` (the default) never refuses."""
    if cap is None:
        return True
    return cumulative + per_trial_cap <= cap


# ---------------------------------------------------------------- resume


def completed_trial_ids(rows: Iterable[dict[str, Any]]) -> set[str]:
    """Trial ids that are final: a row exists that is not a retryable infra failure awaiting retry."""
    done: set[str] = set()
    for row in rows:
        if row.get("smoke"):
            continue
        if row.get("final", True):
            done.add(row["trial_id"])
    return done


def load_rows(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    rows = []
    for line in path.read_text("utf-8").splitlines():
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


# ---------------------------------------------------------------- pins


PIN_INFO_KEYS = (
    "release",
    "cua_driver_git_sha",
    "cua_driver_tarball",
    "cua_skills_tarball",
)  # recorded, checked elsewhere


def compare_pins(
    expected: dict[str, Any], observed: dict[str, Any]
) -> list[tuple[str, Any, Any, bool]]:
    """One row per expected pin: (name, expected, observed, matches). A pin that was not observed fails."""
    rows = []
    for key, want in expected.items():
        if key.startswith("_") or key in PIN_INFO_KEYS:
            continue
        got = observed.get(key)
        rows.append((key, want, got, got == want))
    return rows


# ---------------------------------------------------------------- latest known quota (no model call)


def persist_quota(path: Path, quota: dict[str, Any], source: str) -> None:
    """Remember the latest quota reading so the launcher can gate without a model call."""
    path.parent.mkdir(parents=True, exist_ok=True)
    keep = {
        k: quota.get(k)
        for k in (
            "five_hour",
            "seven_day",
            "status",
            "type",
            "five_hour_resets_at",
            "seven_day_resets_at",
            "resets_at",
        )
    }
    keep.update(ts=datetime.now(timezone.utc).isoformat(timespec="seconds"), source=source)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(keep, indent=2) + "\n", "utf-8")
    tmp.replace(path)


def latest_known_quota(latest_path: Path, ledger_path: Path) -> dict[str, Any] | None:
    """The most recent seven-day reading from quota_latest.json or from ledger entries that carry
    ``seven_day_after``. Returns None when nothing is known."""
    candidates: list[dict[str, Any]] = []
    if latest_path.exists():
        try:
            data = json.loads(latest_path.read_text("utf-8"))
            if data.get("seven_day") is not None:
                candidates.append(data)
        except json.JSONDecodeError:
            pass
    if ledger_path.exists():
        for line in ledger_path.read_text("utf-8").splitlines():
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            if entry.get("seven_day_after") is not None:
                candidates.append(
                    {
                        "ts": entry["ts"],
                        "seven_day": entry["seven_day_after"],
                        "five_hour": None,
                        "source": f"ledger:{entry.get('purpose', '')[:40]}",
                    }
                )
    if not candidates:
        return None
    return max(candidates, key=lambda c: c.get("ts", ""))


def gate_decision(
    latest: dict[str, Any] | None, stop: float = SEVEN_DAY_STOP, now: float | None = None
) -> dict[str, Any]:
    """Launch gate from the latest known quota. The seven-day figure only grows until its reset, so a
    reading at or above ``stop`` blocks the launch until the window has reset."""
    now = time.time() if now is None else now
    if not latest or latest.get("seven_day") is None:
        return {
            "action": "unknown",
            "reason": "no quota reading on file; the runner's preflight probes the quota itself",
        }
    seven = float(latest["seven_day"])
    resets = latest.get("seven_day_resets_at") or latest.get("resets_at")
    window_over = bool(resets and float(resets) < now and latest.get("status") != "rejected")
    if seven >= stop and not window_over:
        until = (
            datetime.fromtimestamp(float(resets), timezone.utc).isoformat(timespec="minutes")
            if resets
            else "unknown"
        )
        return {
            "action": "gated",
            "seven_day": seven,
            "as_of": latest.get("ts"),
            "reason": f"seven_day utilization {seven:.3f} >= {stop} as of {latest.get('ts')}; window resets {until}",
        }
    if window_over:
        return {
            "action": "unknown",
            "reason": f"last reading ({seven:.3f}) is from a window that has reset; the runner will probe",
        }
    return {
        "action": "go",
        "seven_day": seven,
        "as_of": latest.get("ts"),
        "reason": f"seven_day utilization {seven:.3f} < {stop} as of {latest.get('ts')}",
    }
