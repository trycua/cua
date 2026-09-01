"""Bundled JS implementations of the Yutori Navigator n1.5 expanded browser tools.

These scripts are vendored verbatim from the Apache-2.0 licensed Yutori Python
SDK (https://github.com/yutori-ai/yutori-sdk-python, yutori/navigator/tools/js/).
Each file contains a single JS function expression; build_tool_expression wraps
it in an IIFE call with JSON-serialized arguments suitable for page.evaluate().
"""

from __future__ import annotations

import json
from importlib.resources import files
from typing import Any, Dict


def _load_script(name: str) -> str:
    return (files(__package__) / name).read_text(encoding="utf-8")


EXECUTE_JS_SCRIPT = _load_script("execute_js.js")
EXTRACT_ELEMENTS_SCRIPT = _load_script("extract_elements.js")
FIND_SCRIPT = _load_script("find.js")
GET_ELEMENT_BY_REF_SCRIPT = _load_script("get_element_by_ref.js")
SET_ELEMENT_VALUE_SCRIPT = _load_script("set_element_value.js")


def build_tool_expression(script: str, *args: Any) -> str:
    """Wrap a tool script in an IIFE call with JSON-serialized arguments."""
    escaped_args = ", ".join(json.dumps(arg) for arg in args)
    return f"({script})({escaped_args})"


def coerce_result(raw: Any) -> Dict[str, Any]:
    """Normalize a page.evaluate() result into a consistent dict.

    - None -> {"success": False, "message": "Script returned no result"}
    - dict -> passed through unchanged
    - JSON string that parses to a dict -> that dict
    - anything else -> {"value": raw}
    """
    if raw is None:
        return {"success": False, "message": "Script returned no result"}
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, str):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return {"value": raw}
        if isinstance(parsed, dict):
            return parsed
        return {"value": parsed}
    return {"value": raw}


__all__ = [
    "EXECUTE_JS_SCRIPT",
    "EXTRACT_ELEMENTS_SCRIPT",
    "FIND_SCRIPT",
    "GET_ELEMENT_BY_REF_SCRIPT",
    "SET_ELEMENT_VALUE_SCRIPT",
    "build_tool_expression",
    "coerce_result",
]
