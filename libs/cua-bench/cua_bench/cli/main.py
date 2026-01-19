"""Main CLI entry point for cua-bench."""

import argparse
import atexit
import sys
from importlib import metadata as _metadata

from .commands import (
    agent,
    dataset,
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


def main():
    """Main CLI entry point."""
    print_banner()
    parser = argparse.ArgumentParser(
        description="cua-bench - toolkit for verifiable cross-platform computer-use RL environments and benchmarks"
    )
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # run (top-level)
    run_parser = subparsers.add_parser("run", help="Run tasks with optional agent evaluation")
    run_subparsers = run_parser.add_subparsers(dest="run_command", help="Run command")

    # Shared arguments for task and dataset subcommands
    def add_common_run_args(parser):
        """Add common arguments shared by task and dataset subcommands."""
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
            "--oracle", action="store_true", help="Run the oracle solution instead of agent"
        )
        parser.add_argument(
            "--max-steps",
            dest="max_steps",
            type=int,
            default=100,
            help="Maximum number of steps for agent execution (default: 100)",
        )
        parser.add_argument(
            "--image",
            help="Image name to use (e.g., windows-qemu, windows-waa). See: cb image list",
        )
        parser.add_argument(
            "--platform",
            dest="platform",
            choices=["linux-docker", "linux-qemu", "windows-qemu", "android-qemu"],
            help="Platform type (auto-detected from task if not set)",
        )
        parser.add_argument(
            "--output-dir", dest="output_dir", help="Output directory for session results"
        )
        parser.add_argument(
            "--vnc-port",
            dest="vnc_port",
            type=int,
            help="Map VNC port to host for debugging (e.g., 8006)",
        )
        parser.add_argument(
            "--api-port",
            dest="api_port",
            type=int,
            help="Map API port to host for debugging (e.g., 5000)",
        )
        parser.add_argument(
            "--debug", action="store_true", help="Auto-allocate VNC and API ports for debugging"
        )
        parser.add_argument(
            "--provider-type",
            dest="provider_type",
            help="Provider type (simulated, native) - usually auto-detected from task config",
        )

    # cb run task <path>
    run_task_parser = run_subparsers.add_parser(
        "task", help="Run a single task (2-container architecture)"
    )
    run_task_parser.add_argument("task_path", help="Path to task directory")
    run_task_parser.add_argument(
        "--variant-id",
        dest="variant_id",
        type=int,
        default=0,
        help="Task variant index (default: 0)",
    )
    run_task_parser.add_argument(
        "--wait",
        "-w",
        action="store_true",
        help="Wait for task completion (default: run async and return immediately)",
    )
    run_task_parser.add_argument(
        "--run-id", dest="run_id", help="Run ID (used internally for subprocess communication)"
    )
    run_task_parser.add_argument(
        "--session-id",
        dest="session_id",
        help="Session ID (used internally for dataset subprocess communication)",
    )
    add_common_run_args(run_task_parser)

    # cb run dataset <path>
    run_dataset_parser = run_subparsers.add_parser(
        "dataset", help="Run all tasks in a dataset (parallel 2-container architecture)"
    )
    run_dataset_parser.add_argument(
        "dataset_path", help="Path to dataset directory, or dataset name from registry"
    )
    run_dataset_parser.add_argument(
        "--max-parallel",
        dest="max_parallel",
        type=int,
        default=4,
        help="Maximum number of parallel task runners (default: 4)",
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
    run_dataset_parser.add_argument(
        "--wait",
        "-w",
        action="store_true",
        help="Wait for all tasks and launch watch TUI (default: run async and return immediately)",
    )
    run_dataset_parser.add_argument(
        "--run-id", dest="run_id", help="Run ID (used internally for subprocess communication)"
    )
    add_common_run_args(run_dataset_parser)

    # cb run list
    run_list_parser = run_subparsers.add_parser("list", help="List all runs with status")
    run_list_parser.add_argument(
        "--verbose", "-v", action="store_true", help="Show verbose debugging information"
    )

    # cb run info <id>
    run_info_parser = run_subparsers.add_parser("info", help="Show detailed info about a run")
    run_info_parser.add_argument("run_id", help="Run ID to show info for")

    # cb run watch <id>
    run_watch_parser = run_subparsers.add_parser(
        "watch", help="Watch a run in real-time with live updates"
    )
    run_watch_parser.add_argument("run_id", help="Run ID to watch")

    # cb run stop <id>
    run_stop_parser = run_subparsers.add_parser("stop", help="Stop/cancel a run")
    run_stop_parser.add_argument("run_id", help="Run ID to stop")

    # cb run logs <id>
    run_logs_parser = run_subparsers.add_parser(
        "logs", help="View combined logs from a run or session"
    )
    run_logs_parser.add_argument("identifier", help="Run ID or Session ID to view logs for")
    run_logs_parser.add_argument("--tail", type=int, help="Number of lines to show from end")

    # interact (top-level)
    interact_parser = subparsers.add_parser(
        "interact", help="Interactively run a task with browser visible"
    )
    interact_parser.add_argument(
        "env_path",
        help="Path to environment directory, or task name when using --dataset or --dataset-path",
    )
    interact_parser.add_argument(
        "--variant-id",
        dest="variant_id",
        type=int,
        default=0,
        help="Task variant index (default: 0)",
    )
    interact_parser.add_argument(
        "--dataset", help="Dataset name to resolve task from CUA_REGISTRY_HOME"
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
    interact_parser.add_argument("--screenshot", help="Save screenshot to file")
    interact_parser.add_argument(
        "--trace-out",
        dest="trace_out",
        help="If set, start tracing and save dataset to this path on exit",
    )
    interact_parser.add_argument(
        "--view", action="store_true", help="Open a trace viewer when done"
    )
    interact_parser.add_argument(
        "--no-wait",
        dest="no_wait",
        action="store_true",
        help="Skip the interactive prompt (useful for SSH/CI testing)",
    )

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

    # login command (top-level) - authenticate with CUA Cloud
    login_parser = subparsers.add_parser(
        "login", help="Authenticate with CUA Cloud to get API token"
    )
    login_parser.add_argument(
        "--force", action="store_true", help="Force re-authentication even if already logged in"
    )
    login_parser.add_argument(
        "--auth-url", dest="auth_url", help="Custom auth URL (default: https://cua.ai/cli-auth)"
    )

    args = parser.parse_args()

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
            "platform",
            "oracle",
            "wait",
            "debug",
            "max_parallel",
        ]:
            if hasattr(args, key) and getattr(args, key) is not None:
                cmd_args[key] = getattr(args, key)

        track_command_invoked(args.command, subcommand, cmd_args)

    # Execute command
    if args.command == "run":
        run.execute(args)
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
    elif args.command == "login":
        login.execute(args)
    else:
        parser.print_help()
        sys.exit(1)


if __name__ == "__main__":
    main()
