#!/usr/bin/env python3
"""Run the cua SDK e2e suite and print the guide x language x lane matrix.

    tests/e2e/cua-sdk/run.py --lanes hermetic,container --langs py,ts,rust
    tests/e2e/cua-sdk/run.py --lanes cua-sandbox,qemu --strict  # nightly
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
cua-e2e-<run>-* that a crashed test may have left behind.

Sharded CI (`--shard I/N`, see sharding.py): a shard runs only its units and
checks only what it ran (failures, unexpected skips); the per-shard results
are concatenated and `--matrix-only --strict` then applies the full-suite
checks (every expected cell present, docs coverage) to the merged set.
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
sys.path.insert(0, str(SUITE))
import sharding  # noqa: E402

GATES = {
    "container": {"CUA_E2E_CONTAINER": "1"},
    "qemu": {"CUA_E2E_QEMU": "1"},
    "lume": {"CUA_E2E_LUME": "1"},
    "cua-sandbox": {"CUA_E2E_CUA_SANDBOX": "1"},
    "conformance": {"CUA_E2E_CONFORMANCE": "1", "CUA_E2E_CONTAINER": "1"},
}
# Skips that stay legitimate under --strict (a precondition the lane itself
# cannot provide), matched as substrings of the recorded reason.
ALLOWED_SKIPS = (
    "playwright is not installed",
    "Lume runs on macOS hosts only",
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


def py_command() -> list[str]:
    uv = shutil.which("uv")
    if not uv:
        return [sys.executable, "-m", "pytest", "-q", "-rs"]
    return [
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


def collect_py(env: dict, timeout: int) -> list[str]:
    """The node ids pytest would run, in collection order."""
    out = subprocess.run(
        [*py_command(), "--collect-only"],
        env=env,
        cwd=SUITE / "python",
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    ids = []
    for line in out.stdout.splitlines():
        if not line.strip():
            break  # the node ids end at the first blank line
        if re.match(r"[\w./-]+\.py::", line):
            ids.append(line.strip())
    if out.returncode != 0 or not ids:
        sys.stderr.write(out.stdout[-4000:] + out.stderr[-4000:])
        raise SystemExit(f"cannot collect the Python tests (exit {out.returncode})")
    return ids


def ts_files() -> list[str]:
    return sorted(p.name for p in (SUITE / "typescript").glob("*.test.mjs"))


def rust_binaries(prebuilt: Path) -> dict[str, str]:
    """Prebuilt integration-test binaries: name -> file, from the manifest
    ci-setup.sh --only rust-tests writes next to them."""
    manifest = prebuilt / "manifest.json"
    if not manifest.exists():
        raise SystemExit(f"no {manifest}: build the Rust tests with ci-setup.sh --only rust-tests")
    return {m["name"]: m["file"] for m in json.loads(manifest.read_text())}


def run_langs(
    langs: list[str],
    env: dict,
    timeout: int,
    *,
    select: dict[str, list[str]] | None = None,
    ts_concurrency: int = 1,
    rust_threads: int = 2,
    rust_prebuilt: Path | None = None,
    unit_secs: dict[str, float] | None = None,
) -> dict[str, int]:
    """Runs each language half. `select` (a shard) limits a half to its units;
    without it every test runs, as on a developer machine and in the nightly."""
    codes = {}
    unit_secs = {} if unit_secs is None else unit_secs
    for lang in langs:
        if select is not None and not select.get(lang):
            continue  # nothing of this half in this shard
        if lang == "py":
            run_env = env
            if select is not None:
                chosen = Path(env["CUA_E2E_RESULTS"]) / "py-select.txt"
                chosen.write_text("\n".join(select["py"]) + "\n")
                run_env = dict(env, CUA_E2E_SELECT=str(chosen))
            codes[lang] = sh(py_command(), run_env, SUITE / "python", timeout)
        elif lang == "ts":
            names = select["ts"] if select is not None else ts_files()
            code = 0
            for name in names:
                t0 = time.monotonic()
                rc = sh(
                    [
                        "node",
                        "--test",
                        f"--test-concurrency={ts_concurrency}",
                        str(SUITE / "typescript" / name),
                    ],
                    env,
                    SUITE / "typescript",
                    timeout,
                )
                unit_secs[f"ts:{name}"] = round(time.monotonic() - t0, 1)
                code = code or rc
            codes[lang] = code
        elif lang == "rust" and rust_prebuilt is not None:
            binaries = rust_binaries(rust_prebuilt)
            code = 0
            for name in select["rust"] if select is not None else sorted(binaries):
                t0 = time.monotonic()
                rc = sh(
                    [str(rust_prebuilt / binaries[name]), f"--test-threads={rust_threads}"],
                    env,
                    SUITE / "rust",
                    timeout,
                )
                unit_secs[f"rust:{name}"] = round(time.monotonic() - t0, 1)
                code = code or rc
            codes[lang] = code
        elif lang == "rust":
            codes[lang] = sh(
                [
                    "cargo",
                    "test",
                    "--manifest-path",
                    str(SUITE / "rust" / "Cargo.toml"),
                    "--",
                    f"--test-threads={rust_threads}",
                ],
                env,
                SUITE / "rust",
                timeout,
            )
        else:
            raise SystemExit(f"unknown language {lang}")
        janitor(env["CUA_E2E_RUN"])
    return codes


def plan_shard(
    langs: list[str], env: dict, timeout: int, spec: str, timings: dict, rust_prebuilt: Path | None
) -> tuple[dict[str, list[str]], int, float]:
    """This shard's units per language, the total unit count and the shard's
    predicted seconds. Every shard computes the same plan."""
    index, count = sharding.parse_shard(spec)
    units: list[sharding.Unit] = []
    if "py" in langs:
        units += [("py", n) for n in collect_py(env, timeout)]
    if "ts" in langs:
        units += [("ts", n) for n in ts_files()]
    if "rust" in langs:
        if rust_prebuilt is None:
            raise SystemExit("--shard needs --rust-prebuilt for the Rust half")
        units += [("rust", n) for n in rust_binaries(rust_prebuilt)]
    mine = sharding.assign(units, count, timings)[index - 1]
    chosen = {lang: [u for lg, u in mine if lg == lang] for lang in ("py", "ts", "rust")}
    return chosen, len(set(units)), sharding.predicted(mine, timings)


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
    all_langs = ["py", "ts", "rust"]
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


def skip_problems(rows: list[dict], lanes: list[str]) -> list[str]:
    return [
        f"unexpected skip in requested lane: {r['scenario']}/{r['lane']}/{r['lang']}: {r['reason']}"
        for r in rows
        if r["lane"] in lanes
        and r["status"] == "skip"
        and not any(a in r.get("reason", "") for a in ALLOWED_SKIPS)
    ]


def strict_problems(out: Path, lanes: list[str], langs: list[str]) -> list[str]:
    reg = json.loads((SUITE / "scenarios.json").read_text())
    rows = load(out)
    probs = skip_problems(rows, lanes)
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


def run_shard(
    a: argparse.Namespace, lanes: list[str], langs: list[str], env: dict, out: Path
) -> int:
    """One shard of the suite. Only this shard's results exist here, so the
    checks that need the whole matrix (missing cells, docs coverage) wait for
    the merge; failures and unexpected skips are checked now."""
    timings = sharding.load_timings(a.timings)
    chosen, total, expect = plan_shard(langs, env, a.timeout, a.shard, timings, a.rust_prebuilt)
    index, count = sharding.parse_shard(a.shard)
    print(
        f"\nshard {index}/{count}: "
        + ", ".join(f"{len(chosen[lang])} {lang}" for lang in ("py", "ts", "rust"))
        + f" of {total} units, predicted {expect:.0f}s",
        flush=True,
    )
    unit_secs: dict[str, float] = {}
    t0 = time.monotonic()
    codes = run_langs(
        langs,
        env,
        a.timeout,
        select=chosen,
        ts_concurrency=a.ts_concurrency,
        rust_threads=a.rust_threads,
        rust_prebuilt=a.rust_prebuilt,
        unit_secs=unit_secs,
    )
    wall = time.monotonic() - t0
    rows = load(out)
    problems = [
        f"{r['status']}: {r['scenario']}/{r['lane']}/{r['lang']}: {r['test']}"
        for r in rows
        if r["status"] in ("fail", "xpass")
    ]
    if a.strict:
        problems += skip_problems(rows, lanes)
    (out / "shard.json").write_text(
        json.dumps(
            {
                "shard": index,
                "of": count,
                "total_units": total,
                "units": chosen,
                "predicted_secs": round(expect, 1),
                "wall_secs": round(wall, 1),
                "unit_secs": unit_secs,
                "settings": {"ts_concurrency": a.ts_concurrency, "rust_threads": a.rust_threads},
            },
            indent=1,
        )
        + "\n"
    )
    print(f"\nshard {index}/{count} exit codes: {codes} in {wall:.0f}s (predicted {expect:.0f}s)")
    if problems:
        print("\n".join(["", "PROBLEMS:"] + problems))
        return 1
    return 1 if any(c != 0 for c in codes.values()) else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument(
        "--lanes",
        default="hermetic",
        help="comma list: hermetic,container,qemu,lume,cua-sandbox,conformance",
    )
    ap.add_argument("--langs", default="py,ts,rust", help="comma list: py,ts,rust")
    ap.add_argument(
        "--out", type=Path, default=None, help="results dir (default: a fresh temp dir)"
    )
    ap.add_argument("--run", default=os.environ.get("CUA_E2E_RUN") or secrets.token_hex(3))
    ap.add_argument("--timeout", type=int, default=5400, help="per-language timeout (s)")
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--matrix-only", action="store_true")
    ap.add_argument(
        "--shard",
        metavar="I/N",
        help="run only shard I of N (sharding.py); merge the shards' results, then run "
        "`--matrix-only --strict` on the merge for the full-suite checks",
    )
    ap.add_argument(
        "--timings", type=Path, default=sharding.TIMINGS, help="unit weights for --shard"
    )
    ap.add_argument(
        "--rust-prebuilt",
        type=Path,
        help="directory of prebuilt Rust test binaries + manifest.json (ci-setup.sh --only "
        "rust-tests); run instead of `cargo test`",
    )
    ap.add_argument(
        "--ts-concurrency",
        type=int,
        default=int(os.environ.get("CUA_E2E_TS_CONCURRENCY", "1")),
        help="node --test-concurrency (files run one at a time by default)",
    )
    ap.add_argument(
        "--rust-threads",
        type=int,
        default=int(os.environ.get("CUA_E2E_RUST_THREADS", "2")),
        help="--test-threads of the Rust test binaries",
    )
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
        if a.shard:
            return run_shard(a, lanes, langs, env, out)
        t0 = time.monotonic()
        codes = run_langs(
            langs,
            env,
            a.timeout,
            ts_concurrency=a.ts_concurrency,
            rust_threads=a.rust_threads,
            rust_prebuilt=a.rust_prebuilt,
        )
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
