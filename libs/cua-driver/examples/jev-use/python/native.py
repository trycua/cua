"""Native accessibility observations for jev-use (RFC #4268, Phase 1).

This module turns one ``get_window_state`` result into the eligible native
controls that ``NativeAccessibilitySource`` (``sources.py``) offers as
candidates. It owns the element rules, stable candidate IDs, and the
risky-action policy. It never builds Driver arguments; the source does.

One observation is exactly one ``get_window_state`` call with both the tree
and the screenshot, so its ``snapshot_id`` (element tokens) and ``capture_id``
(visual regions and capture-bound clicks) describe the same moment. A second
call would publish a new snapshot and invalidate every token in the set.

Element rules (all must hold):

1. the raw role maps to a role class (``native_roles.py``), and no actionable
   ancestor is window chrome (the Windows title bar);
2. ``enabled`` is not ``false``;
3. ``frame`` exists, has positive size, and intersects ``window_bounds``;
4. the redacted label is non-empty, is not a copy of the element's own value,
   and the element is not marked ``unlabelled``;
5. ``in_web_content`` is not ``true`` (web content uses the browser source).

An observation *has no application elements* when every element it reports is
a window root (``window``, ``application``, ``frame``) or window chrome (the
Windows title bar and its descendants). On macOS, the application's global menu
bar and its descendants, and unlabeled, valueless direct children of the window
root that are buttons (the standard window buttons), are not window content either. Driver
reports such a tree for a custom-painted surface: X11 property metadata only on
Linux, only the title bar on Windows, and only the window buttons and menu bar
on macOS. That view has no accessibility tree to act through, so, like an empty
tree, it may fall back to visual regions. This check never changes which
elements become candidates.

Stable IDs never use ``element_index``: ``ax:<role_class>:<slug(label)>``,
plus a short hash of the actionable-ancestor path and ordinal when the base
repeats in one observation.
"""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from typing import Any, Callable, Literal, Mapping

from native_roles import (
    ROLE_CLASS_ACTION,
    ActionKind,
    Platform,
    RoleClass,
    is_window_chrome,
    normalized_role,
    role_class,
)

ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,63}\Z")
PARAMETER_NAME_PATTERN = re.compile(r"[a-z][a-z0-9_]{0,7}\Z")
MAX_LABEL_CHARS = 120
SLUG_CHARS = 32

RiskCategory = Literal["destructive", "send", "purchase", "close_unsaved"]
RISK_CATEGORIES: tuple[RiskCategory, ...] = ("destructive", "send", "purchase", "close_unsaved")

# Whole-word, case-insensitive label phrases. This guards against accidents;
# it is not an authorization boundary. Localized lists extend these tuples.
# ``close_unsaved`` is matched whether or not the window reports unsaved
# state, because Driver does not expose that state; it is the safe default.
RISK_PHRASES: Mapping[RiskCategory, tuple[str, ...]] = {
    "destructive": (
        "delete", "remove", "erase", "trash", "discard", "clear all", "format", "reset",
    ),
    "send": ("send", "submit", "post", "publish", "share", "reply"),
    "purchase": ("buy", "purchase", "pay", "checkout", "order", "subscribe"),
    "close_unsaved": ("close", "quit", "exit", "don't save", "discard changes"),
}


def _phrase_pattern(phrase: str) -> re.Pattern[str]:
    return re.compile(r"(?<![a-z0-9])" + re.escape(phrase) + r"(?![a-z0-9])")


_RISK_PATTERNS = {
    category: tuple(_phrase_pattern(phrase) for phrase in phrases)
    for category, phrases in RISK_PHRASES.items()
}


def risk_categories(label: str) -> frozenset[str]:
    """Return the risk categories whose phrases occur in ``label``."""
    text = label.lower().replace("’", "'")
    return frozenset(
        category
        for category, patterns in _RISK_PATTERNS.items()
        if any(pattern.search(text) for pattern in patterns)
    )


def slug(label: str) -> str:
    """Lowercase ASCII, collapse other runs to ``-``, and trim to 32 characters.

    A label without ASCII alphanumerics becomes ``hex8(sha256(label))``.
    """
    lowered = "".join(
        char.lower() if char.isascii() and char.isalnum() else "-" for char in label
    )
    value = re.sub(r"-+", "-", lowered).strip("-")[:SLUG_CHARS].strip("-")
    return value or hashlib.sha256(label.encode("utf-8")).hexdigest()[:8]


def _hex4(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()[:4]


def _string(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return float(value)


def _rect(value: Any) -> tuple[float, float, float, float] | None:
    """Return ``(x, y, w, h)`` from a Driver frame or window bounds."""
    if not isinstance(value, Mapping):
        return None
    x = _number(value.get("x"))
    y = _number(value.get("y"))
    w = _number(value.get("w", value.get("width")))
    h = _number(value.get("h", value.get("height")))
    if None in (x, y, w, h):
        return None
    return (x, y, w, h)  # type: ignore[return-value]


def _intersects(a: tuple[float, float, float, float], b: tuple[float, float, float, float]) -> bool:
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    return min(ax + aw, bx + bw) > max(ax, bx) and min(ay + ah, by + bh) > max(ay, by)


class NativeObservationError(ValueError):
    def __init__(self, message: str, code: str = "invalid_window_state") -> None:
        super().__init__(message)
        self.code = code


@dataclass(frozen=True)
class NativeObservation:
    """A validated projection of one ``get_window_state`` result.

    ``complete`` is Driver's ``elements_complete`` claim. ``truncated`` means
    the walk ran out of budget (``truncated`` or a ``truncation_reason``).
    ``tree_empty`` means Driver reported the tree empty (``degraded_reason``
    ``ax_tree_empty``). A view without native candidates may fall back to
    visual regions only then, for a complete tree, or when the tree has no
    application elements (see ``has_application_elements``).
    """

    pid: int
    window_id: int
    snapshot_id: str | None
    capture_id: str | None
    window_bounds: tuple[float, float, float, float] | None
    elements: tuple[Mapping[str, Any], ...]
    complete: bool
    truncated: bool
    degraded_reason: str | None
    tree_markdown: str

    @property
    def partial(self) -> bool:
        return not self.complete

    @property
    def tree_empty(self) -> bool:
        return bool(self.degraded_reason and self.degraded_reason.startswith("ax_tree_empty"))

    @classmethod
    def from_window_state(
        cls, payload: Mapping[str, Any], *, expected_pid: int, expected_window_id: int
    ) -> "NativeObservation":
        if payload.get("pid") != expected_pid or payload.get("window_id") != expected_window_id:
            raise NativeObservationError(
                "window state belongs to a different window", code="window_mismatch"
            )
        raw_elements = payload.get("elements", [])
        if not isinstance(raw_elements, list) or not all(
            isinstance(item, Mapping) for item in raw_elements
        ):
            raise NativeObservationError("window state has malformed elements")
        elements = tuple(
            sorted(
                raw_elements,
                key=lambda item: item.get("element_index")
                if isinstance(item.get("element_index"), int)
                else -1,
            )
        )
        degraded = _string(payload.get("degraded_reason")) if payload.get("degraded") else None
        return cls(
            pid=expected_pid,
            window_id=expected_window_id,
            snapshot_id=_string(payload.get("snapshot_id")),
            capture_id=_string(payload.get("capture_id")),
            window_bounds=_rect(payload.get("window_bounds")),
            elements=elements,
            complete=payload.get("elements_complete") is True,
            truncated=payload.get("truncated") is True or bool(payload.get("truncation_reason")),
            degraded_reason=degraded,
            tree_markdown=_string(payload.get("tree_markdown")) or "",
        )


WINDOW_ROOT_ROLES = frozenset({"window", "application", "frame"})
MACOS_MENU_BAR_ROLE = "menubar"


def _index_elements(observation: NativeObservation) -> dict[int, Mapping[str, Any]]:
    return {
        item["element_index"]: item
        for item in observation.elements
        if isinstance(item.get("element_index"), int)
    }


def _in_window_chrome(
    element: Mapping[str, Any], by_index: Mapping[int, Mapping[str, Any]], platform: Platform
) -> bool:
    """Whether an ancestor of ``element`` is a window-chrome container."""
    seen: set[int] = set()
    parent = element.get("parent_index")
    while isinstance(parent, int) and parent in by_index and parent not in seen:
        seen.add(parent)
        if is_window_chrome(by_index[parent].get("role"), platform):
            return True
        parent = by_index[parent].get("parent_index")
    return False


def _normalized(element: Mapping[str, Any]) -> str:
    role = element.get("role")
    return normalized_role(role) if isinstance(role, str) else ""


def _in_macos_menu_bar(element: Mapping[str, Any], by_index: Mapping[int, Mapping[str, Any]]) -> bool:
    seen: set[int] = set()
    current: Mapping[str, Any] | None = element
    while current is not None:
        if _normalized(current) == MACOS_MENU_BAR_ROLE:
            return True
        parent = current.get("parent_index")
        if not isinstance(parent, int) or parent in seen:
            return False
        seen.add(parent)
        current = by_index.get(parent)
    return False


def _is_macos_window_button(element: Mapping[str, Any], by_index: Mapping[int, Mapping[str, Any]]) -> bool:
    """An unlabeled, valueless button directly under the window root (close, minimize, zoom)."""
    parent = by_index.get(element.get("parent_index")) if isinstance(element.get("parent_index"), int) else None
    return (
        _normalized(element) == "button"
        and parent is not None
        and _normalized(parent) in WINDOW_ROOT_ROLES
        and not _string(element.get("label"))
        and not _string(element.get("value"))
    )


def has_application_elements(observation: NativeObservation, platform: Platform) -> bool:
    """Whether any element is window content (see the module docstring)."""
    by_index = _index_elements(observation)
    for element in observation.elements:
        if _normalized(element) in WINDOW_ROOT_ROLES:
            continue
        if is_window_chrome(element.get("role"), platform) or _in_window_chrome(element, by_index, platform):
            continue
        if platform == "macos" and (
            _in_macos_menu_bar(element, by_index) or _is_macos_window_button(element, by_index)
        ):
            continue
        return True
    return False


@dataclass(frozen=True)
class NativeControl:
    """One eligible native element in one observation.

    ``value`` is runner-only (never sent to a model). ``element_token`` is bound
    to the observation's snapshot and is never sent to a model either.
    """

    id: str
    role_class: RoleClass
    action: ActionKind
    label: str
    value: str | None
    selected: bool | None
    element_index: int
    element_token: str
    risk: frozenset[str]


ExclusionReason = Literal[
    "unknown_role", "window_chrome", "disabled", "off_screen", "unlabeled", "web_content", "no_token"
]


@dataclass(frozen=True)
class NativeControls:
    controls: tuple[NativeControl, ...]
    excluded: Mapping[str, int]


def eligible_controls(
    observation: NativeObservation,
    platform: Platform,
    *,
    redact: Callable[[str], str] = lambda value: value,
) -> NativeControls:
    """Apply the element rules and assign stable IDs, in ``element_index`` order."""
    by_index = _index_elements(observation)
    excluded: dict[str, int] = {}

    def exclude(reason: ExclusionReason) -> None:
        excluded[reason] = excluded.get(reason, 0) + 1

    def label_of(element: Mapping[str, Any]) -> str:
        label = _string(element.get("label")) or ""
        return " ".join(redact(label).split())

    def path_of(element: Mapping[str, Any]) -> tuple[tuple[str, str], ...]:
        path: list[tuple[str, str]] = []
        seen: set[int] = set()
        parent = element.get("parent_index")
        while isinstance(parent, int) and parent in by_index and parent not in seen:
            seen.add(parent)
            ancestor = by_index[parent]
            raw_role = _string(ancestor.get("role")) or ""
            klass = role_class(raw_role, platform) or normalized_role(raw_role)
            path.append((klass, label_of(ancestor)))
            parent = ancestor.get("parent_index")
        return tuple(reversed(path))

    pending: list[tuple[Mapping[str, Any], RoleClass, str, tuple[tuple[str, str], ...]]] = []
    for element in observation.elements:
        klass = role_class(element.get("role"), platform)
        if klass is None:
            exclude("unknown_role")
            continue
        if _in_window_chrome(element, by_index, platform):
            exclude("window_chrome")
            continue
        if element.get("enabled") is False:
            exclude("disabled")
            continue
        frame = _rect(element.get("frame"))
        if (
            frame is None
            or frame[2] <= 0
            or frame[3] <= 0
            or observation.window_bounds is None
            or not _intersects(frame, observation.window_bounds)
        ):
            exclude("off_screen")
            continue
        label = label_of(element)
        value = _string(element.get("value"))
        redacted_value = " ".join(redact(value).split()) if value is not None else None
        if element.get("unlabelled") is True or not label or label == redacted_value:
            # macOS and Windows labels can fall back to the value (or an empty
            # field's placeholder); typed content never becomes an ID.
            exclude("unlabeled")
            continue
        if element.get("in_web_content") is True:
            exclude("web_content")
            continue
        if not isinstance(element.get("element_token"), str) or not element["element_token"]:
            exclude("no_token")
            continue
        pending.append((element, klass, label, path_of(element)))

    bases = [f"ax:{klass}:{slug(label)}" for _, klass, label, _ in pending]
    counts: dict[str, int] = {}
    for base in bases:
        counts[base] = counts.get(base, 0) + 1
    ordinals: dict[tuple[Any, ...], int] = {}
    controls: list[NativeControl] = []
    for (element, klass, label, path), base in zip(pending, bases):
        key = (klass, label, path)
        ordinal = ordinals.get(key, 0)
        ordinals[key] = ordinal + 1
        candidate_id = base if counts[base] == 1 else f"{base}:{_hex4([list(map(list, path)), ordinal])}"
        if not ID_PATTERN.fullmatch(candidate_id):  # pragma: no cover - slug bounds this
            raise NativeObservationError("derived candidate ID is invalid")
        selected = element.get("selected")
        controls.append(
            NativeControl(
                id=candidate_id,
                role_class=klass,
                action=ROLE_CLASS_ACTION[klass],
                label=label[:MAX_LABEL_CHARS],
                value=_string(element.get("value")),
                selected=selected if isinstance(selected, bool) else None,
                element_index=element["element_index"],
                element_token=element["element_token"],
                risk=risk_categories(label),
            )
        )
    return NativeControls(tuple(controls), excluded)


def element_state(control: NativeControl) -> str:
    """The compact, value-free state sent to the model for one control."""
    if control.role_class in {"checkbox", "toggle"}:
        return "checked" if control.selected else "unchecked"
    if control.role_class == "radio":
        return "selected" if control.selected else "not_selected"
    if control.role_class == "text_input":
        return "has_text" if control.value else "empty"
    return "enabled"


def field_state(value: str | None, required: str) -> str:
    """The existing field vocabulary; never includes the value itself."""
    if not value:
        return "empty"
    if value == required:
        return "contains_required_value"
    return "contains_other_value"
