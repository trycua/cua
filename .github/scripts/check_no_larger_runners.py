#!/usr/bin/env python3
"""Fail when a workflow asks for a paid GitHub larger runner.

Standard GitHub-hosted runners are free on public repositories; larger runners
are billed there too (macOS xlarge, Linux 16-core and Windows 8-core cost $3.4k
on trycua/cua in October 2026). This check reads every `runs-on` in
.github/workflows, resolves `matrix.*` and `inputs.*` references to their
values, and fails on:

  * labels ending in -large / -xlarge (macos-26-xlarge, macos-latest-large)
  * labels naming a core count (ubuntu-latest-8-cores, cua-linux-16core)
  * `group:` runners (custom larger-runner groups)

Self-hosted labels that do not look like those pass. Opt-outs live in
.github/larger-runners-allowed.txt (starts empty).

Usage: check_no_larger_runners.py [--list] [repo_root]
  --list   print every workflow that uses a larger runner, even if allowed
"""

from __future__ import annotations

import re
import shutil
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # e.g. a host whose system Python has no PyYAML (g17-4)
    import os

    if os.environ.get("CUA_LARGER_RUNNERS_REEXEC") or not shutil.which("uv"):
        sys.exit("check_no_larger_runners.py needs PyYAML: pip install pyyaml (or install uv)")
    os.environ["CUA_LARGER_RUNNERS_REEXEC"] = "1"
    os.execvp("uv", ["uv", "run", "--quiet", "--with", "pyyaml", "python", *sys.argv])

LARGER = re.compile(r"(?:^|-)(?:x{0,2}large)$|\d+-?cores?(?:$|-)", re.IGNORECASE)
LITERAL = re.compile(r"'([^']*)'|\"([^\"]*)\"")
REF = re.compile(r"\b(matrix|inputs|vars)\.([A-Za-z0-9_-]+)")


def is_larger(label: str) -> bool:
    return bool(LARGER.search(label.strip()))


def load_allowed(root: Path) -> set[tuple[str, str]]:
    path = root / ".github" / "larger-runners-allowed.txt"
    allowed: set[tuple[str, str]] = set()
    if not path.exists():
        return allowed
    for raw in path.read_text().splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) != 2:
            raise SystemExit(f"{path}: expected '<workflow> <label>', got: {raw!r}")
        allowed.add((parts[0], parts[1]))
    return allowed


def _strings(value) -> list[str]:
    """Every string reachable inside a YAML value."""
    if isinstance(value, str):
        return [value]
    if isinstance(value, (int, float, bool)) or value is None:
        return []
    if isinstance(value, dict):
        return [s for v in value.values() for s in _strings(v)]
    if isinstance(value, list):
        return [s for v in value for s in _strings(v)]
    return []


def _matrix_values(matrix, key: str) -> list[str]:
    if not isinstance(matrix, dict):
        return []
    out: list[str] = []
    if key in matrix:
        out += _strings(matrix[key])
    for entry in matrix.get("include") or []:
        if isinstance(entry, dict) and key in entry:
            out += _strings(entry[key])
    if isinstance(matrix.get("include"), str):  # fromJSON(...) literal
        out += [s for s in LITERAL_ALL(matrix["include"]) if s]
    return out


def LITERAL_ALL(text: str) -> list[str]:
    """Strings inside an expression, including JSON embedded in fromJSON('...')."""
    found = [a or b for a, b in LITERAL.findall(text)]
    extra: list[str] = []
    for chunk in found:
        extra += re.findall(r'"(?:os|runner|runs-on|platform)"\s*:\s*"([^"]+)"', chunk)
    return found + extra


def labels_for(workflow: dict, job: dict) -> list[str]:
    """Candidate runner labels of a job: literals plus referenced matrix/input values."""
    runs_on = job.get("runs-on")
    if runs_on is None:
        return []
    labels: list[str] = []
    if isinstance(runs_on, dict):
        if "group" in runs_on:
            labels.append(f"group:{runs_on['group']}")
        runs_on = runs_on.get("labels", [])
    texts = _strings(runs_on)
    matrix = (job.get("strategy") or {}).get("matrix")
    # `on` parses as boolean True in YAML 1.1.
    triggers = workflow.get("on", workflow.get(True)) or {}
    inputs: dict = {}
    if isinstance(triggers, dict):
        for kind in ("workflow_call", "workflow_dispatch"):
            body = triggers.get(kind)
            if isinstance(body, dict) and isinstance(body.get("inputs"), dict):
                inputs.update(body["inputs"])
    for text in texts:
        if "${{" not in text:
            labels.append(text)
            continue
        labels += [s for s in LITERAL_ALL(text) if s]
        for scope, name in REF.findall(text):
            if scope == "matrix":
                labels += _matrix_values(matrix, name)
            elif scope == "inputs":
                spec = inputs.get(name)
                if isinstance(spec, dict):
                    labels += _strings(spec.get("default")) + _strings(spec.get("options"))
    return labels


def find_uses(root: Path) -> list[tuple[str, str, str]]:
    """(workflow file name, job id, label) for every larger-runner label."""
    uses: list[tuple[str, str, str]] = []
    for path in sorted((root / ".github" / "workflows").glob("*.y*ml")):
        try:
            workflow = yaml.safe_load(path.read_text())
        except yaml.YAMLError as err:
            raise SystemExit(f"{path}: cannot parse: {err}")
        if not isinstance(workflow, dict):
            continue
        for job_id, job in (workflow.get("jobs") or {}).items():
            if not isinstance(job, dict):
                continue
            for label in labels_for(workflow, job):
                if label.startswith("group:") or is_larger(label):
                    uses.append((path.name, job_id, label))
    return uses


def main(argv: list[str]) -> int:
    list_only = "--list" in argv
    args = [a for a in argv if not a.startswith("--")]
    root = Path(args[0]) if args else Path(__file__).resolve().parents[2]
    allowed = load_allowed(root)
    uses = find_uses(root)
    if list_only:
        for wf, job, label in uses:
            print(f"{wf}\t{job}\t{label}")
        return 0
    bad = [u for u in uses if (u[0], u[2]) not in allowed]
    for wf, job, label in bad:
        print(f"::error file=.github/workflows/{wf}::job '{job}' uses larger runner '{label}'")
        print(f"FAIL {wf} job {job}: larger runner '{label}' is billed even on public repos; "
              "use ubuntu-latest, macos-26, windows-latest (see .github/larger-runners-allowed.txt)")
    if bad:
        return 1
    print(f"ok: no larger runners ({len(uses)} allowed)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
