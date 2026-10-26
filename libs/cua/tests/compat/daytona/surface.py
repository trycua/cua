#!/usr/bin/env python3
"""The public surface of the Daytona SDKs (Python sync + async, TypeScript) and
the check that coverage.json accounts for all of it.

    surface.py --dump                      print the surface as JSON
    surface.py --check                     coverage.json covers every public method; its cases exist
    surface.py --markdown OUT.md           render the coverage matrix
    surface.py --docs OUT.mdx              render the user-facing page

The Python surface is read from the installed `daytona` (introspection), the
TypeScript one from the `.d.ts` files of the installed `@daytonaio/sdk`
(libs/cua/tests/compat/daytona/ts/node_modules). Both are the pinned version unless
the environment installs another.
"""

from __future__ import annotations

import argparse
import inspect
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

PY_CLASSES = {
    "Daytona": ("daytona", "Daytona"),
    "Sandbox": ("daytona", "Sandbox"),
    "Process": ("daytona._sync.process", "Process"),
    "FileSystem": ("daytona._sync.filesystem", "FileSystem"),
    "Git": ("daytona._sync.git", "Git"),
    "LspServer": ("daytona._sync.lsp_server", "LspServer"),
    "CodeInterpreter": ("daytona._sync.code_interpreter", "CodeInterpreter"),
    "ComputerUse": ("daytona._sync.computer_use", "ComputerUse"),
    "Mouse": ("daytona._sync.computer_use", "Mouse"),
    "Keyboard": ("daytona._sync.computer_use", "Keyboard"),
    "Screenshot": ("daytona._sync.computer_use", "Screenshot"),
    "Display": ("daytona._sync.computer_use", "Display"),
    "RecordingService": ("daytona._sync.computer_use", "RecordingService"),
    "Accessibility": ("daytona._sync.computer_use", "Accessibility"),
    "SnapshotService": ("daytona._sync.snapshot", "SnapshotService"),
    "VolumeService": ("daytona._sync.volume", "VolumeService"),
    "SecretService": ("daytona._sync.secret", "SecretService"),
    "WarmPoolService": ("daytona._sync.warm_pool", "WarmPoolService"),
    "Image": ("daytona", "Image"),
}


def py_surface() -> dict[str, list[str]]:
    import importlib

    out: dict[str, list[str]] = {}
    for key, (mod, cls) in PY_CLASSES.items():
        try:
            c = getattr(importlib.import_module(mod), cls)
        except (ImportError, AttributeError):
            continue
        names = []
        for n in dir(c):
            if n.startswith("_"):
                continue
            # the class (in the MRO) that defines it: ours, not pydantic's or the generated client's
            owner = next((k for k in inspect.getmro(c) if n in vars(k)), None)
            if (
                owner is None
                or not owner.__module__.startswith("daytona.")
                or owner.__module__.startswith(("daytona_api_client", "daytona_toolbox_api_client"))
            ):
                continue
            m = inspect.getattr_static(owner, n)
            if isinstance(m, (property, staticmethod, classmethod)) or inspect.isroutine(m):
                names.append(n)
        out[key] = sorted(names)
    return out


def ts_surface() -> dict[str, list[str]]:
    root = os.path.join(HERE, "ts", "node_modules", "@daytonaio", "sdk", "esm")
    files = {
        "Daytona": "Daytona.d.ts",
        "Sandbox": "Sandbox.d.ts",
        "Process": "Process.d.ts",
        "FileSystem": "FileSystem.d.ts",
        "Git": "Git.d.ts",
        "LspServer": "LspServer.d.ts",
        "CodeInterpreter": "CodeInterpreter.d.ts",
        "ComputerUse": "ComputerUse.d.ts",
        "SnapshotService": "Snapshot.d.ts",
        "VolumeService": "Volume.d.ts",
        "SecretService": "Secret.d.ts",
        "WarmPoolService": "WarmPool.d.ts",
        "Image": "Image.d.ts",
    }
    out: dict[str, list[str]] = {}
    for key, f in files.items():
        p = os.path.join(root, f)
        if not os.path.exists(p):
            continue
        text = open(p).read()
        sub_classes = re.findall(
            r"export declare class (\w+)(?:\s+(?:extends|implements)\s+[\w, .<>]+)*\s*\{(.*?)\n\}",
            text,
            re.S,
        )
        for cname, body in sub_classes:
            target = (
                key
                if cname == key or (key == "Image" and cname == "Image")
                else (
                    cname
                    if f == "ComputerUse.d.ts"
                    and cname
                    in (
                        "Mouse",
                        "Keyboard",
                        "Screenshot",
                        "Display",
                        "RecordingService",
                        "Accessibility",
                    )
                    else None
                )
            )
            if target is None:
                continue
            names = set()
            depth = 0
            for line in body.splitlines():
                m = re.match(
                    r"^    (?:static |readonly |get |set )?([a-zA-Z][A-Za-z0-9_]*)\s*[(<:]", line
                )
                if m and not line.strip().startswith(
                    ("private", "protected", "constructor", "//", "*", "/*")
                ):
                    n = m.group(1)
                    if n != "constructor":
                        names.add(n)
            out[target] = sorted(out.get(target, []) + sorted(names))
    return out


def load_cov() -> dict:
    return json.load(open(os.path.join(HERE, "coverage.json")))


def covered(cov: dict) -> tuple[set[str], set[str]]:
    py, ts = set(), set()
    for row in cov["rows"]:
        for a in row.get("py", []):
            py.add(a)
        for a in row.get("ts", []):
            ts.add(a)
    return py, ts


def check() -> int:
    cov = load_cov()
    cases = {c["id"] for c in json.load(open(os.path.join(HERE, "cases.json")))}
    problems = []
    py_c, ts_c = covered(cov)
    ps = py_surface()
    ts = ts_surface()
    for cls, names in ps.items():
        for n in names:
            if f"{cls}.{n}" not in py_c:
                problems.append(f"python {cls}.{n} is in the SDK and not in coverage.json")
    for cls, names in ts.items():
        for n in names:
            if f"{cls}.{n}" not in ts_c:
                problems.append(f"typescript {cls}.{n} is in the SDK and not in coverage.json")
    known_py = {f"{c}.{n}" for c, ns in ps.items() for n in ns}
    known_ts = {f"{c}.{n}" for c, ns in ts.items() for n in ns}
    for a in sorted(py_c - known_py):
        problems.append(f"coverage.json lists python {a}, which the SDK does not have")
    for a in sorted(ts_c - known_ts):
        problems.append(f"coverage.json lists typescript {a}, which the SDK does not have")
    for row in cov["rows"]:
        if row["status"] not in ("implemented", "native-error"):
            problems.append(f"row {row.get('area')}: unknown status {row['status']}")
        if row["status"] == "implemented" and not row.get("cases"):
            problems.append(f"row {row['area']}: implemented without a test")
        if row["status"] == "native-error" and not row.get("reason"):
            problems.append(f"row {row['area']}: native-error without a reason")
        for c in row.get("cases", []):
            if c not in cases and not c.startswith("rust:"):
                problems.append(f"row {row['area']}: case {c} does not exist")
    for p in problems:
        print("COVERAGE", p)
    print(
        f"{len(cov['rows'])} rows, {len(known_py)} Python and {len(known_ts)} TypeScript public methods, {len(problems)} problems"
    )
    return 1 if problems else 0


def markdown(out: str) -> None:
    cov = load_cov()
    ver = cov["sdk"]
    rows = cov["rows"]
    impl = sum(1 for r in rows if r["status"] == "implemented")
    nat = len(rows) - impl
    lines = [
        "# Daytona SDK coverage matrix",
        "",
        f"SDK: `daytona` (Python) and `@daytonaio/sdk` (TypeScript) {ver}. Generated by `libs/cua/tests/compat/daytona/surface.py --markdown`; `surface.py --check` fails when an SDK method is missing from `coverage.json` or a listed test does not exist.",
        "",
        f"{len(rows)} rows: {impl} implemented, {nat} answered with Daytona's own error and a reason. Every implemented row names the tests that exercise it (`py` = the Python SDK, sync and async; `ts` = the TypeScript SDK; `rust:` = a wire test of the gateway). Test lanes: `mock` runs on every PR over mock Spaces; `space` runs on real Spaces (nightly, and on real Spaces).",
        "",
        "| Area | Python | TypeScript | Status | Tests | Source and notes |",
        "|---|---|---|---|---|---|",
    ]
    for r in rows:
        py = "<br>".join(f"`{a}`" for a in r.get("py", [])) or "-"
        ts = "<br>".join(f"`{a}`" for a in r.get("ts", [])) or "-"
        cases = "<br>".join(f"`{c}`" for c in r.get("cases", [])) or "-"
        note = r.get("note", "")
        if r["status"] == "native-error":
            note = f"**Native error.** {r['reason']} {note}"
        src = r.get("source", "")
        lines.append(
            f"| {r['area']} | {py} | {ts} | {r['status']} | {cases} | {src}{'. ' if src and note else ''}{note} |"
        )
    open(out, "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--dump", action="store_true")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--markdown")
    a = ap.parse_args()
    if a.dump:
        print(json.dumps({"python": py_surface(), "typescript": ts_surface()}, indent=1))
    if a.check:
        sys.exit(check())
    if a.markdown:
        markdown(a.markdown)
