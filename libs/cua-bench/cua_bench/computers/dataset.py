"""``DatasetSession``: the session of a task that needs no environment.

Static datasets (the grounding sets OSWorld-G and ScreenSpot-Pro) declare
``computer={"provider": "dataset"}``. ``cb run`` then starts no sandbox: the
task's setup shows the item's screenshot with :meth:`DatasetSession.show`,
the agent sees it through :meth:`screenshot` and acts through
:meth:`execute_action` as usual, and every action is recorded in
:attr:`DatasetSession.actions` (nothing is executed). The task's evaluate
reads the recorded actions.

``report_infeasible()`` records that the agent declined the task (grounding
refusal items).
"""

from __future__ import annotations

import io
from dataclasses import asdict, is_dataclass
from typing import Any, Optional


class NoEnvironmentError(NotImplementedError, AttributeError):
    """A live-desktop feature asked of a dataset task (also an AttributeError,
    so ``getattr(session, name, default)`` probes still work)."""


class DatasetSession:
    """A screen that is a picture: records actions, executes none."""

    provider = "dataset"

    def __init__(self, width: Optional[int] = None, height: Optional[int] = None) -> None:
        self.width = width
        self.height = height
        self._png: Optional[bytes] = None
        #: Recorded actions: ``{"type": "click", "x": .., "y": ..}``, ``{"type": "infeasible"}``, ...
        self.actions: list[dict] = []
        self.closed = False

    # ── set by the task ───────────────────────────────────────────────────

    def show(self, png: bytes, width: Optional[int] = None, height: Optional[int] = None) -> None:
        """Make ``png`` the screen. Size defaults to the image's own size."""
        if width is None or height is None:
            try:
                from PIL import Image

                with Image.open(io.BytesIO(png)) as im:
                    width, height = im.size
            except Exception:  # noqa: BLE001 - size stays unknown
                pass
        self._png = bytes(png)
        self.width, self.height = width, height
        self.actions.clear()

    # ── what agents use ───────────────────────────────────────────────────

    async def screenshot(self) -> bytes:
        if self._png is None:
            raise RuntimeError("DatasetSession: the task's setup did not show() a screenshot")
        return self._png

    async def get_screen_size(self) -> dict:
        return {"width": self.width, "height": self.height}

    async def get_dimensions(self) -> tuple:
        return (self.width, self.height)

    async def execute_action(self, action: Any) -> None:
        self.actions.append(_record(action))

    async def click(self, x: int, y: int) -> None:
        self.actions.append({"type": "click", "x": int(x), "y": int(y)})

    async def right_click(self, x: int, y: int) -> None:
        self.actions.append({"type": "right_click", "x": int(x), "y": int(y)})

    async def double_click(self, x: int, y: int) -> None:
        self.actions.append({"type": "double_click", "x": int(x), "y": int(y)})

    async def move_to(self, x: int, y: int) -> None:
        self.actions.append({"type": "move_to", "x": int(x), "y": int(y)})

    async def report_infeasible(self, reason: str = "") -> None:
        """The agent says the task cannot be done on this screen."""
        self.actions.append({"type": "infeasible", "reason": str(reason)})

    # ── helpers for evaluators ────────────────────────────────────────────

    @property
    def points(self) -> list[tuple[int, int]]:
        """The (x, y) of every recorded point action, in order."""
        return [(a["x"], a["y"]) for a in self.actions if "x" in a and "y" in a]

    @property
    def infeasible(self) -> bool:
        return any(a.get("type") == "infeasible" for a in self.actions)

    async def close(self) -> None:
        self.closed = True

    async def __aenter__(self) -> "DatasetSession":
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.close()

    def __getattr__(self, name: str) -> Any:
        # Anything a live desktop offers (files, shell, windows, JavaScript).
        if name.startswith("_"):
            raise AttributeError(name)
        raise NoEnvironmentError(
            f"DatasetSession.{name}: a dataset task has no environment (no sandbox, files, "
            "shell or windows); it only shows a screenshot and records actions"
        )


def _record(action: Any) -> dict:
    kind = type(action).__name__
    if kind.endswith("Action"):
        kind = kind[: -len("Action")]
    name = "".join("_" + c.lower() if c.isupper() else c for c in kind).lstrip("_")
    data = asdict(action) if is_dataclass(action) else dict(getattr(action, "__dict__", {}) or {})
    return {"type": name or "unknown", **data}
