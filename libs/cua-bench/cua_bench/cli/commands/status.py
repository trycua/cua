"""System status dashboard.

Shows where sandboxes can run, the canonical images and recent runs.

Usage:
    cb status                           # Show system overview
"""

from ._help import examples
import asyncio
import os
import platform as sys_platform
import shutil

from ._canonical import canonical_images

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def execute(args) -> int:
    """Show system status dashboard."""
    return asyncio.run(_execute_async(args))


def _check(label: str, ok: bool, good: str, bad: str) -> None:
    mark = f"{GREEN}✓ {good}{RESET}" if ok else f"{YELLOW}○ {bad}{RESET}"
    print(f"  {label:<9}{mark}")


def _cloud_auth() -> str:
    """The credential source, without reading the OS credential vault: a
    dashboard is not an explicit sign-in check (``cb run --on cloud`` is)."""
    try:
        from cua_sandbox import fleet_auth_source

        return fleet_auth_source(read_session=False) or ""
    except Exception:  # noqa: BLE001 - SDK not loadable
        return ""


async def _execute_async(args) -> int:
    """Execute status command asynchronously."""
    print("\ncua-bench Status")
    print("=" * 70)

    system = sys_platform.system()
    print(f"\n{BOLD}Local sandboxes{RESET} (the default, --on local)")
    print("-" * 70)
    _check("Docker:", shutil.which("docker") is not None, "installed", "not found (containers)")
    if system == "Linux":
        _check("KVM:", os.path.exists("/dev/kvm"), "available", "not available (VMs)")
    if system == "Darwin":
        _check("Lume:", shutil.which("lume") is not None, "installed", "not installed (macOS VMs)")

    print(f"\n{BOLD}Cloud{RESET} (--on cloud)")
    print("-" * 70)
    source = _cloud_auth()
    _check("Auth:", bool(source), source, "not signed in (cua auth login)")

    print(f"\n{BOLD}Images{RESET} (canonical; any registry image works with --image)")
    print("-" * 70)
    for row in canonical_images():
        print(f"  {row['name']:<9} {row['image']:<34} {'/'.join(row['kinds'])}")

    # Runs (sessions)
    try:
        from cua_bench.sessions import list_sessions, session_status

        sessions = list_sessions()

        # Group by run_id
        from collections import defaultdict

        runs = defaultdict(list)
        for session in sessions:
            run_id = session.get("run_id", "-")
            if run_id != "-":
                runs[run_id].append(session)

        print(f"\n{BOLD}Runs{RESET} ({len(runs)} active)")
        print("-" * 70)

        if runs:
            for run_id, run_sessions in list(runs.items())[:5]:  # Show top 5 runs
                # Count statuses
                running = 0
                completed = 0
                failed = 0

                for session in run_sessions:
                    status = session_status(session)["status"]
                    if status in ("running", "starting"):
                        running += 1
                    elif status == "completed":
                        completed += 1
                    elif status in ("failed", "cancelled"):
                        failed += 1

                total = len(run_sessions)
                agent = run_sessions[0].get("agent", "-") if run_sessions else "-"

                if running > 0:
                    status_icon = f"{GREEN}●{RESET}"
                elif completed == total:
                    status_icon = f"{CYAN}✓{RESET}"
                elif failed > 0:
                    status_icon = f"{RED}✗{RESET}"
                else:
                    status_icon = f"{GREY}○{RESET}"

                print(f"  {status_icon} {run_id:<20} agent={agent} ({completed}/{total} done)")

            if len(runs) > 5:
                print(f"\n  ... and {len(runs) - 5} more runs")
        else:
            print(f"  {GREY}No runs in progress.{RESET}")
            print("\n  Start a run with:")
            print("    cb run <task>")

    except Exception as e:
        print(f"\n{BOLD}Runs{RESET}")
        print("-" * 70)
        print(f"  {GREY}Could not load runs: {e}{RESET}")

    # Quick commands
    print("\n" + "=" * 70)
    print(f"\n{BOLD}Quick Commands{RESET}")
    print("-" * 70)
    print("  cb run <task> --dry-run       # What a run would start")
    print("  cb image list                 # Canonical images")
    print("  cb env ls                     # Managed cloud pools")
    print("  cb run list                   # Show active runs")
    print()

    return 0


def register_parser(subparsers):
    """Register the status command with the main CLI parser."""
    subparsers.add_parser(
        "status",
        help="Show system status dashboard",
        **examples(("Show runs, sandboxes and credentials at a glance", "cb status")),
    )
