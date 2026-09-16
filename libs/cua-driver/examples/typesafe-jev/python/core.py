from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal

Outcome = Literal["verified", "refuted", "unknown", "abstained", "budget_exhausted"]


@dataclass(frozen=True)
class Candidate:
    id: str
    description: str
    tool: str | None
    arguments: dict[str, Any]


def build_candidates(snapshot: dict[str, Any], token: str) -> list[Candidate]:
    common = {
        "target_id": snapshot["target_id"],
        "tab_id": snapshot["tab_id"],
    }
    refs = snapshot.get("refs", [])
    field = next(
        (
            ref
            for ref in refs
            if ref.get("role") == "textbox" and ref.get("name") == "verification value"
        ),
        None,
    )
    button = next(
        (ref for ref in refs if ref.get("role") == "button" and ref.get("name") == "Submit"),
        None,
    )
    candidates: list[Candidate] = []
    if field and field.get("value") != token:
        candidates.append(
            Candidate(
                "type-verification-value",
                "Replace the verification field with the required token.",
                "browser_type",
                {**common, "ref": field["ref"], "text": token, "replace": True},
            )
        )
    elif field and field.get("value") == token and button:
        candidates.append(
            Candidate(
                "submit-form",
                "Submit the form now that the verification field contains the token.",
                "browser_click",
                {**common, "ref": button["ref"], "input_route": "dom_event"},
            )
        )
    candidates.append(
        Candidate(
            "abstain",
            "Stop without acting if none of the proposed actions is safe for the observed state.",
            None,
            {},
        )
    )
    return candidates


def choose_mock(candidates: list[Candidate]) -> tuple[str | None, float, dict[str, float]]:
    ids = {candidate.id for candidate in candidates}
    selected = (
        "type-verification-value"
        if "type-verification-value" in ids
        else "submit-form" if "submit-form" in ids else None
    )
    probabilities = {candidate.id: float(candidate.id == selected) for candidate in candidates}
    return selected, 1.0 if selected else 0.0, probabilities


def validate_choice(choice: str, candidates: list[Candidate]) -> Candidate:
    for candidate in candidates:
        if candidate.id == choice:
            return candidate
    raise ValueError(f"provider selected unknown candidate: {choice}")


def classify(submitted: str | None, token: str, *, steps: int, max_steps: int) -> Outcome:
    if submitted == token:
        return "verified"
    if submitted is not None:
        return "refuted"
    if steps >= max_steps:
        return "budget_exhausted"
    return "unknown"
