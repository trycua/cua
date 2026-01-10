"""Run command - Run tasks with agent evaluation.

The run command executes tasks and provides monitoring capabilities.

Usage:
    cb run task <path>                  # Execute single task (2-container)
    cb run dataset <path>               # Execute dataset (parallel 2-container)
    cb run list                         # List all runs
    cb run info <id>                    # Show run details
    cb run watch <id>                   # Live updates
    cb run stop <id>                    # Cancel run
    cb run logs <id>                    # Combined logs
"""

import asyncio
import os
import re
import signal
import subprocess
import sys
from collections import defaultdict
from pathlib import Path
from typing import Optional

from cua_bench.runner.docker_utils import allocate_ports, generate_task_id

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def _get_runs_dir() -> Path:
    """Get the default runs output directory (XDG compliant)."""
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench" / "runs"


def _get_run_output_dir(run_id: str) -> Path:
    """Get output directory for a specific run."""
    return _get_runs_dir() / run_id


# =============================================================================
# Run Subcommands
# =============================================================================


def cmd_list(args) -> int:
    """List all runs with aggregated statistics."""
    return asyncio.run(_cmd_list_async(args))


async def _cmd_list_async(args) -> int:
    """Execute the runs list command asynchronously."""
    from cua_bench.sessions import list_sessions
    from cua_bench.sessions.providers.docker import DockerProvider

    sessions = list_sessions()
    verbose = getattr(args, "verbose", False)

    if not sessions:
        print(f"{GREY}No runs found.{RESET}")
        print(f"\n{GREY}Start a run with:{RESET}")
        print("  cb run <task>")
        return 0

    docker_provider = DockerProvider()

    # Group sessions by run_id
    runs = defaultdict(list)
    for session in sessions:
        run_id = session.get("run_id", "-")
        runs[run_id].append(session)

    run_rows = []

    for run_id, run_sessions in runs.items():
        if run_id == "-":
            continue

        agent = run_sessions[0].get("agent") or "-"
        model = run_sessions[0].get("model") or "-"

        # Extract dataset from env_path
        env_path = run_sessions[0].get("env_path", "")
        if env_path:
            path_parts = Path(env_path).parts
            if "datasets" in path_parts:
                dataset_idx = path_parts.index("datasets")
                if dataset_idx + 1 < len(path_parts):
                    dataset = path_parts[dataset_idx + 1]
                else:
                    dataset = "-"
            else:
                dataset = Path(env_path).parent.name
        else:
            dataset = "-"

        # Count statuses
        status_counts = defaultdict(int)
        rewards = []

        for session in run_sessions:
            session_id = session.get("session_id")
            provider = session.get("provider", "unknown")

            status = "unknown"
            status_info = {}
            if provider == "docker":
                try:
                    status_info = await docker_provider.get_session_status(session_id)
                    status = status_info.get("status", "unknown")

                    if verbose:
                        print(f"{GREY}[DEBUG] Session {session_id}:{RESET}")
                        print(f"{GREY}  Stored session: {session}{RESET}")
                        print(f"{GREY}  Status info: {status_info}{RESET}")
                except Exception as e:
                    status = "error"
                    if verbose:
                        print(f"{RED}[DEBUG] Error getting status for {session_id}: {e}{RESET}")

            status_counts[status] += 1

            # Use cached reward if available, otherwise extract from logs
            if status == "completed":
                reward = status_info.get("reward")
                if reward is not None:
                    # Use cached reward
                    rewards.append(reward)
                else:
                    # Fallback: extract from logs
                    try:
                        logs = await docker_provider.get_session_logs(session_id, tail=100)
                        match = re.search(r"✓ Evaluation result: \[([^\]]+)\]", logs)
                        if match:
                            reward_str = match.group(1)
                            try:
                                rewards.append(float(reward_str))
                            except ValueError:
                                pass
                    except Exception:
                        pass
            elif status == "failed":
                reward = status_info.get("reward")
                if reward is not None:
                    rewards.append(reward)
                else:
                    rewards.append(0.0)

        # Build status string
        status_parts = []
        for status, count in sorted(status_counts.items()):
            status_parts.append(f"{status}({count})")
        status_str = " ".join(status_parts)

        # Calculate average reward
        avg_reward_str = f"{sum(rewards) / len(rewards):.3f}" if rewards else "-"
        avg_reward_val = sum(rewards) / len(rewards) if rewards else 0.0

        run_rows.append(
            {
                "run_id": run_id,
                "dataset": dataset,
                "sessions": str(len(run_sessions)),
                "agent": agent,
                "model": model,
                "status": status_str,
                "avg_reward": avg_reward_str,
                "avg_reward_val": avg_reward_val,
            }
        )

    if not run_rows:
        print(f"{GREY}No runs found with run IDs.{RESET}")
        return 0

    # Add spacing after verbose output
    if verbose:
        print()

    # Calculate column widths
    max_run_id = max(len(row["run_id"]) for row in run_rows)
    max_dataset = max(len(row["dataset"]) for row in run_rows)
    max_sessions = max(len(row["sessions"]) for row in run_rows)
    max_agent = max(len(row["agent"]) for row in run_rows)
    max_model = max(len(row["model"]) for row in run_rows)
    max_status = max(len(row["status"]) for row in run_rows)
    max_avg_reward = max(len(row["avg_reward"]) for row in run_rows)

    # Ensure minimum widths
    max_run_id = max(max_run_id, 6)
    max_dataset = max(max_dataset, 7)
    max_sessions = max(max_sessions, 8)
    max_agent = max(max_agent, 5)
    max_model = max(max_model, 5)
    max_status = max(max_status, 6)
    max_avg_reward = max(max_avg_reward, 10)

    # Print table
    header = f"{BOLD}{'RUN ID':<{max_run_id}}  {'DATASET':<{max_dataset}}  {'SESSIONS':<{max_sessions}}  {'AGENT':<{max_agent}}  {'MODEL':<{max_model}}  {'STATUS':<{max_status}}  {'AVG REWARD':<{max_avg_reward}}{RESET}"
    print(header)
    print(
        "-"
        * (
            max_run_id
            + max_dataset
            + max_sessions
            + max_agent
            + max_model
            + max_status
            + max_avg_reward
            + 12
        )
    )

    for row in run_rows:
        reward_str = row["avg_reward"]
        if reward_str != "-":
            reward_val = row["avg_reward_val"]
            if reward_val >= 0.5:
                reward_colored = f"{GREEN}{reward_str:<{max_avg_reward}}{RESET}"
            else:
                reward_colored = f"{RED}{reward_str:<{max_avg_reward}}{RESET}"
        else:
            reward_colored = f"{reward_str:<{max_avg_reward}}"

        print(
            f"{row['run_id']:<{max_run_id}}  {row['dataset']:<{max_dataset}}  {row['sessions']:<{max_sessions}}  {row['agent']:<{max_agent}}  {row['model']:<{max_model}}  {row['status']:<{max_status}}  {reward_colored}"
        )

    print()
    print(f"{GREY}Commands:{RESET}")
    print(f"  cb run info <run_id>    {GREY}# Show run details{RESET}")
    print(f"  cb run watch <run_id>   {GREY}# Watch in real-time{RESET}")
    print(f"  cb trace grid <run_id>  {GREY}# View traces{RESET}")

    return 0


def cmd_watch(args) -> int:
    """Watch a run in real-time with TUI."""
    return asyncio.run(_cmd_watch_async(args))


async def _cmd_watch_async(
    args, expected_session_count: Optional[int] = None, run_output_dir: Optional[str] = None
) -> int:
    """Watch a run in real-time with TUI.

    Args:
        args: Command arguments with run_id
        expected_session_count: Expected total sessions (for progress display)
        run_output_dir: Output directory for this run
    """
    from cua_bench.sessions import list_sessions
    from cua_bench.sessions.providers.docker import DockerProvider

    try:
        from rich.console import Console, Group
        from rich.live import Live
        from rich.table import Table
        from rich.text import Text
    except ImportError:
        print(f"{RED}Error: 'rich' package required for watch mode.{RESET}")
        print("Install with: pip install rich")
        return 1

    run_id = getattr(args, "run_id", None)
    if not run_id:
        print(f"{RED}Error: run_id required for watch action{RESET}")
        return 1

    console = Console()
    docker_provider = DockerProvider()

    # Compute run_output_dir if not provided
    if not run_output_dir:
        sessions = list_sessions()
        run_sessions = [s for s in sessions if s.get("run_id") == run_id]

        if run_sessions:
            # Get run output dir from first session's output_dir (go up one level)
            output_dir = run_sessions[0].get("output_dir", None)
            if output_dir:
                session_output = Path(output_dir)
                # Session output is like: .../runs/<run_id>/<task>_v<variant>
                # We want: .../runs/<run_id>
                if session_output.parent.name == run_id:
                    run_output_dir = str(session_output.parent)

        if not run_output_dir:
            run_output_dir = str(_get_run_output_dir(run_id))

    stop_requested = False
    detach_requested = False

    def signal_handler(signum, frame):
        nonlocal stop_requested, detach_requested
        if signum == signal.SIGINT:
            detach_requested = True
        elif signum == signal.SIGTERM:
            stop_requested = True

    signal.signal(signal.SIGINT, signal_handler)

    spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    spinner_idx = 0

    def make_layout(
        sessions_data, agent, model, completed_count, total_count, rewards, failed_sessions
    ):
        nonlocal spinner_idx

        progress_pct = (completed_count / total_count * 100) if total_count > 0 else 0
        filled = int(progress_pct / 5)
        progress_bar = f"{'█' * filled}{'░' * (20 - filled)}"

        header = Text()
        header.append(f"{progress_bar} {completed_count}/{total_count}\n", style="bold cyan")
        header.append(f"Model: {model}\n", style="white")
        header.append(f"Agent: {agent}\n", style="white")
        header.append(f"Run ID: {run_id}\n", style="dim")
        if run_output_dir:
            header.append(f"Output: {run_output_dir}\n", style="dim")
            header.append(f"Logs:   {run_output_dir}/run.log\n", style="dim")

        table = Table(
            show_header=True,
            header_style="white",
            expand=True,
            box=None,
            show_edge=False,
            pad_edge=False,
        )
        table.add_column("SESSION ID", style="cyan", width=40)
        table.add_column("ENVIRONMENT", style="white", width=15)
        table.add_column("VARIANT", style="white", width=7)
        table.add_column("STATUS", style="white", width=15)
        table.add_column("REWARD", style="white", width=10)

        table.add_row("-" * 40, "-" * 15, "-" * 7, "-" * 15, "-" * 10, style="dim")

        for session_data in sessions_data:
            session_id = session_data["session_id"]
            environment = session_data["environment"]
            variant = session_data["variant"]
            status = session_data["status"]
            reward = session_data["reward"]

            if status == "running":
                status_display = f"{spinner_frames[spinner_idx]} {status}"
                status_style = "green"
            elif status == "completed":
                status_display = f"✓ {status}"
                status_style = "cyan"
            elif status == "failed":
                status_display = f"✗ {status}"
                status_style = "red"
            else:
                status_display = status
                status_style = "yellow"

            if reward != "-":
                try:
                    reward_val = float(reward)
                    reward_style = "green" if reward_val >= 0.5 else "red"
                except ValueError:
                    reward_style = "white"
            else:
                reward_style = "dim"

            table.add_row(
                session_id,
                environment,
                variant,
                Text(status_display, style=status_style),
                Text(reward, style=reward_style),
            )

        stats_parts = []
        if completed_count == total_count and total_count > 0:
            if rewards:
                from collections import Counter

                avg_reward = sum(rewards) / len(rewards)
                stats_parts.append("\n✓ All sessions completed!")
                stats_parts.append(f"Average Reward: {avg_reward:.3f}\n")
                reward_counts = Counter(rewards)
                sorted_rewards = sorted(reward_counts.items(), key=lambda x: x[0], reverse=True)[
                    :10
                ]
                stats_parts.append("REWARD  COUNT")
                stats_parts.append("-" * 13)
                for reward_val, count in sorted_rewards:
                    stats_parts.append(f"{reward_val:<6.1f}  {count}")
            else:
                stats_parts.append("\n⚠ All sessions completed but no rewards found")

        error_parts = []
        if failed_sessions:
            for session_id, logs in failed_sessions.items():
                error_parts.append("\n" + "=" * 60)
                error_parts.append(f"{session_id} FAILED")
                error_parts.append("=" * 60)
                error_parts.append(logs)
                error_parts.append("=" * 60 + "\n")

        parts = [header, Text("\n"), table]
        if stats_parts:
            parts.append(Text("\n"))
            for part in stats_parts:
                parts.append(Text(part))
        if error_parts:
            parts.append(Text("\n"))
            for part in error_parts:
                parts.append(Text(part))

        return Group(*parts)

    with Live(console=console, refresh_per_second=1) as live:
        while not stop_requested and not detach_requested:
            all_sessions = list_sessions()
            run_sessions = [s for s in all_sessions if s.get("run_id") == run_id]

            if not run_sessions:
                if expected_session_count and expected_session_count > 0:
                    waiting_text = Text()
                    waiting_text.append("Waiting for sessions to be created...\n", style="yellow")
                    waiting_text.append(
                        f"Expected: {expected_session_count} sessions\n", style="dim"
                    )
                    waiting_text.append(f"Run ID: {run_id}", style="dim")
                    live.update(waiting_text)
                    await asyncio.sleep(0.5)
                    continue
                else:
                    console.print(f"[red]No sessions found for run: {run_id}[/red]")
                    return 1

            agent = run_sessions[0].get("agent", "-")
            model = run_sessions[0].get("model", "-")

            sessions_data = []
            completed_count = 0
            total_count = expected_session_count if expected_session_count else len(run_sessions)
            rewards = []
            failed_sessions = {}

            for session in run_sessions:
                session_id = session.get("session_id", "unknown")
                if session_id == "unknown":
                    continue

                # Skip queued sessions (not yet started)
                if session.get("status") == "queued":
                    continue

                env_path = session.get("env_path", "unknown")
                environment = Path(env_path).name if env_path != "unknown" else "unknown"
                variant = str(session.get("task_index", 0))

                try:
                    status_info = await docker_provider.get_session_status(session_id)
                    status = status_info.get("status", "unknown")
                except Exception:
                    status = "error"

                if status in ("completed", "failed", "stopped", "deleted"):
                    completed_count += 1

                reward = "-"
                if status == "completed":
                    try:
                        logs = await docker_provider.get_session_logs(session_id, tail=100)
                        match = re.search(r"✓ Evaluation result: \[([^\]]+)\]", logs)
                        if match:
                            reward = match.group(1)
                            try:
                                rewards.append(float(reward))
                            except ValueError:
                                pass
                    except Exception:
                        pass
                elif status == "failed":
                    rewards.append(0.0)
                    reward = "0.0"

                if status == "failed":
                    try:
                        logs = await docker_provider.get_session_logs(session_id, tail=50)
                        failed_sessions[session_id] = logs
                    except Exception as e:
                        failed_sessions[session_id] = f"Could not retrieve logs: {e}"

                sessions_data.append(
                    {
                        "session_id": session_id,
                        "environment": environment,
                        "variant": variant,
                        "status": status,
                        "reward": reward,
                    }
                )

            layout = make_layout(
                sessions_data, agent, model, completed_count, total_count, rewards, failed_sessions
            )
            live.update(layout)

            spinner_idx = (spinner_idx + 1) % len(spinner_frames)

            if completed_count == total_count:
                await asyncio.sleep(2)
                break

            await asyncio.sleep(0.5)

    if detach_requested:
        console.print("\n[yellow]Detached from run. Sessions continue in background.[/yellow]")
        console.print(f"[dim]Resume watching with: cb run watch {run_id}[/dim]")
    elif stop_requested:
        console.print("\n[red]Stopping all sessions...[/red]")
    else:
        # Cleanup child containers
        console.print("\n[cyan]Cleaning up child containers...[/cyan]")
        all_sessions = list_sessions()
        run_sessions = [s for s in all_sessions if s.get("run_id") == run_id]

        containers_to_stop = []
        for session in run_sessions:
            session_id = session.get("session_id")
            if not session_id:
                continue
            try:
                status_info = await docker_provider.get_session_status(session_id)
                status = status_info.get("status", "unknown")
                if status not in ["running", "unknown"]:
                    child_containers = session.get("child_containers", [])
                    for child in child_containers:
                        container_name = child.get("name")
                        if container_name:
                            containers_to_stop.append(container_name)
            except Exception:
                pass

        if containers_to_stop:
            console.print(f"[dim]Stopping {len(containers_to_stop)} child container(s)...[/dim]")

            async def stop_container_safe(container_name):
                try:
                    await docker_provider._stop_container(container_name)
                except Exception:
                    pass

            await asyncio.gather(
                *[stop_container_safe(name) for name in containers_to_stop], return_exceptions=True
            )

        console.print("[green]✓ Cleanup complete[/green]")

    return 0


def cmd_stop(args) -> int:
    """Stop a run and all its sessions."""
    return asyncio.run(_cmd_stop_async(args))


async def _cmd_stop_async(args) -> int:
    """Stop a run asynchronously."""
    from cua_bench.sessions import list_sessions, manager
    from cua_bench.sessions.providers.docker import DockerProvider

    run_id = getattr(args, "run_id", None)
    if not run_id:
        print(f"{RED}Error: run_id required{RESET}")
        return 1

    sessions = list_sessions()
    run_sessions = [s for s in sessions if s.get("run_id") == run_id]

    if not run_sessions:
        print(f"{RED}Error: No sessions found for run: {run_id}{RESET}")
        return 1

    print(f"{CYAN}Stopping run: {run_id} ({len(run_sessions)} sessions)...{RESET}")

    docker_provider = DockerProvider()

    for session in run_sessions:
        session_id = session.get("session_id")
        if not session_id:
            continue

        try:
            await docker_provider.stop_session(session_id)
            manager.remove_session(session_id)
            print(f"  {GREEN}✓{RESET} Stopped: {session_id}")
        except Exception as e:
            print(f"  {YELLOW}⚠{RESET} Failed to stop {session_id}: {e}")

    print(f"\n{GREEN}✓ Run stopped{RESET}")
    return 0


def cmd_logs(args) -> int:
    """View combined logs from a run."""
    return asyncio.run(_cmd_logs_async(args))


async def _cmd_logs_async(args) -> int:
    """View run or session logs asynchronously."""
    from cua_bench.sessions import list_sessions
    from cua_bench.sessions.providers.docker import DockerProvider

    identifier = getattr(args, "identifier", None)
    if not identifier:
        print(f"{RED}Error: identifier (run ID or session ID) required{RESET}")
        return 1

    sessions = list_sessions()
    docker_provider = DockerProvider()
    tail = getattr(args, "tail", 50)

    # Check if identifier is a session ID (starts with "task-")
    if identifier.startswith("task-"):
        # Show logs for specific session
        session = next((s for s in sessions if s.get("session_id") == identifier), None)

        if not session:
            print(f"{RED}Error: Session not found: {identifier}{RESET}")
            return 1

        print(f"\n{'=' * 60}")
        print(f"Session: {identifier}")
        print(f"{'=' * 60}")

        try:
            logs = await docker_provider.get_session_logs(identifier, tail=tail)
            print(logs)
        except Exception as e:
            print(f"{RED}Error retrieving logs: {e}{RESET}")
            return 1
    else:
        # Treat as run ID - show logs for all sessions in the run
        run_id = identifier
        run_sessions = [s for s in sessions if s.get("run_id") == run_id]

        if not run_sessions:
            print(f"{RED}Error: No sessions found for run: {run_id}{RESET}")
            return 1

        for session in run_sessions:
            session_id = session.get("session_id")
            if not session_id:
                continue

            print(f"\n{'=' * 60}")
            print(f"Session: {session_id}")
            print(f"{'=' * 60}")

            try:
                logs = await docker_provider.get_session_logs(session_id, tail=tail)
                print(logs)
            except Exception as e:
                print(f"{RED}Error retrieving logs: {e}{RESET}")

    return 0


def cmd_info(args) -> int:
    """Show detailed info about a run (static snapshot)."""
    return asyncio.run(_cmd_info_async(args))


async def _cmd_info_async(args) -> int:
    """Show run info asynchronously."""
    from cua_bench.sessions import list_sessions
    from cua_bench.sessions.providers.docker import DockerProvider

    run_id = getattr(args, "run_id", None)
    if not run_id:
        print(f"{RED}Error: run_id required{RESET}")
        return 1

    sessions = list_sessions()
    run_sessions = [s for s in sessions if s.get("run_id") == run_id]

    if not run_sessions:
        print(f"{RED}Error: No sessions found for run: {run_id}{RESET}")
        return 1

    docker_provider = DockerProvider()

    # Get metadata
    agent = run_sessions[0].get("agent") or "-"
    model = run_sessions[0].get("model") or "-"
    output_dir = run_sessions[0].get("output_dir", None)

    # Get run output dir from first session's output_dir (go up one level)
    run_output_dir = None
    if output_dir:
        session_output = Path(output_dir)
        # Session output is like: .../runs/<run_id>/<task>_v<variant>
        # We want: .../runs/<run_id>
        if session_output.parent.name == run_id:
            run_output_dir = str(session_output.parent)

    if not run_output_dir:
        run_output_dir = str(_get_run_output_dir(run_id))

    # Extract dataset from first session's env_path
    env_path = run_sessions[0].get("env_path", "")
    dataset = "-"
    if env_path:
        path_parts = Path(env_path).parts
        if "datasets" in path_parts:
            dataset_idx = path_parts.index("datasets")
            if dataset_idx + 1 < len(path_parts):
                dataset = path_parts[dataset_idx + 1]
        else:
            dataset = Path(env_path).parent.name

    # Collect session data
    sessions_data = []
    completed_count = 0
    rewards = []

    for session in run_sessions:
        session_id = session.get("session_id", "unknown")
        if session_id == "unknown" or session.get("status") == "queued":
            continue

        env_path = session.get("env_path", "unknown")
        environment = Path(env_path).name if env_path != "unknown" else "unknown"
        variant = str(session.get("task_index", 0))

        try:
            status_info = await docker_provider.get_session_status(session_id)
            status = status_info.get("status", "unknown")
        except Exception:
            status = "error"

        if status in ("completed", "failed", "stopped", "deleted"):
            completed_count += 1

        reward = "-"
        if status == "completed":
            try:
                logs = await docker_provider.get_session_logs(session_id, tail=100)
                match = re.search(r"✓ Evaluation result: \[([^\]]+)\]", logs)
                if match:
                    reward = match.group(1)
                    try:
                        rewards.append(float(reward))
                    except ValueError:
                        pass
            except Exception:
                pass
        elif status == "failed":
            rewards.append(0.0)
            reward = "0.0"

        # Get session output dir for logs location
        session_output = session.get("output_dir", "-")

        sessions_data.append(
            {
                "session_id": session_id,
                "environment": environment,
                "variant": variant,
                "status": status,
                "reward": reward,
                "output_dir": session_output,
            }
        )

    total_count = len(sessions_data)

    # Print header
    print()
    print(f"{BOLD}Run: {run_id}{RESET}")
    print(f"Dataset:  {dataset}")
    print(f"Agent:    {agent}")
    print(f"Model:    {model}")
    print(f"Progress: {completed_count}/{total_count}")
    if rewards:
        avg_reward = sum(rewards) / len(rewards)
        reward_color = GREEN if avg_reward >= 0.5 else RED
        print(f"Avg Reward: {reward_color}{avg_reward:.3f}{RESET}")
    print(f"\nOutput: {run_output_dir}")
    print(f"Logs:   {run_output_dir}/run.log")
    print()

    # Print sessions table
    print(
        f"{BOLD}{'SESSION ID':<40}  {'ENVIRONMENT':<15}  {'VARIANT':<7}  {'STATUS':<15}  {'REWARD':<10}{RESET}"
    )
    print("-" * 40 + "  " + "-" * 15 + "  " + "-" * 7 + "  " + "-" * 15 + "  " + "-" * 10)

    for session_data in sessions_data:
        session_id = session_data["session_id"]
        environment = session_data["environment"]
        variant = session_data["variant"]
        status = session_data["status"]
        reward = session_data["reward"]

        # Color status
        if status == "running":
            status_display = f"{GREEN}{status:<15}{RESET}"
        elif status == "completed":
            status_display = f"{CYAN}{status:<15}{RESET}"
        elif status == "failed":
            status_display = f"{RED}{status:<15}{RESET}"
        else:
            status_display = f"{GREY}{status:<15}{RESET}"

        # Color reward
        if reward != "-":
            try:
                reward_val = float(reward)
                if reward_val >= 0.5:
                    reward_display = f"{GREEN}{reward:<10}{RESET}"
                else:
                    reward_display = f"{RED}{reward:<10}{RESET}"
            except ValueError:
                reward_display = f"{reward:<10}"
        else:
            reward_display = f"{GREY}{reward:<10}{RESET}"

        print(
            f"{session_id:<40}  {environment:<15}  {variant:<7}  {status_display}  {reward_display}"
        )

    print()
    print(f"{GREY}Commands:{RESET}")
    print(f"  cb run watch {run_id}   {GREY}# Watch in real-time{RESET}")
    print(f"  cb run logs {run_id}    {GREY}# View all logs{RESET}")
    print(f"  cb run stop {run_id}    {GREY}# Stop all sessions{RESET}")
    print(f"  cb trace grid {run_id}  {GREY}# View all traces{RESET}")
    print()

    return 0


# =============================================================================
# Task and Dataset Execution Commands
# =============================================================================


def cmd_run_task(args) -> int:
    """Run a single task using 2-container architecture.

    By default, runs asynchronously and returns immediately with a run ID.
    Use --wait to wait for completion.
    """
    # Load .env file if it exists
    from dotenv import load_dotenv

    env_file = Path.cwd() / ".env"
    if env_file.exists():
        load_dotenv(env_file)
        print(f"{GREY}Loaded environment from: {env_file}{RESET}")

    # Apply configuration defaults from .cua/config.yaml
    args = _apply_config_defaults_for_task(args)

    # Generate run ID upfront
    run_id = generate_task_id()

    # Determine output directory (user-specified or default)
    user_output_dir = getattr(args, "output_dir", None)
    if user_output_dir:
        output_dir = Path(user_output_dir)
    else:
        output_dir = _get_run_output_dir(run_id)

    # Create output directory
    output_dir.mkdir(parents=True, exist_ok=True)

    # Store on args for the async function
    args.output_dir = str(output_dir)
    args._run_id = run_id

    # Check if user wants to wait for completion
    wait_mode = getattr(args, "wait", False)

    if wait_mode:
        # Synchronous mode - wait for completion
        return asyncio.run(_cmd_run_task_async(args))
    else:
        # Async mode - start in background and return
        return asyncio.run(_cmd_run_task_detached(args))


async def _cmd_run_task_async(args) -> int:
    """Execute single task using 2-container architecture."""
    from cua_bench.runner import TaskRunner
    from cua_bench.sessions import manager

    task_path = Path(args.task_path)
    if not task_path.exists():
        print(f"{RED}Error: Task not found: {task_path}{RESET}")
        return 1

    # Get provider type (explicit arg or auto-detect)
    provider_type = getattr(args, "provider_type", None) or _detect_provider_type(task_path)

    # Log provider
    if provider_type in ("simulated", "webtop"):
        print(f"{GREY}Provider: simulated - agent will use local Playwright session{RESET}")
    elif provider_type in ("native", "computer"):
        print(f"{GREY}Provider: native - using 2-container architecture{RESET}")
    else:
        print(f"{GREY}Provider: unknown - using 2-container architecture{RESET}")

    # Get task index
    task_index = getattr(args, "variant_id", 0) or 0

    # Detect env type and image (only used for native providers)
    env_type, image_name = _detect_env_type_and_image(task_path, args)

    # Get optional debug ports - auto-allocate if either is requested
    user_vnc = getattr(args, "vnc_port", None)
    user_api = getattr(args, "api_port", None)
    debug_mode = getattr(args, "debug", False)

    if debug_mode or user_vnc is not None or user_api is not None:
        # Auto-allocate ports for debugging
        if user_vnc is not None and user_api is not None:
            vnc_port = user_vnc
            api_port = user_api
        else:
            vnc_port, api_port = allocate_ports(
                vnc_default=user_vnc if user_vnc else 8006,
                api_default=user_api if user_api else 5000,
            )
            if not user_vnc or not user_api:
                print(f"{CYAN}Auto-allocated debug ports: VNC={vnc_port}, API={api_port}{RESET}")
    else:
        vnc_port = None
        api_port = None

    # Resolve agent configuration
    agent_name = getattr(args, "agent", None)
    agent_image = None
    agent_command = None
    agent_import_path = getattr(args, "agent_import_path", None)

    # If agent is specified, check if it's a Docker image agent
    if agent_name:
        config_loader = getattr(args, "_config_loader", None)
        if config_loader:
            agent_entry = config_loader.get_agent_by_name(agent_name)
            if agent_entry:
                if agent_entry.is_docker_agent():
                    agent_image = agent_entry.get_image()
                    agent_command = agent_entry.command
                    print(f"{CYAN}Using Docker image agent: {agent_image}{RESET}")
                elif agent_entry.import_path:
                    agent_import_path = agent_entry.import_path

    # Get run info
    # Use run_id from args if provided (from parent or subprocess), otherwise generate new one
    run_id = getattr(args, "_run_id", None) or getattr(args, "run_id", None) or generate_task_id()

    # Get or generate run output directory
    run_output_dir = getattr(args, "output_dir", None)
    if not run_output_dir:
        run_output_dir = str(_get_run_output_dir(run_id))

    # Create session output directory (nested inside run directory, like dataset runs)
    session_output_dir = str(Path(run_output_dir) / f"{task_path.name}_v{task_index}")

    # Register or update session before starting
    # Use provided session_id if available (from dataset command), otherwise generate one
    session_id = getattr(args, "session_id", None) or f"task-{run_id}"

    # Determine agent display value
    oracle_mode = getattr(args, "oracle", False)
    agent_display = "oracle" if oracle_mode else agent_name

    session_data = {
        "session_id": session_id,
        "run_id": run_id,
        "provider": "docker",
        "env_path": str(task_path),
        "task_index": task_index,
        "env_type": env_type,
        "image": image_name,
        "agent": agent_display,
        "model": getattr(args, "model", None) or "-",
        "output_dir": session_output_dir,  # Use session output dir
        "status": "starting",
    }

    # Check if session already exists (from dataset pre-registration)
    existing_sessions = manager.list_sessions()
    session_exists = any(s.get("session_id") == session_id for s in existing_sessions)

    if session_exists:
        # Update status from queued to starting
        manager.update_session(session_id, {"status": "starting"})
    else:
        # New session - register it
        manager.add_session(session_data)

    # Get the actual agent image that will be used (for display purposes)
    from cua_bench.runner.task_runner import DEFAULT_AGENT_IMAGE

    display_agent_image = agent_image or DEFAULT_AGENT_IMAGE

    print(f"{CYAN}Starting 2-container task execution{RESET}")
    print(f"  Run ID: {run_id}")

    # Show environment type based on provider
    if provider_type in ("simulated", "webtop"):
        print("  Environment: simulated (Playwright)")
    else:
        print(f"  Environment: {image_name} ({env_type})")

    print(f"  Task: {task_path.name} index={task_index}")
    print(f"  Output: {run_output_dir}")
    print(f"  Session: {session_output_dir}")
    print(f"  Agent image: {display_agent_image}")
    if agent_name:
        print(f"  Agent: {agent_name}")
    if vnc_port:
        print(f"  VNC: http://localhost:{vnc_port}")

    # Create runner
    runner = TaskRunner()

    try:
        result = await runner.run_task(
            env_path=task_path,
            task_index=task_index,
            env_type=env_type,
            golden_name=image_name,
            agent=agent_name,
            agent_image=agent_image,
            agent_command=agent_command,
            agent_import_path=agent_import_path,
            model=getattr(args, "model", None),
            max_steps=getattr(args, "max_steps", 100),
            oracle=getattr(args, "oracle", False),
            memory=getattr(args, "memory", "8G"),
            cpus=getattr(args, "cpus", "8"),
            vnc_port=vnc_port,
            api_port=api_port,
            output_dir=session_output_dir,  # Use the session output dir
            stream_agent_logs=True,  # Stream logs to run.log
            provider_type=provider_type,  # Pass detected provider type
        )

        if result.success:
            print(f"\n{GREEN}✓ Task completed successfully!{RESET}")
        else:
            print(f"\n{RED}✗ Task failed with exit code {result.exit_code}{RESET}")
            if result.error:
                print(f"{RED}Error: {result.error}{RESET}")

        if result.agent_logs:
            print(f"\n{CYAN}--- Agent Logs (last 250 lines) ---{RESET}")
            log_lines = result.agent_logs.strip().split("\n")
            for line in log_lines[-250:]:
                print(line)
            print(f"{CYAN}--- End Agent Logs ---{RESET}")

        # Show log file location
        log_file = Path(session_output_dir) / "run.log"
        if log_file.exists():
            print(f"\n{GREY}Full logs saved to: {log_file}{RESET}")

        # Update session status before cleanup
        try:
            final_status = "completed" if result.success else "failed"
            manager.update_session(session_id, {"status": final_status})
        except Exception:
            # Non-fatal - session might not exist if run in sync mode
            pass

        return 0 if result.success else 1

    except Exception as e:
        print(f"{RED}Error: {e}{RESET}")
        import traceback

        traceback.print_exc()

        # Update session status to failed before cleanup
        try:
            manager.update_session(session_id, {"status": "failed"})
        except Exception:
            pass

        return 1

    finally:
        await runner.cleanup_all()


async def _cmd_run_task_detached(args) -> int:
    """Start task in background and return immediately with status hints.

    This is the default mode - tasks run asynchronously and users can
    check status with `cb run watch/status/logs`.
    """
    from cua_bench.sessions import manager

    task_path = Path(args.task_path)
    if not task_path.exists():
        print(f"{RED}Error: Task not found: {task_path}{RESET}")
        return 1

    # Get provider type (explicit arg or auto-detect)
    provider_type = getattr(args, "provider_type", None) or _detect_provider_type(task_path)

    # Get task index
    task_index = getattr(args, "variant_id", 0) or 0

    # Detect env type and image (only used for native providers)
    env_type, image_name = _detect_env_type_and_image(task_path, args)

    # Get run info
    run_id = getattr(args, "_run_id", generate_task_id())

    # Get or generate run output directory
    run_output_dir = getattr(args, "output_dir", None)
    if not run_output_dir:
        run_output_dir = str(_get_run_output_dir(run_id))

    # Create session output directory (nested inside run directory, like dataset runs)
    session_output_dir = str(Path(run_output_dir) / f"{task_path.name}_v{task_index}")

    # Get optional debug ports - auto-allocate if either is requested
    user_vnc = getattr(args, "vnc_port", None)
    user_api = getattr(args, "api_port", None)
    debug_mode = getattr(args, "debug", False)

    if debug_mode or user_vnc is not None or user_api is not None:
        if user_vnc is not None and user_api is not None:
            vnc_port = user_vnc
            api_port = user_api
        else:
            vnc_port, api_port = allocate_ports(
                vnc_default=user_vnc if user_vnc else 8006,
                api_default=user_api if user_api else 5000,
            )
    else:
        vnc_port = None
        api_port = None

    # Resolve agent configuration
    agent_name = getattr(args, "agent", None)
    agent_import_path = getattr(args, "agent_import_path", None)

    if agent_name:
        config_loader = getattr(args, "_config_loader", None)
        if config_loader:
            agent_entry = config_loader.get_agent_by_name(agent_name)
            if agent_entry:
                if agent_entry.is_docker_agent():
                    agent_entry.get_image()
                elif agent_entry.import_path:
                    agent_import_path = agent_entry.import_path

    # Register or update session before starting
    # Use provided session_id if available (from dataset command), otherwise generate one
    session_id = getattr(args, "session_id", None) or f"task-{run_id}"

    # Determine agent display value
    # If oracle flag is set, show 'oracle', otherwise show agent name or None
    oracle_mode = getattr(args, "oracle", False)
    agent_display = "oracle" if oracle_mode else agent_name

    session_data = {
        "session_id": session_id,
        "run_id": run_id,
        "provider": "docker",
        "env_path": str(task_path),
        "task_index": task_index,
        "env_type": env_type,
        "image": image_name,
        "agent": agent_display,
        "model": getattr(args, "model", None) or "-",
        "output_dir": session_output_dir,  # Use session output dir
        "status": "starting",
    }

    # Check if session already exists (from dataset pre-registration)
    existing_sessions = manager.list_sessions()
    session_exists = any(s.get("session_id") == session_id for s in existing_sessions)

    if session_exists:
        # Update status from queued to starting
        manager.update_session(session_id, {"status": "starting"})
    else:
        # New session - register it
        manager.add_session(session_data)

    # Print startup info
    print(f"\n{GREEN}✓ Run started{RESET}")
    print(f"\n  {BOLD}Run ID:{RESET}  {run_id}")
    print(f"  {BOLD}Task:{RESET}    {task_path.name} (variant {task_index})")

    # Show environment based on provider
    if provider_type in ("simulated", "webtop"):
        print(f"  {BOLD}Env:{RESET}     simulated (Playwright)")
    else:
        print(f"  {BOLD}Image:{RESET}   {image_name}")
    print(f"  {BOLD}Output:{RESET}  {run_output_dir}")
    print(f"  {BOLD}Session:{RESET} {session_output_dir}")
    if vnc_port:
        print(f"  {BOLD}VNC:{RESET}     http://localhost:{vnc_port}")

    print(f"\n{CYAN}Commands:{RESET}")
    print(f"  cb run watch {run_id}   {GREY}# Watch progress in real-time{RESET}")
    print(f"  cb run info {run_id}    {GREY}# Show run details{RESET}")
    print(f"  cb run logs {run_id}    {GREY}# View logs{RESET}")
    print(f"  cb run stop {run_id}    {GREY}# Stop the run{RESET}")

    # Debug: Show Python executable being used
    print(f"\n{GREY}Python: {sys.executable}{RESET}")

    # Get Python version
    import platform

    print(f"{GREY}Version: {platform.python_version()}{RESET}")

    # Start task in background using subprocess
    # This spawns a new process that runs the task synchronously
    cmd = [
        sys.executable,
        "-m",
        "cua_bench.cli.main",
        "run",
        "task",
        str(task_path),
        "--wait",  # The subprocess waits for completion
        "--variant-id",
        str(task_index),
        "--output-dir",
        str(run_output_dir),  # Pass the run directory (subprocess will create session subdir)
        "--run-id",
        run_id,  # Pass the run ID so subprocess updates the correct session
    ]

    # Pass through relevant args
    if agent_name:
        cmd.extend(["--agent", agent_name])
    if agent_import_path:
        cmd.extend(["--agent-import-path", agent_import_path])
    if getattr(args, "model", None):
        cmd.extend(["--model", args.model])
    if getattr(args, "max_steps", None):
        cmd.extend(["--max-steps", str(args.max_steps)])
    if getattr(args, "oracle", False):
        cmd.append("--oracle")
    if getattr(args, "image", None):
        cmd.extend(["--image", args.image])
    if getattr(args, "platform", None):
        cmd.extend(["--platform", args.platform])
    if vnc_port:
        cmd.extend(["--vnc-port", str(vnc_port)])
    if api_port:
        cmd.extend(["--api-port", str(api_port)])

    # Add provider type to command (will be passed to subprocess which calls run_task)
    if provider_type in ("simulated", "webtop"):
        cmd.extend(["--provider-type", "simulated"])

    # Start background process
    # Set environment variables for UTF-8 encoding on Windows
    # This prevents UnicodeEncodeError with the banner
    env = os.environ.copy()
    env["PYTHONIOENCODING"] = "utf-8"
    env["PYTHONLEGACYWINDOWSSTDIO"] = "utf-8"

    log_file = Path(session_output_dir) / "run.log" if session_output_dir else None
    if log_file:
        log_file.parent.mkdir(parents=True, exist_ok=True)
        with open(log_file, "w", encoding="utf-8") as f:
            process = subprocess.Popen(
                cmd,
                stdout=f,
                stderr=subprocess.STDOUT,
                start_new_session=True,  # Detach from parent
                env=env,
            )
        print(f"\n{GREY}Logs: {log_file}{RESET}")
    else:
        process = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
            env=env,
        )

    # Update session with PID
    manager.update_session(session_id, {"pid": process.pid, "status": "running"})

    print(f"\n{GREEN}Task running in background (PID: {process.pid}){RESET}\n")
    return 0


def cmd_run_dataset(args) -> int:
    """Run all tasks in a dataset using parallel 2-container architecture."""
    # Load .env file if it exists
    from dotenv import load_dotenv

    env_file = Path.cwd() / ".env"
    if env_file.exists():
        load_dotenv(env_file)
        print(f"{GREY}Loaded environment from: {env_file}{RESET}")

    # Apply configuration defaults
    args = _apply_config_defaults_for_task(args)

    return asyncio.run(_cmd_run_dataset_async(args))


async def _cmd_run_dataset_async(args) -> int:
    """Execute dataset tasks using parallel 2-container architecture.

    In --wait mode: Runs tasks directly with semaphore, prints progress
    In detached mode: Spawns subprocess with --wait
    """
    import fnmatch

    from cua_bench.runner import TaskRunner
    from cua_bench.sessions import manager

    from .registry import resolve_dataset_path

    dataset_path = Path(args.dataset_path)

    # Try to resolve from registry if not a path
    if not dataset_path.exists():
        resolved = resolve_dataset_path(args.dataset_path, update_registry=True)
        if resolved:
            dataset_path = resolved
        else:
            print(f"{RED}Error: Dataset not found: {args.dataset_path}{RESET}")
            return 1

    # Discover tasks in dataset
    tasks = []

    # Check if it's a single task directory (has main.py)
    if (dataset_path / "main.py").exists():
        tasks.append(dataset_path)
    else:
        # Find all subdirectories with main.py
        for task_dir in sorted(dataset_path.iterdir()):
            if task_dir.is_dir() and (task_dir / "main.py").exists():
                tasks.append(task_dir)

    if not tasks:
        print(f"{RED}Error: No tasks found in dataset: {dataset_path}{RESET}")
        return 1

    # Get provider type (explicit arg or auto-detect from first task)
    provider_type = getattr(args, "provider_type", None)
    if not provider_type and tasks:
        provider_type = _detect_provider_type(tasks[0])
    if not provider_type:
        provider_type = "unknown"

    # Log provider
    if provider_type in ("simulated", "webtop"):
        print(f"{GREY}Provider: simulated - agents will use local Playwright sessions{RESET}")
    elif provider_type in ("native", "computer"):
        print(f"{GREY}Provider: native - using 2-container architecture{RESET}")
    else:
        print(f"{GREY}Provider: unknown - using 2-container architecture{RESET}")

    # Apply task filter if specified
    task_filter = getattr(args, "task_filter", None)
    if task_filter:
        tasks = [t for t in tasks if fnmatch.fnmatch(t.name, task_filter)]
        if not tasks:
            print(f"{RED}Error: No tasks match filter: {task_filter}{RESET}")
            return 1

    # Get max variants per task
    max_variants = getattr(args, "max_variants", None)

    # Expand tasks to (task_path, variant_id) tuples
    task_variants = []
    for task_path in tasks:
        # Try to get variant count from task
        try:
            from cua_bench import make

            env = make(str(task_path))
            if env.tasks_config_fn:
                variant_count = len(env.tasks_config_fn())
            else:
                variant_count = 1
        except Exception:
            variant_count = 1

        if max_variants:
            variant_count = min(variant_count, max_variants)

        for variant_id in range(variant_count):
            task_variants.append((task_path, variant_id))

    # Generate run ID (or use provided one from parent process)
    run_id = getattr(args, "run_id", None) or generate_task_id()

    # Determine output directory
    user_output_dir = getattr(args, "output_dir", None)
    if user_output_dir:
        output_dir = Path(user_output_dir)
    else:
        output_dir = _get_run_output_dir(run_id)

    output_dir.mkdir(parents=True, exist_ok=True)

    # Detect env type from first task
    first_task = task_variants[0][0] if task_variants else None
    if first_task:
        env_type, image_name = _detect_env_type_and_image(first_task, args)
    else:
        env_type = getattr(args, "platform", "linux-docker")
        image_name = getattr(args, "image", env_type)

    # Get agent configuration
    agent_name = getattr(args, "agent", None)
    oracle_mode = getattr(args, "oracle", False)
    agent_display = "oracle" if oracle_mode else agent_name

    agent_image = None
    agent_command = None
    agent_import_path = getattr(args, "agent_import_path", None)

    if agent_name:
        config_loader = getattr(args, "_config_loader", None)
        if config_loader:
            agent_entry = config_loader.get_agent_by_name(agent_name)
            if agent_entry:
                if agent_entry.is_docker_agent():
                    agent_image = agent_entry.get_image()
                    agent_command = agent_entry.command
                elif agent_entry.import_path:
                    agent_import_path = agent_entry.import_path

    # Get max parallel workers
    max_parallel = getattr(args, "max_parallel", 4)

    # Register all sessions as queued
    for task_path, variant_id in task_variants:
        session_id = f"task-{run_id}-{task_path.name}-v{variant_id}"
        session_data = {
            "session_id": session_id,
            "run_id": run_id,
            "provider": "docker",
            "env_path": str(task_path),
            "task_index": variant_id,
            "env_type": env_type,
            "image": image_name,
            "agent": agent_display,
            "model": getattr(args, "model", None) or "-",
            "output_dir": str(output_dir / f"{task_path.name}_v{variant_id}"),
            "status": "queued",
        }
        manager.add_session(session_data)

    # Check if --wait mode
    wait_mode = getattr(args, "wait", False)

    if wait_mode:
        # --wait mode: Run tasks directly with semaphore
        print(f"{CYAN}Running dataset: {dataset_path.name}{RESET}")
        print(f"  Tasks: {len(tasks)}")
        print(f"  Total variants: {len(task_variants)}")
        print(f"  Max parallel: {max_parallel}")
        print()

        # Open global run.log for dataset progress output
        global_log_file = output_dir / "run.log"

        def log_print(msg):
            """Print and write to global log file."""
            print(msg)
            with open(global_log_file, "a", encoding="utf-8") as f:
                # Strip ANSI codes for log file
                import re

                clean_msg = re.sub(r"\033\[[0-9;]*m", "", msg)
                f.write(clean_msg + "\n")

        async def run_single_task(task_path: Path, variant_id: int, task_num: int):
            """Run a single task variant."""
            task_output_dir = output_dir / f"{task_path.name}_v{variant_id}"
            task_output_dir.mkdir(parents=True, exist_ok=True)

            session_id = f"task-{run_id}-{task_path.name}-v{variant_id}"

            # Update session status to starting
            manager.update_session(session_id, {"status": "starting"})

            log_print(
                f"{GREY}[{task_num}/{len(task_variants)}] Starting {task_path.name} variant={variant_id}{RESET}"
            )

            runner = TaskRunner()
            try:
                result = await runner.run_task(
                    env_path=task_path,
                    task_index=variant_id,
                    env_type=env_type,
                    golden_name=image_name,
                    agent=agent_name,
                    agent_image=agent_image,
                    agent_command=agent_command,
                    agent_import_path=agent_import_path,
                    model=getattr(args, "model", None),
                    max_steps=getattr(args, "max_steps", 100),
                    oracle=oracle_mode,
                    memory=getattr(args, "memory", "8G"),
                    cpus=getattr(args, "cpus", "8"),
                    output_dir=str(task_output_dir),
                    stream_agent_logs=True,  # Stream agent logs to <task_output_dir>/run.log
                    cleanup_before=False,
                    provider_type=provider_type,  # Pass detected provider type
                )

                # Extract reward from logs if available
                reward_str = "?"
                reward_color = ""
                if result.agent_logs:
                    import re

                    match = re.search(r"Evaluation result: \[([^\]]+)\]", result.agent_logs)
                    if match:
                        reward_str = match.group(1)
                        try:
                            reward_val = float(reward_str)
                            reward_color = GREEN if reward_val >= 0.5 else RED
                        except ValueError:
                            pass

                # Update session status
                final_status = "completed" if result.success else "failed"
                manager.update_session(session_id, {"status": final_status})

                if result.success:
                    log_print(
                        f"{GREEN}[{task_num}/{len(task_variants)}] ✓ {task_path.name} variant={variant_id}{RESET} reward={reward_color}{reward_str}{RESET}"
                    )
                else:
                    log_print(
                        f"{RED}[{task_num}/{len(task_variants)}] ✗ {task_path.name} variant={variant_id}{RESET} reward={reward_color}{reward_str}{RESET}"
                    )

                return result

            except Exception as e:
                manager.update_session(session_id, {"status": "failed"})
                log_print(
                    f"{RED}[{task_num}/{len(task_variants)}] ✗ {task_path.name} variant={variant_id} error={str(e)}{RESET}"
                )
                return None
            finally:
                await runner.cleanup_all()

        # Process with semaphore for parallelism control
        semaphore = asyncio.Semaphore(max_parallel)

        async def run_with_semaphore(task_path, variant_id, task_num):
            async with semaphore:
                return await run_single_task(task_path, variant_id, task_num)

        # Create all tasks
        coroutines = [
            run_with_semaphore(task_path, variant_id, i + 1)
            for i, (task_path, variant_id) in enumerate(task_variants)
        ]

        # Run all tasks (semaphore controls parallelism)
        results = await asyncio.gather(*coroutines, return_exceptions=True)

        # Summarize results
        success_count = sum(1 for r in results if r and hasattr(r, "success") and r.success)
        failed_count = len(results) - success_count

        summary = f"\n{CYAN}{'=' * 60}{RESET}\n"
        summary += f"{CYAN}Dataset Complete: {dataset_path.name}{RESET}\n"
        summary += f"{CYAN}{'=' * 60}{RESET}\n"
        summary += f"  Total:   {len(results)}\n"
        summary += f"  Success: {success_count}\n"
        summary += f"  Failed:  {failed_count}\n"

        log_print(summary)

        return 0 if failed_count == 0 else 1

    else:
        # Detached mode - spawn subprocess with --wait
        print(f"\n{GREEN}✓ Dataset started{RESET}")
        print(f"\n  {BOLD}Run ID:{RESET}      {run_id}")
        print(f"  {BOLD}Dataset:{RESET}     {dataset_path.name}")
        print(f"  {BOLD}Tasks:{RESET}       {len(tasks)}")
        print(f"  {BOLD}Variants:{RESET}    {len(task_variants)}")
        print(f"  {BOLD}Agent:{RESET}       {agent_display or '-'}")

        # Show environment based on provider
        if provider_type in ("simulated", "webtop"):
            print(f"  {BOLD}Env:{RESET}         simulated (Playwright)")
        else:
            print(f"  {BOLD}Image:{RESET}       {image_name}")

        print(f"  {BOLD}Parallelism:{RESET} {max_parallel}")
        print(f"  {BOLD}Output:{RESET}      {output_dir}")

        print(f"\n{CYAN}Commands:{RESET}")
        print(f"  cb run watch {run_id}   {GREY}# Watch progress in real-time{RESET}")
        print(f"  cb run info {run_id}    {GREY}# Show run details{RESET}")
        print(f"  cb run logs {run_id}    {GREY}# View logs{RESET}")
        print(f"  cb run stop {run_id}    {GREY}# Stop the run{RESET}")

        # Build command for subprocess
        cmd = [
            sys.executable,
            "-m",
            "cua_bench.cli.main",
            "run",
            "dataset",
            str(dataset_path),
            "--wait",
            "--run-id",
            run_id,  # Pass run ID to subprocess
        ]

        # Pass through all relevant args
        if agent_name:
            cmd.extend(["--agent", agent_name])
        if agent_import_path:
            cmd.extend(["--agent-import-path", agent_import_path])
        if getattr(args, "model", None):
            cmd.extend(["--model", args.model])
        if getattr(args, "max_steps", None):
            cmd.extend(["--max-steps", str(args.max_steps)])
        if oracle_mode:
            cmd.append("--oracle")
        if getattr(args, "platform", None):
            cmd.extend(["--platform", args.platform])
        if getattr(args, "image", None):
            cmd.extend(["--image", args.image])
        if max_parallel != 4:
            cmd.extend(["--max-parallel", str(max_parallel)])
        if max_variants:
            cmd.extend(["--max-variants", str(max_variants)])
        if task_filter:
            cmd.extend(["--task-filter", task_filter])
        if user_output_dir:
            cmd.extend(["--output-dir", str(user_output_dir)])

        # Set UTF-8 encoding
        env_vars = os.environ.copy()
        env_vars["PYTHONIOENCODING"] = "utf-8"
        env_vars["PYTHONLEGACYWINDOWSSTDIO"] = "utf-8"

        # Redirect output to global run.log
        global_log_file = output_dir / "run.log"
        log_handle = open(global_log_file, "w", encoding="utf-8", buffering=1)

        # Start detached subprocess
        subprocess.Popen(
            cmd,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            env=env_vars,
        )

        return 0


def _apply_config_defaults_for_task(args):
    """Apply configuration defaults from .cua/config.yaml for task/dataset commands.

    Loads config and applies defaults for any unspecified CLI arguments.
    CLI arguments always take priority over config file settings.
    """
    from cua_bench.config import ConfigLoader, detect_env_type

    # Determine search path for config
    if hasattr(args, "task_path") and args.task_path:
        search_path = Path(args.task_path).resolve()
        if not search_path.exists():
            search_path = Path.cwd()
    elif hasattr(args, "dataset_path") and args.dataset_path:
        search_path = Path(args.dataset_path).resolve()
        if not search_path.exists():
            search_path = Path.cwd()
    else:
        search_path = Path.cwd()

    # Load config
    config_loader = ConfigLoader(search_path)
    config_dir = config_loader.find_config_dir()

    if config_dir:
        print(f"{GREY}Found config at: {config_dir}{RESET}")

    # Detect environment type for env-specific overrides
    env_type = None
    if hasattr(args, "task_path") and args.task_path:
        env_type = detect_env_type(str(args.task_path))
    elif hasattr(args, "dataset_path") and args.dataset_path:
        env_type = detect_env_type(str(args.dataset_path))

    # Get effective config
    cli_args = {
        "agent": getattr(args, "agent", None),
        "agent_import_path": getattr(args, "agent_import_path", None),
        "model": getattr(args, "model", None),
        "max_steps": getattr(args, "max_steps", None),
        "output_dir": getattr(args, "output_dir", None),
    }

    effective = config_loader.get_effective_config(cli_args, env_type)

    # Apply effective config back to args (only for unset values)
    if not getattr(args, "agent", None) and effective.get("agent"):
        args.agent = effective["agent"]
    if not getattr(args, "agent_import_path", None) and effective.get("agent_import_path"):
        args.agent_import_path = effective["agent_import_path"]
    if not getattr(args, "model", None) and effective.get("model"):
        args.model = effective["model"]
    if not getattr(args, "max_steps", None) and effective.get("max_steps"):
        args.max_steps = effective["max_steps"]
    if not getattr(args, "output_dir", None) and effective.get("output_dir"):
        args.output_dir = effective["output_dir"]

    # Store config loader for later use
    args._config_loader = config_loader

    return args


def _apply_config_defaults(args):
    """Apply configuration defaults from .cua/config.yaml.

    Loads config and applies defaults for any unspecified CLI arguments.
    CLI arguments always take priority over config file settings.
    """
    from cua_bench.config import ConfigLoader, detect_env_type

    # Determine search path for config
    # Start from env_path if available, otherwise use cwd
    if hasattr(args, "task_id") and args.task_id:
        search_path = Path(args.task_id).resolve()
        if not search_path.exists():
            search_path = Path.cwd()
    elif hasattr(args, "dataset_path") and args.dataset_path:
        search_path = Path(args.dataset_path).resolve()
    else:
        search_path = Path.cwd()

    # Load config
    config_loader = ConfigLoader(search_path)
    config_dir = config_loader.find_config_dir()

    if config_dir:
        print(f"{GREY}Found config at: {config_dir}{RESET}")

    # Detect environment type for env-specific overrides
    env_type = None
    if hasattr(args, "task_id") and args.task_id:
        env_type = detect_env_type(str(args.task_id))
    elif hasattr(args, "dataset_path") and args.dataset_path:
        env_type = detect_env_type(str(args.dataset_path))

    # Get effective config (merges config file with CLI args)
    cli_args = {
        "agent": getattr(args, "agent", None),
        "agent_import_path": getattr(args, "agent_import_path", None),
        "model": getattr(args, "model", None),
        "max_steps": getattr(args, "max_steps", None),
        "output_dir": getattr(args, "output_dir", None),
    }

    effective = config_loader.get_effective_config(cli_args, env_type)

    # Apply effective config back to args (only for unset values)
    if not getattr(args, "agent", None) and effective.get("agent"):
        args.agent = effective["agent"]
    if not getattr(args, "agent_import_path", None) and effective.get("agent_import_path"):
        args.agent_import_path = effective["agent_import_path"]
    if not getattr(args, "model", None) and effective.get("model"):
        args.model = effective["model"]
    if not getattr(args, "max_steps", None) and effective.get("max_steps"):
        args.max_steps = effective["max_steps"]
    if not getattr(args, "output_dir", None) and effective.get("output_dir"):
        args.output_dir = effective["output_dir"]

    # Store config loader for later use (e.g., agent resolution)
    args._config_loader = config_loader

    return args


def execute(args):
    """Execute the run command."""
    # Check for subcommands first
    run_command = getattr(args, "run_command", None)

    if run_command == "task":
        return cmd_run_task(args)
    elif run_command == "dataset":
        return cmd_run_dataset(args)
    elif run_command == "list":
        return cmd_list(args)
    elif run_command == "watch":
        return cmd_watch(args)
    elif run_command == "stop":
        return cmd_stop(args)
    elif run_command == "logs":
        return cmd_logs(args)
    elif run_command == "info":
        return cmd_info(args)
    else:
        # No subcommand specified - show help
        print(f"{YELLOW}Please specify a subcommand:{RESET}")
        print("\n  cb run task <path>      Run a single task")
        print("  cb run dataset <path>   Run all tasks in a dataset")
        print("  cb run list             List all runs")
        print("  cb run watch <id>       Watch a run in real-time")
        print("  cb run info <id>        Show run info")
        print("  cb run stop <id>        Stop a run")
        print("  cb run logs <id>        View run logs")
        return 1


def _check_adapter_setup(env_path: Path, args) -> bool:
    """Check if adapter requires setup and handle it.

    Returns True if we should continue, False if we should abort.
    """
    # Only check for local provider (cloud handles setup automatically)
    provider = getattr(args, "provider", "local")
    if provider != "local":
        return True

    # Try to import check_setup from the adapter
    try:
        import importlib

        main_py = env_path / "main.py"
        if not main_py.exists():
            return True

        # Add both the env_path's parent and parent's parent to sys.path
        # This handles both "tasks/winarena_adapter" and direct paths
        env_path_resolved = env_path.resolve()
        parent_dir = str(env_path_resolved.parent)
        if parent_dir not in sys.path:
            sys.path.insert(0, parent_dir)

        # Also add current working directory if not present
        cwd = str(Path.cwd())
        if cwd not in sys.path:
            sys.path.insert(0, cwd)

        # Import the module using standard import mechanism
        package_name = env_path_resolved.name
        module = importlib.import_module(f"{package_name}.main")

        # Check if adapter has check_setup function
        if not hasattr(module, "check_setup"):
            return True

        status = module.check_setup()

        if status.ready:
            return True

        # Setup is required
        print(f"\n{YELLOW}⚠ Setup Required{RESET}")
        print(f"{GREY}{status.message}{RESET}\n")

        if getattr(args, "setup", False):
            # User requested setup
            if not status.can_setup:
                print(f"{RED}Error: Setup cannot be performed on this system.{RESET}")
                return False

            print(f"{CYAN}Running setup...{RESET}\n")

            if hasattr(module, "run_setup"):
                # Pass iso-related arguments if the run_setup function accepts them
                iso_path = getattr(args, "iso", None)
                download_iso = getattr(args, "download_iso", False)

                import inspect

                sig = inspect.signature(module.run_setup)
                if len(sig.parameters) >= 2:
                    # New signature with iso support
                    success = module.run_setup(iso_path=iso_path, download_iso=download_iso)
                else:
                    # Legacy signature
                    success = module.run_setup()

                if success:
                    print(f"\n{GREEN}✓ Setup complete!{RESET}\n")
                    return True
                else:
                    print(f"\n{RED}✗ Setup failed.{RESET}")
                    return False
            elif status.setup_command:
                print(f"Run: {BOLD}{status.setup_command}{RESET}")
                return False
        else:
            # Show instructions
            if status.can_setup:
                print(f"Run with {BOLD}--setup{RESET} to prepare, or manually:")
                if status.setup_command:
                    print(f"  {GREY}{status.setup_command}{RESET}\n")
            return False

    except Exception as e:
        # If we can't check, just continue
        print(f"{GREY}Note: Could not check adapter setup: {e}{RESET}")
        return True

    return True


def _detect_provider_type(env_path: Path) -> str:
    """Detect provider type from task configuration.

    Args:
        env_path: Path to task environment directory

    Returns:
        Provider type ("simulated", "webtop", "native", "computer", or "unknown")
    """
    import importlib.util
    import sys

    # Try to import and inspect the task
    try:
        # Load main.py module
        main_file = env_path / "main.py"
        if not main_file.exists():
            return "unknown"

        spec = importlib.util.spec_from_file_location("env_module", main_file)
        if spec is None or spec.loader is None:
            return "unknown"

        module = importlib.util.module_from_spec(spec)

        # Add env_path to Python path temporarily for imports
        sys.path.insert(0, str(env_path))
        try:
            spec.loader.exec_module(module)

            # Look for tasks_config function
            for name in dir(module):
                obj = getattr(module, name)
                if callable(obj) and hasattr(obj, "_td_type"):
                    if getattr(obj, "_td_type") == "tasks_config":
                        # Call the function to get tasks
                        tasks = obj()
                        if tasks and len(tasks) > 0:
                            task = tasks[0]
                            if hasattr(task, "computer") and task.computer:
                                provider = task.computer.get("provider", "unknown")
                                return provider

            return "unknown"

        finally:
            sys.path.pop(0)
            # Clean up module
            if "env_module" in sys.modules:
                del sys.modules["env_module"]

    except Exception:
        return "unknown"


def _detect_env_type_and_image(env_path: Path, args) -> tuple[str, str]:
    """Detect environment type and image name from task config or args.

    Returns:
        Tuple of (env_type, image_name)
    """
    # Check if explicitly specified
    image_name = getattr(args, "image", None)
    env_type = getattr(args, "platform", None)

    # If image_name is specified but env_type isn't, derive env_type from image_name
    if image_name and not env_type:
        if "windows" in image_name:
            env_type = "windows-qemu"
        elif "android" in image_name:
            env_type = "android-qemu"
        elif "linux-qemu" in image_name:
            env_type = "linux-qemu"
        else:
            env_type = "linux-docker"

    if image_name and env_type:
        return env_type, image_name

    # Try to detect from task config
    try:
        from cua_bench import make

        env = make(str(env_path))

        if env.tasks_config_fn:
            tasks = env.tasks_config_fn()
            # Use the specific variant if specified, otherwise use first task
            variant_id = getattr(args, "variant_id", None) or 0
            task_idx = int(variant_id) if variant_id is not None else 0
            task_idx = min(task_idx, len(tasks) - 1) if tasks else 0

            if tasks and task_idx < len(tasks):
                task = tasks[task_idx]
                if hasattr(task, "computer") and task.computer:
                    computer = task.computer
                    # Handle both dict and object access patterns
                    os_type = None
                    if isinstance(computer, dict):
                        setup_config = computer.get("setup_config", {})
                        os_type = setup_config.get("os_type")
                    elif hasattr(computer, "os_type"):
                        os_type = computer.os_type
                    elif hasattr(computer, "setup_config"):
                        os_type = getattr(
                            computer.setup_config, "os_type", None
                        ) or computer.setup_config.get("os_type")

                    if os_type and "windows" in os_type.lower():
                        env_type = env_type or "windows-qemu"
                        image_name = image_name or "windows-qemu"
                    elif os_type and "android" in os_type.lower():
                        env_type = env_type or "android-qemu"
                        image_name = image_name or "android-qemu"
                    else:
                        env_type = env_type or "linux-docker"
                        image_name = image_name or "linux-docker"
    except Exception:
        import traceback

        traceback.print_exc()

    # Defaults
    env_type = env_type or "linux-docker"
    image_name = image_name or env_type

    return env_type, image_name
