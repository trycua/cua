"""Locator — auto-waiting element selector for cua-driver windows.

Mirrors the Playwright Locator API:  create one from a Window, then call
action methods (click, fill, type, set_value) or query methods
(text_content, get_value, is_visible).

Selector kwargs
---------------
role  : AX role prefix to match, e.g. ``"AXButton"``, ``"AXTextField"``.
label : Matches the parenthesised description ``(label)``, ``id=label``,
        ``help="label``, or a quoted title ``"label"``.
title : Matches only the quoted title ``"title"``.
text  : Loose substring match anywhere in the tree line.
value : Matches ``value="..."``.
"""

from __future__ import annotations

import re
import time
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from ._window import Window


class Locator:
    def __init__(
        self,
        window: "Window",
        role: Optional[str] = None,
        *,
        label: Optional[str] = None,
        title: Optional[str] = None,
        text: Optional[str] = None,
        value: Optional[str] = None,
        _nth: int = 0,
    ) -> None:
        self._window = window
        self._role = role
        self._label = label
        self._title = title
        self._text = text
        self._value = value
        self._nth = _nth

    # ------------------------------------------------------------------
    # Selector chaining
    # ------------------------------------------------------------------

    def nth(self, n: int) -> "Locator":
        """Return a new Locator selecting the *n*-th match (0-based)."""
        return Locator(
            self._window,
            self._role,
            label=self._label,
            title=self._title,
            text=self._text,
            value=self._value,
            _nth=n,
        )

    # ------------------------------------------------------------------
    # Actions
    # ------------------------------------------------------------------

    def click(self, timeout: float = 5.0) -> None:
        """Click the element, waiting up to *timeout* seconds for it to appear."""
        element_index, _ = self._wait_for_element(timeout)
        self._window._client.call_tool("click", {
            "pid": self._window._pid,
            "window_id": self._window._window_id,
            "element_index": element_index,
        })

    def fill(self, text: str, timeout: float = 5.0) -> None:
        """Focus the element and write *text* via the AX ``type_text`` path.

        Preferred for native text fields (NSTextField, NSTextView).
        For web inputs consider :meth:`type` instead.
        """
        element_index, _ = self._wait_for_element(timeout)
        self._window._client.call_tool("click", {
            "pid": self._window._pid,
            "window_id": self._window._window_id,
            "element_index": element_index,
        })
        time.sleep(0.1)
        self._window._client.call_tool("type_text", {
            "pid": self._window._pid,
            "text": text,
        })

    def type(self, text: str, timeout: float = 5.0) -> None:
        """Focus the element and synthesise individual keystrokes via ``type_text_chars``.

        More reliable than :meth:`fill` for web inputs (WebKit AXTextField).
        """
        element_index, _ = self._wait_for_element(timeout)
        self._window._client.call_tool("click", {
            "pid": self._window._pid,
            "window_id": self._window._window_id,
            "element_index": element_index,
        })
        time.sleep(0.1)
        self._window._client.call_tool("type_text_chars", {
            "pid": self._window._pid,
            "text": text,
        })

    def set_value(self, value: str, timeout: float = 5.0) -> None:
        """Set the AX value attribute directly.

        Used for dropdowns (``AXPopUpButton``) and other controls that
        expose ``set_value`` in their available actions.
        """
        element_index, _ = self._wait_for_element(timeout)
        self._window._client.call_tool("set_value", {
            "pid": self._window._pid,
            "window_id": self._window._window_id,
            "element_index": element_index,
            "value": value,
        })

    def double_click(self, timeout: float = 5.0) -> None:
        """Double-click the element."""
        element_index, _ = self._wait_for_element(timeout)
        self._window._client.call_tool("double_click", {
            "pid": self._window._pid,
            "window_id": self._window._window_id,
            "element_index": element_index,
        })

    # ------------------------------------------------------------------
    # Queries
    # ------------------------------------------------------------------

    def text_content(self, timeout: float = 5.0) -> str:
        """Return the visible text of the element (title, label, or value).

        Handles both indexed elements (``[N] AXRole "Title"``) and
        non-indexed elements (``- AXStaticText = "value"``), the latter
        being common for display labels such as the Calculator result.
        """
        query = self._build_query()
        deadline = time.monotonic() + timeout
        while True:
            tree = self._window._get_tree(query=query)

            # --- indexed element ---
            idx = self._find_in_tree(tree)
            if idx is not None:
                for line in tree.split("\n"):
                    m = re.search(r'\[(\d+)\]', line)
                    if m and int(m.group(1)) == idx:
                        title_m = re.search(r'"([^"]*)"', line)
                        if title_m:
                            return title_m.group(1)
                        val_m = re.search(r'value="([^"]*)"', line)
                        if val_m:
                            return val_m.group(1)
                        label_m = re.search(r'\(([^)]+)\)', line)
                        if label_m:
                            return label_m.group(1)
                        return ""

            # --- non-indexed element (e.g. "- AXStaticText = \"value\"") ---
            nth_count = 0
            for line in tree.split("\n"):
                if not line.strip() or re.search(r'\[\d+\]', line):
                    continue
                if self._matches_line(line):
                    eq_m = re.search(r'=\s*"([^"]*)"', line)
                    if eq_m:
                        if nth_count == self._nth:
                            return eq_m.group(1)
                        nth_count += 1

            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"Element text not found within {timeout}s: "
                    f"role={self._role!r} label={self._label!r} "
                    f"title={self._title!r} text={self._text!r}"
                )
            time.sleep(0.3)

    def get_value(self, timeout: float = 5.0) -> str:
        """Return the ``value=`` attribute of the element."""
        element_index, tree = self._wait_for_element(timeout)
        for line in tree.split("\n"):
            m = re.search(r'\[(\d+)\]', line)
            if m and int(m.group(1)) == element_index:
                val_m = re.search(r'value="([^"]*)"', line)
                if val_m:
                    return val_m.group(1)
        return ""

    def is_visible(self, timeout: float = 0.0) -> bool:
        """Return ``True`` if the element is found within *timeout* seconds."""
        try:
            self._wait_for_element(max(timeout, 0.0))
            return True
        except TimeoutError:
            return False

    def all_text_contents(self) -> list[str]:
        """Return text from ALL matching elements in the current AX tree.

        Unlike :meth:`text_content` (which returns only the *nth* match),
        this scans the whole tree and returns every matched element's text.
        Useful when assertions need to check across multiple dynamic elements
        (e.g. the Calculator display adds a history element after pressing =).
        """
        tree = self._window._get_tree(query=self._build_query())
        return self._all_text_from_tree(tree)

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _build_query(self) -> Optional[str]:
        """Build a hint string for ``get_window_state``'s ``query`` param.

        Only the AX role is passed as a query: the cua-driver does a text
        search and multi-word queries (e.g. "AXButton 2") may return no
        results when the label is encoded differently in the tree (e.g. as
        ``(2)`` or ``id=Two``).  Filtering by role alone gives a smaller
        subtree while preserving all matching elements.
        """
        return self._role or None

    def _matches_line(self, line: str) -> bool:
        """Return ``True`` if *line* satisfies every constraint in this Locator."""
        if self._role and self._role not in line:
            return False
        if self._label:
            lb = self._label
            # Match the parenthesised description (most common in AX trees),
            # the id= attribute, the help= attribute, or a quoted title.
            # The loose substring fallback is intentionally omitted — digit
            # labels like "2" would otherwise match "[2]" in any element index.
            if (
                f"({lb})" not in line
                and f"id={lb}" not in line
                and f'help="{lb}' not in line
                and f'"{lb}"' not in line
            ):
                return False
        if self._title and f'"{self._title}"' not in line:
            return False
        if self._text and self._text not in line:
            return False
        if self._value:
            v = self._value
            if f'value="{v}"' not in line and f'value="{v.lower()}"' not in line:
                return False
        return True

    def _find_in_tree(self, tree: str) -> Optional[int]:
        """Return the element_index of the *n*-th matching tree line, or ``None``."""
        matches: list[int] = []
        for line in tree.split("\n"):
            if not line.strip() or "[" not in line:
                continue
            m = re.search(r'\[(\d+)\]', line)
            if m and self._matches_line(line):
                matches.append(int(m.group(1)))

        if self._nth >= len(matches):
            return None
        return matches[self._nth]

    def _all_text_from_tree(self, tree: str) -> list[str]:
        """Collect text from every matching line (indexed and non-indexed)."""
        texts: list[str] = []
        # Indexed elements
        for line in tree.split("\n"):
            if not line.strip() or "[" not in line:
                continue
            m = re.search(r'\[(\d+)\]', line)
            if m and self._matches_line(line):
                title_m = re.search(r'"([^"]*)"', line)
                if title_m:
                    texts.append(title_m.group(1))
                else:
                    val_m = re.search(r'value="([^"]*)"', line)
                    if val_m:
                        texts.append(val_m.group(1))
        # Non-indexed elements (e.g. "- AXStaticText = \"value\"")
        for line in tree.split("\n"):
            if not line.strip() or re.search(r'\[\d+\]', line):
                continue
            if self._matches_line(line):
                eq_m = re.search(r'=\s*"([^"]*)"', line)
                if eq_m:
                    texts.append(eq_m.group(1))
        return texts

    def _wait_for_element(
        self,
        timeout: float = 5.0,
        poll_interval: float = 0.3,
    ) -> tuple[int, str]:
        """Poll ``get_window_state`` until element is found.

        Returns ``(element_index, tree_markdown)`` on success.
        Raises ``TimeoutError`` when *timeout* elapses.
        """
        query = self._build_query()
        deadline = time.monotonic() + timeout

        while True:
            tree = self._window._get_tree(query=query)
            idx = self._find_in_tree(tree)
            if idx is not None:
                return idx, tree

            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"Element not found within {timeout}s: "
                    f"role={self._role!r} label={self._label!r} "
                    f"title={self._title!r} text={self._text!r}"
                )
            time.sleep(poll_interval)
