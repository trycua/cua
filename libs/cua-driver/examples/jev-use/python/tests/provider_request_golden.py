"""Record the exact TypeSafe Jev request payloads for the page and visual fixtures.

The golden file ``fixtures/jev-provider-request-golden-python-v1.json`` was
captured from this module on ``main`` before the candidate-source and task-spec
refactor (RFC #4268 Phase 0). ``test_provider_request_golden.py`` replays the
same scenarios and requires byte-identical payloads, so any change to the model
input or to the candidate table is a test failure, not a silent drift.

Regenerate only for an intentional contract change::

    uv run python python/tests/provider_request_golden.py --write
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import sys
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import (  # noqa: E402
    SUBMIT_IDS,
    build_candidates,
    has_executable_candidate,
    history_entry,
    parse_visual_regions,
    validate_choice,
)
from jev_adapter import choose_with_typesafe  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "fixtures"
GOLDEN = FIXTURES / "jev-provider-request-golden-python-v1.json"
PAGE = json.loads((FIXTURES / "jev-page-structure-replay-v1.json").read_text(encoding="utf-8"))
VISUAL = json.loads((FIXTURES / "jev-visual-replay-v1.json").read_text(encoding="utf-8"))

# build(snapshot, token, visual, capture_bound_click, visual_delivery) -> candidates
Build = Callable[..., list]
# choose(client, candidates, snapshot, visual, history, token, visual_path) -> choice tuple
Choose = Callable[..., tuple]


def observation(payload: dict[str, Any] | None = None):
    payload = payload or VISUAL["visual_regions"]
    source = payload["capture"]["source"]
    return parse_visual_regions(
        payload,
        expected_capture_id=payload["capture"]["capture_id"],
        expected_pid=source["pid"],
        expected_window_id=source["window_id"],
    )


class RecordingClient:
    """Answer with scripted IDs and keep every request exactly as sent.

    A scripted ID absent from the offered criteria (for example a visual Submit
    when visual parsing is off) is answered with ``reobserve``.
    """

    def __init__(self, choices: list[str]) -> None:
        self.choices = list(choices)
        self.requests: list[dict[str, Any]] = []

    def system_one(self, **request: Any) -> Any:
        self.requests.append(request)
        choice = self.choices.pop(0)
        if choice not in request["questions"]["driver_action"].criteria:
            choice = "reobserve"
        answer = SimpleNamespace(choice=choice, confidence=1.0, probabilities={choice: 1.0})
        return SimpleNamespace(choices={"driver_action": answer})


def wire_request(request: dict[str, Any]) -> dict[str, Any]:
    question = request["questions"]["driver_action"]
    return {
        "state": request["state"],
        "questions": {
            "driver_action": {
                "instructions": question.instructions,
                "criteria": question.criteria,
            }
        },
    }


def wire_candidates(candidates: list) -> list[dict[str, Any]]:
    return [
        {
            "id": candidate.id,
            "description": candidate.description,
            "tool": candidate.tool,
            "arguments": json.loads(json.dumps(dict(candidate.arguments))),
            "capture_id": candidate.capture_id,
            "screenshot_reference": candidate.screenshot_reference,
        }
        for candidate in candidates
    ]


def replay(
    build: Build,
    choose: Choose,
    fixture: dict[str, Any],
    run: dict[str, Any],
    *,
    capture_bound_click: bool,
    visual_mode: str,
) -> list[dict[str, Any]]:
    """Replay one recorded run the way the runner steps through it."""
    token = fixture["token"]
    before = fixture["snapshots"]["before_typing"]
    after = fixture["snapshots"]["after_typing"]
    visual_path = capture_bound_click and visual_mode != "off"
    history: list[dict[str, Any]] = []
    typed = False
    delivery = "background"
    records: list[dict[str, Any]] = []
    client = RecordingClient([step["selected_id"] for step in run["steps"]])
    for index, recorded in enumerate(run["steps"], start=1):
        step = recorded.get("step", index)
        snapshot = after if typed else before
        candidates = build(snapshot, token, None, capture_bound_click, delivery)
        visual = None
        if visual_mode == "always" or (
            visual_mode == "auto" and not has_executable_candidate(candidates)
        ):
            if capture_bound_click:
                visual = observation()
                candidates = build(snapshot, token, visual, capture_bound_click, delivery)
        choice, _, _ = choose(client, candidates, snapshot, visual, history, token, visual_path)
        candidate = validate_choice(
            choice, candidates, current_capture_id=visual.capture_id if visual else None
        )
        records.append(
            {
                "step": step,
                "request": wire_request(client.requests[-1]),
                "candidates": wire_candidates(candidates),
            }
        )
        refusal = recorded.get("action_error")
        history.append(history_entry(step, candidate.id, refusal=refusal))
        if candidate.id == "abstain":
            break
        if candidate.id == "type-verification-value":
            typed = True
        elif candidate.id in SUBMIT_IDS:
            if refusal:
                delivery = "foreground"
            else:
                break
    return records


def with_field_value(snapshot: dict[str, Any], value: str | None) -> dict[str, Any]:
    changed = copy.deepcopy(snapshot)
    for ref in changed["refs"]:
        if ref.get("role") == "textbox" and ref.get("name") == "verification value":
            ref["value"] = value
    return changed


def without_button(snapshot: dict[str, Any]) -> dict[str, Any]:
    changed = copy.deepcopy(snapshot)
    changed["refs"] = [ref for ref in changed["refs"] if ref.get("role") != "button"]
    return changed


def single_requests(build: Build, choose: Choose) -> dict[str, Any]:
    """One request per distinct form and candidate state, with no history."""
    page_token = PAGE["token"]
    page_before = PAGE["snapshots"]["before_typing"]
    page_after = PAGE["snapshots"]["after_typing"]
    visual_token = VISUAL["token"]
    visual_after = VISUAL["snapshots"]["after_typing"]
    visual = observation()
    no_submit = replace(visual, regions=tuple(r for r in visual.regions if r.text != "Submit"))
    submit = next(r for r in visual.regions if r.text == "Submit")
    duplicated = replace(visual, regions=visual.regions + (replace(submit, id="dup"),))
    empty = {"target_id": "target", "tab_id": "tab", "refs": []}
    cases = {
        "page_other_value": (with_field_value(page_before, "other"), page_token, None, False, False, "background"),
        "page_after_with_visual": (page_after, page_token, visual, True, True, "background"),
        "page_after_no_button": (without_button(page_after), page_token, None, False, False, "background"),
        "page_after_no_button_visual_path": (without_button(page_after), page_token, None, True, True, "background"),
        "no_form_refs": (empty, page_token, None, False, False, "background"),
        "visual_after_no_capture_bound_click": (visual_after, visual_token, visual, False, True, "background"),
        "visual_after_without_submit_region": (visual_after, visual_token, no_submit, True, True, "background"),
        "visual_after_duplicate_submit_region": (visual_after, visual_token, duplicated, True, True, "background"),
        "visual_after_foreground": (visual_after, visual_token, visual, True, True, "foreground"),
        "visual_after_other_value": (with_field_value(visual_after, "other"), visual_token, visual, True, True, "background"),
    }
    result: dict[str, Any] = {}
    for name, (snapshot, token, current, cbc, visual_path, delivery) in cases.items():
        candidates = build(snapshot, token, current, cbc, delivery)
        client = RecordingClient(["abstain"])
        choose(client, candidates, snapshot, current, [], token, visual_path)
        result[name] = {
            "request": wire_request(client.requests[-1]),
            "candidates": wire_candidates(candidates),
        }
    return result


def payloads(build: Build, choose: Choose) -> dict[str, Any]:
    runs: dict[str, Any] = {}
    modes = (
        ("page", PAGE, False, "auto"),
        ("page_capture_bound_auto", PAGE, True, "auto"),
        ("page_capture_bound_always", PAGE, True, "always"),
        ("visual_capture_bound_auto", VISUAL, True, "auto"),
        ("visual_capture_bound_always", VISUAL, True, "always"),
        ("visual_capture_bound_off", VISUAL, True, "off"),
    )
    for prefix, fixture, cbc, mode in modes:
        for name, run in fixture["recorded_live_runs"].items():
            runs[f"{prefix}/{name}"] = replay(
                build, choose, fixture, run, capture_bound_click=cbc, visual_mode=mode
            )
    return deduplicate(
        {
            "schema": "cua.jev_use_provider_request_golden_v1",
            "language": "python",
            "runs": runs,
            "single_requests": single_requests(build, choose),
        }
    )


def _digest(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    ).hexdigest()[:16]


def deduplicate(result: dict[str, Any]) -> dict[str, Any]:
    """Store each distinct request and candidate table once, keyed by digest.

    Scenario records keep their order and reference the exact payloads, so the
    golden stays reviewable without repeating the visual regions per step.
    """
    requests: dict[str, Any] = {}
    tables: dict[str, Any] = {}

    def reference(record: dict[str, Any]) -> dict[str, Any]:
        request_id = _digest(record["request"])
        table_id = _digest(record["candidates"])
        requests.setdefault(request_id, record["request"])
        tables.setdefault(table_id, record["candidates"])
        refs = {"request": request_id, "candidates": table_id}
        return {"step": record["step"], **refs} if "step" in record else refs

    runs = {
        name: [reference(record) for record in records]
        for name, records in result["runs"].items()
    }
    singles = {name: reference(record) for name, record in result["single_requests"].items()}
    return {
        "schema": result["schema"],
        "language": result["language"],
        "runs": runs,
        "single_requests": singles,
        "requests": requests,
        "candidate_tables": tables,
    }


def legacy_build(snapshot, token, visual, capture_bound_click, visual_delivery):
    return build_candidates(
        snapshot,
        token,
        visual,
        capture_bound_click=capture_bound_click,
        visual_delivery=visual_delivery,
    )


def legacy_choose(client, candidates, snapshot, visual, history, token, visual_path):
    return choose_with_typesafe(client, candidates, snapshot, visual, history, token, visual_path)


def encode(value: Any) -> str:
    return json.dumps(value, indent=1, ensure_ascii=False) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    text = encode(payloads(legacy_build, legacy_choose))
    if args.write:
        GOLDEN.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
