# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Run one raw-macos script in this process and print one JSON result line on stdout.

Usage: runner_mac.py IMAGE_DIR < script.py

The MCP server starts one runner per run_script call (a fresh process, killed at the call's time limit), so a script
cannot keep state between calls or outlive its call. The script body is wrapped in a function, so it may ``return`` a
value. Builtins are restricted (no open, eval, exec, compile, input, breakpoint) and ``import`` only admits
rawlint_mac.IMPORT_OK. The lint runs first; a blocked script never starts.
"""

from __future__ import annotations

import ast
import builtins
import contextlib
import io
import json
import os
import sys
import traceback
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rawlint_mac as rawlint  # noqa: E402

DENIED_BUILTINS = {
    "open",
    "eval",
    "exec",
    "compile",
    "input",
    "breakpoint",
    "help",
    "exit",
    "quit",
    "__import__",
}


def _jsonable(value: Any) -> Any:
    try:
        json.dumps(value)
        return value
    except (TypeError, ValueError):
        if isinstance(value, (list, tuple)):
            return [_jsonable(v) for v in value]
        if isinstance(value, dict):
            return {str(k): _jsonable(v) for k, v in value.items()}
        return str(value)


def _guarded_getattr(obj: Any, name: str, *default: Any) -> Any:
    """getattr for scripts: the same refusals as the lint, applied to the name the script computed."""
    if not isinstance(name, str) or name.startswith("_") or name in rawlint.ESCAPES:
        raise AttributeError(f"{name!r} is not available to raw-macos scripts (rule GUI2)")
    import raw_mac

    rule = rawlint.NAMES.get(name)
    if rule in {"GUI1", "MB5"}:
        raw_mac.STATE.refuse(rule, f"getattr {name}")
    raw_mac.STATE.check(rule, f"getattr {name}")
    return getattr(obj, name, *default)


def _guarded_import(
    name: str, globals_: Any = None, locals_: Any = None, fromlist: Any = (), level: int = 0
) -> Any:
    if name not in rawlint.IMPORT_OK or level:
        raise ImportError(
            f"import {name} is not available to raw-macos scripts (GUI-only, rule GUI1); use raw.*"
        )
    if name == "raw":
        return sys.modules["raw_runtime"]
    return builtins.__import__(name, globals_, locals_, fromlist, level)


def run(source: str, image_dir: Path) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "lint": [], "lint_blocked": False}
    try:
        report = rawlint.lint(source, granted=os.environ.get("RAW_GRANT") == "1")
    except SyntaxError as error:
        out["error"] = f"SyntaxError: {error.msg} (line {error.lineno})"
        return out
    out["lint"] = [
        {"rule": f.rule, "line": f.line, "detail": f.detail, "severity": f.severity}
        for f in report.findings
    ]
    out["lint_text"] = report.render()
    out["granted"] = report.granted
    if not report.ok:
        out["lint_blocked"] = True
        out["error"] = "blocked by the background-use lint; nothing was run"
        return out

    import raw_mac  # the frameworks load only for a script that passed the lint

    raw_mac.STATE.image_dir = image_dir
    handle = raw_mac.make()
    sys.modules["raw_runtime"] = handle
    tree = ast.parse(source, "<script>", "exec")
    func = ast.FunctionDef(
        name="__script",
        args=ast.arguments(posonlyargs=[], args=[], kwonlyargs=[], kw_defaults=[], defaults=[]),
        body=tree.body or [ast.Pass()],
        decorator_list=[],
        returns=None,
        type_params=[],
    )
    module = ast.Module(body=[func], type_ignores=[])
    ast.fix_missing_locations(module)
    safe = {k: v for k, v in vars(builtins).items() if k not in DENIED_BUILTINS}
    safe["__import__"] = _guarded_import
    safe["getattr"] = _guarded_getattr
    for name in ("setattr", "delattr", "vars", "globals", "locals", "memoryview"):
        safe.pop(name, None)
    env: dict[str, Any] = {"__builtins__": safe, "raw": handle, "__name__": "script"}
    exec(compile(module, "<script>", "exec"), env)  # noqa: S102 - the script, after the lint
    printed = io.StringIO()
    watch = raw_mac.Watch().start()
    try:
        with contextlib.redirect_stdout(printed):
            value = env["__script"]()
        out["ok"] = True
        out["value"] = _jsonable(value)
    except raw_mac.Blocked as error:
        out["error"] = str(error)
    except Exception:  # noqa: BLE001 - the model gets the traceback of its own script
        lines = traceback.format_exc().splitlines()
        keep = [ln for ln in lines if "<script>" in ln or not ln.startswith("  File")]
        out["error"] = "\n".join(keep[-12:])
    finally:
        out["watch"] = watch.finish()
        out["stdout"] = printed.getvalue()[-4000:]
        out["runtime_blocked"] = list(raw_mac.STATE.runtime_blocked)
        out["runtime_warned"] = list(raw_mac.STATE.runtime_warned)
        out["driver_calls"] = list(raw_mac.STATE.driver_calls)
        out["raw_actions"] = dict(raw_mac.STATE.raw_actions)
        out["images"] = [str(p) for p in raw_mac.STATE.images]
    return out


def main() -> int:
    image_dir = Path(sys.argv[1])
    image_dir.mkdir(parents=True, exist_ok=True)
    source = sys.stdin.read()
    real_stdout = sys.stdout
    try:
        result = run(source, image_dir)
    except Exception:  # noqa: BLE001
        result = {"ok": False, "error": "runtime failure: " + traceback.format_exc()[-1500:]}
    real_stdout.write(json.dumps(result) + "\n")
    real_stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
