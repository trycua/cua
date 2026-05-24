"""AX tree_markdown parser and query helpers.

tree_markdown is the line-based format emitted by get_window_state, e.g.::

    - [0] AXWindow "Safari" [id=main actions=[raise]]
      - [1] AXButton "Click Me" [actions=[press]]
      - [2] AXTextField [value="hello"] [actions=[set_value]]
      - [3] AXStaticText = "clicks: 3"

Helpers:
  find(tree, role=None, label=None)  → first matching element index
  find_all(tree, role=None, label=None) → all matching element indices
  ax_value(tree, index) → value= attribute or static text value
  ax_text(tree, index) → the label / title / description text on that line
"""

from __future__ import annotations

import re
from typing import Optional


def _lines(tree: str) -> list[tuple[int, str]]:
    """Return (element_index, raw_line) pairs for every indexed line."""
    result: list[tuple[int, str]] = []
    for line in tree.split("\n"):
        m = re.search(r'\[(\d+)\]', line)
        if m:
            result.append((int(m.group(1)), line))
    return result


def find(
    tree: str,
    role: Optional[str] = None,
    label: Optional[str] = None,
    skip_url_bar: bool = True,
) -> Optional[int]:
    """Return the element_index of the first matching element."""
    for idx, line in _lines(tree):
        if role is not None and role not in line:
            continue
        if label is not None and label not in line:
            continue
        if skip_url_bar and "smart search field" in line:
            continue
        return idx
    return None


def find_all(
    tree: str,
    role: Optional[str] = None,
    label: Optional[str] = None,
) -> list[int]:
    """Return all matching element indices."""
    result: list[int] = []
    for idx, line in _lines(tree):
        if role is not None and role not in line:
            continue
        if label is not None and label not in line:
            continue
        result.append(idx)
    return result


def ax_value(tree: str, index: int) -> Optional[str]:
    """Return value= attribute or static text (= "...") for `index`."""
    prefix = f"[{index}]"
    for line in tree.split("\n"):
        if prefix not in line:
            continue
        # value="..." attribute
        m = re.search(r'value="([^"]*)"', line)
        if m:
            return m.group(1)
        # AXStaticText = "..."
        m = re.search(r'= "([^"]*)"', line)
        if m:
            return m.group(1)
    return None


def ax_text(tree: str, index: int) -> Optional[str]:
    """Return the display text (title or quoted string) for `index`."""
    prefix = f"[{index}]"
    for line in tree.split("\n"):
        if prefix not in line:
            continue
        # Quoted string after role: AXButton "label"
        m = re.search(r'"([^"]+)"', line)
        if m:
            return m.group(1)
        return line.strip()
    return None


def contains_text(tree: str, text: str) -> bool:
    return text in tree
