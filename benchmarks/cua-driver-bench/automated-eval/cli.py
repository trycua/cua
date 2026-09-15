"""Command-line entry point for local Cua Driver release comparisons."""

from __future__ import annotations

import argparse
import shutil
import sys
from datetime import UTC, datetime
from pathlib import Path

from compare_drivers import (
    SHARED_TASKS,
    ComparisonConfig,
    build_plan,
    normalize_release_selection,
    normalize_tasks,
    resolve_platform,
    run_comparison,
)


def _executable(value: str) -> Path:
    candidate = Path(value).expanduser()
    if candidate.is_file():
        return candidate.resolve()
    resolved = shutil.which(value)
    if resolved is None:
        raise argparse.ArgumentTypeError(f"executable not found: {value}")
    return Path(resolved).resolve()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="compare_drivers",
        description="Run one or compare two local Cua Driver releases.",
        epilog=(
            "example: python automated-eval/compare_drivers --model large "
            "--reasoning-effort high --tasks-root /path/to/tasks --task CDB-S01"
        ),
    )
    parser.add_argument(
        "--baseline",
        help=(
            "baseline release directory name; pass without --candidate to run "
            "only this release (default comparison: 0.22.2 vs 0.23.2)"
        ),
    )
    parser.add_argument(
        "--candidate",
        help=(
            "candidate release directory name; pass without --baseline to run "
            "only this release (default comparison: 0.22.2 vs 0.23.2)"
        ),
    )
    parser.add_argument(
        "--task",
        action="append",
        dest="tasks",
        help="shared task ID; repeat to select multiple (default: all four)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help=(
            "new directory for reports and raw trial artifacts; it must not "
            "already exist (default: artifacts/automated-eval/<UTC timestamp>)"
        ),
    )
    parser.add_argument(
        "--platform",
        choices=("auto", "linux", "windows", "macos"),
        default="auto",
        help="task launch descriptor platform; auto selects the current host",
    )
    parser.add_argument(
        "--tasks-root",
        type=Path,
        required=True,
        help=("authorized task-pack directory containing shared/<task-id>/task.cuabench.json"),
    )
    parser.add_argument(
        "--drivers-root",
        type=Path,
        help=(
            "directory containing semantic-version release folders, manifests, "
            "and binaries (default: <repository>/cua-drivers)"
        ),
    )
    parser.add_argument(
        "--codex",
        type=_executable,
        default="codex",
        help="Codex CLI executable name on PATH or an explicit executable path",
    )
    parser.add_argument(
        "--codex-home",
        type=Path,
        help=(
            "source Codex config and authentication directory; trials create "
            "isolated temporary homes (default: ~/.codex)"
        ),
    )
    parser.add_argument(
        "--model",
        required=True,
        help="Codex model or configured provider tier, for example large",
    )
    parser.add_argument(
        "--reasoning-effort",
        choices=("low", "medium", "high"),
        default="high",
        help="reasoning effort passed to Codex (default: high)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=1800.0,
        help="maximum seconds allowed for each trial (default: 1800)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help=(
            "validate release and task selection and print the trial matrix "
            "without launching apps, drivers, Codex, or evaluators"
        ),
    )
    return parser


def _config(arguments: argparse.Namespace) -> ComparisonConfig:
    repo_root = Path(__file__).resolve().parents[1]
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    output = arguments.output or repo_root / "artifacts" / "automated-eval" / stamp
    codex_home = arguments.codex_home or Path.home() / ".codex"
    baseline, candidate = normalize_release_selection(arguments.baseline, arguments.candidate)
    return ComparisonConfig(
        repo_root=repo_root,
        tasks_root=arguments.tasks_root.expanduser().resolve(),
        drivers_root=(arguments.drivers_root or repo_root / "cua-drivers").resolve(),
        baseline=baseline,
        candidate=candidate,
        tasks=normalize_tasks(arguments.tasks or SHARED_TASKS),
        output=output.expanduser().resolve(),
        platform=resolve_platform(arguments.platform),
        codex=arguments.codex,
        codex_home=codex_home.expanduser().resolve(),
        model=arguments.model,
        reasoning_effort=arguments.reasoning_effort,
        timeout_seconds=arguments.timeout,
    )


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    try:
        arguments = parser.parse_args(argv)
        config = _config(arguments)
        if config.timeout_seconds <= 30:
            raise ValueError("timeout must be greater than 30 seconds")
        if arguments.dry_run:
            plan = build_plan(config)
            print(f"planned {len(plan['trials'])} diagnostic trials on {plan['platform']}")
            for trial in plan["trials"]:
                print(f"{trial['task']} | {trial['version']}")
            return 0
        report, json_path, markdown_path = run_comparison(config)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    passed = sum(1 for trial in report["trials"] if trial["passed"])
    print(f"completed {len(report['trials'])} diagnostic trials; {passed} passed")
    print(f"JSON: {json_path}")
    print(f"Markdown: {markdown_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
