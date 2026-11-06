# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Runner helpers for the shared-desktop tasks (SHARED-DESKTOP.md), used by every platform's runner.

* PA-01 (`"parallel": true` in task.json): N copies of BenchLab in `desk` mode, one agent each, started at the
  same moment. `pa_agents` plans the copies (label, frame, brief, artifact directory), `write_meta` records the
  agents' start for the checker, and `aggregate_agents` folds the N agents' results into one trial row.
* UM-01 (`"mouse_user": true`): one desk, and the sentinel's user moves and clicks the mouse from the script
  `write_mouse_script` writes.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "probes" / "_common"))
import benchlab_common as C  # noqa: E402

PARALLEL_META = "parallel.json"
# PA-01 foreground: the witness window sits below the grid (which ends at y = 900) so it covers no copy.
PA_WITNESS_FRAME = (1000, 905, 600, 120)


def is_parallel(spec: dict[str, Any]) -> bool:
    return bool(spec.get("parallel"))


def is_mouse_user(spec: dict[str, Any]) -> bool:
    return bool(spec.get("mouse_user"))


def instances_for(spec: dict[str, Any], override: int | None = None) -> int:
    n = int(override or spec.get("instances") or C.PA_DEFAULT_INSTANCES)
    if not 1 <= n <= C.PA_MAX_INSTANCES:
        raise ValueError(f"PA-01 instances must be 1..{C.PA_MAX_INSTANCES}, got {n}")
    return n


def _renderer(task_id: str) -> Any:
    path = HERE / "probes" / task_id / "render_brief.py"
    spec = importlib.util.spec_from_file_location(f"render_{task_id.replace('-', '_')}", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def desk_dir(root: Path, label: str) -> Path:
    return root / f"desk-{label}"


def pa_agents(
    seed: int,
    instances: int,
    root: Path,
    platform_words: tuple[str, str] | None = None,
) -> list[dict[str, Any]]:
    """One entry per copy, in cell order: label, title, frame, position, brief (before the condition's
    paragraph), its desk directory (state.json, events.jsonl) and the BenchLab arguments after `--mode desk`."""
    render = _renderer("PA-01")
    out = []
    for cell in C.derive_parallel(seed, instances)["cells"]:
        brief = render.render_cell(seed, cell["cell"], instances)
        if platform_words:
            brief = brief.replace(*platform_words)
        d = desk_dir(root, cell["label"])
        out.append(
            {
                "cell": cell["cell"],
                "label": cell["label"],
                "title": cell["title"],
                "frame": tuple(cell["frame"]),
                "position": cell["position"],
                "brief": brief,
                "dir": d,
                "state": d / "state.json",
                "events": d / "events.jsonl",
                "app_args": ["--seed", str(seed), "--state", str(d / "state.json"),
                             "--events", str(d / "events.jsonl"), *C.desk_app_args(cell)],
            }
        )  # fmt: skip
    return out


def um_desk(seed: int) -> dict[str, Any]:
    return C.derive_um_desk(seed)


def um_app_args(seed: int) -> list[str]:
    """BenchLab arguments for UM-01 after `--mode desk --seed S --state P --events E`."""
    return C.desk_app_args(C.derive_um_desk(seed))


def write_mouse_script(seed: int, path: Path) -> dict[str, Any]:
    return C.write_mouse_script(seed, path)


def write_meta(root: Path, instances: int, t0_ms: float, budget_s: float, **extra: Any) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    path = root / PARALLEL_META
    path.write_text(
        json.dumps(
            {"instances": instances, "t0_ms": t0_ms, "budget_s": budget_s, **extra}, indent=1
        )
        + "\n",
        "utf-8",
    )
    return path


def evaluate_argv(python: str, seed: int, root: Path, result: Path) -> list[str]:
    """The PA-01 checker's command line (it reads `root/parallel.json` for N, the start and the budget)."""
    return [python, str(HERE / "probes" / "PA-01" / "evaluate.py"), "--seed", str(seed),
            "--dir", str(root), "--result", str(result)]  # fmt: skip


def aggregate_agents(agents: list[dict[str, Any]]) -> dict[str, Any]:
    """Fold N agents into one trial: wall time is the longest agent's, turns, tool calls and cost add up, the
    trial timed out if any agent did, and an infrastructure failure of any agent makes the trial a re-run.
    Each input dict may carry `label`, `wall_s`, `turns`, `cost_usd`, `timed_out`, `returncode`, `status`,
    `infra` (failure kind or None) and `tokens`."""
    tokens: dict[str, int] = {}
    for a in agents:
        for k, v in (a.get("tokens") or {}).items():
            if isinstance(v, (int, float)):
                tokens[k] = tokens.get(k, 0) + int(v)
    costs = [a.get("cost_usd") for a in agents]
    infra = [a for a in agents if a.get("infra")]
    return {
        "agents": len(agents),
        "wall_s": max((float(a.get("wall_s") or 0.0) for a in agents), default=0.0),
        "turns": sum(int(a.get("turns") or 0) for a in agents),
        "max_turns_any": max((int(a.get("turns") or 0) for a in agents), default=0),
        "cost_usd": None if any(c is None for c in costs) else round(sum(costs), 6),
        "tokens": tokens,
        "timed_out": any(a.get("timed_out") for a in agents),
        "timed_out_agents": [a.get("label") for a in agents if a.get("timed_out")],
        "infra": infra[0].get("infra") if infra else None,
        "infra_agents": [a.get("label") for a in infra],
        "per_agent": [
            {
                k: a.get(k)
                for k in (
                    "label",
                    "wall_s",
                    "turns",
                    "cost_usd",
                    "timed_out",
                    "returncode",
                    "status",
                    "infra",
                )
            }
            for a in agents
        ],  # fmt: skip
    }


def parallel_of(evaluation: dict[str, Any]) -> dict[str, Any] | None:
    """The PA-01 checker's `parallel` diagnostic (instances, passed, cross_talk, makespan, throughput)."""
    diag = (evaluation.get("diagnostics") or {}).get("parallel")
    value = diag.get("value") if isinstance(diag, dict) else None
    return value if isinstance(value, dict) else None
