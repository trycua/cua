"""Bounded no-progress detection for native jev-use runs.

This stays recipe-local. It never inspects model scores, pixels, element values,
or action arguments. App-owned progress is available only for built-in tasks
whose oracle exposes an intermediate monotonic score. For tasks without one,
only repetition of the same delivered candidate is treated as evidence; a
different delivered candidate starts a fresh evidence window.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal, Mapping

NO_PROGRESS_LIMIT = 3
StepKind = Literal["reobserve", "stale", "refused", "performed"]


@dataclass(frozen=True)
class NoProgressStop:
    pattern: str
    streak: int


def observed_progress_score(task_id: str, state: Mapping[str, Any]) -> int | None:
    """Return a value-free monotonic progress score when the app oracle exposes one."""
    if task_id.endswith("-counter"):
        counter = state.get("counter")
        return counter if isinstance(counter, int) and not isinstance(counter, bool) and counter >= 0 else None
    if task_id.endswith("-choose-size"):
        return int(state.get("size") == "large") + int(state.get("agreed") is True)
    if task_id == "canvas-cancel":
        return int(state.get("selected") == "cancel" and state.get("action_count") == 1)
    return None


class NoProgressGuard:
    def __init__(self, task_id: str, *, limit: int = NO_PROGRESS_LIMIT) -> None:
        if limit < 1:
            raise ValueError("no-progress limit must be positive")
        self.task_id = task_id
        self.limit = limit
        self.best_progress: int | None = None
        self.pending: tuple[StepKind, str] | None = None
        self.streak = 0
        self.recent: list[str] = []

    def note(self, kind: StepKind, candidate_id: str) -> None:
        self.pending = (kind, candidate_id.removesuffix(":foreground"))

    def before_step(self, oracle_state: Mapping[str, Any]) -> NoProgressStop | None:
        score = observed_progress_score(self.task_id, oracle_state)
        pending = self.pending
        self.pending = None

        if score is not None and (self.best_progress is None or score > self.best_progress):
            self.best_progress = score
            self._reset_evidence()
            return None

        if pending is None:
            return None
        kind, candidate_id = pending
        token = f"performed:{candidate_id}" if kind == "performed" else kind

        if kind == "performed" and score is None:
            # A successful dispatch is not proof of progress. When the app
            # exposes no intermediate score, only repeated delivery of the
            # same candidate is comparable evidence. Switching candidates
            # starts a fresh window (for example set note -> save note).
            if self.recent and any(item != token for item in self.recent):
                self._reset_evidence()

        return self._record(token)

    def _record(self, token: str) -> NoProgressStop | None:
        self.streak += 1
        self.recent.append(token)
        self.recent = self.recent[-self.limit :]
        if self.streak < self.limit:
            return None
        return NoProgressStop(self._pattern(), self.streak)

    def _reset_evidence(self) -> None:
        self.streak = 0
        self.recent.clear()

    def _pattern(self) -> str:
        if self.recent and all(item == "reobserve" for item in self.recent):
            return "reobserve"
        if self.recent and all(item.startswith("performed:") for item in self.recent):
            if len(set(self.recent)) == 1:
                return "same_candidate"
        if any(item in {"stale", "refused"} for item in self.recent):
            return "recovery"
        return "unchanged_progress"
