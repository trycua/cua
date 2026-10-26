"""The E2B coverage matrix: every public symbol of the Python and TypeScript
SDKs, and what the Cua gateway does with it.

The symbols come from the committed surface snapshots (surface.py for Python,
surface_ts.mjs for TypeScript, one per SDK version). `coverage.json` says, for
each symbol (by regular expression over a normalised `Class.member`), whether
it is `implemented` (with the cases that test it), `client` (the SDK does it
without calling a server), or `native-error` (the gateway answers with E2B's
own error and a documented reason). A symbol no rule covers fails the check:
that is how a method that ships in a new SDK release fails the weekly drift
job until someone decides what the gateway does with it.

    python coverage.py --check            # exit 1 on an unmapped symbol or a missing test
    python coverage.py --markdown OUT.md  # the matrix, for maps/e2b-coverage.md
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIX = HERE / "fixtures"


def snake(s: str) -> str:
    return re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", s).lower()


def vkey(v: str):
    return tuple(int(x) for x in re.findall(r"\d+", v))


def norm_class(name: str) -> str:
    """`e2b.AsyncSandbox` and `Sandbox` are one symbol."""
    name = name.removeprefix("e2b.")
    name = name.split(".")[-1] if name.count(".") > 0 else name
    for pre in ("Async",):
        if name.startswith(pre) and len(name) > len(pre) and name[len(pre)].isupper():
            name = name[len(pre) :]
    return name


KINDS: dict[str, str] = {}


def py_rows() -> dict[str, set[str]]:
    rows: dict[str, set[str]] = defaultdict(set)
    for f in sorted(FIX.glob("surface-[0-9]*.json"), key=lambda p: vkey(p.stem)):
        ver = f.stem.removeprefix("surface-")
        d = json.loads(f.read_text())
        for name, e in d.get("exports", {}).items():
            cls = norm_class(name)
            if e["kind"] in ("module",):
                continue
            rows[cls].add(ver)
            for m, sig in e.get("members", {}).items():
                if m.startswith("__") and m != "__init__":
                    continue
                rows[f"{cls}.{m}"].add(ver)
                KINDS.setdefault(f"{cls}.{m}", "method" if str(sig).startswith("(") else "field")
        for path, members in d.get("classes", {}).items():
            cls = norm_class(path)
            rows[cls].add(ver)
            for m in members:
                if not m.startswith("__"):
                    rows[f"{cls}.{m}"].add(ver)
        for route in d.get("routes", []):
            rows["route " + route].add(ver)
    return rows


def ts_rows() -> dict[str, set[str]]:
    rows: dict[str, set[str]] = defaultdict(set)
    for f in sorted(FIX.glob("surface-ts-*.json"), key=lambda p: vkey(p.stem)):
        ver = f.stem.removeprefix("surface-ts-")
        d = json.loads(f.read_text())
        for name, e in d["exports"].items():
            if name in ("components", "paths", "default"):
                if name == "paths":
                    for p in e.get("members", {}):
                        rows["route " + p].add(ver)
                continue
            rows[name].add(ver)
            for part in ("members", "static"):
                for m, sig in e.get(part, {}).items():
                    if m == "prototype":
                        continue
                    rows[f"{name}.{snake(m)}"].add(ver)
                    KINDS.setdefault(
                        f"{name}.{snake(m)}",
                        "method" if str(sig).lstrip().startswith(("(", "<")) else "field",
                    )
    return rows


def load_rules():
    d = json.loads((HERE / "coverage.json").read_text())
    rules = []
    for r in d["rules"]:
        r["_re"] = [
            re.compile(p) for p in (r["match"] if isinstance(r["match"], list) else [r["match"]])
        ]
        rules.append(r)
    return d, rules


def classify(sym: str, rules):
    for r in rules:
        if any(p.fullmatch(sym) for p in r["_re"]):
            return r
    return None


def case_names() -> set[str]:
    names: set[str] = set()
    for f in (HERE / "cases").glob("*.json"):
        names |= {c["name"] for c in json.loads(f.read_text())["cases"]}
    return names


def rust_tests() -> set[str]:
    out: set[str] = set()
    root = HERE.parents[4] / "libs/cua/crates/cua-compat"
    for f in list((root / "src").rglob("*.rs")) + list((root / "tests").rglob("*.rs")):
        out |= set(re.findall(r"fn\s+(\w+)\s*\(", f.read_text()))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--markdown")
    a = ap.parse_args()
    cfg, rules = load_rules()
    py, ts = py_rows(), ts_rows()
    symbols = sorted(set(py) | set(ts))
    cases, rust = case_names(), rust_tests()
    problems: list[str] = []
    table = []
    for sym in symbols:
        r = classify(sym, rules)
        if r is None:
            problems.append(
                f"unmapped: {sym}  (py {sorted(py.get(sym, []), key=vkey)[-1:]} ts {sorted(ts.get(sym, []), key=vkey)[-1:]})"
            )
            continue
        if r["status"] == "todo":
            problems.append(f"todo: {sym}")
        for t in r.get("tests", []):
            if t.startswith("rust:"):
                if t[5:] not in rust:
                    problems.append(f"{r['id']}: unknown Rust test {t}")
            elif t.startswith("conformance:"):
                pass
            elif t not in cases:
                problems.append(f"{r['id']}: unknown case {t}")
        if r["status"] == "implemented" and not r.get("tests"):
            problems.append(f"{r['id']}: implemented without a test")
        if r["status"] == "native-error" and not r.get("reason"):
            problems.append(f"{r['id']}: native-error without a reason")
        table.append((sym, r))
    if a.markdown:
        write_markdown(Path(a.markdown), cfg, table, py, ts)
    counts = defaultdict(int)
    for _, r in table:
        counts[r["status"]] += 1
    print(f"{len(symbols)} symbols: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    for p in problems:
        print("  " + p)
    return 1 if (a.check and problems) else 0


PRIMARY = {
    "Sandbox",
    "Commands",
    "Filesystem",
    "Pty",
    "Git",
    "CommandHandle",
    "CommandInputHandle",
    "WatchHandle",
    "SandboxPaginator",
    "SnapshotPaginator",
    "SecretPaginator",
    "Secret",
    "Volume",
    "Template",
    "TemplateBase",
    "TemplateBuilder",
    "ConnectionConfig",
}


def first_seen(sym: str, rows: dict[str, set[str]]) -> str:
    v = sorted(rows.get(sym, []), key=vkey)
    return v[0] if v else "-"


def write_markdown(path: Path, cfg: dict, table, py, ts) -> None:
    groups: dict[str, list] = defaultdict(list)
    for sym, r in table:
        groups[r["id"]].append(sym)
    by_id = {r["id"]: r for _, r in table}
    lines = [cfg["preamble"].rstrip(), ""]
    pyv = sorted({v for s in py.values() for v in s}, key=vkey)
    tsv = sorted({v for s in ts.values() for v in s}, key=vkey)
    lines.append(
        f"SDK versions covered: Python `e2b` {', '.join(pyv)}; npm `e2b` {', '.join(tsv)}. Sources: the installed wheels (`surface.py`) and the `.d.ts` of the npm packages (`surface_ts.mjs`), read as data."
    )
    lines.append("")
    lines.append("## Methods of the main classes")
    lines.append("")
    lines.append(
        "One row per method, with the first SDK version that has it (`py` Python sync and async, `ts` TypeScript; the Python `Async*` classes and the sync ones are one row). Names are Python's snake_case; the TypeScript method is the camelCase of the same name."
    )
    lines.append("")
    lines.append("| Method | py since | ts since | Status | Cases | Notes |")
    lines.append("|---|---|---|---|---|---|")
    rowsets = []
    for sym, r in table:
        cls = sym.split(".")[0]
        if (
            "." in sym
            and cls in PRIMARY
            and KINDS.get(sym) == "method"
            and not sym.endswith(".__init__")
        ):
            rowsets.append((sym, r))
    for sym, r in sorted(rowsets):
        tests = ", ".join(f"`{t}`" for t in r.get("tests", [])[:4]) + (
            " ..." if len(r.get("tests", [])) > 4 else ""
        )
        note = r.get("reason") or r.get("note", "")
        note = (note[:160] + "...") if len(note) > 160 else note
        lines.append(
            f"| `{sym}` | {first_seen(sym, py)} | {first_seen(sym, ts)} | {r['status']} | {tests or 'n/a'} | {note} |"
        )
    lines.append("")
    lines.append("## Everything else, by family")
    lines.append("")
    lines.append(
        "Option objects, result types, enums, exceptions, routes and members not listed above, grouped by the rule that covers them (`libs/cua/tests/compat/e2b/coverage.json`)."
    )
    lines.append("")
    sections: dict[str, list[str]] = defaultdict(list)
    for rid in groups:
        sections[by_id[rid].get("section", "Other")].append(rid)
    for sec in cfg["sections"]:
        ids = sections.get(sec, [])
        if not ids:
            continue
        lines.append(f"### {sec}")
        lines.append("")
        lines.append("| Rule | Symbols | Status | Cases | Notes |")
        lines.append("|---|---|---|---|---|")
        for rid in ids:
            r = by_id[rid]
            syms = groups[rid]
            tests = ", ".join(f"`{t}`" for t in r.get("tests", [])) or "n/a"
            note = r.get("reason") or r.get("note", "")
            lines.append(f"| `{rid}` | {len(syms)} | {r['status']} | {tests} | {note} |")
        lines.append("")
    path.write_text("\n".join(lines))


if __name__ == "__main__":
    sys.exit(main())
