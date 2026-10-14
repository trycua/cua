#!/usr/bin/env python3
"""Run the cua SDK e2e suite and print the guide x language x lane matrix.

    tests/e2e/cua-sdk/run.py --lanes hermetic,container --langs py,ts,rust
    tests/e2e/cua-sdk/run.py --lanes fleet --strict        # nightly
    tests/e2e/cua-sdk/run.py --matrix-only --out DIR       # re-render results

Lanes map to the env gates in scenarios.json (the runner sets them). Every
language half appends {scenario, lang, lane, status, secs, reason} lines to
$CUA_E2E_RESULTS/<lang>.jsonl; this script renders them against the
registry and writes matrix.md + results.json.

--strict (CI): a requested lane may not *skip* (a skip in CI means a missing
dependency that would otherwise read as green, the practice the clean-room
test survey recommends), no expected cell may be missing, and every language
must record at least one result. Known bugs are `xfail`, never skips.

After each language, a janitor removes docker containers named
cua-e2e-<run>-* that a crashed test may have left behind. Fleet pools are
cleaned by the tests themselves (finally) and carry a TTL as a backstop.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path

SUITE = Path(__file__).resolve().parent
REPO = SUITE.parents[2]
GATES = {
    "container": {"CUA_E2E_CONTAINER": "1"},
    "qemu": {"CUA_E2E_QEMU": "1"},
    "lume": {"CUA_E2E_LUME": "1"},
    "fleet": {"CUA_E2E_FLEET": "1"},
    "fleet-env": {"CUA_E2E_FLEET": "1"},
    "cua-sandbox": {"CUA_E2E_CUA_SANDBOX": "1"},
    "conformance": {"CUA_E2E_CONFORMANCE": "1", "CUA_E2E_CONTAINER": "1"},
}
# Skips that stay legitimate under --strict (a precondition the lane itself
# cannot provide), matched as substrings of the recorded reason.
ALLOWED_SKIPS = (
    "CUA_E2E_FLEET_TAKEN_POOL",
    "CUA_E2E_FLEET_ENV_IMAGE",
    "needs a per-claim secret field (cloud PR)",
    "CUA_E2E_FLEET_PUSH_REPO is unset",
    "terraform (or tofu) is not installed",
    "@trycua/fleet is not built",
    "playwright is not installed",
    "Lume runs on macOS hosts only",
    # A guide input the reader supplies (e.g. OMARCHY_IMAGE) and CI does not.
    "docs block input",
    # The TypeScript Fleet guide page was removed upstream.
    "no Fleet TypeScript docs block is tagged",
)
STATUS_ICON = {
    "pass": "pass",
    "fail": "FAIL",
    "skip": "skip",
    "xfail": "xfail",
    "xpass": "XPASS",
    "missing": "—",
}


def sh(cmd: list[str], env: dict, cwd: Path, timeout: int) -> int:
    print(
        f"\n$ {' '.join(cmd)}  (cwd={cwd.relative_to(REPO) if cwd.is_relative_to(REPO) else cwd})",
        flush=True,
    )
    try:
        return subprocess.run(cmd, env=env, cwd=cwd, timeout=timeout).returncode
    except subprocess.TimeoutExpired:
        print(f"timed out after {timeout}s", flush=True)
        return 124


def janitor_targets(run: str, listing: str) -> list[str]:
    """Container ids (from `docker ps --format '{{.ID}} {{.Names}}'`) whose
    name starts with this run's own prefix. The run id must be a real
    per-run id, so a short or empty one can never widen the match to other
    runs' (or a user's) containers."""
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{5,39}", run):
        raise ValueError(f"janitor: refusing run id {run!r} (want 6 to 40 of [a-z0-9-])")
    prefix = f"cua-e2e-{run}-"
    ids = []
    for line in listing.splitlines():
        parts = line.split(maxsplit=1)
        if len(parts) == 2 and parts[1].lstrip("/").startswith(prefix):
            ids.append(parts[0])
    return ids


def janitor(run: str) -> None:
    if not shutil.which("docker"):
        return
    listing = subprocess.run(
        [
            "docker",
            "ps",
            "-a",
            "--filter",
            f"name=^/?cua-e2e-{run}-",
            "--format",
            "{{.ID}} {{.Names}}",
        ],
        capture_output=True,
        text=True,
    ).stdout
    ids = janitor_targets(run, listing)
    if ids:
        print(f"janitor: removing {len(ids)} leftover cua-e2e-{run}-* containers", flush=True)
        subprocess.run(["docker", "rm", "-f", *ids], capture_output=True)


def run_langs(langs: list[str], env: dict, timeout: int) -> dict[str, int]:
    codes = {}
    uv = shutil.which("uv")
    for lang in langs:
        if lang == "py":
            cmd = (
                [
                    uv,
                    "run",
                    "--no-project",
                    "--python",
                    "3.12",
                    "--with",
                    "pytest",
                    # The browse-the-web scenario is an MCP client.
                    "--with",
                    "mcp",
                    "pytest",
                    "-q",
                    "-rs",
                ]
                if uv
                else [sys.executable, "-m", "pytest", "-q", "-rs"]
            )
            codes[lang] = sh(cmd, env, SUITE / "python", timeout)
        elif lang == "ts":
            tests = sorted(str(p) for p in (SUITE / "typescript").glob("*.test.mjs"))
            codes[lang] = sh(
                ["node", "--test", "--test-concurrency=1", *tests],
                env,
                SUITE / "typescript",
                timeout,
            )
        elif lang == "rust":
            codes[lang] = sh(
                [
                    "cargo",
                    "test",
                    "--manifest-path",
                    str(SUITE / "rust" / "Cargo.toml"),
                    "--",
                    "--test-threads=2",
                ],
                env,
                SUITE / "rust",
                timeout,
            )
        elif lang == "go":
            codes[lang] = sh([str(SUITE / "go" / "terraform-smoke.sh")], env, SUITE / "go", timeout)
        else:
            raise SystemExit(f"unknown language {lang}")
        janitor(env["CUA_E2E_RUN"])
    return codes


def expected(sc: dict, lane: str, lang: str) -> bool:
    return lang in sc["langs"] and lang in sc.get("only", {}).get(lane, sc["langs"])


def load(out: Path) -> list[dict]:
    rows = []
    for f in sorted(out.glob("*.jsonl")):
        if f.name == "docs-blocks.jsonl":  # the docs coverage manifest (docs/coverage.py)
            continue
        for line in f.read_text().splitlines():
            if line.strip():
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    print(
                        f"warning: unreadable result line in {f.name}: {line[:120]}",
                        file=sys.stderr,
                    )
    return rows


def cell(results: list[dict]) -> tuple[str, float]:
    order = ["fail", "xpass", "pass", "xfail", "skip"]
    statuses = {r["status"] for r in results}
    worst = next((s for s in order if s in statuses), "missing")
    return worst, sum(r.get("secs", 0) for r in results)


def render(out: Path, lanes: list[str] | None) -> tuple[str, list[str]]:
    reg = json.loads((SUITE / "scenarios.json").read_text())
    rows = load(out)
    by = defaultdict(list)
    for r in rows:
        by[(r["scenario"], r["lane"], r["lang"])].append(r)
    all_langs = ["py", "ts", "rust", "go"]
    lines = [
        "| scenario (guide) | lane | " + " | ".join(all_langs) + " |",
        "|---|---|" + "---|" * len(all_langs),
    ]
    problems = []
    for sc in reg["scenarios"]:
        for lane in sc["lanes"]:
            if lanes and lane not in lanes and not (lane == "docs" and "hermetic" in lanes):
                continue
            cells = []
            for lang in all_langs:
                if not expected(sc, lane, lang):
                    cells.append("")
                    continue
                res = by.get((sc["id"], lane, lang), [])
                status, secs = cell(res)
                txt = STATUS_ICON[status] + (f" {secs:.0f}s" if res and status != "skip" else "")
                n = len(res)
                if n > 1:
                    txt += f" ({sum(r['status'] == 'pass' for r in res)}/{n})"
                cells.append(txt)
            lines.append(f"| {sc['id']} | {lane} | " + " | ".join(cells) + " |")
    # Per-test detail for anything that is not a plain pass.
    detail = [r for r in rows if r["status"] != "pass"]
    if detail:
        lines += [
            "",
            "| status | scenario | lane | lang | test | reason |",
            "|---|---|---|---|---|---|",
        ]
        for r in sorted(detail, key=lambda r: (r["status"], r["scenario"], r["lang"])):
            reason = str(r.get("reason", "")).replace("|", "/").replace("\n", " ")[:160]
            lines.append(
                f"| {r['status']} | {r['scenario']} | {r['lane']} | {r['lang']} | {r['test'][:70]} | {reason} |"
            )
    for r in rows:
        if r["status"] in ("fail", "xpass"):
            problems.append(f"{r['status']}: {r['scenario']}/{r['lane']}/{r['lang']}: {r['test']}")
    md = "\n".join(lines) + "\n"
    (out / "matrix.md").write_text(md)
    (out / "results.json").write_text(json.dumps(rows, indent=1))
    return md, problems


def strict_problems(out: Path, lanes: list[str], langs: list[str]) -> list[str]:
    reg = json.loads((SUITE / "scenarios.json").read_text())
    rows = load(out)
    probs = []
    for r in rows:
        if (
            r["lane"] in lanes
            and r["status"] == "skip"
            and not any(a in r.get("reason", "") for a in ALLOWED_SKIPS)
        ):
            probs.append(
                f"unexpected skip in requested lane: {r['scenario']}/{r['lane']}/{r['lang']}: {r['reason']}"
            )
    seen = {(r["scenario"], r["lane"], r["lang"]) for r in rows}
    for sc in reg["scenarios"]:
        for lane in sc["lanes"]:
            if lane not in lanes:
                continue
            for lang in sc["langs"]:
                if (
                    lang in langs
                    and expected(sc, lane, lang)
                    and (sc["id"], lane, lang) not in seen
                ):
                    probs.append(f"missing cell: {sc['id']}/{lane}/{lang}")
    for lang in langs:
        if not any(r["lang"] == lang for r in rows):
            probs.append(f"{lang} recorded no results (zero tests ran)")
    return probs


def docs_coverage(out: Path, lanes: list[str], langs: list[str]) -> list[str]:
    """Every docs block tagged for a lane that ran produced a passing result
    (docs/coverage.py). The docs lane runs with hermetic, as in render(); the
    docs runners live in the Python half."""
    if "py" not in langs:
        return []
    sys.path.insert(0, str(SUITE / "docs"))
    import coverage  # noqa: PLC0415

    ran = set(lanes) | ({"docs"} if "hermetic" in lanes else set())
    rows, problems = coverage.join(
        coverage.extract.all_blocks(),
        coverage.load_results(out),
        coverage.extract.load_policy(),
        ran,
    )
    (out / "docs-coverage.json").write_text(json.dumps(rows, indent=1) + "\n")
    if rows:
        table = coverage.summary(rows)
        (out / "docs-coverage.md").write_text(table)
        print(f"\n## docs code-block coverage\n\n{table}")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument(
        "--lanes",
        default="hermetic",
        help="comma list: hermetic,container,qemu,lume,fleet,fleet-env,cua-sandbox,conformance",
    )
    ap.add_argument("--langs", default="py,ts,rust", help="comma list: py,ts,rust,go")
    ap.add_argument(
        "--out", type=Path, default=None, help="results dir (default: a fresh temp dir)"
    )
    ap.add_argument("--run", default=os.environ.get("CUA_E2E_RUN") or secrets.token_hex(3))
    ap.add_argument("--timeout", type=int, default=5400, help="per-language timeout (s)")
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--matrix-only", action="store_true")
    a = ap.parse_args()
    lanes = [x for x in a.lanes.split(",") if x]
    langs = [x for x in a.langs.split(",") if x]
    out = (
        a.out
        or Path(os.environ.get("RUNNER_TEMP") or os.environ.get("TMPDIR") or "/tmp")
        / f"cua-e2e-{a.run}"
    )
    out.mkdir(parents=True, exist_ok=True)
    if not a.matrix_only:
        env = dict(os.environ, CUA_E2E_RUN=a.run, CUA_E2E_RESULTS=str(out))
        for lane in lanes:
            env.update(GATES.get(lane, {}))
        if not {"fleet", "fleet-env"} & set(lanes):
            # No live Fleet lane: never read a registry for the Fleet runtime
            # rule. Unreadable manifests fall back to the image reference.
            env.setdefault("CUA_FLEET_IMAGE_INSPECT", "0")
        t0 = time.monotonic()
        codes = run_langs(langs, env, a.timeout)
        print(f"\nexit codes: {codes} in {time.monotonic() - t0:.0f}s")
    md, problems = render(out, lanes)
    print(f"\n## cua SDK e2e matrix (run {a.run})\n\n{md}")
    problems += docs_coverage(out, lanes, langs)
    if a.strict:
        problems += strict_problems(out, lanes, langs)
    if problems:
        print("\n".join(["", "PROBLEMS:"] + problems))
        return 1
    if not a.matrix_only and any(c != 0 for c in codes.values()):
        return 1
    print(f"results: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
