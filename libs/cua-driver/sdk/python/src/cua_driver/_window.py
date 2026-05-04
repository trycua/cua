"""Window — represents a single macOS window in a running app."""

from __future__ import annotations

import base64
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from ._client import DriverClient

from ._locator import Locator


class Window:
    """A handle to a specific window (pid + window_id pair).

    Obtain via :meth:`App.main_window` or :meth:`App.get_window`; do not
    construct directly unless you already know the window_id from
    ``list_windows``.
    """

    def __init__(
        self,
        client: "DriverClient",
        pid: int,
        window_id: int,
    ) -> None:
        self._client = client
        self._pid = pid
        self._window_id = window_id

    # ------------------------------------------------------------------
    # Locator factory
    # ------------------------------------------------------------------

    def locator(
        self,
        role: Optional[str] = None,
        *,
        label: Optional[str] = None,
        title: Optional[str] = None,
        text: Optional[str] = None,
        value: Optional[str] = None,
    ) -> Locator:
        """Return a :class:`Locator` for elements in this window.

        Parameters
        ----------
        role:
            AX role to match, e.g. ``"AXButton"``, ``"AXTextField"``.
        label:
            Matches the parenthesised description ``(label)``, ``id=label``,
            ``help="label``, or a quoted title ``"label"``.
        title:
            Matches the quoted title ``"title"`` precisely.
        text:
            Loose substring match anywhere in the AX tree line.
        value:
            Matches the ``value="..."`` attribute.

        Examples
        --------
        >>> window.locator("AXButton", label="OK").click()
        >>> window.locator("AXTextField").nth(1).fill("hello")
        >>> window.locator(text="Submit").click()
        """
        return Locator(
            self,
            role=role,
            label=label,
            title=title,
            text=text,
            value=value,
        )

    # ------------------------------------------------------------------
    # Raw access
    # ------------------------------------------------------------------

    def screenshot(self) -> bytes:
        """Capture the window and return raw PNG bytes."""
        result = self._client.call_tool("screenshot", {
            "pid": self._pid,
            "window_id": self._window_id,
        })
        content = result.get("content", [])
        for item in content:
            if item.get("type") == "image":
                return base64.b64decode(item["data"])
        raise RuntimeError("No image content in screenshot response")

    def get_tree(self, query: Optional[str] = None) -> str:
        """Return the raw AX tree markdown for this window.

        Optionally filter to lines matching *query* (passed to
        ``get_window_state``'s ``query`` parameter).
        """
        return self._get_tree(query=query)

    # ------------------------------------------------------------------
    # Properties
    # ------------------------------------------------------------------

    @property
    def title(self) -> str:
        """Window title from ``list_windows``."""
        result = self._client.call_tool("list_windows", {"pid": self._pid})
        windows = result.get("structuredContent", {}).get("windows", [])
        for w in windows:
            if w.get("window_id") == self._window_id:
                return w.get("title", "")
        return ""

    @property
    def pid(self) -> int:
        return self._pid

    @property
    def window_id(self) -> int:
        return self._window_id

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _get_tree(self, query: Optional[str] = None) -> str:
        """Fetch AX tree markdown (used internally by Locator).

        ``get_window_state`` returns the AX tree in the text content block,
        not in structuredContent.  We scan the content array for the first
        text item.
        """
        params: dict = {"pid": self._pid, "window_id": self._window_id}
        if query:
            params["query"] = query
        result = self._client.call_tool("get_window_state", params)
        for item in result.get("content", []):
            if item.get("type") == "text":
                return item.get("text", "")
        return ""
