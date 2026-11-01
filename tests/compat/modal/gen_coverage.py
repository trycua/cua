#!/usr/bin/env python3
"""Coverage matrix of the public Modal Python SDK surface (pinned version).

    gen_coverage.py --symbols               # the surface of the installed `modal`
    gen_coverage.py --check                 # every symbol has a row; every named test exists
    gen_coverage.py --markdown > maps/modal-coverage.md

`coverage.json` maps symbols (and Sandbox.create parameters) to a status and
the tests that exercise them:

  implemented   the gateway serves it; the tests show the SDK-visible behaviour
  client-side   the SDK never sends it to the gateway; tests use it anyway
  native-error  the gateway answers with Modal's own error and a reason; a
                test asserts that error

Test ids: py:<suite>:<case> (tests/compat/modal/<suite>.py), js:<case>
(js/hermetic.mjs), jsfn:<case> (js/functions.mjs), go:<case> (go/hermetic.go),
rust:<test file>::<fn> or rust:src::<fn> (crates/cua-compat). A row's `lane`
says where its tests run: `mock` (every PR, over mock Spaces) or `space`
(real Spaces, nightly).
"""

from __future__ import annotations

import fnmatch
import inspect
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
CRATE = REPO / "libs/cua/crates/cua-compat"


def surface() -> list[str]:
    """Public symbols of the installed client."""
    import modal
    import modal.billing
    import modal.container_process as cp
    import modal.exception as ex
    import modal.experimental as exp
    import modal.io_streams as io
    import modal.sandbox._filesystem as fsm
    import modal.sandbox._sidecar as sc

    syms: set[str] = set()

    def members(o):
        return [m for m in dir(o) if not m.startswith("_")]

    for n in modal.__all__:
        if n == "__version__":
            continue
        o = getattr(modal, n)
        syms.add(n)
        if inspect.isclass(o) and n != "Error":
            syms.update(f"{n}.{m}" for m in members(o))
    for n, c in [
        ("ContainerProcess", cp.ContainerProcess),
        ("StreamReader", io.StreamReader),
        ("StreamWriter", io.StreamWriter),
    ]:
        syms.add(n)
        syms.update(f"{n}.{m}" for m in members(c) if m != "aio")
    syms.add("SandboxFilesystem")
    syms.update(f"SandboxFilesystem.{m}" for m in members(fsm._SandboxFilesystem) if m != "aio")
    for n, c in [
        ("SidecarManager", sc._SidecarManager),
        ("SidecarContainer", sc._SidecarContainer),
    ]:
        syms.add(n)
        syms.update(f"{n}.{m}" for m in members(c) if m != "aio")
    for m in dir(modal.Sandbox):
        if m.startswith("_experimental") or m == "_legacy_snapshot_filesystem":
            syms.add(f"Sandbox.{m}")
    for n, o in vars(ex).items():
        if inspect.isclass(o) and issubclass(o, BaseException) and not n.startswith("_"):
            syms.add(f"exception.{n}")
    for n in members(exp):
        o = getattr(exp, n)
        if n != "deprecation_warning" and (
            inspect.isfunction(o)
            or inspect.ismethod(o)
            or callable(o)
            and not inspect.isclass(o)
            and not inspect.ismodule(o)
        ):
            if getattr(o, "__module__", "").startswith("modal"):
                syms.add(f"experimental.{n}")
    syms.add("billing.workspace_billing_report")
    return sorted(syms)


def create_params() -> list[str]:
    import modal

    return sorted(
        p
        for p in inspect.signature(modal.Sandbox.create).parameters
        if p not in ("self", "args", "client")
    )


def load() -> dict:
    return json.loads((HERE / "coverage.json").read_text())


def covers(pattern: str, sym: str) -> bool:
    return pattern == sym or ("*" in pattern and fnmatch.fnmatchcase(sym, pattern))


def test_exists(t: str) -> bool:
    kind, _, rest = t.partition(":")
    if kind == "py":
        suite, _, case = rest.partition(":")
        text = (HERE / f"{suite}.py").read_text()
        return bool(re.search(rf'(check|case)\("{re.escape(case)}"|def {re.escape(case)}\(', text))
    if kind in ("js", "jsfn"):
        f = "hermetic.mjs" if kind == "js" else "functions.mjs"
        return f'check("{rest}"' in (HERE / "js" / f).read_text()
    if kind == "go":
        return f'check("{rest}"' in (HERE / "go/hermetic.go").read_text()
    if kind == "rust":
        file, _, fn = rest.partition("::")
        paths = (
            [CRATE / "tests" / f"{file}.rs"]
            if file != "src"
            else list((CRATE / "src").rglob("*.rs"))
        )
        return any(re.search(rf"fn {re.escape(fn)}\b", p.read_text()) for p in paths if p.exists())
    return False


def check(doc: dict) -> list[str]:
    bad: list[str] = []
    syms = surface()
    rows = doc["rows"]
    for i, r in enumerate(rows):
        for k in ("area", "symbols", "status", "tests", "lane", "notes"):
            if k not in r:
                bad.append(f"row {i} ({r.get('area')}): missing {k}")
        if r.get("status") not in ("implemented", "client-side", "native-error"):
            bad.append(f"row {i} ({r.get('area')}): bad status {r.get('status')!r}")
        if not r.get("tests"):
            bad.append(f"row {i} ({r.get('area')}): no tests")
        if r.get("status") == "native-error" and len(r.get("notes", "")) < 20:
            bad.append(
                f"row {i} ({r.get('area')}): a native-error row needs its justification in notes"
            )
        for t in r.get("tests", []):
            if not test_exists(t):
                bad.append(f"row {i} ({r.get('area')}): test {t} does not exist")
        for p in r.get("symbols", []):
            if not any(covers(p, s) for s in syms):
                bad.append(f"row {i} ({r.get('area')}): symbol {p} is not in the client")
    for s in syms:
        n = [r["area"] for r in rows if any(covers(p, s) for p in r["symbols"])]
        if not n:
            bad.append(f"symbol {s} has no row")
        elif len(n) > 1:
            exact = [r["area"] for r in rows if s in r["symbols"]]
            if len(exact) != 1:
                bad.append(f"symbol {s} is in several rows: {n}")
    params = create_params()
    have = {p["name"]: p for p in doc["create_params"]}
    for p in params:
        if p not in have:
            bad.append(f"Sandbox.create parameter {p} has no row")
    for p in have:
        if p not in params:
            bad.append(f"Sandbox.create parameter {p} is not in the client")
    for p in doc["create_params"]:
        for t in p["tests"]:
            if not test_exists(t):
                bad.append(f"parameter {p['name']}: test {t} does not exist")
        if p["status"] not in ("implemented", "native-error"):
            bad.append(f"parameter {p['name']}: bad status")
    return bad


def markdown(doc: dict) -> str:
    rows = doc["rows"]
    syms = surface()
    count = {"implemented": 0, "client-side": 0, "native-error": 0}
    for s in syms:
        for r in rows:
            if any(covers(p, s) for p in r["symbols"]):
                count[r["status"]] += 1
                break
    pc = {"implemented": 0, "native-error": 0}
    for p in doc["create_params"]:
        pc[p["status"]] += 1
    out = [
        "# Modal SDK coverage matrix",
        "",
        f"SDK: `modal` (Python) {doc['modal']}. Generated by `tests/compat/modal/gen_coverage.py --markdown`; "
        "`gen_coverage.py --check` fails when a public symbol of the pinned client has no row, a `Sandbox.create` "
        "parameter has no row, or a listed test does not exist. The workflow `ci-cua-compat-modal.yml` runs it.",
        "",
        f"{len(syms)} public symbols in {len(rows)} rows: {count['implemented']} implemented, "
        f"{count['client-side']} client-side (the SDK never sends them to the gateway), "
        f"{count['native-error']} answered with Modal's own error and a reason. "
        f"{len(doc['create_params'])} `Sandbox.create` parameters: {pc['implemented']} implemented, "
        f"{pc['native-error']} answered with an error.",
        "",
        "Tests: `py:` the Python SDK (`tests/compat/modal/<suite>.py`), `js:`/`jsfn:` the JavaScript SDK, `go:` the Go SDK, "
        "`rust:` a wire test of the gateway. Lanes: `mock` runs on every PR over mock Spaces; `space` runs on real Spaces "
        "(real Spaces, nightly). Statuses: implemented, client-side, native-error (the error is Modal's `UnimplementedError`, "
        "`InvalidError` or `NotFoundError` with the reason in its message; a test asserts it).",
        "",
        "## Public symbols",
        "",
        "| Area | Symbols | Status | Lane | Tests | Notes |",
        "|---|---|---|---|---|---|",
    ]
    for r in rows:
        sy = "<br>".join(f"`{s}`" for s in r["symbols"])
        te = "<br>".join(f"`{t}`" for t in r["tests"])
        out.append(f"| {r['area']} | {sy} | {r['status']} | {r['lane']} | {te} | {r['notes']} |")
    out += [
        "",
        "## `Sandbox.create` parameters",
        "",
        "| Parameter | Status | Tests | Behaviour |",
        "|---|---|---|---|",
    ]
    for p in doc["create_params"]:
        te = "<br>".join(f"`{t}`" for t in p["tests"])
        out.append(f"| `{p['name']}` | {p['status']} | {te} | {p['notes']} |")
    out.append("")
    return "\n".join(out)


def main() -> int:
    arg = sys.argv[1] if len(sys.argv) > 1 else "--check"
    if arg == "--symbols":
        print("\n".join(surface()))
        return 0
    if arg == "--params":
        print("\n".join(create_params()))
        return 0
    doc = load()
    if arg == "--markdown":
        print(markdown(doc))
        return 0
    bad = check(doc)
    for b in bad:
        print("FAIL", b)
    if not bad:
        syms = surface()
        print(
            f"ok: {len(syms)} symbols, {len(doc['rows'])} rows, {len(doc['create_params'])} parameters"
        )
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
