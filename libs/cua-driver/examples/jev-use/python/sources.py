"""Candidate sources: where a decision's executable candidates come from.

A candidate source turns one Driver observation into addressable controls and
builds fully specified candidates for them. The model only ever sees a
candidate's ID and description; the tool name and exact arguments stay in the
runner. A task (see ``tasks.py``) decides which controls matter and which
candidate IDs and descriptions to offer; the source decides how to address a
control and which Driver tool acts on it.

Implemented sources:

- ``BrowserSemanticSource`` reads a ``get_browser_state`` ``semantic_v2``
  snapshot and acts through ``browser_type`` / ``browser_click`` on its refs.
- ``VisualRegionSource`` reads a validated ``parse_visual_regions`` result and
  acts through a capture-bound ``click`` on a region's center.
- ``NativeAccessibilitySource`` (RFC #4268) reads the eligible native elements
  of one ``get_window_state`` observation (see ``native.py``), finds controls
  by role class and label, and acts through that snapshot's ``element_token``.
"""

from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
from typing import TYPE_CHECKING, Any, ClassVar, Literal, Mapping, Protocol

from native import NativeControl, NativeControls, NativeObservation, eligible_controls
from native_roles import Platform

if TYPE_CHECKING:
    from core import VisualDelivery, VisualObservation, VisualRegion

SourceKind = Literal["page", "visual", "ax"]
TextMethod = Literal["set_value", "type_text"]


def _freeze(value: Any) -> Any:
    if isinstance(value, dict):
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(_freeze(item) for item in value)
    return value


def _ascii_lower(value: str) -> str:
    return "".join(chr(ord(char) + 32) if "A" <= char <= "Z" else char for char in value)


@dataclass(frozen=True)
class Candidate:
    id: str
    description: str
    tool: str | None
    arguments: Mapping[str, Any]
    capture_id: str | None = None
    screenshot_reference: str | None = None
    # RFC #4268: the source that built the candidate (None for reserved
    # candidates), the native snapshot it is bound to, and its risk categories.
    source: SourceKind | None = None
    snapshot_id: str | None = None
    risk: frozenset[str] = frozenset()

    def __post_init__(self) -> None:
        object.__setattr__(self, "arguments", _freeze(dict(self.arguments)))
        object.__setattr__(self, "risk", frozenset(self.risk))


@dataclass(frozen=True)
class Control:
    """One addressable element a source found in its observation.

    ``value`` is the element's current value when the source can read it (never
    sent to a model; tasks summarize it). ``handle`` is source-private: a page
    ref for the browser source, a ``VisualRegion`` for the visual source.
    """

    source: SourceKind
    role: str
    name: str
    value: Any
    handle: Any


class CandidateSource(Protocol):
    """The interface every candidate source implements.

    ``find`` returns the unique control matching a role and accessible name, or
    ``None``. ``click`` and ``type_text`` return a fully specified candidate for
    that control, or ``None`` when this source cannot perform the action in the
    current observation (for example a visual region cannot receive text, and a
    visual click needs a capture-bound Driver ``click``).
    """

    kind: ClassVar[SourceKind]

    def find(self, role: str, name: str) -> Control | None: ...

    def click(self, control: Control, *, candidate_id: str, description: str) -> Candidate | None: ...

    def type_text(
        self, control: Control, text: str, *, candidate_id: str, description: str
    ) -> Candidate | None: ...


@dataclass(frozen=True)
class BrowserSemanticSource:
    """Controls from a ``get_browser_state`` snapshot, acted on through its refs."""

    snapshot: Mapping[str, Any]
    kind: ClassVar[SourceKind] = "page"

    def require_target(self) -> dict[str, Any]:
        """Return the browser target every page candidate addresses.

        Raises ``KeyError`` when the snapshot has no target or tab.
        """
        return {"target_id": self.snapshot["target_id"], "tab_id": self.snapshot["tab_id"]}

    def find(self, role: str, name: str) -> Control | None:
        refs = self.snapshot.get("refs") or []
        ref = next(
            (item for item in refs if item.get("role") == role and item.get("name") == name),
            None,
        )
        if ref is None:
            return None
        return Control("page", role, name, ref.get("value"), ref)

    def click(self, control: Control, *, candidate_id: str, description: str) -> Candidate:
        return Candidate(
            candidate_id,
            description,
            "browser_click",
            {**self.require_target(), "ref": control.handle["ref"], "input_route": "dom_event"},
            source="page",
        )

    def type_text(
        self, control: Control, text: str, *, candidate_id: str, description: str
    ) -> Candidate:
        return Candidate(
            candidate_id,
            description,
            "browser_type",
            {**self.require_target(), "ref": control.handle["ref"], "text": text, "replace": True},
            source="page",
        )


@dataclass(frozen=True)
class VisualRegionSource:
    """Controls from validated OmniParser regions, clicked through their capture.

    Regions carry no role, so ``find`` matches the visible text or icon label
    (ASCII case-insensitive) of regions at or above ``min_confidence`` (default
    ``MIN_CONFIDENCE``; a task may opt into a lower bar) and
    returns a control only when exactly one region matches. ``click`` is offered
    only when Driver advertises a capture-bound ``click`` (``capture_bound``); it
    targets the region's screenshot-pixel center with the exact ``capture_id``
    and the source's ``delivery`` mode.
    """

    observation: VisualObservation
    delivery: VisualDelivery = "background"
    capture_bound: bool = False
    min_confidence: float = 0.8
    kind: ClassVar[SourceKind] = "visual"
    MIN_CONFIDENCE: ClassVar[float] = 0.8

    def find(self, role: str, name: str) -> Control | None:
        wanted = _ascii_lower(name)
        matches = [
            region
            for region in self.observation.regions
            if region.confidence >= self.min_confidence
            and _ascii_lower(region.text or region.label or "") == wanted
        ]
        if len(matches) != 1:
            return None
        region: VisualRegion = matches[0]
        return Control("visual", role, region.text or region.label or "", None, region)

    def click(self, control: Control, *, candidate_id: str, description: str) -> Candidate | None:
        if not self.capture_bound:
            return None
        visual = self.observation
        x, y = visual.screenshot_center(control.handle)
        return Candidate(
            candidate_id,
            description,
            "click",
            {
                "pid": visual.pid,
                "window_id": visual.window_id,
                "x": x,
                "y": y,
                "capture_id": visual.capture_id,
                "delivery_mode": self.delivery,
            },
            capture_id=visual.capture_id,
            screenshot_reference=visual.screenshot_reference,
            source="visual",
        )

    def type_text(
        self, control: Control, text: str, *, candidate_id: str, description: str
    ) -> None:
        return None


@dataclass(frozen=True)
class NativeAccessibilitySource:
    """Controls from one ``get_window_state`` observation, acted on by token.

    ``controls`` holds the eligible elements in ``element_index`` order with
    stable IDs (see ``native.py``). ``click`` binds the control's
    ``element_token`` with ``delivery_mode`` (background unless Driver refused
    background delivery for that control and the task allows foreground).
    ``type_text`` sets text through ``set_value`` (default) or an element-bound
    ``type_text``, with the text supplied by the task, never by the model.
    """

    observation: NativeObservation
    platform: Platform
    native: NativeControls
    text_method: TextMethod = "set_value"
    kind: ClassVar[SourceKind] = "ax"

    @classmethod
    def from_observation(
        cls,
        observation: NativeObservation,
        platform: Platform,
        *,
        redact: Any = None,
        text_method: TextMethod = "set_value",
    ) -> "NativeAccessibilitySource":
        native = (
            eligible_controls(observation, platform)
            if redact is None
            else eligible_controls(observation, platform, redact=redact)
        )
        return cls(observation, platform, native, text_method)

    @property
    def controls(self) -> tuple[NativeControl, ...]:
        return self.native.controls

    def control(self, native: NativeControl) -> Control:
        return Control("ax", native.role_class, native.label, native.value, native)

    def find(self, role: str, name: str) -> Control | None:
        matches = [
            item for item in self.controls if item.role_class == role and item.label == name
        ]
        return self.control(matches[0]) if len(matches) == 1 else None

    def _target(self) -> dict[str, Any]:
        return {"pid": self.observation.pid, "window_id": self.observation.window_id}

    def click(
        self,
        control: Control,
        *,
        candidate_id: str,
        description: str,
        delivery: VisualDelivery = "background",
    ) -> Candidate:
        native: NativeControl = control.handle
        return Candidate(
            candidate_id,
            description,
            "click",
            {**self._target(), "element_token": native.element_token, "delivery_mode": delivery},
            source="ax",
            snapshot_id=self.observation.snapshot_id,
            risk=native.risk,
        )

    def type_text(
        self, control: Control, text: str, *, candidate_id: str, description: str
    ) -> Candidate:
        native: NativeControl = control.handle
        if self.text_method == "type_text":
            tool, arguments = "type_text", {"text": text}
        else:
            tool, arguments = "set_value", {"value": text}
        return Candidate(
            candidate_id,
            description,
            tool,
            {**self._target(), "element_token": native.element_token, **arguments},
            source="ax",
            snapshot_id=self.observation.snapshot_id,
            risk=native.risk,
        )
