#!/usr/bin/env python3
"""docs-coverage: join the docs block inventory with what the runners ran.

Every runner appends ``{block_id, page, line, lane, lang, status, test}`` to
``$CUA_E2E_RESULTS/docs-blocks.jsonl`` (the Python half through
../python/conftest.py). This step fails when a ``test=`` block that belongs to
a requested lane has no result, or only failing ones, so a tagged block can
never be silently dropped. Lanes map to the e2e lane that runs them through
``docs/code-block-policy.json`` (``lanes.<name>.e2e``).

    coverage.py --results DIR --lanes docs[,fleet] [--summary FILE]

Writes ``docs-coverage.json`` into DIR and a per-page Markdown table to
``--summary`` (e.g. $GITHUB_STEP_SUMMARY). Stdlib only.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

import extract

OK = {"pass", "xfail"}


def e2e_lane(policy: dict, lane: str) -> str | None:
    spec = policy["lanes"].get(lane)
    if isinstance(spec, dict):
        return spec.get("e2e")
    return None


def load_results(results: Path) -> list[dict]:
    path = results / "docs-blocks.jsonl"
    if not path.exists():
        return []
    return [
        json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()
    ]


def join(
    blocks: list[extract.Block], results: list[dict], policy: dict, lanes: set[str]
) -> tuple[list[dict], list[str]]:
    """Returns (rows, problems): one row per (block, docs lane) that a requested
    e2e lane owns."""
    by_id: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for r in results:
        by_id[(r["block_id"], r["lane"])].append(r)
    rows, problems = [], []
    for b in blocks:
        for lane in b.lanes:
            owner = e2e_lane(policy, lane)
            if owner is None or owner not in lanes:
                continue
            got = by_id.get((b.id, owner), [])
            statuses = sorted({r["status"] for r in got})
            if not got:
                status = "missing"
            elif "fail" in statuses or "xpass" in statuses:
                status = "fail"
            elif set(statuses) & OK:
                status = "pass" if "pass" in statuses else "xfail"
            else:
                status = "skip"
            rows.append(
                {
                    "block_id": b.id,
                    "page": b.guide,
                    "line": b.line,
                    "lang": b.lang,
                    "docs_lane": lane,
                    "lane": owner,
                    "status": status,
                    "tests": sorted({r["test"] for r in got}),
                    "reason": next((r.get("reason", "") for r in got if r.get("reason")), ""),
                }
            )
            if status == "missing":
                problems.append(f"docs block {b.id} ({lane}) produced no result in lane {owner}")
            elif status == "fail":
                problems.append(f"docs block {b.id} ({lane}) failed in lane {owner}")
    return rows, problems


def summary(rows: list[dict]) -> str:
    pages: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for r in rows:
        pages[r["page"]][r["status"]] += 1
    cols = ["pass", "xfail", "skip", "fail", "missing"]
    out = ["| page | " + " | ".join(cols) + " |", "|---|" + "---|" * len(cols)]
    for page in sorted(pages):
        out.append(f"| {page} | " + " | ".join(str(pages[page].get(c, 0)) for c in cols) + " |")
    return "\n".join(out) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--results", type=Path, required=True)
    ap.add_argument("--lanes", required=True, help="comma list of e2e lanes that ran")
    ap.add_argument("--docs", type=Path, default=extract.DOCS)
    ap.add_argument("--policy", type=Path, default=extract.POLICY)
    ap.add_argument("--summary", type=Path)
    a = ap.parse_args()
    lanes = {x for x in a.lanes.split(",") if x}
    rows, problems = join(
        extract.all_blocks(a.docs), load_results(a.results), extract.load_policy(a.policy), lanes
    )
    a.results.mkdir(parents=True, exist_ok=True)
    (a.results / "docs-coverage.json").write_text(
        json.dumps(rows, indent=1) + "\n", encoding="utf-8"
    )
    table = summary(rows)
    if a.summary:
        with a.summary.open("a", encoding="utf-8") as f:
            f.write("## Docs code-block coverage\n\n" + table)
    print(table, end="")
    if problems:
        print("docs coverage:", *problems, sep="\n  ", file=sys.stderr)
        return 1
    print(f"docs coverage OK: {len(rows)} block run(s) in lanes {','.join(sorted(lanes))}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
