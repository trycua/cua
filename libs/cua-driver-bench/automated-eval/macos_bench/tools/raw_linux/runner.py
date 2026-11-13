# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Run one raw-linux script in this process and print one JSON result line on stdout.

Usage: runner.py IMAGE_DIR < script.py

The MCP server starts one runner per run_script call (a fresh process, killed at the call's time limit), so a
script cannot keep state between calls or outlive its call. The script body is wrapped in a function, so it may
``return`` a value. Builtins are restricted (no open, eval, exec, compile, input, breakpoint) and ``import`` only
admits rawlint.IMPORT_OK. The lint runs first; a blocked script never starts.

With RAW_NATIVE=1 (the no-wrapper arm, pi-raw-linux-native) the script gets no ``raw`` runtime. It imports Linux's
own bindings (rawlint.NATIVE_IMPORTS / NATIVE_GI: python-xlib, gi's Atspi and Gdk, pyatspi, dbus, ctypes for libX11,
libXtst or libxdo) and calls them directly. Its globals carry ``SHOTS`` (a directory: every PNG written there comes
back to the model as an image) and ``xdotool(*args)`` (the xdotool CLI, checked as before). The harness side stays
the same: the AT-SPI timeout, the per-app action count, the foreground checks on the native routes and the hook.
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
import rawlint  # noqa: E402

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
        return repr(value)


NATIVE = os.environ.get("RAW_NATIVE") == "1"


def _guarded_getattr(obj: Any, name: str, *default: Any) -> Any:
    """getattr for scripts: the same refusals as the lint, applied to the name the script computed."""
    escapes = rawlint.NATIVE_ESCAPES if NATIVE else rawlint.ESCAPES
    if (
        not isinstance(name, str)
        or (name.startswith("_") and not rawlint.private_ok(name, NATIVE))
        or name in escapes
    ):
        raise AttributeError(f"{name!r} is not available to raw-linux scripts (rule GUI2)")
    import raw

    rule = ({**rawlint.NAMES, **rawlint.NATIVE_NAMES} if NATIVE else rawlint.NAMES).get(name)
    if NATIVE and name in rawlint.NATIVE_LOADERS:
        rule = None
    if rule == "GUI1":
        raw.STATE.refuse("GUI1", f"getattr {name}")
    raw.STATE.check(rule, f"getattr {name}")
    return getattr(obj, name, *default)


RUNTIME_IMPORTS = {"gi.repository.Atspi"}


def _guarded_import(
    name: str, globals_: Any = None, locals_: Any = None, fromlist: Any = (), level: int = 0
) -> Any:
    if NATIVE:
        if level or not rawlint.import_ok(name, True):
            raise ImportError(f"import {name} is not available to raw-linux scripts (GUI-only, rule GUI1)")
        if name == "gi.repository" or (name == "gi" and fromlist and "repository" in fromlist):
            for sub in fromlist or ():
                if sub != "repository" and sub not in rawlint.NATIVE_GI:
                    raise ImportError(f"gi.repository.{sub} is not available to raw-linux scripts (rule GUI1)")
        return builtins.__import__(name, globals_, locals_, fromlist, level)
    if name in RUNTIME_IMPORTS and name in sys.modules and not fromlist:
        # gi's lazy attribute loader re-imports its own, already loaded module through the caller's __import__
        # (Atspi.Action -> import gi.repository.Atspi); hand that one back, refuse every other gi import
        return sys.modules[name]
    if name not in rawlint.IMPORT_OK or level:
        raise ImportError(
            f"import {name} is not available to raw-linux scripts (GUI-only, rule GUI1); use raw.*"
        )
    if name == "raw":
        return sys.modules["raw_runtime"]
    return builtins.__import__(name, globals_, locals_, fromlist, level)


def run(source: str, image_dir: Path) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "lint": [], "lint_blocked": False}
    try:
        report = rawlint.lint(source, granted=os.environ.get("RAW_GRANT") == "1", native=NATIVE)
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

    import raw  # the runtime opens X and AT-SPI only for a script that passed the lint

    raw.STATE.image_dir = image_dir
    handle = raw.make_native() if NATIVE else raw.make()
    if not NATIVE:
        sys.modules["raw_runtime"] = handle
    shots = image_dir / "shots"
    shots.mkdir(parents=True, exist_ok=True)
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
    env: dict[str, Any] = (
        {"__builtins__": safe, "SHOTS": str(shots), "xdotool": handle.xdotool, "__name__": "script"}
        if NATIVE
        else {"__builtins__": safe, "raw": handle, "Atspi": raw.Atspi, "__name__": "script"}
    )
    exec(compile(module, "<script>", "exec"), env)  # noqa: S102 - the script, after the lint
    printed = io.StringIO()
    watch = raw.Watch().start()
    try:
        with contextlib.redirect_stdout(printed):
            value = env["__script"]()
        out["ok"] = True
        out["value"] = _jsonable(value)
    except raw.Blocked as error:
        out["error"] = str(error)
    except Exception:  # noqa: BLE001 - the model gets the traceback of its own script
        lines = traceback.format_exc().splitlines()
        keep = [ln for ln in lines if "<script>" in ln or not ln.startswith("  File")]
        out["error"] = "\n".join(keep[-12:])
    finally:
        try:
            out["foreground_restore"] = handle.restore_foreground()
        except Exception as error:  # noqa: BLE001
            out["foreground_restore"] = {"error": str(error)[:200]}
        out["watch"] = watch.finish()
        out["stdout"] = printed.getvalue()[-4000:]
        out["runtime_blocked"] = list(raw.STATE.runtime_blocked)
        out["runtime_warned"] = list(raw.STATE.runtime_warned)
        out["raw_actions"] = dict(raw.STATE.raw_actions)
        out["routes"] = dict(raw.STATE.routes)
        out["atspi_stalled"] = {str(k): v for k, v in raw.STATE.stalled.items()}
        out["images"] = [str(p) for p in raw.STATE.images] + [str(p) for p in sorted(shots.glob("*.png"))[:8]]
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
