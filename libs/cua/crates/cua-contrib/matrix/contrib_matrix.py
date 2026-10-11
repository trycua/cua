#!/usr/bin/env python3
"""The contrib providers' support matrix, measured by ``cua doctor``.

Each nightly lane creates a sandbox on one provider, runs ``cua doctor <ref>
--strict --json`` inside it and ingests the report here. The ledger keeps the
latest result per provider and kind (``ledger/<provider>-<kind>.json``); the
docs matrix is generated from the ledger, so a provider's verdict is what its
last doctor run measured, never hand-written, and a regression shows up in
the docs on the next run.

Verdicts:

* ``full``: the desktop works: display, input (cua-driver), streaming,
  files and auth all pass.
* ``container-only``: cua-spacesd answers (processes and files pass) but
  the desktop groups do not all pass.
* ``headless-only``: the sandbox ran but no cua-spacesd answered.
* ``failed``: the latest run could not create the sandbox.
* ``not measured``: no run yet (no key, or never scheduled).

Usage::

    contrib_matrix.py ingest --provider e2b --kind container --image IMG \\
        --report doctor.json [--sha SHA] [--run-url URL]
    contrib_matrix.py fail --provider e2b --kind container --image IMG --reason TEXT
    contrib_matrix.py skip --provider e2b --reason "E2B_API_KEY is not set"
    contrib_matrix.py render                      # the markdown table
    contrib_matrix.py write|check [--page MDX]    # the docs region

Stdlib only.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
LEDGER = HERE / "ledger"
REPO = HERE.parents[4]
PAGE = REPO / "docs" / "content" / "docs" / "cua-sdk" / "guides" / "contrib-providers.mdx"
REGION = "contrib-matrix"

#: Providers this cua implements, in display order, with the kinds each
#: lane measures.
PROVIDERS = {"e2b": ["container"], "daytona": ["container"], "modal": ["container"]}

#: Matrix columns: name -> doctor check groups.
GROUPS = {
    "display": ["screenshot"],
    "input": ["input", "driver"],
    "streaming": ["stream"],
    "audio": ["audio"],
    "files": ["files"],
    "process": ["process"],
    "teleport": ["teleport"],
    "tunnels": ["tunnel", "hotspot"],
    "mcp": ["mcp"],
    "auth": ["auth"],
}
DESKTOP = ("display", "input", "streaming", "files", "auth")
SYMBOL = {"pass": "pass", "fail": "FAIL", "skip": "skip", "n/a": "n/a"}


def group_status(checks: list[dict], groups: list[str]) -> str:
    """``fail`` if any check failed, ``pass`` if any passed, ``skip`` if all
    skipped, ``n/a`` when the report has none of these groups."""
    statuses = {c.get("status") for c in checks if c.get("group") in groups}
    if not statuses:
        return "n/a"
    if "fail" in statuses:
        return "fail"
    if "pass" in statuses or "warn" in statuses:
        return "pass"
    return "skip"


def guest_reached(checks: list[dict]) -> bool:
    """Whether cua-spacesd answered (no ``guest.available`` skip, and guest
    groups are present)."""
    if any(c.get("id") == "guest.available" and c.get("status") != "pass" for c in checks):
        return False
    return any(c.get("group") in ("meta", "capabilities", "process") for c in checks)


def summarize(report: dict) -> dict:
    """Per-column statuses and the verdict of one doctor report."""
    checks = report.get("checks") or []
    groups = {name: group_status(checks, members) for name, members in GROUPS.items()}
    reached = guest_reached(checks)
    return {"groups": groups, "guest": reached, "verdict": verdict(groups, reached)}


def verdict(groups: dict, reached: bool) -> str:
    if not reached:
        return "headless-only"
    if all(groups.get(g) == "pass" for g in DESKTOP):
        return "full"
    if groups.get("process") == "pass" and groups.get("files") == "pass":
        return "container-only"
    return "headless-only"


def _now() -> str:
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _entry_path(provider: str, kind: str) -> Path:
    return LEDGER / f"{provider}-{kind}.json"


def _write(path: Path, entry: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(entry, indent=2, sort_keys=True) + "\n")


def ingest(provider: str, kind: str, image: str, report: dict, **run: str) -> dict:
    s = summarize(report)
    entry = {
        "provider": provider,
        "kind": kind,
        "image": image,
        "outcome": "report",
        "verdict": s["verdict"],
        "guest": s["guest"],
        "groups": s["groups"],
        "summary": report.get("summary"),
        "run": {k: v for k, v in run.items() if v} | {"at": run.get("at") or _now()},
    }
    _write(_entry_path(provider, kind), entry)
    return entry


def fail(provider: str, kind: str, image: str, reason: str, **run: str) -> dict:
    entry = {
        "provider": provider,
        "kind": kind,
        "image": image,
        "outcome": "create-failed",
        "verdict": "failed",
        "reason": reason[:500],
        "run": {k: v for k, v in run.items() if v} | {"at": run.get("at") or _now()},
    }
    _write(_entry_path(provider, kind), entry)
    return entry


def load() -> dict[tuple[str, str], dict]:
    out = {}
    for f in sorted(LEDGER.glob("*.json")):
        e = json.loads(f.read_text())
        out[(e["provider"], e["kind"])] = e
    return out


def render(ledger: dict[tuple[str, str], dict] | None = None) -> str:
    ledger = load() if ledger is None else ledger
    cols = list(GROUPS)
    head = "| Provider | Kind | Verdict | " + " | ".join(cols) + " | Measured |"
    rule = "|" + "---|" * (len(cols) + 4)
    rows = [head, rule]
    for provider, kinds in PROVIDERS.items():
        for kind in kinds:
            e = ledger.get((provider, kind))
            if e is None:
                cells = ["-"] * len(cols)
                rows.append(
                    f"| {provider} | {kind} | not measured | " + " | ".join(cells) + " | never |"
                )
                continue
            run = e.get("run", {})
            when = run.get("at", "")[:10]
            link = f"[{when}]({run['url']})" if run.get("url") else when
            if e.get("outcome") != "report":
                cells = ["-"] * len(cols)
                rows.append(
                    f"| {provider} | {kind} | failed | " + " | ".join(cells) + f" | {link} |"
                )
                continue
            cells = [SYMBOL[e["groups"].get(c, "n/a")] for c in cols]
            rows.append(
                f"| {provider} | {kind} | {e['verdict']} | " + " | ".join(cells) + f" | {link} |"
            )
    return "\n".join(rows) + "\n"


_REGION = re.compile(
    r"(\{/\*\s*GENERATED:" + REGION + r":start\s*\*/\}\n)(.*?)(\{/\*\s*GENERATED:" + REGION
    + r":end\s*\*/\})",
    re.S,
)


def apply(page_text: str, table: str) -> str:
    if not _REGION.search(page_text):
        raise SystemExit(f"no GENERATED:{REGION} region in the page")
    return _REGION.sub(lambda m: m.group(1) + "\n" + table + "\n" + m.group(3), page_text)


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("ingest", "fail"):
        s = sub.add_parser(name)
        s.add_argument("--provider", required=True, choices=sorted(PROVIDERS))
        s.add_argument("--kind", default="container")
        s.add_argument("--image", required=True)
        s.add_argument("--sha", default="")
        s.add_argument("--run-url", default="")
        if name == "ingest":
            s.add_argument("--report", required=True, type=Path)
        else:
            s.add_argument("--reason", required=True)
    s = sub.add_parser("skip", help="print why a provider's lane did not run")
    s.add_argument("--provider", required=True)
    s.add_argument("--reason", required=True)
    sub.add_parser("render")
    for name in ("write", "check"):
        s = sub.add_parser(name)
        s.add_argument("--page", type=Path, default=PAGE)
    a = p.parse_args(argv)
    if a.cmd == "ingest":
        e = ingest(
            a.provider, a.kind, a.image, json.loads(a.report.read_text()),
            sha=a.sha, url=a.run_url,
        )
        print(f"{a.provider} {a.kind}: {e['verdict']}")
    elif a.cmd == "fail":
        fail(a.provider, a.kind, a.image, a.reason, sha=a.sha, url=a.run_url)
        print(f"{a.provider} {a.kind}: failed")
    elif a.cmd == "skip":
        # Skips keep the previous ledger entry; they are reported, not recorded.
        print(f"skipped {a.provider}: {a.reason}")
    elif a.cmd == "render":
        sys.stdout.write(render())
    else:
        text = a.page.read_text()
        want = apply(text, render())
        if a.cmd == "write":
            a.page.write_text(want)
        elif want != text:
            print(
                f"{a.page} is out of date with the doctor ledger; run "
                "libs/cua/crates/cua-contrib/matrix/contrib_matrix.py write",
                file=sys.stderr,
            )
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
