"""Typed MCP driver wrapper for v2 integration tests.

Wraps DriverClient with:
- Typed WindowState return value
- Helper to find a window for a given pid
- Convenient call shortcuts

Usage::

    with Driver(binary_path) as d:
        state = d.get_window_state(pid, wid)
        idx = state.find_element(label="Click Me")
        d.click(pid, wid, element_index=idx)
        state2 = d.get_window_state(pid, wid)
        assert "clicks: 1" in state2.tree
"""

from __future__ import annotations

import os
import sys
import time
from dataclasses import dataclass, field
from typing import Optional

# Allow importing driver_client from the integration directory (parent of harness/).
_HERE = os.path.dirname(os.path.abspath(__file__))
_INTEG_DIR = os.path.dirname(_HERE)
sys.path.insert(0, _INTEG_DIR)

from driver_client import DriverClient, default_binary_path, MCPCallError  # noqa: E402


@dataclass
class WindowState:
    tree: str
    screenshot_b64: str
    screenshot_width: int
    screenshot_height: int
    raw: dict = field(default_factory=dict)

    def find_element(self, label: str) -> Optional[int]:
        """Return the first element index whose line contains `label`."""
        import re
        for line in self.tree.split("\n"):
            if label in line:
                m = re.search(r'\[(\d+)\]', line)
                if m:
                    return int(m.group(1))
        return None

    def find_button(self, label: str) -> Optional[int]:
        """Return the first AXButton element index matching `label`."""
        import re
        for line in self.tree.split("\n"):
            if "AXButton" not in line:
                continue
            if label not in line:
                continue
            m = re.search(r'\[(\d+)\]', line)
            if m:
                return int(m.group(1))
        return None

    def find_text_field(self, skip_url_bar: bool = True) -> Optional[int]:
        """Return the first AXTextField index (skipping browser URL bars)."""
        import re
        _URL_BAR_HINTS = ("smart search field", "address and search bar", "location")
        for line in self.tree.split("\n"):
            if "AXTextField" not in line:
                continue
            if skip_url_bar and any(h in line.lower() for h in _URL_BAR_HINTS):
                continue
            m = re.search(r'\[(\d+)\]', line)
            if m:
                return int(m.group(1))
        return None

    def ax_value(self, element_index: int) -> Optional[str]:
        """Return the value= attribute of the element at `element_index`."""
        import re
        prefix = f"[{element_index}]"
        for line in self.tree.split("\n"):
            if prefix not in line:
                continue
            m = re.search(r'value="([^"]*)"', line)
            if m:
                return m.group(1)
        return None

    def has_text(self, text: str) -> bool:
        return text in self.tree


class Driver:
    """Thin typed wrapper around DriverClient."""

    def __init__(self, binary_path: Optional[str] = None) -> None:
        self.binary_path = binary_path or default_binary_path()
        self._client: Optional[DriverClient] = None

    def __enter__(self) -> "Driver":
        self._client = DriverClient(self.binary_path).__enter__()
        return self

    def __exit__(self, *exc) -> None:
        if self._client is not None:
            self._client.__exit__(*exc)
            self._client = None

    # ── window helpers ────────────────────────────────────────────────────────

    def list_apps(self) -> list[dict]:
        return self._c().call_tool("list_apps")["structuredContent"]["apps"]

    def find_app(self, bundle_id: str) -> Optional[dict]:
        for app in self.list_apps():
            if app.get("bundle_id") == bundle_id:
                return app
        return None

    def find_window(self, pid: int, min_area: float = 400.0, probe_ax: bool = False) -> int:
        """Return a usable window_id for `pid`.

        Prefers on-screen windows on the current space.  Among those, picks
        the one with the LARGEST area so that Electron/Chromium overlay/helper
        sub-windows (which are typically tiny or zero-size) are skipped in
        favour of the main content window.

        When `probe_ax=True`, additionally verifies the chosen window actually
        has AX tree content; if not, falls back to any window that does.  This
        handles browsers like Chrome where the AX tree is exposed on a toolbar
        helper window rather than the main render window.
        """
        result = self._c().call_tool("list_windows", {"pid": pid})
        windows = result["structuredContent"]["windows"]
        if not windows:
            raise RuntimeError(f"pid {pid} has no windows")

        def _area(w: dict) -> float:
            b = w.get("bounds", {})
            return float(b.get("width", 0)) * float(b.get("height", 0))

        # Filter to on-screen windows with meaningful size.
        candidates = [
            w for w in windows
            if w.get("is_on_screen") and w.get("on_current_space") is not False
            and _area(w) >= min_area
        ]
        if not candidates:
            # Fallback: all on-screen windows, then all windows.
            candidates = [w for w in windows if w.get("is_on_screen")] or windows

        # Among candidates, pick the one with the largest area.
        candidates.sort(key=_area, reverse=True)
        primary_wid = candidates[0]["window_id"]

        if not probe_ax:
            return primary_wid

        # Probe each candidate for AX tree content; return the first one that
        # has a non-empty tree.  This is needed for Chrome, whose AX tree is
        # attached to toolbar/helper windows rather than the large content window.
        probe_order = [primary_wid] + [
            w["window_id"] for w in candidates[1:]
        ]
        # Also include off-screen windows as a last resort (Chrome toolbar
        # windows are often flagged off-screen even while Chrome is running).
        for w in sorted(windows, key=_area, reverse=True):
            wid = w["window_id"]
            if wid not in probe_order:
                probe_order.append(wid)

        for wid in probe_order:
            state = self.get_window_state(pid, wid)
            if state.tree.strip():
                return wid

        # Nothing had AX content — return the primary choice.
        return primary_wid

    # ── MCP tool wrappers ─────────────────────────────────────────────────────

    def get_window_state(
        self,
        pid: int,
        window_id: int,
        query: Optional[str] = None,
    ) -> WindowState:
        args: dict = {"pid": pid, "window_id": window_id}
        if query is not None:
            args["query"] = query
        result = self._c().call_tool("get_window_state", args)
        sc = result.get("structuredContent") or {}
        tree = sc.get("tree_markdown", "")
        screenshot = ""
        text_content = ""
        for item in result.get("content", []):
            if item.get("type") == "image":
                screenshot = item.get("data", "")
            elif item.get("type") == "text":
                text_content = item.get("text", "")
        # If structuredContent has no tree, extract it from the text block.
        # Format: "<header line>\n\n<tree>" where the tree starts with "- AX..."
        if not tree and text_content:
            import re
            m = re.search(r'\n\n(- .+)', text_content, re.DOTALL)
            if m:
                tree = m.group(1)
        width = sc.get("screenshot_width", 0)
        height = sc.get("screenshot_height", 0)
        return WindowState(
            tree=tree,
            screenshot_b64=screenshot,
            screenshot_width=width,
            screenshot_height=height,
            raw=result,
        )

    def click(
        self,
        pid: int,
        window_id: int,
        element_index: Optional[int] = None,
        x: Optional[float] = None,
        y: Optional[float] = None,
    ) -> dict:
        args: dict = {"pid": pid, "window_id": window_id}
        if element_index is not None:
            args["element_index"] = element_index
        if x is not None:
            args["x"] = x
        if y is not None:
            args["y"] = y
        return self._c().call_tool("click", args)

    def double_click(
        self,
        pid: int,
        window_id: int,
        element_index: Optional[int] = None,
        x: Optional[float] = None,
        y: Optional[float] = None,
    ) -> dict:
        args: dict = {"pid": pid, "window_id": window_id}
        if element_index is not None:
            args["element_index"] = element_index
        if x is not None:
            args["x"] = x
        if y is not None:
            args["y"] = y
        return self._c().call_tool("double_click", args)

    def type_text(self, pid: int, text: str) -> dict:
        return self._c().call_tool("type_text", {"pid": pid, "text": text})

    def type_text_chars(self, pid: int, text: str) -> dict:
        return self._c().call_tool("type_text_chars", {"pid": pid, "text": text})

    def press_key(self, pid: int, key: str) -> dict:
        return self._c().call_tool("press_key", {"pid": pid, "key": key})

    def hotkey(self, pid: int, keys: list[str]) -> dict:
        return self._c().call_tool("hotkey", {"pid": pid, "keys": keys})

    def scroll(
        self,
        pid: int,
        window_id: int,
        x: float,
        y: float,
        delta_x: float = 0.0,
        delta_y: float = -3.0,
    ) -> dict:
        return self._c().call_tool("scroll", {
            "pid": pid,
            "window_id": window_id,
            "x": x,
            "y": y,
            "delta_x": delta_x,
            "delta_y": delta_y,
        })

    def launch_app(self, bundle_id: str) -> dict:
        return self._c().call_tool("launch_app", {"bundle_id": bundle_id})

    def call_tool(self, name: str, args: Optional[dict] = None) -> dict:
        return self._c().call_tool(name, args or {})

    # ── internals ────────────────────────────────────────────────────────────

    def _c(self) -> DriverClient:
        if self._client is None:
            raise RuntimeError("Driver not started — use as context manager")
        return self._client
