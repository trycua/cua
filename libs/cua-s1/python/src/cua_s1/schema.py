"""Typed inputs and pointer-action encoding for form-filling decisions."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Literal

Action = Literal["fill", "check", "click", "skip"]
FIXED_ACTIONS: tuple[Action, ...] = ("check", "click", "skip")


@dataclass
class Entity:
    """A labeled source-document value that the model may point to."""

    label: str
    value: str

    def option(self) -> str:
        """Render this entity as one scorer option."""
        return f"fill {self.label}: {self.value}"


@dataclass
class Element:
    """A normalized UI element observation.

    ``element_token`` is the stable runtime action target. ``index`` is retained
    only as observation metadata and must not be used to target an action.
    """

    role: str
    label: str
    value: str = ""
    index: int = -1
    actions: tuple[str, ...] = ()
    checked: bool | None = None
    frame: tuple[int, int, int, int] | None = None
    element_token: str | None = None


@dataclass
class Decision:
    """A decoded action, including its confidence and option distribution."""

    element: Element
    action: Action
    entity_index: int | None
    probability: float
    distribution: list[float] = field(default_factory=list)


def render_context(form_title: str, element: Element, placeholder: str = "") -> str:
    """Render the compact byte-level context used to score one element."""
    if element.role == "CheckBox":
        state = "checked" if element.checked else "unchecked"
    else:
        state = f'value="{element.value[:48]}"'
    hint = f' hint="{placeholder[:72]}"' if placeholder else ""
    return (
        "TASK fill the form from the document, then submit\n"
        f"FORM {form_title[:64]}\n"
        f'ELEMENT {element.role} "{element.label[:72]}" {state}{hint}'
    )


# Execution order for a single-pass plan: fills, checkboxes, then clicks.
ACTION_ORDER: dict[Action, int] = {"fill": 0, "check": 1, "click": 2, "skip": 3}


def render_options(entities: list[Entity]) -> list[str]:
    """Render entity pointer options followed by the fixed actions."""
    return [entity.option() for entity in entities] + list(FIXED_ACTIONS)


def decode(option_index: int, entities: list[Entity]) -> tuple[Action, int | None]:
    """Decode a scorer option index into an action and optional entity pointer."""
    option_count = len(entities) + len(FIXED_ACTIONS)
    if isinstance(option_index, bool) or not 0 <= option_index < option_count:
        raise ValueError(
            f"option index {option_index!r} is outside the valid range 0..{option_count - 1}"
        )
    if option_index < len(entities):
        return "fill", option_index
    return FIXED_ACTIONS[option_index - len(entities)], None


def encode(action: Action, entity_index: int | None, entities: list[Entity]) -> int:
    """Encode an action and optional entity pointer as a scorer option index."""
    if action == "fill":
        if (
            entity_index is None
            or isinstance(entity_index, bool)
            or not 0 <= entity_index < len(entities)
        ):
            raise ValueError("fill actions require a valid entity index")
        return entity_index
    if entity_index is not None:
        raise ValueError(f"{action!r} actions must not include an entity index")
    try:
        return len(entities) + FIXED_ACTIONS.index(action)
    except ValueError as exc:
        raise ValueError(f"unsupported action: {action!r}") from exc
