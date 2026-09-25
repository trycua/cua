"""Static diagnostic report generation for benchmark trials."""

from __future__ import annotations

import json
import os
import re
import shutil
from pathlib import Path
from typing import Any, Mapping

from jinja2 import Environment, FileSystemLoader, select_autoescape


PAPERCUT_START = "<PAPERCUTS>"
PAPERCUT_END = "</PAPERCUTS>"
MAX_EVENT_TEXT = 500_000
_PAPERCUT_ITEM = re.compile(r"^\s*(?:[-*+]\s+|\d+[.)]\s+)(.+?)\s*$")


def _environment() -> Environment:
    return Environment(
        loader=FileSystemLoader(Path(__file__).with_name("templates")),
        autoescape=select_autoescape(
            enabled_extensions=("html", "xml", "j2"),
            default_for_string=True,
            default=True,
        ),
        keep_trailing_newline=True,
    )


def _redaction_values() -> tuple[str, ...]:
    values = []
    for name in (
        "CUA_CLIENT_SECRET",
        "OPENAI_API_KEY",
        "FLEETS_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_ACCESS_KEY_ID",
    ):
        value = os.environ.get(name, "").strip()
        if value:
            values.append(value)
    return tuple(values)


def _redact(value: str) -> str:
    for secret in _redaction_values():
        value = value.replace(secret, "<REDACTED_SECRET>")
    return value


def _format_value(value: Any) -> str:
    if isinstance(value, str):
        text = value
    else:
        try:
            text = json.dumps(value, indent=2, ensure_ascii=False)
        except (TypeError, ValueError):
            text = repr(value)
    text = _redact(text)
    if len(text) > MAX_EVENT_TEXT:
        text = text[:MAX_EVENT_TEXT] + "\n[truncated after 500,000 characters]"
    return text


def _read_jsonl(path: Path) -> list[dict[str, Any] | None]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError):
        return []
    events: list[dict[str, Any] | None] = []
    for line in lines:
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            events.append(None)
            continue
        events.append(event if isinstance(event, dict) else None)
    return events


def _event(kind: str, label: str, text: Any = "") -> dict[str, Any]:
    return {
        "kind": kind,
        "label": label,
        "text": _format_value(text),
        "images": [],
    }


def _tool_call(item: Mapping[str, Any]) -> dict[str, Any]:
    if item.get("type") == "mcp_tool_call":
        tool = str(item.get("tool") or "unknown")
        return _event("tool_call", f"TOOL CALL: {tool}", item.get("arguments", {}))
    return _event(
        "tool_call",
        "TOOL CALL: command_execution",
        item.get("command", ""),
    )


def _tool_result(item: Mapping[str, Any]) -> dict[str, Any]:
    if item.get("type") == "mcp_tool_call":
        tool = str(item.get("tool") or "unknown")
        result = item.get("error") or item.get("result")
        return _event("tool_result", f"TOOL RESULT: {tool}", result or "")
    return _event(
        "tool_result",
        "TOOL RESULT: command_execution",
        {
            "exit_code": item.get("exit_code"),
            "status": item.get("status"),
            "output": item.get("aggregated_output", ""),
        },
    )


def _recording_images(trial_dir: Path) -> list[Path]:
    root = trial_dir / "observer" / "cua-driver-recording"
    images: list[Path] = []
    for turn in sorted(root.glob("turn-*")):
        candidates = [
            turn / "screenshot.jpg",
            turn / "screenshot.jpeg",
            turn / "screenshot.png",
            turn / "after.jpg",
            turn / "after.jpeg",
            turn / "after.png",
        ]
        selected = next((path for path in candidates if path.is_file()), None)
        if selected is not None:
            images.append(selected)
    return images


def load_trajectory_events(trial_dir: Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    brief = trial_dir / "inputs" / "artifacts" / "brief.md"
    if brief.is_file():
        events.append(_event("user", "USER", brief.read_text(encoding="utf-8")))

    pending: dict[str, dict[str, Any]] = {}
    event_path = trial_dir / "artifacts" / "codex-events.jsonl"
    for raw in _read_jsonl(event_path):
        if raw is None:
            events.append(
                _event(
                    "tool_result",
                    "UNPARSEABLE EVENT",
                    "raw event could not be parsed",
                )
            )
            continue
        item = raw.get("item")
        if not isinstance(item, dict):
            continue
        item_type = item.get("type")
        if item_type == "agent_message" and raw.get("type") == "item.completed":
            events.append(_event("agent", "AGENT", item.get("text", "")))
            continue
        if item_type not in {"mcp_tool_call", "command_execution"}:
            continue
        item_id = str(item.get("id") or "")
        if raw.get("type") == "item.started":
            call = _tool_call(item)
            events.append(call)
            if item_id:
                pending[item_id] = call
            continue
        if raw.get("type") != "item.completed":
            continue
        if item_id not in pending:
            events.append(_tool_call(item))
        events.append(_tool_result(item))

    images = _recording_images(trial_dir)
    result_events = [event for event in events if event["kind"] == "tool_result"]
    for index, image in enumerate(images):
        if not result_events:
            break
        result_event = result_events[min(index, len(result_events) - 1)]
        result_event.setdefault("source_images", []).append(image)
    return events


def extract_papercuts(final_agent_message: str) -> list[str] | None:
    if PAPERCUT_START not in final_agent_message or PAPERCUT_END not in final_agent_message:
        return None
    body = final_agent_message.split(PAPERCUT_START, 1)[1].split(PAPERCUT_END, 1)[0]
    if body.strip().lower() == "none":
        return []
    items = []
    for line in body.splitlines():
        match = _PAPERCUT_ITEM.match(line)
        if match:
            items.append(match.group(1).strip())
    return items


def _final_agent_message(events: list[Mapping[str, Any]]) -> str:
    messages = [str(event.get("text", "")) for event in events if event.get("kind") == "agent"]
    return messages[-1] if messages else ""


def papercut_count_for_trial(trial_dir: Path) -> int | None:
    if not trial_dir.is_dir():
        return None
    items = extract_papercuts(_final_agent_message(load_trajectory_events(trial_dir)))
    return len(items) if items is not None else None


def _write_papercuts(path: Path, trial: Mapping[str, Any], items: list[str] | None) -> None:
    lines = [
        "# Papercuts",
        "",
        f"Task: {trial['task']}",
        f"Cua Driver: {trial['version']}",
        "",
    ]
    if items is None:
        lines.append("Papercut section was not produced by the agent.")
    elif not items:
        lines.append("No papercuts reported.")
    else:
        lines.extend(f"{index}. {item}" for index, item in enumerate(items, 1))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def write_html_bundle(report: dict[str, Any], output_dir: Path) -> None:
    environment = _environment()
    report_dir = output_dir / "report"
    report_dir.mkdir(parents=True, exist_ok=True)
    for trial in report.get("trials", []):
        trial_id = str(trial.get("trial_id", "trial"))
        trial_dir = output_dir / "trials" / trial_id
        relative_id = trial_id.replace("/", "_")
        trial_report_dir = report_dir / "trials" / relative_id
        assets_dir = trial_report_dir / "assets"
        trial_report_dir.mkdir(parents=True, exist_ok=True)
        if assets_dir.exists():
            shutil.rmtree(assets_dir)
        assets_dir.mkdir(parents=True, exist_ok=True)
        events = load_trajectory_events(trial_dir) if trial_dir.is_dir() else []
        items = extract_papercuts(_final_agent_message(events))
        for event in events:
            for source in event.pop("source_images", []):
                destination = (
                    assets_dir / f"{len(list(assets_dir.iterdir())) + 1:04d}{source.suffix.lower()}"
                )
                try:
                    shutil.copy2(source, destination)
                    event["images"].append(f"assets/{destination.name}")
                except OSError:
                    event["text"] += f"\n[missing image artifact: {source}]"
        trial["trajectory_html"] = f"report/trials/{relative_id}/trajectory.html"
        trial["papercuts_md"] = f"report/trials/{relative_id}/papercuts.md"
        trial["trajectory_html_relative"] = f"trials/{relative_id}/trajectory.html"
        trial["papercuts_md_relative"] = f"trials/{relative_id}/papercuts.md"
        trial["papercut_count"] = len(items) if items is not None else None
        _write_papercuts(trial_report_dir / "papercuts.md", trial, items)
        trajectory = environment.get_template("trajectory.html.j2").render(
            trial=trial, events=events
        )
        (trial_report_dir / "trajectory.html").write_text(
            trajectory, encoding="utf-8", newline="\n"
        )

    index = environment.get_template("comparison.html.j2").render(report=report)
    (report_dir / "index.html").write_text(index, encoding="utf-8", newline="\n")
    (report_dir / "comparison.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
