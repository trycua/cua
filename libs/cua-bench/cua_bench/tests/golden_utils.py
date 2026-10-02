"""Helpers for golden snapshots (CLI tree, export schemas).

Set ``CUA_BENCH_UPDATE_GOLDENS=1`` to rewrite a golden deliberately; the diff
then shows up in review. Without it a mismatch fails the test.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import Any

GOLDEN_DIR = Path(__file__).parent / "golden"


def _jsonable(value: Any) -> Any:
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, (list, tuple)):
        return [_jsonable(v) for v in value]
    if value is argparse.SUPPRESS:
        return "==SUPPRESS=="
    return repr(value)


def _action(action: argparse.Action) -> dict:
    return {
        "options": list(action.option_strings),
        "dest": action.dest,
        "action": type(action).__name__,
        "default": _jsonable(action.default),
        "const": _jsonable(action.const),
        "choices": _jsonable(list(action.choices)) if action.choices else None,
        "nargs": _jsonable(action.nargs),
        "type": getattr(action.type, "__name__", None) if action.type else None,
        "required": bool(action.required),
        "hidden": action.help == argparse.SUPPRESS,
        "metavar": _jsonable(action.metavar),
    }


def parser_tree(parser: argparse.ArgumentParser) -> dict:
    """Every command, subcommand and flag of ``parser`` as plain data."""
    node: dict[str, Any] = {"arguments": [], "commands": {}}
    for action in parser._actions:
        if isinstance(action, argparse._HelpAction):
            continue
        if isinstance(action, argparse._SubParsersAction):
            node["subcommand_dest"] = action.dest
            seen: dict[int, str] = {}
            for name, sub in action.choices.items():
                if id(sub) in seen:  # an alias of a command already recorded
                    node["commands"][seen[id(sub)]].setdefault("aliases", []).append(name)
                    continue
                seen[id(sub)] = name
                node["commands"][name] = parser_tree(sub)
            continue
        node["arguments"].append(_action(action))
    return node


def check_golden(name: str, data: Any) -> None:
    """Compare ``data`` with ``golden/<name>.json`` (or rewrite it on request)."""
    path = GOLDEN_DIR / f"{name}.json"
    text = json.dumps(data, indent=2, sort_keys=True) + "\n"
    if os.environ.get("CUA_BENCH_UPDATE_GOLDENS") == "1" or not path.exists():
        if not path.exists() and os.environ.get("CI"):
            raise AssertionError(f"missing golden {path}; generate it locally")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return
    expected = path.read_text()
    assert text == expected, (
        f"{name} changed. If the change is intended and additive, rerun with "
        f"CUA_BENCH_UPDATE_GOLDENS=1 and review the diff of {path}."
    )
