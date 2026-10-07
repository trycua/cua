"""Command-line entry point for local Cua Driver release comparisons."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import sys
from datetime import UTC, datetime
from pathlib import Path

from dotenv import load_dotenv

from compare_drivers import (
    SHARED_TASKS,
    ComparisonConfig,
    build_plan,
    normalize_release_selection,
    normalize_tasks,
    resolve_platform,
    run_comparison,
)
from s3_publish import publish_report


def _executable(value: str) -> Path:
    candidate = Path(value).expanduser()
    if candidate.is_file():
        return candidate.resolve()
    resolved = shutil.which(value)
    if resolved is None:
        raise argparse.ArgumentTypeError(f"executable not found: {value}")
    return Path(resolved).resolve()


def _positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("value must be at least 1")
    return parsed


def build_parser(*, fleet: bool = False) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="compare_drivers fleet" if fleet else "compare_drivers",
        description=(
            "Run one or compare two Cua Driver releases on isolated Fleet workers. "
            "Each active task shard uses one worker. Fleet provisions the "
            "supported external Linux apps required by the "
            "selected task descriptors."
            if fleet
            else "Run one or compare two local Cua Driver releases."
        ),
        epilog=(
            "example: python automated-eval/cli.py fleet --model small "
            "--reasoning-effort high --tasks-root tasks --baseline 0.28.0 "
            "--candidate 0.26.1 --task CDB-S01 --task CDB-S04"
            if fleet
            else "example: python automated-eval/compare_drivers --model large "
            "--reasoning-effort high --tasks-root tasks --task CDB-S01"
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
        "--tasks-root",
        type=Path,
        required=True,
        help=(
            "authorized task-pack root containing shared/<task-id> directories; "
            "task files are not assumed to exist in the source checkout"
        ),
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
        "--max-parallel-tasks",
        type=_positive_int,
        default=2 if fleet else 1,
        help=(
            "maximum task shards to run concurrently (default: 2; use 1 for serial)"
            if fleet
            else (
                "maximum local task shards to run concurrently; parallel runs "
                "require one --local-display for each active shard (default: 1)"
            )
        ),
    )
    if not fleet:
        parser.add_argument(
            "--local-display",
            action="append",
            dest="local_displays",
            metavar="DISPLAY",
            help=(
                "isolated Linux/X11 display for a local task shard; repeat the "
                "option for parallel local runs"
            ),
        )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help=(
            "validate release and task selection and print the trial matrix "
            + (
                "without claiming or provisioning a Fleet worker"
                if fleet
                else "without launching apps, drivers, Codex, or evaluators"
            )
        ),
    )
    parser.add_argument(
        "--publish",
        action="store_true",
        help="publish the generated static report bundle to the configured S3 bucket",
    )
    return parser


def build_publish_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="compare_drivers publish",
        description="Publish an existing static benchmark report bundle to S3.",
    )
    parser.add_argument(
        "--run-dir",
        type=Path,
        required=True,
        help="benchmark run containing comparison files and report/index.html",
    )
    return parser


def _config(arguments: argparse.Namespace, *, fleet: bool = False) -> ComparisonConfig:
    repo_root = Path(__file__).resolve().parents[1]
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    default_output = (
        repo_root / "automated-eval" / "fleet-results" / stamp
        if fleet
        else repo_root / "artifacts" / "automated-eval" / stamp
    )
    output = arguments.output or default_output
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
        platform=(
            _fleet_platform(arguments.platform) if fleet else resolve_platform(arguments.platform)
        ),
        codex=arguments.codex,
        codex_home=codex_home.expanduser().resolve(),
        model=arguments.model,
        reasoning_effort=arguments.reasoning_effort,
        timeout_seconds=arguments.timeout,
        max_parallel_tasks=arguments.max_parallel_tasks,
        local_displays=tuple(getattr(arguments, "local_displays", None) or ()),
    )


def _fleet_platform(value: str) -> str:
    if value not in {"auto", "linux"}:
        raise ValueError("Fleet evaluation currently supports only Linux/X11")
    return "linux"


def _load_environment() -> None:
    repo_root = Path(__file__).resolve().parents[1]
    load_dotenv(repo_root / ".env")
    load_dotenv(repo_root / "automated-eval" / ".env")


def _command_mode(argv: list[str] | None) -> tuple[str, list[str]]:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments[:1] and arguments[0] in {"compare", "fleet", "publish"}:
        return arguments[0], arguments[1:]
    return "compare", arguments


def _publish_run(run_dir: Path) -> tuple[str, str]:
    bucket = os.environ.get("AWS_S3_BUCKET", "").strip()
    if not bucket:
        raise ValueError("AWS_S3_BUCKET is required to publish a report")
    prefix = os.environ.get("AWS_S3_REPORT_PREFIX", "cua-driver-bench")
    profile = os.environ.get("AWS_PROFILE") or None
    region = os.environ.get("AWS_REGION") or os.environ.get("AWS_DEFAULT_REGION") or None
    report_base_url = os.environ.get("CDB_REPORT_BASE_URL", "https://bench.trycua.com")
    return publish_report(
        run_dir,
        bucket=bucket,
        prefix=prefix,
        profile=profile,
        region=region,
        report_base_url=report_base_url,
    )


def _result_exit_code(report: dict[str, object], *, fleet: bool) -> int:
    execution = report.get("execution")
    if isinstance(execution, dict) and execution.get("complete") is False:
        return 2
    return 0


def main(argv: list[str] | None = None) -> int:
    _load_environment()
    mode, arguments_list = _command_mode(argv)
    if mode == "publish":
        parser = build_publish_parser()
        try:
            arguments = parser.parse_args(arguments_list)
            s3_uri, report_url = _publish_run(arguments.run_dir)
        except (OSError, RuntimeError, ValueError) as error:
            parser.error(str(error))
        print(f"S3_URI={s3_uri}")
        print(f"REPORT_URL={report_url}")
        return 0

    fleet_mode = mode == "fleet"
    parser = build_parser(fleet=fleet_mode)
    try:
        arguments = parser.parse_args(arguments_list)
        config = _config(arguments, fleet=fleet_mode)
        if config.timeout_seconds <= 30:
            raise ValueError("timeout must be greater than 30 seconds")
        if arguments.dry_run:
            if arguments.publish:
                raise ValueError("--publish cannot be used with --dry-run")
            plan = build_plan(config)
            print(f"planned {len(plan['trials'])} diagnostic trials on {plan['platform']}")
            for trial in plan["trials"]:
                print(f"{trial['task']} | {trial['version']}")
            return 0
        if fleet_mode:
            from fleet import run_on_fleet

            json_path, markdown_path = asyncio.run(run_on_fleet(config))
            report = json.loads(json_path.read_text(encoding="utf-8"))
        else:
            report, json_path, markdown_path = run_comparison(config)
    except (OSError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    passed = sum(1 for trial in report["trials"] if trial["passed"])
    print(f"completed {len(report['trials'])} diagnostic trials; {passed} passed")
    print(f"JSON: {json_path}")
    print(f"Markdown: {markdown_path}")
    print(f"HTML: {json_path.parent / 'report' / 'index.html'}")
    if arguments.publish:
        try:
            s3_uri, report_url = _publish_run(json_path.parent)
        except (OSError, RuntimeError, ValueError) as error:
            parser.error(str(error))
        print(f"S3_URI={s3_uri}")
        print(f"REPORT_URL={report_url}")
    return _result_exit_code(report, fleet=fleet_mode)


if __name__ == "__main__":
    raise SystemExit(main())
