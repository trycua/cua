"""Model-agnostic planning and explicitly authorized form execution."""

from __future__ import annotations

import math
import re
import time
from collections.abc import Callable, Sequence
from dataclasses import asdict, is_dataclass
from typing import Any, Protocol

from .driver import BaseDriver, DeliveryMode, DriverError, WindowSnapshot, WindowTarget
from .schema import ACTION_ORDER, Decision, Element, Entity

ACTIONABLE_ROLES = {
    "button",
    "checkbox",
    "combobox",
    "edit",
    "textfield",
    "axbutton",
    "axcheckbox",
    "axcombobox",
    "axtextfield",
}
ALLOWED_ACTIONS = {"fill", "check", "click", "skip"}
SUBMIT_CONTROL_ROLES = {"button", "axbutton"}
SUBMIT_CONTROL_LABELS = {"submit", "submit form"}
APP_SUFFIXES = (
    " - Google Chrome",
    " - Microsoft Edge",
    " - Mozilla Firefox",
    " - Brave",
    " - Safari",
)


class PlannerError(RuntimeError):
    def __init__(self, code: str, message: str, **details: Any) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details

    def to_dict(self) -> dict[str, Any]:
        return {"code": self.code, "message": self.message, "details": self.details}


class PlanningBackend(Protocol):
    """Minimal interface implemented by local, remote, or test scorers."""

    def plan(
        self,
        form_title: str,
        elements: list[Element],
        entities: list[Entity],
    ) -> Sequence[Decision]: ...


class Planner:
    """Validate decisions from an injected backend without owning a model."""

    def __init__(
        self,
        backend: PlanningBackend | Callable[[str, list[Element], list[Entity]], Sequence[Decision]],
    ) -> None:
        self.backend = backend

    def plan(
        self,
        form_title: str,
        elements: list[Element],
        entities: list[Entity],
    ) -> list[Decision]:
        normalized = normalize_title(form_title)
        if hasattr(self.backend, "plan"):
            decisions = self.backend.plan(normalized, elements, entities)  # type: ignore[union-attr]
        else:
            decisions = self.backend(normalized, elements, entities)  # type: ignore[operator]
        supplied = {_element_identity(element): element for element in elements}
        if len(supplied) != len(elements):
            raise PlannerError(
                "duplicate_element_identity",
                "Supplied actionable elements must have unique identities",
                elements=len(elements),
                identities=len(supplied),
            )

        output = []
        decided: set[tuple[str, str | int]] = set()
        for index, decision in enumerate(decisions):
            identity = _element_identity(decision.element)
            element = supplied.get(identity)
            if element is None:
                raise PlannerError(
                    "foreign_element",
                    "Decision refers to an element outside the supplied snapshot",
                    decision_index=index,
                )
            if identity in decided:
                raise PlannerError(
                    "duplicate_decision",
                    "Planning backend returned more than one decision for an element",
                    decision_index=index,
                    element_index=element.index,
                )
            if decision.action not in ALLOWED_ACTIONS:
                raise PlannerError(
                    "invalid_action",
                    "Planning backend returned an unsupported action",
                    decision_index=index,
                    action=decision.action,
                )
            if (
                isinstance(decision.probability, bool)
                or not isinstance(decision.probability, (int, float))
                or not math.isfinite(decision.probability)
                or not 0 <= decision.probability <= 1
            ):
                raise PlannerError(
                    "invalid_probability",
                    "Decision probability must be finite and between zero and one",
                    decision_index=index,
                    probability=decision.probability,
                )
            if decision.action == "fill":
                if decision.entity_index is None or not 0 <= decision.entity_index < len(entities):
                    raise PlannerError(
                        "invalid_entity_index",
                        "Fill decision does not identify a supplied entity",
                        decision_index=index,
                        entity_index=decision.entity_index,
                    )
            decided.add(identity)
            output.append(
                Decision(
                    element=element,
                    action=decision.action,
                    entity_index=decision.entity_index,
                    probability=decision.probability,
                    distribution=decision.distribution,
                )
            )
        missing = [
            element.index for identity, element in supplied.items() if identity not in decided
        ]
        if missing:
            raise PlannerError(
                "missing_decisions",
                "Planning backend omitted decisions for supplied actionable elements",
                missing_element_indices=missing,
                elements=len(elements),
                decisions=len(output),
            )
        return output


def normalize_title(title: str) -> str:
    for suffix in APP_SUFFIXES:
        if title.endswith(suffix):
            return title[: -len(suffix)]
    return title


def filter_elements(elements: list[Element]) -> list[Element]:
    return [element for element in elements if _normalized_role(element.role) in ACTIONABLE_ROLES]


def order_decisions(
    decisions: list[Decision],
    min_confidence: float,
    *,
    allow_submit: bool = False,
) -> list[Decision]:
    """Order bounded actions and retain at most one recognized submit click."""

    if (
        isinstance(min_confidence, bool)
        or not isinstance(min_confidence, (int, float))
        or not math.isfinite(min_confidence)
        or not 0 <= min_confidence <= 1
    ):
        raise PlannerError(
            "invalid_min_confidence",
            "min_confidence must be finite and between zero and one",
            min_confidence=min_confidence,
        )
    keep = [
        decision
        for decision in decisions
        if decision.action != "skip" and decision.probability >= min_confidence
    ]
    submit_clicks = sorted(
        (
            decision
            for decision in keep
            if decision.action == "click" and _is_submit_control(decision.element)
        ),
        key=lambda decision: -decision.probability,
    )
    keep = [decision for decision in keep if decision.action != "click"]
    if allow_submit and submit_clicks:
        keep.append(submit_clicks[0])
    return sorted(
        keep,
        key=lambda decision: (ACTION_ORDER[decision.action], decision.element.index),
    )


def run_form(
    planner: Planner,
    driver: BaseDriver,
    entities: list[Entity],
    *,
    target: WindowTarget,
    form_title: str,
    min_confidence: float = 0.5,
    execute: bool = False,
    submit: bool = False,
    delivery_mode: DeliveryMode = "background",
    log: Callable[[dict[str, Any]], None] | None = None,
) -> dict[str, Any]:
    """Plan by default; mutate only with explicit execute and submit flags.

    Every action uses an opaque token from the latest exact-window snapshot.
    The window is re-observed after every mutation before another token is used.
    """

    if not isinstance(execute, bool) or not isinstance(submit, bool):
        raise PlannerError(
            "invalid_authorization",
            "execute and submit authorization flags must be booleans",
            execute_type=type(execute).__name__,
            submit_type=type(submit).__name__,
        )
    if submit and not execute:
        # This is a valid preview of a submit-authorized run, but make the lack
        # of execution explicit in the report rather than silently mutating.
        authorization_note = "submit requested, but execute is false; no actions ran"
    else:
        authorization_note = None
    if delivery_mode not in ("background", "foreground"):
        raise PlannerError(
            "invalid_delivery_mode",
            "delivery_mode must be 'background' or 'foreground'",
            delivery_mode=delivery_mode,
        )

    started = time.perf_counter()
    snapshot = driver.window_state(target)
    actionable = filter_elements(snapshot.elements)
    observed_at = time.perf_counter()
    decisions = planner.plan(form_title, actionable, entities)
    planned_at = time.perf_counter()
    ordered = order_decisions(decisions, min_confidence, allow_submit=submit)
    if execute and any(decision.action == "fill" for decision in ordered):
        supports_value_mutation = getattr(driver, "supports_value_mutation", None)
        if not callable(supports_value_mutation) or not supports_value_mutation():
            raise PlannerError(
                "unsupported_driver_capability",
                "The connected driver does not advertise token-based value mutation",
                capability="set_value",
            )

    executed: list[dict[str, Any]] = []
    current_snapshot = snapshot
    stopped_after_error = False
    for decision in ordered:
        entry = _describe_decision(decision, entities)
        entry.update(
            {
                "executed": False,
                "target": target.to_driver_target(),
                "delivery_mode": "background" if decision.action == "fill" else delivery_mode,
                "snapshot_id": current_snapshot.snapshot_id,
            }
        )
        if not execute:
            entry["status"] = "planned"
            executed.append(entry)
            if log:
                log(entry)
            continue

        try:
            current = current_snapshot.element_for(decision.element)
            if decision.action == "check":
                if _normalized_role(current.role) not in {"checkbox", "axcheckbox"}:
                    raise DriverError(
                        "check_requires_checkbox",
                        "A check action requires a checkbox element",
                        details={"element_index": current.index, "role": current.role},
                    )
                if current.checked is None:
                    raise DriverError(
                        "checkbox_state_unknown",
                        "The checkbox state is unknown; refusing to toggle it",
                        details={"element_index": current.index},
                    )
                if current.checked:
                    entry["status"] = "already_satisfied"
                    executed.append(entry)
                    if log:
                        log(entry)
                    continue
            token = current_snapshot.token_for(decision.element)
            if decision.action == "fill":
                assert decision.entity_index is not None
                mutation = driver.set_value(
                    target,
                    token,
                    entities[decision.entity_index].value,
                )
            else:
                mutation = driver.click(
                    target,
                    token,
                    delivery_mode=delivery_mode,
                )
            _require_confirmed_action(decision.action, mutation.action, mutation.observation)
            entry["executed"] = True
            entry["status"] = "delivered"
            entry["action_result"] = mutation.action
            entry["post_snapshot_id"] = mutation.observation.snapshot_id
            current_snapshot = mutation.observation
            if decision.action == "check":
                post_element = current_snapshot.element_for(decision.element)
                if post_element.checked is not True:
                    raise DriverError(
                        "checkbox_postcondition_failed",
                        "The checkbox was not confirmed checked after the action",
                        details={
                            "element_index": post_element.index,
                            "checked": post_element.checked,
                        },
                    )
        except DriverError as exc:
            entry["status"] = "error"
            entry["error"] = exc.to_dict()
            stopped_after_error = True
        executed.append(entry)
        if log:
            log(entry)
        if stopped_after_error:
            break

    finished = time.perf_counter()
    return {
        "form": form_title,
        "target": target.to_driver_target(),
        "authorization": {
            "execute": execute,
            "submit": submit,
            "delivery_mode": delivery_mode,
            "note": authorization_note,
        },
        "entities": [_serialize(entity) for entity in entities],
        "elements_scored": len(actionable),
        "snapshot": _snapshot_report(snapshot),
        "all_decisions": [_describe_decision(decision, entities) for decision in decisions],
        "execution_order": executed,
        "stopped_after_error": stopped_after_error,
        "final_snapshot": _snapshot_report(current_snapshot),
        "timings_ms": {
            "snapshot": round((observed_at - started) * 1000, 1),
            "plan": round((planned_at - observed_at) * 1000, 1),
            "execute": round((finished - planned_at) * 1000, 1),
            "total": round((finished - started) * 1000, 1),
        },
        "driver_calls": driver.timings[-100:],
    }


def _describe_decision(decision: Decision, entities: list[Entity]) -> dict[str, Any]:
    entity = (
        _serialize(entities[decision.entity_index])
        if decision.entity_index is not None and 0 <= decision.entity_index < len(entities)
        else None
    )
    return {
        "element_index": decision.element.index,
        "role": decision.element.role,
        "label": decision.element.label,
        "action": decision.action,
        "confidence": round(decision.probability, 4),
        "entity": entity,
    }


def _snapshot_report(snapshot: WindowSnapshot) -> dict[str, Any]:
    return {
        "snapshot_id": snapshot.snapshot_id,
        "element_count": len(snapshot.elements),
        "elements_complete": snapshot.elements_complete,
    }


def _serialize(value: Any) -> Any:
    if is_dataclass(value):
        return asdict(value)
    if hasattr(value, "__dict__"):
        return dict(value.__dict__)
    return value


def _normalized_role(role: str) -> str:
    return role.replace("_", "").replace(" ", "").casefold()


def _normalized_control_label(label: str) -> str:
    return " ".join(re.findall(r"[a-z0-9]+", label.casefold()))


def _is_submit_control(element: Element) -> bool:
    return (
        _normalized_role(element.role) in SUBMIT_CONTROL_ROLES
        and _normalized_control_label(element.label) in SUBMIT_CONTROL_LABELS
    )


def _element_identity(element: Element) -> tuple[str, str | int]:
    if element.element_token:
        return ("token", element.element_token)
    return ("object", id(element))


def _require_confirmed_action(
    action_name: str,
    action: dict[str, Any],
    observation: WindowSnapshot,
) -> None:
    effect = action.get("effect")
    if effect == "confirmed":
        return
    raise DriverError(
        "action_effect_unconfirmed",
        "Cua Driver did not confirm the action effect",
        tool="set_value" if action_name == "fill" else "click",
        details={
            "effect": effect if isinstance(effect, str) else "missing",
            "route": action.get("route"),
            "escalation": action.get("escalation"),
            "post_action_observation": _snapshot_report(observation),
        },
        outcome_unknown=effect not in {"refused", "suspected_noop"},
    )
