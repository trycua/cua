"""Main CLI entry point for cua-bench."""

import argparse
import atexit
import sys
from importlib import metadata as _metadata

from .commands._help import examples
from .commands import (
    agent,
    dataset,
    env,
    image,
    interact,
    login,
    platform,
    prune,
    run,
    status,
    task,
    trace,
)

# Telemetry imports
try:
    from cua_bench.telemetry import (
        flush_telemetry,
        track_command_invoked,
    )

    _telemetry_available = True
except ImportError:
    _telemetry_available = False


def _get_version() -> str:
    try:
        return _metadata.version("cua_bench")
    except Exception:
        return "dev"


def print_banner() -> None:
    """Print the koala banner in white with a grey version suffix."""
    white = "\033[97m"
    grey = "\033[90m"
    reset = "\033[0m"
    ver = _get_version()
    lines = [
        "",
        "    ⠀⣀⣀⡀⠀⠀⠀⠀⢀⣀⣀⣀⡀⠘⠋⢉⠙⣷⠀⠀ ⠀ ",
        " ⠀⠀⢀⣴⣿⡿⠋⣉⠁⣠⣾⣿⣿⣿⣿⡿⠿⣦⡈⠀⣿⡇⠃⠀",
        " ⠀⠀⠀⣽⣿⣧⠀⠃⢰⣿⣿⡏⠙⣿⠿⢧⣀⣼⣷⠀⡿⠃⠀⠀ ",
        " ⠀⠀⠀⠉⣿⣿⣦⠀⢿⣿⣿⣷⣾⡏⠀⠀⢹⣿⣿⠀⠀⠀⠀⠀⠀",
        " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁⠀" + white + "cua-bench" + grey + f"==v{ver}" + reset,
        "           " + grey + "toolkit for computer-use RL environments and benchmarks",
        "",
    ]
    for i, line in enumerate(lines):
        if i < len(lines) - 1:
            print(white + line + reset)
        else:
            # Last line already includes grey segment and reset
            print(white + line + reset)
    # flush stdout
    import sys

    sys.stdout.flush()


RUN_SUBCOMMANDS = ("task", "dataset", "list", "info", "watch", "stop", "logs")

#: Every exit status ``cb`` returns (documented in the generated CLI reference).
EXIT_CODES = (
    (0, "Success: every variant completed"),
    (1, "Failure: a variant failed or was cancelled, or the command could not run"),
    (2, "Usage error: an unknown command, flag or value"),
    (130, "Interrupted (Ctrl-C); running variants are cancelled"),
)


def normalize_argv(argv: list[str]) -> list[str]:
    """``cb run <path> ...`` means ``cb run task|dataset <path> ...``.

    A directory with a ``main.py`` is a task; anything else (a directory of
    tasks or a registry dataset name) is a dataset.
    """
    from pathlib import Path

    if len(argv) >= 2 and argv[0] == "run":
        first = argv[1]
        if first not in RUN_SUBCOMMANDS and not first.startswith("-"):
            kind = "task" if (Path(first) / "main.py").exists() else "dataset"
            return ["run", kind, *argv[1:]]
    return list(argv)


def build_parser() -> argparse.ArgumentParser:
    """The full ``cb`` argument parser (no side effects; used by the CLI goldens)."""
    parser = argparse.ArgumentParser(
        prog="cb",
        description="cua-bench: run computer-use tasks and benchmark datasets on local and cloud sandboxes",
        **examples(
            ("Run a task with its oracle solution", "cb run ./tasks/hello_file_env"),
            ("List the tasks in a directory", "cb task list ./tasks"),
        ),
    )
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # run (top-level)
    run_parser = subparsers.add_parser(
        "run",
        help="Run a task or dataset, and manage runs",
        description="Run a task (a directory with main.py) or a dataset, with an agent or the "
        "task's oracle. `cb run <path>` is shorthand for `cb run task|dataset <path>`.",
        **examples(
            ("Run a task with its oracle solution", "cb run ./tasks/hello_file_env"),
            (
                "Run a dataset with an agent, four variants at once",
                "cb run cua-bench-basic --agent cua-agent "
                "--model anthropic/claude-sonnet-4-20250514 -j 4",
            ),
            ("List runs", "cb run list"),
        ),
    )
    run_subparsers = run_parser.add_subparsers(dest="run_command", help="Run command")

    # Shared arguments for task and dataset subcommands
    def add_common_run_args(parser):
        """Agent, target and output arguments shared by task and dataset."""
        parser.add_argument("--agent", help="Agent to use for evaluation (e.g., cua-agent)")
        parser.add_argument(
            "--agent-import-path",
            dest="agent_import_path",
            help='Import path for custom agent (e.g., "path.to.agent:MyCustomAgent")',
        )
        parser.add_argument(
            "--model", help="Model to use with the agent (e.g., anthropic/claude-sonnet-4-20250514)"
        )
        parser.add_argument(
            "--oracle",
            action="store_true",
            help="Run the oracle solution (the default when no agent is given)",
        )
        parser.add_argument(
            "--noop",
            action="store_true",
            help="Set up and evaluate with no actions (the null baseline for eval parity)",
        )
        parser.add_argument(
            "--max-steps",
            dest="max_steps",
            type=int,
            default=100,
            help="Maximum number of steps for agent execution (default: 100)",
        )
        run.add_target_args(parser)
        parser.add_argument(
            "--output-dir", dest="output_dir", help="Output directory for session results"
        )
        parser.add_argument(
            "--detach",
            "-d",
            action="store_true",
            help="Run in the background and return (follow with cb run watch <id>)",
        )
        parser.add_argument(
            "--attempts",
            type=int,
            default=1,
            help="Run every variant this many times (fresh sandbox each); summary.json "
            "reports pass@k (default: 1)",
        )
        parser.add_argument(
            "--retries",
            type=int,
            default=0,
            help="Retry a variant that failed before evaluation (sandbox start, connection) "
            "up to this many times, with backoff (default: 0)",
        )
        parser.add_argument(
            "--dry-run",
            dest="dry_run",
            action="store_true",
            help="Print each variant's image, kind, variant and backend, then exit "
            "without starting a sandbox",
        )
        parser.add_argument(
            "--wait", "-w", action="store_true", help=argparse.SUPPRESS
        )  # foreground is the default now
        parser.add_argument(
            "--with",
            dest="dev_paths",
            action="append",
            metavar="PATH",
            help=argparse.SUPPRESS,
        )  # obsolete: agents run in this Python environment
        parser.add_argument("--run-id", dest="run_id", help=argparse.SUPPRESS)  # used by --detach
        parser.add_argument(
            "--keep-runs",
            dest="keep_runs",
            type=int,
            default=None,
            help="After the run, delete all but the N newest runs in the runs directory "
            "(default: keep everything; env CUA_BENCH_KEEP_RUNS)",
        )
        parser.add_argument(
            "--max-age",
            dest="max_age",
            type=float,
            default=None,
            metavar="DAYS",
            help="After the run, delete runs older than DAYS (env CUA_BENCH_MAX_AGE_DAYS)",
        )
        parser.add_argument(
            "--max-results-size",
            dest="max_results_size",
            default=None,
            metavar="SIZE",
            help="After the run, delete the oldest runs until the runs directory fits in "
            "SIZE, e.g. 20G (env CUA_BENCH_MAX_RESULTS_SIZE)",
        )

    # cb run task <path>
    run_task_parser = run_subparsers.add_parser(
        "task",
        help="Run one task variant",
        **examples(
            ("Run variant 0 with the oracle solution", "cb run task ./tasks/hello_file_env"),
            (
                "Run variant 2 with an agent in the cloud",
                "cb run task ./tasks/hello_file_env --variant-id 2 --agent cua-agent "
                "--model anthropic/claude-sonnet-4-20250514 --on cloud",
            ),
            (
                "Show what would run without starting a sandbox",
                "cb run ./tasks/hello_file_env --dry-run",
            ),
        ),
    )
    run_task_parser.add_argument("task_path", help="Path to task directory (containing main.py)")
    run_task_parser.add_argument(
        "--variant-id",
        dest="variant_id",
        type=int,
        default=0,
        help="Task variant index (default: 0)",
    )
    run_task_parser.add_argument("--session-id", dest="session_id", help=argparse.SUPPRESS)
    add_common_run_args(run_task_parser)

    # cb run dataset <path>
    run_dataset_parser = run_subparsers.add_parser(
        "dataset",
        help="Run every task and variant of a dataset in parallel",
        **examples(
            ("Run a registry dataset with its oracle solutions", "cb run dataset cua-bench-basic"),
            (
                "Run one variant of each matching task, eight at once, in the background",
                "cb run ./tasks --task-filter 'click*' --max-variants 1 -j 8 --detach",
            ),
            (
                "Run every variant three times and report pass@k",
                "cb run dataset cua-bench-basic --agent cua-agent --attempts 3",
            ),
        ),
    )
    run_dataset_parser.add_argument(
        "dataset_path", help="Path to dataset directory, or dataset name from registry"
    )
    run_dataset_parser.add_argument(
        "--max-parallel",
        "-j",
        dest="max_parallel",
        type=int,
        default=4,
        help="Variants running at once; in the cloud also the managed pool's max size (default: 4)",
    )
    run_dataset_parser.add_argument(
        "--max-variants",
        dest="max_variants",
        type=int,
        help="Maximum number of variants to run per task (default: all)",
    )
    run_dataset_parser.add_argument(
        "--task-filter", dest="task_filter", help="Filter tasks by name pattern (glob)"
    )
    add_common_run_args(run_dataset_parser)

    # cb run list
    run_list_parser = run_subparsers.add_parser(
        "list",
        help="List all runs with status",
        **examples(
            ("List runs", "cb run list"), ("Include debugging details", "cb run list --verbose")
        ),
    )
    run_list_parser.add_argument(
        "--verbose", "-v", action="store_true", help="Show verbose debugging information"
    )

    # cb run info <id>
    run_info_parser = run_subparsers.add_parser(
        "info",
        help="Show detailed info about a run",
        **examples(("Show one run", "cb run info 30c12572")),
    )
    run_info_parser.add_argument("run_id", help="Run ID to show info for")

    # cb run watch <id>
    run_watch_parser = run_subparsers.add_parser(
        "watch",
        help="Watch a run in real-time with live updates",
        **examples(("Follow a detached run", "cb run watch 30c12572")),
    )
    run_watch_parser.add_argument("run_id", help="Run ID to watch")

    # cb run stop <id>
    run_stop_parser = run_subparsers.add_parser(
        "stop",
        help="Stop a run and release its sandboxes",
        **examples(("Stop a run", "cb run stop 30c12572")),
    )
    run_stop_parser.add_argument("run_id", help="Run ID to stop")

    # cb run logs <id>
    run_logs_parser = run_subparsers.add_parser(
        "logs",
        help="View combined logs from a run or session",
        **examples(
            ("Print a run's logs", "cb run logs 30c12572"),
            ("Print the last 50 lines", "cb run logs 30c12572 --tail 50"),
        ),
    )
    run_logs_parser.add_argument("identifier", help="Run ID or Session ID to view logs for")
    run_logs_parser.add_argument("--tail", type=int, help="Show only the last N lines")

    # interact (top-level)
    interact_parser = subparsers.add_parser(
        "interact",
        help="Run a task's setup with its desktop visible, then evaluate",
        **examples(
            (
                "Open a task and its display, wait for Enter, then evaluate",
                "cb interact ./tasks/hello_file_env",
            ),
            (
                "Run variant 1 with its oracle and save a screenshot",
                "cb interact ./tasks/hello_file_env --variant-id 1 --oracle --screenshot shot.png",
            ),
            (
                "Resolve a task from a registry dataset",
                "cb interact click-button --dataset cua-bench-basic",
            ),
        ),
    )
    interact_parser.add_argument(
        "env_path",
        help="Path to the task directory, or a task name with --dataset or --dataset-path",
    )
    interact_parser.add_argument(
        "--variant-id",
        dest="variant_id",
        type=int,
        default=0,
        help="Task variant index (default: 0)",
    )
    interact_parser.add_argument(
        "--dataset", help="Registry dataset to resolve the task from (CUA_REGISTRY_HOME)"
    )
    interact_parser.add_argument(
        "--dataset-path",
        dest="dataset_path",
        help="Path to dataset directory containing multiple tasks",
    )
    interact_parser.add_argument(
        "--oracle", action="store_true", help="Run the solution after setup"
    )
    interact_parser.add_argument(
        "--max-steps",
        type=int,
        dest="max_steps",
        help="Maximum number of env.step() calls before stopping",
    )
    interact_parser.add_argument("--screenshot", help="Save a screenshot to this file")
    interact_parser.add_argument(
        "--trace-out",
        dest="trace_out",
        help="Record a trace and save it as a dataset to this path on exit",
    )
    interact_parser.add_argument(
        "--view", action="store_true", help="Open the trace viewer when done"
    )
    interact_parser.add_argument(
        "--no-wait",
        dest="no_wait",
        action="store_true",
        help="Skip the interactive prompt (useful for SSH/CI testing)",
    )
    interact_parser.add_argument(
        "--no-browser",
        dest="no_browser",
        action="store_true",
        help="Print the Display: URL without opening it (also CUA_BENCH_NO_BROWSER=1)",
    )
    run.add_target_args(interact_parser)

    # agent command (top-level)
    agent.register_parser(subparsers)

    # platform command (top-level) - show available platforms
    platform.register_parser(subparsers)

    # image command (top-level) - manage images
    image.register_parser(subparsers)

    # status command (top-level) - system overview dashboard
    status.register_parser(subparsers)

    # task command (top-level) - inspect and manage tasks
    task.register_parser(subparsers)

    # trace command (top-level) - view and manage traces
    trace.register_parser(subparsers)

    # dataset command (top-level) - manage datasets
    dataset.register_parser(subparsers)

    # prune command (top-level) - clean up data and docker resources
    prune.register_parser(subparsers)

    # env command (top-level) - managed cloud pools and local sandboxes
    env.register_parser(subparsers)

    # login command (top-level) - authenticate with CUA Cloud
    login_parser = subparsers.add_parser(
        "login",
        help="Sign in for --on cloud (runs `cua auth login`)",
        **examples(
            ("Sign in with a browser", "cb login"),
            ("Print the sign-in URL instead of opening a browser", "cb login --no-browser"),
        ),
    )
    login_parser.add_argument(
        "--no-browser", dest="no_browser", action="store_true", help="Print the sign-in URL only"
    )

    return parser


def main(argv: list[str] | None = None):
    """Main CLI entry point."""
    import os

    argv = normalize_argv(list(sys.argv[1:] if argv is None else argv))
    if not os.environ.get("CUA_BENCH_NO_BANNER"):
        print_banner()
    parser = build_parser()
    args = parser.parse_args(argv)
    args._argv = argv

    if args.command is None:
        parser.print_help()
        sys.exit(1)

    # Track command invocation with telemetry
    if _telemetry_available:
        # Register flush on exit
        atexit.register(flush_telemetry)

        # Extract subcommand for run command
        subcommand = None
        if args.command == "run" and hasattr(args, "run_command"):
            subcommand = args.run_command

        # Collect safe args for analytics
        cmd_args = {}
        for key in [
            "agent",
            "model",
            "max_steps",
            "on",
            "kind",
            "runtime",
            "oracle",
            "detach",
            "max_parallel",
        ]:
            if hasattr(args, key) and getattr(args, key) is not None:
                cmd_args[key] = getattr(args, key)

        track_command_invoked(args.command, subcommand, cmd_args)

    # Execute command
    if args.command == "run":
        sys.exit(run.execute(args) or 0)
    elif args.command == "interact":
        interact.execute(args)
    elif args.command == "agent":
        agent.execute(args)
    elif args.command == "platform":
        platform.execute(args)
    elif args.command == "image":
        image.execute(args)
    elif args.command == "status":
        status.execute(args)
    elif args.command == "task":
        task.execute(args)
    elif args.command == "trace":
        trace.execute(args)
    elif args.command == "dataset":
        dataset.execute(args)
    elif args.command == "prune":
        prune.execute(args)
    elif args.command == "env":
        sys.exit(env.execute(args) or 0)
    elif args.command == "login":
        sys.exit(login.execute(args) or 0)
    else:
        parser.print_help()
        sys.exit(1)


if __name__ == "__main__":
    main()
