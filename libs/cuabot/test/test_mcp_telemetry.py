"""computer-use-mcp.py telemetry never sends typed text, keys or paths.

Loads only the telemetry helpers from the script (no fastmcp, no network).
Run: python3 -m pytest libs/cuabot/test/test_mcp_telemetry.py
"""

import ast
from pathlib import Path

SRC = Path(__file__).resolve().parents[1] / "src" / "mcp" / "computer-use-mcp.py"


def _load_helpers():
    tree = ast.parse(SRC.read_text())
    keep = [
        node
        for node in tree.body
        if (isinstance(node, ast.Assign) and node.targets[0].id.startswith("_TELEMETRY"))
        or (isinstance(node, ast.FunctionDef) and node.name == "_telemetry_safe_args")
    ]
    ns: dict = {}
    exec(compile(ast.Module(body=keep, type_ignores=[]), str(SRC), "exec"), ns)
    return ns["_telemetry_safe_args"]


def test_safe_args_drop_text_keys_and_paths():
    safe = _load_helpers()
    assert safe({"text": "hunter2", "delay": 12}) == {"delay": 12}
    assert safe({"key": "ctrl+c"}) == {}
    assert safe({"save_path": "/Users/alice/x.jpg"}) == {}
    assert safe({"x": 1, "y": 2.5, "button": "left"}) == {"x": 1, "y": 2.5, "button": "left"}
    assert safe({"x": "1", "button": "alice"}) == {}


def test_call_sites_do_not_pass_sensitive_values():
    src = SRC.read_text()
    for needle in ('"text": text', '"key": key', '"save_path": save_path', '"keys"'):
        calls = [ln for ln in src.splitlines() if "log_mcp_tool_call(" in ln and needle in ln]
        assert not calls, calls
