from __future__ import annotations

from typing import Any

from .types import ClickAction, RightClickAction


class Bot:
    """Helper class for writing trajectories for task solutions."""

    def __init__(self, env: Any):
        self.env = env

    def click_element(self, pid: int, selector: str) -> None:
        """Find element by CSS selector and click its center.

        Uses provider's bench-ui bridge to fetch element rect in screen space
        and then dispatches a ClickAction via env.step().
        """
        rect = self.env.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        self.env.step(ClickAction(x=cx, y=cy))

    def right_click_element(self, pid: int, selector: str) -> None:
        rect = self.env.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        self.env.step(RightClickAction(x=cx, y=cy))
